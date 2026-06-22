use super::commands::{
    CommandDocumentPosition, active_realm_command, gmod_api_coverage_command,
    module_exports_command,
};
use super::completion::{
    completion_item, general_binding_completions, import_completion_item, keyword_completion_items,
    namespace_member_completion_items,
};
use super::cursor::{CompletionContext, completion_context, identifier_prefix};
use super::diagnostics::manifest_section_insert_position;
use super::gmod_api::{
    GmodTypeFacts, api_completion_candidates, api_entry_completion_item,
    api_hover_markdown_from_text, api_path_at_offset, api_root_completion_candidates,
    external_api_hover_markdown, hook_name_at_offset, infer_receiver_class, method_path_at_offset,
    resolve_typed_method_path, signature_help_at,
};
use super::lexical_completion::lexical_binding_completions;
use super::protocol;
use super::protocol::{
    document_uri_key, encode_semantic_tokens, path_to_url, server_capabilities, url_to_path,
};
use super::text_sync::apply_document_changes;
use super::workspace::same_path;
use super::{Server, analysis_configs, is_lux_analysis_watched_path, std_package_code_actions};
use crate::analysis::{
    AnalysisConfig, AnalysisDiagnostic, AnalysisFile, AnalysisPosition, AnalysisRange,
    AnalysisSemanticToken, AnalysisWorkspace, CompletionCandidate, SemanticTokenKind,
    analyze_files,
};
use crate::diag::Severity;
use crate::package_manager::{LockRequest, lock_project};
use crate::source::{SourceFile, SourceSpan};
use gmod_api_db::ApiIndex;
use lsp_types::notification::{Notification as _, PublishDiagnostics};
use lsp_types::{
    CodeActionOrCommand, CompletionItemKind, Documentation, Hover, HoverContents, HoverParams,
    InitializeParams, InsertTextFormat, PublishDiagnosticsParams, SemanticToken, SignatureHelp,
    TextDocumentContentChangeEvent,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "lux_lsp_{name}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

fn write_runtime_package(root: &std::path::Path, package_id: &str, export: &str) {
    let package_path = package_id.trim_start_matches('@').replace('/', "_");
    let package_src = root.join(format!("packages/{package_path}/src"));
    std::fs::create_dir_all(&package_src).expect("package source");
    std::fs::write(
            root.join("lux.package.toml"),
            format!(
                "name = \"test-packages\"\n\n[[package]]\nid = \"{package_id}\"\nversion = \"0.1.0\"\npath = \"packages/{package_path}\"\n",
            ),
        )
        .expect("package manifest");
    std::fs::write(
        package_src.join("module.lux"),
        format!("export fn {export}() = true\n"),
    )
    .expect("package module");
}

fn write_runtime_package_exports(root: &std::path::Path, package_id: &str, exports: &[&str]) {
    let package_path = package_id.trim_start_matches('@').replace('/', "_");
    let package_src = root.join(format!("packages/{package_path}/src"));
    std::fs::create_dir_all(&package_src).expect("package source");
    std::fs::write(
            root.join("lux.package.toml"),
            format!(
                "name = \"test-packages\"\n\n[[package]]\nid = \"{package_id}\"\nversion = \"0.1.0\"\npath = \"packages/{package_path}\"\n",
            ),
        )
        .expect("package manifest");
    let text = exports
        .iter()
        .map(|export| format!("export client fn {export}() = true\n"))
        .collect::<String>();
    std::fs::write(package_src.join("cl_module.lux"), text).expect("package module");
}

fn write_reexported_mgfx_package(root: &std::path::Path) {
    let package_root = root.join("package-set");
    let mgfx_src = package_root.join("vendor/mgfx/src");
    let paint_src = package_root.join("vendor/mgfx/paint/src");
    std::fs::create_dir_all(&mgfx_src).expect("mgfx package source");
    std::fs::create_dir_all(&paint_src).expect("paint package source");
    std::fs::write(
            package_root.join("lux.package.toml"),
            "name = \"test-packages\"\n\n[[package]]\nid = \"@vendor/mgfx\"\nversion = \"0.1.0\"\npath = \"vendor/mgfx\"\n\n[[package]]\nid = \"@vendor/mgfx/paint\"\nversion = \"0.1.0\"\npath = \"vendor/mgfx/paint\"\n",
        )
        .expect("package manifest");
    std::fs::write(
            mgfx_src.join("cl_module.lux"),
            "import * as paint_mod from \"@vendor/mgfx/paint\"\nlocal paint = paint_mod\nexport client { paint }\n",
        )
        .expect("mgfx root module");
    std::fs::write(
        paint_src.join("cl_module.lux"),
        "export client fn chamferBoxEx(x, y, w, h, drawStyle = nil) = nil\n",
    )
    .expect("paint module");
}

fn write_package_set_macro_package(root: &std::path::Path) -> (PathBuf, PathBuf) {
    let runtime_src = root.join("packages/vendor_caps/src");
    let macro_src = root.join("packages/vendor_caps/compiletime");
    std::fs::create_dir_all(&runtime_src).expect("runtime source");
    std::fs::create_dir_all(&macro_src).expect("macro source");
    std::fs::write(
            root.join("lux.package.toml"),
            "name = \"test-packages\"\n\n[[package]]\nid = \"@vendor/caps\"\nversion = \"0.1.0\"\npath = \"packages/vendor_caps\"\n",
        )
        .expect("package manifest");
    let runtime_path = runtime_src.join("module.lux");
    std::fs::write(
        &runtime_path,
        "import macro { defineValue } from \"@vendor/caps\"\n\ndefineValue()\n",
    )
    .expect("runtime module");
    let macro_path = macro_src.join("module.lux");
    std::fs::write(
        &macro_path,
        "export macro fn defineValue(ctx, call) = nil\n",
    )
    .expect("macro module");
    (runtime_path, macro_path)
}

fn published_diagnostics_for(
    connection: &lsp_server::Connection,
    path: &std::path::Path,
) -> Vec<lsp_types::Diagnostic> {
    let uri = path_to_url(path).expect("uri");
    let mut latest = None;
    while let Ok(message) = connection.receiver.try_recv() {
        let lsp_server::Message::Notification(notification) = message else {
            continue;
        };
        if notification.method != PublishDiagnostics::METHOD {
            continue;
        }
        let params: PublishDiagnosticsParams =
            serde_json::from_value(notification.params).expect("publish diagnostics params");
        if params.uri == uri {
            latest = Some(params.diagnostics);
        }
    }
    latest.unwrap_or_default()
}

#[test]
fn initialize_capabilities_are_not_double_wrapped() {
    let value = serde_json::to_value(server_capabilities()).expect("capabilities");
    assert!(value.get("completionProvider").is_some());
    assert!(value.get("hoverProvider").is_some());
    assert!(value.get("semanticTokensProvider").is_some());
    let execute_commands = value
        .get("executeCommandProvider")
        .and_then(|provider| provider.get("commands"))
        .and_then(|commands| commands.as_array())
        .expect("execute commands");
    let completion_triggers = value
        .get("completionProvider")
        .and_then(|provider| provider.get("triggerCharacters"))
        .and_then(|triggers| triggers.as_array())
        .expect("completion trigger characters");
    assert!(
        completion_triggers
            .iter()
            .any(|trigger| trigger.as_str() == Some(","))
    );
    assert!(
        completion_triggers
            .iter()
            .any(|trigger| trigger.as_str() == Some(" "))
    );
    let signature_triggers = value
        .get("signatureHelpProvider")
        .and_then(|provider| provider.get("triggerCharacters"))
        .and_then(|triggers| triggers.as_array())
        .expect("signature trigger characters");
    assert!(
        signature_triggers
            .iter()
            .any(|trigger| trigger.as_str() == Some(" "))
    );
    assert!(
        execute_commands
            .iter()
            .any(|command| command.as_str() == Some(protocol::INSTALL_STD_PACKAGES_COMMAND))
    );
    assert!(value.get("capabilities").is_none());
}

#[test]
fn unresolved_official_lux_package_offers_install_std_packages_fix() {
    let root = temp_root("std_package_fix");
    let source_root = root.join("src");
    std::fs::create_dir_all(&source_root).expect("source root");
    std::fs::write(
            root.join("lux.toml"),
            "package_id = \"demo\"\nbundle_id = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n[dependencies]\n",
        )
        .expect("manifest");
    let path = source_root.join("ui.lux");
    let analysis = analyze_files(
        AnalysisConfig::new(&source_root),
        [AnalysisFile {
            path: path.clone(),
            text: "import { signal } from \"@lux/reactive\"\nexport fn run() = signal(0)\n".into(),
        }],
    )
    .expect("analysis");
    assert!(
        analysis
            .diagnostics_for_path(&path)
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("MODULE001")),
        "{:#?}",
        analysis.diagnostics
    );

    let uri = path_to_url(&path).expect("uri");
    let actions = std_package_code_actions(&analysis, &path, &root, &uri);
    let action = actions
        .iter()
        .find_map(|action| match action {
            CodeActionOrCommand::CodeAction(action)
                if action.title == "Fix: Install std packages" =>
            {
                Some(action)
            }
            _ => None,
        })
        .expect("install std packages action");
    let command = action.command.as_ref().expect("command");
    assert_eq!(command.command, protocol::INSTALL_STD_PACKAGES_COMMAND);
    let arguments = command.arguments.as_ref().expect("arguments");
    assert_eq!(arguments.len(), 1);
    assert_eq!(arguments[0]["packages"][0], "@lux/reactive");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn event_loop_exits_after_shutdown_exit_sequence() {
    let root = temp_root("shutdown_exit");
    std::fs::create_dir_all(&root).expect("root");
    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&root).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    client_connection
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            lsp_server::RequestId::from(1),
            "shutdown".to_string(),
            (),
        )))
        .expect("send shutdown");
    client_connection
        .sender
        .send(lsp_server::Message::Notification(
            lsp_server::Notification::new("exit".to_string(), ()),
        ))
        .expect("send exit");

    let mut server = Server::new(server_connection, initialize);
    server.event_loop().expect("event loop");

    let response = client_connection
        .receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("shutdown response");
    let lsp_server::Message::Response(response) = response else {
        panic!("expected shutdown response");
    };
    assert_eq!(response.id, lsp_server::RequestId::from(1));
    assert!(response.error.is_none());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reanalysis_keeps_package_set_diagnostics_when_nested_project_document_opens() {
    let root = temp_root("mixed_publish");
    let package_source = root.join("lux/mgfx/src/module.lux");
    std::fs::create_dir_all(package_source.parent().expect("package parent")).expect("package dir");
    std::fs::write(
            root.join("lux.package.toml"),
            "name = \"mixed\"\n\n[[package]]\nid = \"@lux/mgfx\"\nversion = \"0.1.0\"\npath = \"lux/mgfx\"\n",
        )
        .expect("package manifest");
    std::fs::write(
        &package_source,
        "export fn needsArg(value) = value\nneedsArg()\n",
    )
    .expect("package source");

    let nested_root = root.join("precompiled");
    let nested_source = nested_root.join("src/cl_mgfx.lux");
    std::fs::create_dir_all(nested_source.parent().expect("nested parent")).expect("nested dir");
    std::fs::write(
            nested_root.join("lux.toml"),
            "package_id = \"mgfx_precompiled\"\nbundle_id = \"mgfx\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"../dist/lua\"\nruntime_base = \"mgfx\"\nautorun = true\nsource_comments = \"none\"\n\n[dependencies]\n",
        )
        .expect("nested manifest");
    std::fs::write(&nested_source, "local ok = true\n").expect("nested source");

    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&root).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    let mut server = Server::new(server_connection, initialize);

    server.reanalyze_and_publish();
    assert!(
        published_diagnostics_for(&client_connection, &package_source)
            .iter()
            .any(
                |diagnostic| diagnostic.code.as_ref().and_then(|code| match code {
                    lsp_types::NumberOrString::String(value) => Some(value.as_str()),
                    lsp_types::NumberOrString::Number(_) => None,
                }) == Some("CALL001")
            )
    );

    server.documents.insert(
        path_to_url(&nested_source).expect("nested uri"),
        std::fs::read_to_string(&nested_source).expect("nested text"),
    );
    server.reanalyze_and_publish();
    assert!(
        published_diagnostics_for(&client_connection, &package_source)
            .iter()
            .any(
                |diagnostic| diagnostic.code.as_ref().and_then(|code| match code {
                    lsp_types::NumberOrString::String(value) => Some(value.as_str()),
                    lsp_types::NumberOrString::Number(_) => None,
                }) == Some("CALL001")
            )
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn completion_context_detects_import_source_and_specifier_lists() {
    assert_eq!(
        completion_context("import { p_", " } from \"inventory\""),
        CompletionContext::ImportSpecifierList {
            source: Some("inventory".into())
        }
    );
    assert_eq!(
        completion_context("import { p_", ""),
        CompletionContext::ImportSpecifierList { source: None }
    );
    assert_eq!(
        completion_context("  import { } from \"", ""),
        CompletionContext::ImportSource
    );
    assert_eq!(
        completion_context("export { player_", " }"),
        CompletionContext::ExportList
    );
    assert_eq!(
        completion_context("net.", ""),
        CompletionContext::ApiMember {
            prefix: "net.".into()
        }
    );
    assert_eq!(
        completion_context("fn run() = inv", ""),
        CompletionContext::General
    );
}

#[test]
fn general_completion_prefix_is_extracted_from_current_token() {
    assert_eq!(identifier_prefix("fn run() = Cre"), "Cre");
    assert_eq!(identifier_prefix("local x = draw.Simple"), "Simple");
    assert_eq!(identifier_prefix("  "), "");
}

#[test]
fn hook_hover_context_extracts_hook_names() {
    let text = "hook.Add(\"PlayerInitialSpawn\", \"id\", function(ply) end)";
    let offset = text.find("Initial").expect("offset");
    assert_eq!(
        hook_name_at_offset(text, offset),
        Some("PlayerInitialSpawn".into())
    );
}

#[test]
fn transient_parse_identifier_diagnostics_are_suppressed_only_for_open_documents() {
    let diagnostic = AnalysisDiagnostic {
        path: PathBuf::from("module.lux"),
        range: AnalysisRange {
            start: AnalysisPosition {
                line: 0,
                character: "import { ".len() as u32,
            },
            end: AnalysisPosition {
                line: 0,
                character: "import { ".len() as u32,
            },
        },
        severity: Severity::Error,
        code: Some("PARSE005".into()),
        message: "expected identifier".into(),
        notes: Vec::new(),
        help: None,
    };
    assert!(!super::should_publish_diagnostic(
        &diagnostic,
        "import { ",
        true,
        false
    ));
    assert!(super::should_publish_diagnostic(
        &diagnostic,
        "import { ",
        false,
        false
    ));
    assert!(!super::should_publish_diagnostic(
        &AnalysisDiagnostic {
            code: Some("PARSE006".into()),
            message: "expected `from`".into(),
            ..diagnostic.clone()
        },
        "import { bind",
        true,
        true
    ));
}

#[test]
fn document_changes_apply_all_incremental_completion_edits() {
    let initial = "import {  } from \"@lux/reactive\"\n".to_string();
    let text = apply_document_changes(
        initial,
        vec![
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range {
                    start: lsp_types::Position {
                        line: 0,
                        character: 9,
                    },
                    end: lsp_types::Position {
                        line: 0,
                        character: 9,
                    },
                }),
                range_length: None,
                text: "batch".into(),
            },
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range {
                    start: lsp_types::Position {
                        line: 1,
                        character: 0,
                    },
                    end: lsp_types::Position {
                        line: 1,
                        character: 0,
                    },
                }),
                range_length: None,
                text: "local ok = true\n".into(),
            },
        ],
    );
    assert_eq!(
        text,
        "import { batch } from \"@lux/reactive\"\nlocal ok = true\n"
    );
}

