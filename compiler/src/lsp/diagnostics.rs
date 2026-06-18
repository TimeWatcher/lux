use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::analysis::{AnalysisCodeAction, AnalysisDiagnostic, AnalysisEditKind, ProjectAnalysis};
use crate::diag::Severity;
use gmod_api_db::ApiIndex;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, DiagnosticRelatedInformation,
    DiagnosticSeverity, Location, Position, Range, TextEdit, Uri,
};
use url::Url;

use super::protocol::{INSTALL_STD_PACKAGES_COMMAND, lsp_range, path_to_url, uri_from_url};
use super::workspace::find_manifest_for_path;

pub(crate) fn manifest_extern_code_actions(
    analysis: &ProjectAnalysis,
    path: &Path,
    root: &Path,
) -> Vec<CodeActionOrCommand> {
    let Some(manifest_path) = find_manifest_for_path(root, path) else {
        return Vec::new();
    };
    analysis
        .diagnostics_for_path(path)
        .into_iter()
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("REALM_UNKNOWN"))
        .filter_map(|diagnostic| diagnostic_symbol_name(&diagnostic.message))
        .flat_map(|symbol| {
            ["shared", "client", "server"]
                .into_iter()
                .map(move |realm| (symbol.clone(), realm))
        })
        .filter_map(|(symbol, realm)| {
            let uri = path_to_url(&manifest_path)?;
            let edit = manifest_extern_edit(&manifest_path, &symbol, realm);
            let mut changes = HashMap::<Uri, Vec<TextEdit>>::new();
            changes.insert(uri, vec![edit]);
            Some(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Add package extern {realm} {symbol}"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: None,
                edit: Some(lsp_types::WorkspaceEdit {
                    changes: Some(changes),
                    document_changes: None,
                    change_annotations: None,
                }),
                command: None,
                is_preferred: None,
                disabled: None,
                data: None,
            }))
        })
        .collect()
}

pub(crate) fn std_package_code_actions(
    analysis: &ProjectAnalysis,
    path: &Path,
    root: &Path,
    _uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let Some(manifest_path) = find_manifest_for_path(root, path) else {
        return Vec::new();
    };
    let Some(project_root) = manifest_path.parent() else {
        return Vec::new();
    };
    let packages = analysis
        .diagnostics_for_path(path)
        .into_iter()
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("MODULE001"))
        .filter_map(|diagnostic| diagnostic_symbol_name(&diagnostic.message))
        .filter(|module| is_official_lux_package(module))
        .collect::<BTreeSet<_>>();
    if packages.is_empty() {
        return Vec::new();
    }
    let packages = packages.into_iter().collect::<Vec<_>>();
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: "Fix: Install std packages".into(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: None,
        edit: None,
        command: Some(lsp_types::Command {
            title: "Fix: Install std packages".into(),
            command: INSTALL_STD_PACKAGES_COMMAND.into(),
            arguments: Some(vec![serde_json::json!({
                "projectRoot": project_root,
                "packages": packages,
            })]),
        }),
        is_preferred: None,
        disabled: None,
        data: None,
    })]
}

pub(crate) fn is_official_lux_package(package: &str) -> bool {
    matches!(
        package,
        "@lux/std"
            | "@lux/reactive"
            | "@lux/gmod"
            | "@lux/gmod/macros"
            | "@lux/ui"
            | "@lux/macros"
            | "@lux/compile/macro"
            | "@lux/compile/host"
    )
}

fn manifest_extern_edit(manifest_path: &Path, symbol: &str, realm: &str) -> TextEdit {
    let text = std::fs::read_to_string(manifest_path).unwrap_or_default();
    let escaped_symbol = symbol.replace('\\', "\\\\").replace('"', "\\\"");
    let new_entry = format!("{escaped_symbol} = \"{realm}\"\n");
    if let Some((line, character)) = manifest_section_insert_position(&text, "target.gmod.extern") {
        TextEdit {
            range: Range {
                start: Position { line, character },
                end: Position { line, character },
            },
            new_text: new_entry,
        }
    } else {
        let prefix = if text.trim().is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        TextEdit {
            range: end_of_document_range(&text),
            new_text: format!("{prefix}\n[target.gmod.extern]\n{new_entry}"),
        }
    }
}