#[test]
fn document_changes_accept_full_document_replacement() {
    let text = apply_document_changes(
        "broken".into(),
        vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "fn ok() = 1".into(),
        }],
    );
    assert_eq!(text, "fn ok() = 1");
}

#[test]
fn signature_help_uses_gmod_api_database() {
    let api = ApiIndex::bundled();
    let file = SourceFile::new(0, None, "net.Start(");
    let help = signature_help_at(&file, &api, file.text.len()).expect("signature help");
    assert_eq!(
        help.signatures[0].label,
        "net.Start(messageName, unreliable = false)"
    );
    assert_eq!(help.signatures[0].parameters.as_ref().unwrap().len(), 2);
}

#[test]
fn manifest_extern_insert_position_targets_existing_section() {
    let text = "[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\n\n[target.gmod.extern]\nA = \"shared\"\n";
    assert_eq!(
        manifest_section_insert_position(text, "target.gmod.extern"),
        Some((6, 0))
    );
}

#[test]
fn analysis_configs_include_nearest_open_document_manifest() {
    let root = std::env::temp_dir().join(format!(
        "lux_lsp_manifest_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let project = root.join("examples/gmod_project");
    let source = project.join("src/client/ui.lux");
    std::fs::create_dir_all(source.parent().expect("source parent")).expect("source dir");
    std::fs::write(
        root.join("lux.toml"),
        "[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\n",
    )
    .expect("root manifest");
    std::fs::write(
        project.join("lux.toml"),
        "[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\n",
    )
    .expect("project manifest");
    std::fs::write(&source, "").expect("source");

    let mut documents = HashMap::new();
    documents.insert(path_to_url(&source).expect("source uri"), String::new());
    let configs = analysis_configs(&root, &documents);
    assert!(
        configs
            .iter()
            .any(|config| same_path(&config.source_root, &project.join("src"))),
        "{configs:#?}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn analysis_configs_keep_package_set_root_when_nested_project_exists() {
    let root = temp_root("mixed_workspace");
    std::fs::create_dir_all(&root).expect("root");
    let package_runtime = root.join("lux/mgfx/src/module.lux");
    std::fs::create_dir_all(package_runtime.parent().expect("runtime parent"))
        .expect("runtime dir");
    std::fs::write(
            root.join("lux.package.toml"),
            "name = \"mixed\"\n\n[[package]]\nid = \"@lux/mgfx\"\nversion = \"0.1.0\"\npath = \"lux/mgfx\"\n",
        )
        .expect("package manifest");
    std::fs::write(
        &package_runtime,
        "import { installGlobal } from \"@lux/mgfx\"\n",
    )
    .expect("package source");

    let nested_root = root.join("precompiled");
    std::fs::create_dir_all(nested_root.join("src")).expect("nested dir");
    std::fs::write(
            nested_root.join("lux.toml"),
            "package_id = \"mgfx_precompiled\"\nbundle_id = \"mgfx\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"../dist/lua\"\nruntime_base = \"mgfx\"\nautorun = true\nsource_comments = \"none\"\n\n[dependencies]\n",
        )
        .expect("nested manifest");
    std::fs::write(
        nested_root.join("src/cl_mgfx.lux"),
        "import { installGlobal } from \"@lux/mgfx\"\n",
    )
    .expect("nested source");

    let documents = HashMap::from([
        (
            path_to_url(&package_runtime).expect("runtime uri"),
            std::fs::read_to_string(&package_runtime).expect("runtime text"),
        ),
        (
            path_to_url(&nested_root.join("src/cl_mgfx.lux")).expect("nested uri"),
            "import { installGlobal } from \"@lux/mgfx\"\n".to_string(),
        ),
    ]);

    let configs = super::analysis_configs(&root, &documents);
    assert!(
        configs.iter().any(|config| config.is_package_set()),
        "{configs:#?}"
    );
    assert!(
        configs.iter().any(|config| !config.is_package_set()
            && same_path(&config.source_root, &nested_root.join("src"))),
        "{configs:#?}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn overlays_are_filtered_to_the_owning_analysis_config() {
    let root = temp_root("mixed_overlays");
    std::fs::create_dir_all(&root).expect("root");
    let package_runtime = root.join("lux/mgfx/src/module.lux");
    std::fs::create_dir_all(package_runtime.parent().expect("runtime parent"))
        .expect("runtime dir");
    std::fs::write(
            root.join("lux.package.toml"),
            "name = \"mixed\"\n\n[[package]]\nid = \"@lux/mgfx\"\nversion = \"0.1.0\"\npath = \"lux/mgfx\"\n",
        )
        .expect("package manifest");
    std::fs::write(&package_runtime, "export fn installGlobal() = true\n").expect("package source");

    let nested_root = root.join("precompiled");
    let nested_source = nested_root.join("src/cl_mgfx.lux");
    std::fs::create_dir_all(nested_source.parent().expect("nested parent")).expect("nested dir");
    std::fs::write(
            nested_root.join("lux.toml"),
            "package_id = \"mgfx_precompiled\"\nbundle_id = \"mgfx\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"../dist/lua\"\nruntime_base = \"mgfx\"\nautorun = true\nsource_comments = \"none\"\n\n[dependencies]\n",
        )
        .expect("nested manifest");
    std::fs::write(&nested_source, "local ok = true\n").expect("nested source");

    let documents = HashMap::from([
        (
            path_to_url(&package_runtime).expect("runtime uri"),
            std::fs::read_to_string(&package_runtime).expect("runtime text"),
        ),
        (
            path_to_url(&nested_source).expect("nested uri"),
            std::fs::read_to_string(&nested_source).expect("nested text"),
        ),
    ]);
    let overlays = documents
        .iter()
        .map(|(uri, text)| AnalysisFile {
            path: url_to_path(uri).expect("overlay path"),
            text: text.clone(),
        })
        .collect::<Vec<_>>();
    let configs = analysis_configs(&root, &documents);
    let package_config = configs
        .iter()
        .find(|config| config.is_package_set())
        .expect("package-set config");
    let project_config = configs
        .iter()
        .find(|config| !config.is_package_set())
        .expect("project config");

    let package_overlays = super::overlays_for_config(package_config, &overlays);
    assert_eq!(package_overlays.len(), 1, "{package_overlays:#?}");
    assert!(same_path(&package_overlays[0].path, &package_runtime));

    let project_overlays = super::overlays_for_config(project_config, &overlays);
    assert_eq!(project_overlays.len(), 1, "{project_overlays:#?}");
    assert!(same_path(&project_overlays[0].path, &nested_source));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn analysis_config_waits_for_open_document_when_root_has_no_manifest() {
    let root = std::env::temp_dir().join(format!(
        "lux_lsp_empty_manifest_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("root");
    let documents = HashMap::new();

    assert!(super::analysis_configs(&root, &documents).is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn no_manifest_workspace_uses_standalone_analysis_for_open_lux_files() {
    let root = temp_root("standalone_workspace");
    let examples = root.join("examples");
    std::fs::create_dir_all(&examples).expect("examples");
    let features = examples.join("features.lux");
    let diagnostics = examples.join("match_diagnostics.lux");
    std::fs::write(&features, "fn feature() = 1").expect("features");
    std::fs::write(&diagnostics, "fn demo() = 2").expect("diagnostics");

    let mut documents = HashMap::new();
    documents.insert(
        path_to_url(&features).expect("features uri"),
        std::fs::read_to_string(&features).expect("features text"),
    );
    documents.insert(
        path_to_url(&diagnostics).expect("diagnostics uri"),
        std::fs::read_to_string(&diagnostics).expect("diagnostics text"),
    );

    let configs = analysis_configs(&root, &documents);
    assert_eq!(configs.len(), 1, "{configs:#?}");
    assert!(configs[0].is_standalone());

    let overlays = documents
        .iter()
        .map(|(uri, text)| AnalysisFile {
            path: url_to_path(uri).expect("overlay path"),
            text: text.clone(),
        })
        .collect::<Vec<_>>();
    let workspace = AnalysisWorkspace::load(configs[0].clone(), overlays).expect("analysis");
    let analysis = workspace.analysis();

    assert!(
        analysis
            .lsp_diagnostics_for_path(&features)
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("PART007")),
        "{:#?}",
        analysis.lsp_diagnostics_for_path(&features)
    );
    assert!(
        analysis
            .module_for_path(&features)
            .is_some_and(|module| module.module_path == "examples/features")
    );
    assert!(
        analysis
            .module_for_path(&diagnostics)
            .is_some_and(|module| module.module_path == "examples/match_diagnostics")
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn analysis_config_loads_package_set_workspace_without_project_manifest() {
    let root = temp_root("package_set_workspace");
    std::fs::create_dir_all(&root).expect("root");
    let (runtime_path, macro_path) = write_package_set_macro_package(&root);

    let mut documents = HashMap::new();
    documents.insert(path_to_url(&runtime_path).expect("runtime uri"), {
        std::fs::read_to_string(&runtime_path).expect("runtime text")
    });
    let config = analysis_configs(&root, &documents)
        .into_iter()
        .find(AnalysisConfig::is_package_set)
        .expect("analysis config");
    assert!(config.is_package_set());
    assert!(
        config.package_roots.iter().any(|path| path == &root),
        "{:?}",
        config.package_roots
    );

    let workspace = AnalysisWorkspace::load(config, Vec::new()).expect("analysis");
    let analysis = workspace.analysis();
    assert!(analysis.file_by_path(&runtime_path).is_some());
    assert!(analysis.file_by_path(&macro_path).is_some());
    assert!(
        analysis
            .lsp_diagnostics_for_path(&macro_path)
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("RESOLVE006")),
        "{:#?}",
        analysis.lsp_diagnostics_for_path(&macro_path)
    );
    assert!(
        analysis
            .lsp_diagnostics_for_path(&runtime_path)
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("MACRO001")),
        "{:#?}",
        analysis.lsp_diagnostics_for_path(&runtime_path)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn analysis_config_loads_locked_package_roots_for_completion_and_diagnostics() {
    let root = std::env::temp_dir().join(format!(
        "lux_lsp_lock_package_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let project = root.join("project");
    let source_root = project.join("src");
    let package_root = root.join("package-set");
    std::fs::create_dir_all(&source_root).expect("project source");
    write_runtime_package(&package_root, "@vendor/ui", "mount");
    std::fs::write(
            project.join("lux.toml"),
            "package_id = \"demo\"\nbundle_id = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n[dependencies]\n\"@vendor/ui\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
    let source = source_root.join("module.lux");
    std::fs::write(&source, "import { mount } from \"@vendor/ui\"\nmount()\n").expect("source");
    lock_project(&LockRequest {
        project_root: project.clone(),
    })
    .expect("lock project");

    let config = analysis_configs(&project, &HashMap::new())
        .into_iter()
        .next()
        .expect("analysis config");
    assert!(
        config
            .package_roots
            .iter()
            .any(|root| root == &package_root),
        "{:?}",
        config.package_roots
    );
    let workspace = AnalysisWorkspace::load(config, Vec::new()).expect("analysis");
    let analysis = workspace.analysis();
    let diagnostics = analysis.lsp_diagnostics_for_path(&source);
    assert!(
        diagnostics.iter().all(|diagnostic| !diagnostic
            .message
            .contains("failed to load Lux runtime package metadata")),
        "{diagnostics:#?}"
    );
    let exports =
        analysis.importable_exports(&source, "@vendor/ui", crate::module::RealmSet::SHARED);
    assert!(exports.iter().any(|candidate| candidate.label == "mount"));
    let all_exports =
        analysis.importable_exports_for_all_sources(&source, crate::module::RealmSet::SHARED);
    assert!(all_exports.iter().any(|candidate| {
        candidate.label == "mount" && candidate.source.as_deref() == Some("@vendor/ui")
    }));
    let offset = analysis
        .offset_for_position(&source, 1, "mount".len())
        .expect("offset");
    let symbol = analysis
        .symbol_at_path_offset(&source, offset)
        .expect("symbol");
    assert!(
        symbol
            .definition_path
            .as_ref()
            .is_some_and(|path| path.ends_with("package-set/packages/vendor_ui/src/module.lux")),
        "{:?}",
        symbol.definition_path
    );
    let hover = analysis
        .hover_markdown_at_path_offset(&source, offset)
        .expect("hover");
    assert!(hover.contains("**Signature:** `mount()`"), "{hover}");
    let signature_help = analysis
        .signature_help_at_path_offset(
            &source,
            "import { mount } from \"@vendor/ui\"\nmount(".len(),
        )
        .expect("signature help");
    assert_eq!(signature_help.signature.label, "mount()");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn analysis_watched_paths_include_manifest_and_lockfile() {
    assert!(is_lux_analysis_watched_path(
        PathBuf::from("lux.toml").as_path()
    ));
    assert!(is_lux_analysis_watched_path(
        PathBuf::from("lux.lock").as_path()
    ));
    assert!(!is_lux_analysis_watched_path(
        PathBuf::from("module.lux").as_path()
    ));
}

#[test]
fn infers_gmod_receiver_class_from_common_constructors() {
    let text = "fn current() = LocalPlayer()\nlocal ply = current()\nlocal alias = ply\nlocal button = vgui.Create(\"DButton\")\n";
    assert_eq!(infer_receiver_class(text, "ply"), Some("Player".into()));
    assert_eq!(infer_receiver_class(text, "alias"), Some("Player".into()));
    assert_eq!(infer_receiver_class(text, "button"), Some("DButton".into()));
}

#[test]
fn api_completion_uses_receiver_class_methods_for_colon_calls() {
    let api = ApiIndex::bundled();
    let text = "local ply = LocalPlayer()\nply:";
    let labels = api_completion_candidates(&api, "ply:", Some(text))
        .into_iter()
        .map(|candidate| candidate.label)
        .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label == "Nick"), "{labels:#?}");
}

#[test]
fn api_hover_extracts_dot_paths_without_project_analysis() {
    let api = ApiIndex::bundled();
    let text = "draw.SimpleText(\"HP\", \"DermaDefault\", 0, 0)";
    let offset = text.find("SimpleText").expect("offset");
    assert_eq!(
        api_path_at_offset(text, offset),
        Some("draw.SimpleText".into())
    );
    let markdown = api_hover_markdown_from_text(&api, text, offset).expect("official API hover");
    assert!(markdown.contains("draw.SimpleText"), "{markdown}");
    assert!(markdown.contains("Official documentation"), "{markdown}");
}

#[test]
fn gmod_api_hover_and_signature_follow_local_external_aliases() {
    let root = PathBuf::from("src");
    let path = root.join("client/ui.lux");
    let text = "client const hookAdd = hook?.Add\nclient fn setup() {\n  hookAdd(\"Initialize\", \"id\", () => nil)\n}\n";
    let analysis = analyze_files(
        AnalysisConfig::new(&root).with_package_id("game"),
        [AnalysisFile {
            path: path.clone(),
            text: text.into(),
        }],
    )
    .expect("analysis");
    assert!(
        analysis
            .lsp_diagnostics_for_path(&path)
            .iter()
            .all(|diagnostic| diagnostic.severity != Severity::Error),
        "{:#?}",
        analysis.lsp_diagnostics_for_path(&path)
    );

    let offset = analysis
        .offset_for_position(&path, 2, "  hookAdd".len())
        .expect("offset");
    let symbol = analysis
        .symbol_at_path_offset(&path, offset)
        .expect("symbol");
    assert_eq!(symbol.name, "hookAdd");
    assert_eq!(symbol.external_name.as_deref(), Some("hook.Add"));
    assert_eq!(
        symbol
            .signature
            .as_ref()
            .map(|signature| signature.label.as_str()),
        Some("hook.Add(eventName, identifier, func)")
    );

    let api = ApiIndex::bundled();
    let hover = external_api_hover_markdown(&analysis, &api, &path, offset).expect("hover");
    assert!(hover.contains("hook.Add"), "{hover}");
    assert!(hover.contains("Official documentation"), "{hover}");

    let help = analysis
        .signature_help_at_path_offset(
            &path,
            "client const hookAdd = hook?.Add\nclient fn setup() {\n  hookAdd(".len(),
        )
        .expect("signature help");
    assert_eq!(
        help.signature.label,
        "hook.Add(eventName, identifier, func)"
    );
}

#[test]
fn gmod_api_signature_follows_external_aliases_across_parts() {
    let root = PathBuf::from("src");
    let alias_path = root.join("shop/base/cl_state.lux");
    let use_path = root.join("shop/base/module.lux");
    let analysis = analyze_files(
        AnalysisConfig::new(&root).with_package_id("game"),
        [
            AnalysisFile {
                path: alias_path,
                text: "client const hookAdd = hook.Add\n".into(),
            },
            AnalysisFile {
                path: use_path.clone(),
                text: "client fn setup() {\n  hookAdd(\"Initialize\", \"id\", () => nil)\n}\n"
                    .into(),
            },
        ],
    )
    .expect("analysis");

    let offset = analysis
        .offset_for_position(&use_path, 1, "  hookAdd".len())
        .expect("offset");
    let symbol = analysis
        .symbol_at_path_offset(&use_path, offset)
        .expect("symbol");
    assert_eq!(symbol.name, "hookAdd");
    assert_eq!(symbol.external_name.as_deref(), Some("hook.Add"));

    let help = analysis
        .signature_help_at_path_offset(&use_path, "client fn setup() {\n  hookAdd(".len())
        .expect("signature help");
    assert_eq!(
        help.signature.label,
        "hook.Add(eventName, identifier, func)"
    );
}

#[test]
fn gmod_api_alias_call_diagnostics_respect_vararg_parameters() {
    let root = PathBuf::from("src");
    let path = root.join("client/ui.lux");
    let analysis = analyze_files(
        AnalysisConfig::new(&root).with_package_id("game"),
        [AnalysisFile {
            path: path.clone(),
            text: "const mathMax = math.max\nfn ok() = mathMax(8, math.Round(4.2))\nfn bad() = mathMax()\n"
                .into(),
        }],
    )
    .expect("analysis");
    let diagnostics = analysis.lsp_diagnostics_for_path(&path);
    assert!(
        diagnostics.iter().all(|diagnostic| {
            !(diagnostic.code.as_deref() == Some("CALL001") && diagnostic.message.contains("got 2"))
        }),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("CALL001") && diagnostic.message.contains("got 0")
        }),
        "{diagnostics:#?}"
    );

    let help = analysis
        .signature_help_at_path_offset(&path, "const mathMax = math.max\nfn ok() = mathMax(".len())
        .expect("signature help");
    assert_eq!(help.signature.label, "math.max(numbers)");
    assert!(help.signature.vararg);
}

#[test]
fn lux_import_hover_takes_precedence_over_gmod_api_names() {
    let root = PathBuf::from("src");
    let path = root.join("client/ui.lux");
    let text = "import { Button } from \"@lux/ui\"\nexport fn mount(panel) = Button({})\n";
    let analysis = analyze_files(
        AnalysisConfig::new(&root).with_package_id("game"),
        [AnalysisFile {
            path: path.clone(),
            text: text.into(),
        }],
    )
    .expect("analysis");
    let offset = analysis
        .offset_for_position(&path, 0, "import { Bu".len())
        .expect("offset");
    let lux_hover = analysis
        .hover_markdown_at_path_offset(&path, offset)
        .expect("Lux hover");
    assert!(lux_hover.contains("Imported from"), "{lux_hover}");

    let api = ApiIndex::bundled();
    assert!(
        external_api_hover_markdown(&analysis, &api, &path, offset).is_none(),
        "Lux import binding must not be treated as GMod API"
    );
}

#[test]
fn import_completion_without_from_inserts_source() {
    let root = temp_root("import_completion");
    let project = root.join("project");
    let source_root = project.join("src");
    let package_root = root.join("package-set");
    std::fs::create_dir_all(&source_root).expect("project source");
    write_runtime_package(&package_root, "@vendor/ui", "Button");
    std::fs::write(
            project.join("lux.toml"),
            "package_id = \"game\"\nbundle_id = \"game\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/game\"\nautorun = true\n\n[dependencies]\n\"@vendor/ui\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
    lock_project(&LockRequest {
        project_root: project.clone(),
    })
    .expect("lock project");
    let path = source_root.join("client/ui.lux");
    let config = analysis_configs(&project, &HashMap::new())
        .into_iter()
        .find(|config| !config.is_package_set())
        .expect("analysis config");
    let analysis = analyze_files(
        config,
        [AnalysisFile {
            path: path.clone(),
            text: "import { Bu".into(),
        }],
    )
    .expect("analysis");
    let candidate = analysis
        .importable_exports_for_all_sources(&path, crate::module::RealmSet::CLIENT)
        .into_iter()
        .find(|candidate| {
            candidate.label == "Button" && candidate.source.as_deref() == Some("@vendor/ui")
        })
        .expect("Button import candidate");
    let item = import_completion_item(candidate, true);
    assert_eq!(item.label, "Button");
    assert_eq!(
        item.insert_text.as_deref(),
        Some("Button } from \"@vendor/ui\"")
    );
    assert!(item.label_details.is_some());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn import_completion_after_comma_space_uses_installed_package_exports() {
    let root = temp_root("import_completion_comma_space");
    let project = root.join("project");
    let source_root = project.join("src");
    let package_root = root.join("package-set");
    std::fs::create_dir_all(&source_root).expect("project source");
    write_runtime_package(&package_root, "@vendor/ui", "Column");
    std::fs::write(
            project.join("lux.toml"),
            "package_id = \"game\"\nbundle_id = \"game\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/game\"\nautorun = true\n\n[dependencies]\n\"@vendor/ui\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
    lock_project(&LockRequest {
        project_root: project.clone(),
    })
    .expect("lock project");
    let path = source_root.join("client/ui.lux");
    std::fs::create_dir_all(path.parent().expect("source parent")).expect("source parent");
    let text = "import { Button,  } from \"@vendor/ui\"\n";
    std::fs::write(&path, text).expect("source");
    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&project).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    let mut server = Server::new(server_connection, initialize);
    server.reanalyze_and_publish();

    let uri = path_to_url(&path).expect("source uri");
    let params: lsp_types::CompletionParams = serde_json::from_value(serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": "import { Button, ".len() },
        "context": { "triggerKind": 2, "triggerCharacter": "," }
    }))
    .expect("completion params");
    let response = server.completion(params).expect("completion");
    let completion: Option<lsp_types::CompletionResponse> =
        serde_json::from_value(response).expect("completion response");
    let labels = match completion.expect("completion result") {
        lsp_types::CompletionResponse::Array(items) => items,
        lsp_types::CompletionResponse::List(list) => list.items,
    }
    .into_iter()
    .map(|item| item.label)
    .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label == "Column"), "{labels:#?}");

    drop(client_connection);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn import_completion_after_alias_trailing_comma_uses_installed_package_exports() {
    let root = temp_root("import_completion_alias_trailing_comma");
    let project = root.join("project");
    let source_root = project.join("src");
    let package_root = root.join("package-set");
    std::fs::create_dir_all(&source_root).expect("project source");
    write_runtime_package_exports(
        &package_root,
        "@vendor/mgfx",
        &["paint", "widgets", "install", "create"],
    );
    std::fs::write(
            project.join("lux.toml"),
            "package_id = \"game\"\nbundle_id = \"game\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/game\"\nautorun = true\n\n[dependencies]\n\"@vendor/mgfx\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
    lock_project(&LockRequest {
        project_root: project.clone(),
    })
    .expect("lock project");
    let path = source_root.join("module.lux");
    std::fs::create_dir_all(path.parent().expect("source parent")).expect("source parent");
    let text = "import { paint as MPaint, widgets as MWidgets,  } from '@vendor/mgfx'\n";
    std::fs::write(&path, text).expect("source");
    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&project).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    let mut server = Server::new(server_connection, initialize);
    server.reanalyze_and_publish();

    let uri = path_to_url(&path).expect("source uri");
    let params: lsp_types::CompletionParams = serde_json::from_value(serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": "import { paint as MPaint, widgets as MWidgets, ".len() },
        "context": { "triggerKind": 2, "triggerCharacter": "," }
    }))
    .expect("completion params");
    let response = server.completion(params).expect("completion");
    let completion: Option<lsp_types::CompletionResponse> =
        serde_json::from_value(response).expect("completion response");
    let labels = match completion.expect("completion result") {
        lsp_types::CompletionResponse::Array(items) => items,
        lsp_types::CompletionResponse::List(list) => list.items,
    }
    .into_iter()
    .map(|item| item.label)
    .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label == "install"), "{labels:#?}");
    assert!(labels.iter().any(|label| label == "create"), "{labels:#?}");

    drop(client_connection);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn import_completion_after_alias_trailing_comma_space_uses_space_trigger() {
    let root = temp_root("import_completion_alias_trailing_comma_space");
    let project = root.join("project");
    let source_root = project.join("src");
    let package_root = root.join("package-set");
    std::fs::create_dir_all(&source_root).expect("project source");
    write_runtime_package_exports(
        &package_root,
        "@vendor/mgfx",
        &["paint", "widgets", "install", "create"],
    );
    std::fs::write(
            project.join("lux.toml"),
            "package_id = \"game\"\nbundle_id = \"game\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/game\"\nautorun = true\n\n[dependencies]\n\"@vendor/mgfx\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
    lock_project(&LockRequest {
        project_root: project.clone(),
    })
    .expect("lock project");
    let path = source_root.join("module.lux");
    std::fs::create_dir_all(path.parent().expect("source parent")).expect("source parent");
    let text = "import { paint as MPaint, widgets as MWidgets,  } from '@vendor/mgfx'\n";
    std::fs::write(&path, text).expect("source");
    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&project).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    let mut server = Server::new(server_connection, initialize);
    server.reanalyze_and_publish();

    let uri = path_to_url(&path).expect("source uri");
    let params: lsp_types::CompletionParams = serde_json::from_value(serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": "import { paint as MPaint, widgets as MWidgets,  ".len() },
        "context": { "triggerKind": 2, "triggerCharacter": " " }
    }))
    .expect("completion params");
    let response = server.completion(params).expect("completion");
    let completion: Option<lsp_types::CompletionResponse> =
        serde_json::from_value(response).expect("completion response");
    let labels = match completion.expect("completion result") {
        lsp_types::CompletionResponse::Array(items) => items,
        lsp_types::CompletionResponse::List(list) => list.items,
    }
    .into_iter()
    .map(|item| item.label)
    .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label == "install"), "{labels:#?}");
    assert!(labels.iter().any(|label| label == "create"), "{labels:#?}");

    drop(client_connection);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ordinary_space_trigger_does_not_return_general_completion_items() {
    let root = temp_root("ordinary_space_completion_trigger");
    let source_root = root.join("src");
    std::fs::create_dir_all(&source_root).expect("source root");
    let path = source_root.join("module.lux");
    std::fs::write(&path, "local value = ").expect("source");
    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&root).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    let mut server = Server::new(server_connection, initialize);
    server.reanalyze_and_publish();

    let uri = path_to_url(&path).expect("source uri");
    let params: lsp_types::CompletionParams = serde_json::from_value(serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": "local value = ".len() },
        "context": { "triggerKind": 2, "triggerCharacter": " " }
    }))
    .expect("completion params");
    let response = server.completion(params).expect("completion");
    let completion: Option<lsp_types::CompletionResponse> =
        serde_json::from_value(response).expect("completion response");
    let labels = match completion.expect("completion result") {
        lsp_types::CompletionResponse::Array(items) => items,
        lsp_types::CompletionResponse::List(list) => list.items,
    }
    .into_iter()
    .map(|item| item.label)
    .collect::<Vec<_>>();
    assert!(labels.is_empty(), "{labels:#?}");

    drop(client_connection);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn multiline_import_completion_after_comma_space_uses_installed_package_exports() {
    let root = temp_root("import_completion_multiline_comma_space");
    let project = root.join("project");
    let source_root = project.join("src");
    let package_root = root.join("package-set");
    std::fs::create_dir_all(&source_root).expect("project source");
    write_runtime_package(&package_root, "@vendor/ui", "Column");
    std::fs::write(
            project.join("lux.toml"),
            "package_id = \"game\"\nbundle_id = \"game\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/game\"\nautorun = true\n\n[dependencies]\n\"@vendor/ui\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
    lock_project(&LockRequest {
        project_root: project.clone(),
    })
    .expect("lock project");
    let path = source_root.join("client/ui.lux");
    std::fs::create_dir_all(path.parent().expect("source parent")).expect("source parent");
    let text = "import {\n  Button,\n  \n} from \"@vendor/ui\"\n";
    std::fs::write(&path, text).expect("source");
    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&project).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    let mut server = Server::new(server_connection, initialize);
    server.reanalyze_and_publish();

    let uri = path_to_url(&path).expect("source uri");
    let params: lsp_types::CompletionParams = serde_json::from_value(serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 2, "character": "  ".len() },
        "context": { "triggerKind": 2, "triggerCharacter": "," }
    }))
    .expect("completion params");
    let response = server.completion(params).expect("completion");
    let completion: Option<lsp_types::CompletionResponse> =
        serde_json::from_value(response).expect("completion response");
    let labels = match completion.expect("completion result") {
        lsp_types::CompletionResponse::Array(items) => items,
        lsp_types::CompletionResponse::List(list) => list.items,
    }
    .into_iter()
    .map(|item| item.label)
    .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label == "Column"), "{labels:#?}");

    drop(client_connection);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn namespace_member_completion_items_use_installed_package_exports() {
    let root = temp_root("namespace_member_completion");
    let project = root.join("project");
    let source_root = project.join("src");
    let package_root = root.join("package-set");
    std::fs::create_dir_all(&source_root).expect("project source");
    write_runtime_package(&package_root, "@vendor/ui", "Button");
    std::fs::write(
            project.join("lux.toml"),
            "package_id = \"game\"\nbundle_id = \"game\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/game\"\nautorun = true\n\n[dependencies]\n\"@vendor/ui\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
    lock_project(&LockRequest {
        project_root: project.clone(),
    })
    .expect("lock project");
    let path = source_root.join("client/ui.lux");
    let text = "import * as ui from \"@vendor/ui\"\nclient fn mount(panel) {\n  ui.Button\n}\n";
    let config = analysis_configs(&project, &HashMap::new())
        .into_iter()
        .find(|config| !config.is_package_set())
        .expect("analysis config");
    let analysis = analyze_files(
        config,
        [AnalysisFile {
            path: path.clone(),
            text: text.into(),
        }],
    )
    .expect("analysis");
    let offset = analysis
        .offset_for_position(&path, 2, "  ui.".len())
        .expect("offset");
    assert_eq!(
        completion_context("  ui.", ""),
        CompletionContext::ApiMember {
            prefix: "ui.".into()
        }
    );

    let labels =
        namespace_member_completion_items(Some(&analysis), Some(path.as_path()), offset, "ui.")
            .into_iter()
            .map(|item| item.label)
            .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label == "Button"), "{labels:#?}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn member_completion_keeps_previous_analysis_for_dot_trigger() {
    let root = temp_root("member_completion_dot_trigger");
    let project = root.join("project");
    let source_root = project.join("src");
    std::fs::create_dir_all(&source_root).expect("project source");
    write_reexported_mgfx_package(&root);
    std::fs::write(
            project.join("lux.toml"),
            "package_id = \"game\"\nbundle_id = \"game\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/game\"\nautorun = true\n\n[dependencies]\n\"@vendor/mgfx\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
    lock_project(&LockRequest {
        project_root: project.clone(),
    })
    .expect("lock project");
    let path = source_root.join("client/ui.lux");
    std::fs::create_dir_all(path.parent().expect("source parent")).expect("source parent");
    let stable_text = "import { paint as MPaint } from \"@vendor/mgfx\"\nclient fn draw() {\n  MPaint.chamferBoxEx\n}\n";
    std::fs::write(&path, stable_text).expect("source");
    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&project).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    let mut server = Server::new(server_connection, initialize);
    server.reanalyze_and_publish();

    let incomplete_text =
        "import { paint as MPaint } from \"@vendor/mgfx\"\nclient fn draw() {\n  MPaint.\n}\n";
    let uri = path_to_url(&path).expect("source uri");
    server.documents.insert(uri.clone(), incomplete_text.into());
    server.analysis_due = Some(std::time::Instant::now() + Duration::from_secs(60));
    let params: lsp_types::CompletionParams = serde_json::from_value(serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 2, "character": "  MPaint.".len() },
        "context": { "triggerKind": 2, "triggerCharacter": "." }
    }))
    .expect("completion params");
    let response = server.completion(params).expect("completion");
    assert!(
        server.analysis_due.is_some(),
        "member completion should not flush pending incomplete analysis"
    );
    let completion: Option<lsp_types::CompletionResponse> =
        serde_json::from_value(response).expect("completion response");
    let labels = match completion.expect("completion result") {
        lsp_types::CompletionResponse::Array(items) => items,
        lsp_types::CompletionResponse::List(list) => list.items,
    }
    .into_iter()
    .map(|item| {
        (
            item.label,
            item.kind,
            item.detail,
            item.documentation.map(|doc| match doc {
                Documentation::String(value) => value,
                Documentation::MarkupContent(markup) => markup.value,
            }),
        )
    })
    .collect::<Vec<_>>();
    let chamfer = labels
        .iter()
        .find(|(label, _, _, _)| label == "chamferBoxEx")
        .expect("chamferBoxEx completion");
    assert_eq!(chamfer.1, Some(CompletionItemKind::FUNCTION));
    assert_eq!(
        chamfer.2.as_deref(),
        Some("chamferBoxEx(x, y, w, h, drawStyle?)")
    );
    assert!(
        chamfer
            .3
            .as_deref()
            .is_some_and(|documentation| documentation
                .contains("**Signature:** `chamferBoxEx(x, y, w, h, drawStyle?)`")),
        "{chamfer:#?}"
    );

    drop(client_connection);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn member_signature_and_hover_survive_incomplete_argument_edits() {
    let root = temp_root("member_signature_incomplete_args");
    let project = root.join("project");
    let source_root = project.join("src");
    std::fs::create_dir_all(&source_root).expect("project source");
    write_reexported_mgfx_package(&root);
    std::fs::write(
            project.join("lux.toml"),
            "package_id = \"game\"\nbundle_id = \"game\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/game\"\nautorun = true\n\n[dependencies]\n\"@vendor/mgfx\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
    lock_project(&LockRequest {
        project_root: project.clone(),
    })
    .expect("lock project");
    let path = source_root.join("client/ui.lux");
    std::fs::create_dir_all(path.parent().expect("source parent")).expect("source parent");
    let stable_text = "import { paint as MPaint } from \"@vendor/mgfx\"\nclient fn draw() {\n  MPaint.chamferBoxEx(1, 2, 3, 4)\n}\n";
    std::fs::write(&path, stable_text).expect("source");
    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&project).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    let mut server = Server::new(server_connection, initialize);
    server.reanalyze_and_publish();

    let incomplete_text = "import { paint as MPaint } from \"@vendor/mgfx\"\nclient fn draw() {\n  MPaint.chamferBoxEx(1, 2,  )\n}\n";
    let uri = path_to_url(&path).expect("source uri");
    server.documents.insert(uri.clone(), incomplete_text.into());
    server.analysis_due = Some(std::time::Instant::now() + Duration::from_secs(60));

    let signature_params: lsp_types::SignatureHelpParams =
        serde_json::from_value(serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": 2, "character": "  MPaint.chamferBoxEx(1, 2, ".len() },
            "context": { "triggerKind": 2, "triggerCharacter": ",", "isRetrigger": false }
        }))
        .expect("signature params");
    let signature_response = server
        .signature_help(signature_params)
        .expect("signature help");
    assert!(
        server.analysis_due.is_some(),
        "signature help should not flush pending incomplete analysis"
    );
    let signature: Option<SignatureHelp> =
        serde_json::from_value(signature_response).expect("signature response");
    let signature = signature.expect("signature help");
    assert_eq!(
        signature.signatures[0].label,
        "chamferBoxEx(x, y, w, h, drawStyle?)"
    );
    assert_eq!(signature.active_parameter, Some(2));

    let signature_params: lsp_types::SignatureHelpParams =
        serde_json::from_value(serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": 2, "character": "  MPaint.chamferBoxEx(1, 2,  ".len() },
            "context": { "triggerKind": 2, "triggerCharacter": " ", "isRetrigger": true }
        }))
        .expect("signature params");
    let signature_response = server
        .signature_help(signature_params)
        .expect("signature help");
    let signature: Option<SignatureHelp> =
        serde_json::from_value(signature_response).expect("signature response");
    let signature = signature.expect("signature help");
    assert_eq!(
        signature.signatures[0].label,
        "chamferBoxEx(x, y, w, h, drawStyle?)"
    );
    assert_eq!(signature.active_parameter, Some(2));

    let hover_params: HoverParams = serde_json::from_value(serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 2, "character": "  MPaint.chamferBoxEx".len() }
    }))
    .expect("hover params");
    let hover_response = server.hover(hover_params).expect("hover");
    let hover: Option<Hover> = serde_json::from_value(hover_response).expect("hover response");
    let hover = hover.expect("hover result");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markdown hover");
    };
    assert!(
        markup
            .value
            .contains("**Signature:** `chamferBoxEx(x, y, w, h, drawStyle?)`"),
        "{}",
        markup.value
    );

    drop(client_connection);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn keyword_completion_includes_import_and_conditional_controls() {
    let import = keyword_completion_items("imp")
        .into_iter()
        .find(|item| item.label == "import")
        .expect("import keyword");
    assert_eq!(import.kind, Some(CompletionItemKind::KEYWORD));
    assert_eq!(import.insert_text.as_deref(), Some("import { "));
    assert_eq!(
        import.insert_text_format,
        Some(InsertTextFormat::PLAIN_TEXT)
    );

    let stop_labels = keyword_completion_items("sto")
        .into_iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    assert!(stop_labels.iter().any(|label| label == "stopif"));
    assert!(stop_labels.iter().any(|label| label == "stopifn"));
}

#[test]
fn general_completion_includes_user_parameters_and_locals() {
    let root = PathBuf::from("src");
    let path = root.join("client/ui.lux");
    let text = "export fn mount(panel, players) {\n  local selected = players\n  pla\n}\n";
    let analysis = analyze_files(
        AnalysisConfig::new(&root).with_package_id("game"),
        [AnalysisFile {
            path: path.clone(),
            text: text.into(),
        }],
    )
    .expect("analysis");
    let offset = analysis
        .offset_for_position(&path, 2, "  pla".len())
        .expect("offset");
    let file = analysis.file_by_path(&path).expect("analysis file");
    let labels = general_binding_completions(&analysis, &path, offset, file)
        .into_iter()
        .map(|candidate| candidate.label)
        .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label == "players"), "{labels:#?}");
    assert!(
        labels.iter().any(|label| label == "selected"),
        "{labels:#?}"
    );
}

#[test]
fn lexical_completion_survives_incomplete_function_body() {
    let text = "export fn mount(panel, players) {\n  local selected = players\n  pla";
    let file = SourceFile::new(0, None, text);
    let labels = lexical_binding_completions(&file, text.len())
        .into_iter()
        .map(|candidate| candidate.label)
        .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label == "panel"), "{labels:#?}");
    assert!(labels.iter().any(|label| label == "players"), "{labels:#?}");
    assert!(
        labels.iter().any(|label| label == "selected"),
        "{labels:#?}"
    );
}