pub(crate) fn manifest_section_insert_position(text: &str, section: &str) -> Option<(u32, u32)> {
    let mut in_section = false;
    let mut insert_line = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_section {
                return Some((index as u32, 0));
            }
            in_section = trimmed == format!("[{section}]");
        } else if in_section && !trimmed.is_empty() {
            insert_line = Some(index + 1);
        }
    }
    in_section.then_some((
        insert_line.unwrap_or_else(|| text.lines().count()) as u32,
        0,
    ))
}

fn end_of_document_range(text: &str) -> Range {
    let line_count = text.lines().count();
    let last_line_len = text.lines().last().map(utf16_len).unwrap_or(0);
    let line = if text.ends_with('\n') {
        line_count as u32
    } else {
        line_count.saturating_sub(1) as u32
    };
    let character = if text.ends_with('\n') {
        0
    } else {
        last_line_len as u32
    };
    Range {
        start: Position { line, character },
        end: Position { line, character },
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub(crate) fn lsp_diagnostic(diagnostic: AnalysisDiagnostic) -> Diagnostic {
    Diagnostic {
        range: lsp_range(diagnostic.range),
        severity: Some(match diagnostic.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Note => DiagnosticSeverity::INFORMATION,
        }),
        code: diagnostic.code.map(lsp_types::NumberOrString::String),
        code_description: None,
        source: Some("luxc".into()),
        message: diagnostic.message,
        related_information: if diagnostic.notes.is_empty() && diagnostic.help.is_none() {
            None
        } else {
            Some(
                diagnostic
                    .notes
                    .into_iter()
                    .chain(diagnostic.help)
                    .map(|message| DiagnosticRelatedInformation {
                        location: Location {
                            uri: path_to_url(&diagnostic.path)
                                .unwrap_or_else(|| uri_from_url(Url::parse("file:///").unwrap())),
                            range: lsp_range(diagnostic.range),
                        },
                        message,
                    })
                    .collect(),
            )
        },
        tags: None,
        data: None,
    }
}

pub(crate) fn should_publish_diagnostic(
    diagnostic: &AnalysisDiagnostic,
    document_text: &str,
    is_open: bool,
    suppress_parse_cascade: bool,
) -> bool {
    if !is_open || diagnostic.severity != Severity::Error {
        return true;
    }
    let Some(code) = diagnostic.code.as_deref() else {
        return true;
    };
    if !code.starts_with("PARSE") {
        return true;
    }
    if suppress_parse_cascade {
        return false;
    }
    !is_transient_parse_diagnostic(diagnostic, document_text)
}

fn is_transient_parse_diagnostic(diagnostic: &AnalysisDiagnostic, document_text: &str) -> bool {
    if is_transient_import_parse_diagnostic(diagnostic, document_text) {
        return true;
    }
    if is_position_at_document_end(document_text, diagnostic.range.start) {
        return true;
    }
    if diagnostic.code.as_deref() != Some("PARSE005") {
        return false;
    }
    let start = diagnostic.range.start;
    let Some(line) = line_at(document_text, start.line) else {
        return false;
    };
    let prefix = line_prefix_utf16(line, start.character).trim_end();
    prefix.is_empty()
        || prefix.ends_with('{')
        || prefix.ends_with('(')
        || prefix.ends_with('[')
        || prefix.ends_with(',')
        || prefix.ends_with('.')
        || prefix.ends_with(':')
        || prefix.ends_with(" import")
        || prefix.ends_with(" export")
        || prefix.ends_with(" from")
        || prefix.ends_with(" as")
}

pub(crate) fn is_transient_import_parse_diagnostic(
    diagnostic: &AnalysisDiagnostic,
    document_text: &str,
) -> bool {
    let Some(code) = diagnostic.code.as_deref() else {
        return false;
    };
    if !matches!(code, "PARSE001" | "PARSE005" | "PARSE006" | "PARSE007") {
        return false;
    }
    let start = diagnostic.range.start;
    let Some(line) = line_at(document_text, start.line) else {
        return false;
    };
    let trimmed = line.trim_start();
    if trimmed.starts_with("import ") || trimmed == "import" {
        return true;
    }

    let mut previous_line = start.line;
    while previous_line > 0 {
        previous_line -= 1;
        let Some(previous) = line_at(document_text, previous_line) else {
            break;
        };
        let previous = previous.trim();
        if previous.is_empty() {
            continue;
        }
        return previous.starts_with("import ")
            && !previous.contains('\n')
            && (previous.contains('{') || previous.contains(" from "));
    }
    false
}

fn is_position_at_document_end(
    document_text: &str,
    position: crate::analysis::AnalysisPosition,
) -> bool {
    let end = end_position_utf16(document_text);
    position.line > end.line || (position.line == end.line && position.character >= end.character)
}

fn end_position_utf16(text: &str) -> crate::analysis::AnalysisPosition {
    let range = end_of_document_range(text);
    crate::analysis::AnalysisPosition {
        line: range.start.line,
        character: range.start.character,
    }
}

fn line_at(text: &str, line: u32) -> Option<&str> {
    text.split('\n')
        .nth(line as usize)
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

fn line_prefix_utf16(line: &str, character: u32) -> &str {
    if character == 0 {
        return "";
    }
    let mut utf16 = 0u32;
    for (index, ch) in line.char_indices() {
        if utf16 >= character {
            return &line[..index];
        }
        utf16 += ch.len_utf16() as u32;
    }
    line
}

pub(crate) fn code_action(action: AnalysisCodeAction, uri: &Uri) -> CodeActionOrCommand {
    let diagnostics = action
        .diagnostics
        .iter()
        .map(|code| Diagnostic {
            range: Range::default(),
            severity: None,
            code: Some(lsp_types::NumberOrString::String(code.clone())),
            code_description: None,
            source: Some("luxc".into()),
            message: code.clone(),
            related_information: None,
            tags: None,
            data: None,
        })
        .collect::<Vec<_>>();
    let mut changes = HashMap::<Uri, Vec<TextEdit>>::new();
    for edit in action.edits {
        let edit_uri = path_to_url(&edit.path).unwrap_or_else(|| uri.clone());
        changes.entry(edit_uri).or_default().push(TextEdit {
            range: lsp_range(edit.range),
            new_text: edit.new_text,
        });
    }
    CodeActionOrCommand::CodeAction(CodeAction {
        title: action.title,
        kind: Some(match action.kind {
            AnalysisEditKind::Safe => CodeActionKind::QUICKFIX,
            AnalysisEditKind::Guided => CodeActionKind::QUICKFIX,
            AnalysisEditKind::Refactor => CodeActionKind::REFACTOR,
        }),
        diagnostics: Some(diagnostics),
        edit: (!changes.is_empty()).then_some(lsp_types::WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: action.command.map(|command| lsp_types::Command {
            title: command.clone(),
            command,
            arguments: None,
        }),
        is_preferred: None,
        disabled: None,
        data: None,
    })
}

pub(crate) fn api_doc_code_actions(
    analysis: &ProjectAnalysis,
    api: &ApiIndex,
    path: &Path,
    _uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    analysis
        .diagnostics_for_path(path)
        .into_iter()
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("REALM001"))
        .filter_map(|diagnostic| diagnostic_symbol_name(&diagnostic.message))
        .filter_map(|symbol| {
            api.entry(&symbol)
                .and_then(|entry| entry.official_url.as_ref())
        })
        .map(|url| {
            CodeActionOrCommand::CodeAction(CodeAction {
                title: "Open official GMod documentation".into(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: None,
                edit: None,
                command: Some(lsp_types::Command {
                    title: "Open official GMod documentation".into(),
                    command: "lux.openGmodDocs".into(),
                    arguments: Some(vec![serde_json::Value::String(url.clone())]),
                }),
                is_preferred: None,
                disabled: None,
                data: None,
            })
        })
        .collect()
}

fn diagnostic_symbol_name(message: &str) -> Option<String> {
    let start = message.find('`')? + 1;
    let end = message[start..].find('`')? + start;
    Some(message[start..end].to_string())
}