#[test]
fn lexical_completion_sorts_before_gmod_api_candidates() {
    let local = completion_item(CompletionCandidate {
        label: "players".into(),
        kind: crate::analysis::CompletionCandidateKind::Parameter,
        detail: Some("function parameter".into()),
        documentation: None,
        source: None,
    });
    let api = api_entry_completion_item(ApiIndex::bundled().entry("player").expect("player"));
    assert!(
        local.sort_text.as_deref() < api.sort_text.as_deref(),
        "local sort={:?}, api sort={:?}",
        local.sort_text,
        api.sort_text
    );
}

#[test]
fn lexical_completion_includes_part_imports_without_word_suggestions() {
    let text =
        "import { Button, Column as Stack } from \"@lux/ui\"\nexport fn mount(panel) {\n  Bu";
    let file = SourceFile::new(0, None, text);
    let labels = lexical_binding_completions(&file, text.len())
        .into_iter()
        .map(|candidate| candidate.label)
        .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label == "Button"), "{labels:#?}");
    assert!(labels.iter().any(|label| label == "Stack"), "{labels:#?}");
    assert!(!labels.iter().any(|label| label == "Column"), "{labels:#?}");
}

#[test]
fn gmod_api_completion_items_use_specific_kinds() {
    let api = ApiIndex::bundled();
    let entry = api.entry("player.GetAll").expect("player.GetAll");
    let item = api_entry_completion_item(entry);
    assert_eq!(item.insert_text.as_deref(), Some("GetAll()"));
    assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));

    let entry = api.entry("draw.SimpleText").expect("draw.SimpleText");
    let item = api_entry_completion_item(entry);
    assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
    assert_eq!(
        item.insert_text.as_deref(),
        Some(
            "SimpleText(${1:text}, ${2:font}, ${3:x}, ${4:y}, ${5:color}, ${6:xAlign}, ${7:yAlign})"
        )
    );
    assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
    let doc = completion_documentation_text(&item.documentation);
    assert!(doc.contains("draw.SimpleText"), "{doc}");
    assert!(doc.contains("**Parameters**"), "{doc}");
    assert!(doc.contains("**Returns**"), "{doc}");
    assert!(doc.contains("Official documentation"), "{doc}");

    let entry = api.entry("Player:Nick").expect("Player:Nick");
    let item = api_entry_completion_item(entry);
    assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
    assert!(item.label_details.is_some());
}

#[test]
fn root_api_completion_uses_typed_prefix() {
    let api = ApiIndex::bundled();
    let labels = api_root_completion_candidates(&api, "CreateClient")
        .into_iter()
        .map(|candidate| candidate.label)
        .collect::<Vec<_>>();
    assert!(
        labels.iter().any(|label| label == "CreateClientConVar"),
        "{labels:#?}"
    );
    assert!(
        labels.iter().all(|label| label.starts_with("CreateClient")),
        "{labels:#?}"
    );
}

#[test]
fn api_member_completion_excludes_root_prefix_matches() {
    let api = ApiIndex::bundled();
    let labels = api_completion_candidates(&api, "player.", None)
        .into_iter()
        .map(|candidate| candidate.label)
        .collect::<Vec<_>>();

    assert!(labels.iter().any(|label| label == "GetAll"), "{labels:#?}");
    assert!(!labels.iter().any(|label| label == "player"), "{labels:#?}");
    assert!(
        !labels.iter().any(|label| label == "player_manager"),
        "{labels:#?}"
    );
}

#[test]
fn api_completion_uses_official_class_parent_chain_for_panels() {
    let api = ApiIndex::bundled();
    let text = "local button = vgui.Create(\"DButton\")\nbutton:";
    let labels = api_completion_candidates(&api, "button:", Some(text))
        .into_iter()
        .map(|candidate| candidate.label)
        .collect::<Vec<_>>();
    assert!(
        labels.iter().any(|label| label == "SetImage"),
        "{labels:#?}"
    );
    assert!(labels.iter().any(|label| label == "SetSize"), "{labels:#?}");
}

#[test]
fn signature_help_uses_receiver_type_facts_for_method_calls() {
    let api = ApiIndex::bundled();
    let file = SourceFile::new(0, None, "local ply = LocalPlayer()\nply:Nick(");
    let help = signature_help_at(&file, &api, file.text.len()).expect("signature help");
    assert_eq!(help.signatures[0].label, "Player:Nick()");
}

#[test]
fn signature_help_uses_official_parent_chain_for_panel_methods() {
    let api = ApiIndex::bundled();
    let file = SourceFile::new(
        0,
        None,
        "local button = vgui.Create(\"DButton\")\nbutton:SetSize(",
    );
    let help = signature_help_at(&file, &api, file.text.len()).expect("signature help");
    assert_eq!(help.signatures[0].label, "Panel:SetSize(width, height)");
}

#[test]
fn hover_method_path_uses_receiver_type_facts() {
    let api = ApiIndex::bundled();
    let text = "local ply = LocalPlayer()\nply:Nick()";
    let offset = text.find("Nick").expect("offset");
    let path = method_path_at_offset(text, offset).expect("method path");
    let facts = GmodTypeFacts::from_text(text);
    assert_eq!(path, "ply:Nick");
    assert_eq!(
        resolve_typed_method_path(&api, &facts, &path),
        Some("Player:Nick".into())
    );
}

#[test]
fn hover_method_path_uses_official_parent_chain_for_panels() {
    let api = ApiIndex::bundled();
    let text = "local button = vgui.Create(\"DButton\")\nbutton:SetSize(24, 24)";
    let offset = text.find("SetSize").expect("offset");
    let path = method_path_at_offset(text, offset).expect("method path");
    let facts = GmodTypeFacts::from_text(text);
    assert_eq!(path, "button:SetSize");
    assert_eq!(
        resolve_typed_method_path(&api, &facts, &path),
        Some("Panel:SetSize".into())
    );
}

#[test]
fn semantic_tokens_are_sorted_and_delta_encoded() {
    let file = SourceFile::new(0, None, "fn run()\n  local value = 1\n");
    let tokens = vec![
        AnalysisSemanticToken {
            span: SourceSpan::new(file.id, 11, 16),
            kind: SemanticTokenKind::Keyword,
        },
        AnalysisSemanticToken {
            span: SourceSpan::new(file.id, 3, 6),
            kind: SemanticTokenKind::Function,
        },
    ];

    let encoded = encode_semantic_tokens(&file, tokens);
    assert_eq!(
        encoded,
        vec![
            SemanticToken {
                delta_line: 0,
                delta_start: 3,
                length: 3,
                token_type: 2,
                token_modifiers_bitset: 0,
            },
            SemanticToken {
                delta_line: 1,
                delta_start: 2,
                length: 5,
                token_type: 0,
                token_modifiers_bitset: 0,
            },
        ]
    );
}

#[test]
fn semantic_tokens_prefer_non_overlapping_tokens_on_the_same_line() {
    let file = SourceFile::new(0, None, "local name = self?:GetName() ?? \"panel\"\n");
    let tokens = vec![
        AnalysisSemanticToken {
            span: SourceSpan::new(file.id, 0, 5),
            kind: SemanticTokenKind::Keyword,
        },
        AnalysisSemanticToken {
            span: SourceSpan::new(file.id, 6, 10),
            kind: SemanticTokenKind::Variable,
        },
        AnalysisSemanticToken {
            span: SourceSpan::new(file.id, 13, 17),
            kind: SemanticTokenKind::Variable,
        },
        AnalysisSemanticToken {
            span: SourceSpan::new(file.id, 13, 28),
            kind: SemanticTokenKind::External,
        },
        AnalysisSemanticToken {
            span: SourceSpan::new(file.id, 29, 31),
            kind: SemanticTokenKind::Operator,
        },
        AnalysisSemanticToken {
            span: SourceSpan::new(file.id, 32, 39),
            kind: SemanticTokenKind::String,
        },
    ];

    let encoded = encode_semantic_tokens(&file, tokens);
    assert_eq!(
        encoded,
        vec![
            SemanticToken {
                delta_line: 0,
                delta_start: 0,
                length: 5,
                token_type: 0,
                token_modifiers_bitset: 0,
            },
            SemanticToken {
                delta_line: 0,
                delta_start: 6,
                length: 4,
                token_type: 4,
                token_modifiers_bitset: 0,
            },
            SemanticToken {
                delta_line: 0,
                delta_start: 7,
                length: 4,
                token_type: 4,
                token_modifiers_bitset: 0,
            },
            SemanticToken {
                delta_line: 0,
                delta_start: 16,
                length: 2,
                token_type: 11,
                token_modifiers_bitset: 0,
            },
            SemanticToken {
                delta_line: 0,
                delta_start: 3,
                length: 7,
                token_type: 8,
                token_modifiers_bitset: 0,
            },
        ]
    );
}

#[test]
fn semantic_tokens_flush_pending_analysis_before_reading() {
    let root = temp_root("semantic_flush");
    std::fs::create_dir_all(&root).expect("root");
    let source = root.join("module.lux");
    std::fs::write(&source, "fn run() = 1\n").expect("source");
    let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
        "processId": null,
        "rootUri": path_to_url(&root).expect("root uri"),
        "capabilities": {}
    }))
    .expect("initialize params");
    let (server_connection, client_connection) = lsp_server::Connection::memory();
    let mut server = Server::new(server_connection, initialize);
    server.analysis_due = Some(std::time::Instant::now() + Duration::from_secs(60));
    server.documents.insert(
        path_to_url(&source).expect("source uri"),
        std::fs::read_to_string(&source).expect("source text"),
    );

    let params = lsp_types::SemanticTokensParams {
        text_document: lsp_types::TextDocumentIdentifier {
            uri: path_to_url(&source).expect("source uri"),
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let _ = server.semantic_tokens(params).expect("semantic tokens");
    assert!(server.analysis_due.is_none());
    drop(client_connection);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn file_uri_round_trip_preserves_paths() {
    let path = std::env::current_dir()
        .expect("cwd")
        .join("src")
        .join("module.lux");
    let uri = path_to_url(&path).expect("file uri");
    let round_tripped = url_to_path(&uri).expect("path");
    assert_eq!(round_tripped, path);
}

#[test]
fn document_uri_key_normalizes_encoded_windows_drive_uris() {
    if !cfg!(windows) {
        return;
    }
    let encoded: lsp_types::Uri =
        "file:///c%3A/Development/gmod/lux/examples/gmod_project/src/client/ui.lux"
            .parse()
            .expect("encoded uri");
    let canonical: lsp_types::Uri =
        "file:///C:/Development/gmod/lux/examples/gmod_project/src/client/ui.lux"
            .parse()
            .expect("canonical uri");
    assert_eq!(document_uri_key(&encoded), document_uri_key(&canonical));
}

#[test]
fn command_document_position_accepts_camel_case_arguments() {
    let uri =
        path_to_url(&std::env::current_dir().expect("cwd").join("src/module.lux")).expect("uri");
    let value = serde_json::json!({
        "uri": uri,
        "line": 2,
        "character": 4
    });
    let parsed = CommandDocumentPosition::from_arguments(&[value])
        .expect("valid args")
        .expect("position");
    assert_eq!(parsed.line, Some(2));
    assert_eq!(parsed.character, Some(4));
}

#[test]
fn command_results_use_analysis_for_exports_and_realm() {
    let root = std::env::temp_dir().join(format!(
        "lux_lsp_command_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("root");
    let source = root.join("module.lux");
    std::fs::write(
        &source,
        "client fn paint() = 1\nserver fn grant() = 2\nexport client { paint }\n",
    )
    .expect("source");
    let workspace =
        AnalysisWorkspace::load(AnalysisConfig::new(&root), Vec::new()).expect("analysis");
    let analysis = workspace.analysis();
    let uri = path_to_url(&source).expect("uri");
    let position = CommandDocumentPosition {
        uri,
        line: Some(0),
        character: Some(3),
    };

    let exports = module_exports_command(analysis, Some(&position));
    assert_eq!(exports.kind, "moduleExports");
    assert!(exports.items.iter().any(|item| item.label == "paint"));

    let realm = active_realm_command(analysis, Some(&position));
    assert_eq!(realm.kind, "activeRealm");
    assert_eq!(realm.items[0].label, "client");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn gmod_api_coverage_command_reports_full_official_docs() {
    let api = ApiIndex::bundled();
    let result = gmod_api_coverage_command(&api);
    assert_eq!(result.kind, "gmodApiCoverage");
    assert!(result.markdown.contains("Official pages"));
    assert!(
        result
            .items
            .iter()
            .any(|item| item.label == "Document records")
    );
}

fn completion_documentation_text(documentation: &Option<Documentation>) -> String {
    match documentation {
        Some(Documentation::MarkupContent(markup)) => markup.value.clone(),
        Some(Documentation::String(value)) => value.clone(),
        None => String::new(),
    }
}
