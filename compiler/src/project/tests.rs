use super::{
    GmodBuildOptions, ProjectConfig, ProjectError, build_gmod_project, compile_paths,
    infer_module_path,
};
use crate::ast::Realm;
use crate::module::{ArtifactRealm, ModuleId};
use crate::resolve::{ExternSymbol, ResolverOptions};
use crate::test_support::test_std_package_root;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_project(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("lux_project_test_{name}_{unique}"))
}

fn write_lux(path: &Path, source: &str) {
    fs::create_dir_all(path.parent().expect("source parent")).expect("create source dir");
    fs::write(path, source).expect("write lux source");
}

#[test]
fn realm_facet_directories_are_not_modules() {
    let root = temp_project("realm_facets");
    let source_root = root.join("src");

    assert_eq!(
        infer_module_path(&source_root, &source_root.join("client/ui.lux")),
        "ui"
    );
    assert_eq!(
        infer_module_path(&source_root, &source_root.join("server/init.lux")),
        "init"
    );
    assert_eq!(
        infer_module_path(&source_root, &source_root.join("shared/hud.lux")),
        "hud"
    );
    assert_eq!(
        infer_module_path(&source_root, &source_root.join("shared/hud/math.lux")),
        "hud"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn directory_module_parts_compile_as_one_logical_module() {
    let root = temp_project("parts");
    let source_root = root.join("src");
    let shared = source_root.join("inventory/module.lux");
    let client = source_root.join("inventory/cl_view.lux");
    let server = source_root.join("inventory/sv_state.lux");
    write_lux(
        &shared,
        "local prefix = \"inv\"\nfn label(kind) = prefix .. \":\" .. kind",
    );
    write_lux(
        &client,
        "export client fn clientLabel() = label(\"client\")",
    );
    write_lux(
        &server,
        "export server fn serverLabel() = label(\"server\")",
    );

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let output = compile_paths(&config, &[shared.clone(), client.clone(), server.clone()])
        .expect("compile project");

    assert!(
        output
            .graph
            .node(&ModuleId::new("game/inventory"))
            .is_some()
    );
    assert_eq!(output.modules.len(), 2);

    let client_module = output
        .modules
        .iter()
        .find(|module| module.artifact_realm == ArtifactRealm::Client)
        .expect("client artifact");
    assert_eq!(client_module.artifact_id, "game/inventory#client");
    assert_eq!(client_module.source_files.len(), 3);
    assert!(client_module.lua.lua.contains("__lux_exports.clientLabel"));
    assert!(!client_module.lua.lua.contains("__lux_exports.serverLabel"));

    let server_module = output
        .modules
        .iter()
        .find(|module| module.artifact_realm == ArtifactRealm::Server)
        .expect("server artifact");
    assert_eq!(server_module.artifact_id, "game/inventory#server");
    assert!(server_module.lua.lua.contains("__lux_exports.serverLabel"));
    assert!(!server_module.lua.lua.contains("__lux_exports.clientLabel"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn multipart_runtime_enum_lift_uses_assignment_not_local_member_decl() {
    let root = temp_project("runtime_enum_lift");
    let source_root = root.join("src");
    let entry = source_root.join("shop/base/module.lux");
    let state = source_root.join("shop/base/cl_state.lux");
    let mut entry_text =
        String::from("part order { \"module\", \"cl_state\" }\nexport { ShopPanelKey }\n");
    for index in 0..170 {
        entry_text.push_str(&format!("local binding{index} = {index}\n"));
    }
    write_lux(&entry, &entry_text);
    write_lux(
        &state,
        "enum ShopPanelKey repr string runtime {\n  Arsenal = \"arsenal\",\n  Worth = \"worth\",\n  Field = \"field\",\n  Remantler = \"remantler\"\n}",
    );

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let output = compile_paths(&config, &[entry.clone(), state.clone()]).expect("compile");
    let module = output
        .modules
        .iter()
        .find(|module| module.artifact_realm == ArtifactRealm::Client)
        .expect("client artifact");

    assert!(
        module.lua.lua.contains("__lux_module_1.ShopPanelKey = {"),
        "{}",
        module.lua.lua
    );
    assert!(
        !module.lua.lua.contains("local __lux_module_1.ShopPanelKey"),
        "{}",
        module.lua.lua
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn lifted_method_decl_uses_module_local_receiver() {
    let root = temp_project("lifted_method");
    let source_root = root.join("src");
    let source = source_root.join("client/ui.lux");

    let mut text = String::new();
    text.push_str("local PANEL = {}\n");
    for index in 0..170 {
        text.push_str(&format!("local binding{index} = {index}\n"));
    }
    text.push_str("fn PANEL:Paint(w, h) {\n  drawBody(self, w, h);\n}\n");
    text.push_str("export client fn panel() = PANEL\n");
    write_lux(&source, &text);

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let output = compile_paths(&config, std::slice::from_ref(&source)).expect("compile");
    let module = output
        .modules
        .iter()
        .find(|module| module.artifact_realm == ArtifactRealm::Client)
        .expect("client artifact");

    assert!(
        module
            .lua
            .lua
            .contains("function __lux_module_1.PANEL:Paint(w, h)"),
        "{}",
        module.lua.lua
    );
    assert!(
        !module.lua.lua.contains("function PANEL:Paint"),
        "{}",
        module.lua.lua
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn module_part_order_reports_use_before_initialization() {
    let root = temp_project("order");
    let source_root = root.join("src");
    let entry = source_root.join("inventory/module.lux");
    let first = source_root.join("inventory/a_first.lux");
    let second = source_root.join("inventory/z_second.lux");
    write_lux(&entry, "");
    write_lux(&first, "local current = later");
    write_lux(&second, "local later = 1");

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let error = compile_paths(&config, &[entry.clone(), first.clone(), second.clone()])
        .expect_err("use before init should fail");
    let ProjectError::Diagnostics(diagnostics) = error else {
        panic!("expected diagnostics, got {error:?}");
    };
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("RESOLVE012")),
        "{diagnostics:#?}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn part_order_list_controls_module_initialization_order() {
    let root = temp_project("order_list");
    let source_root = root.join("src");
    let entry = source_root.join("inventory/module.lux");
    let first_by_path = source_root.join("inventory/later.lux");
    let second_by_path = source_root.join("inventory/setup.lux");
    write_lux(&entry, "part order { \"module\", \"setup\", \"later\" }");
    write_lux(
        &first_by_path,
        "local current = later\nexport fn value() = current",
    );
    write_lux(&second_by_path, "local later = 1");

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let output = compile_paths(
        &config,
        &[entry.clone(), first_by_path.clone(), second_by_path.clone()],
    )
    .expect("part order should make initialization valid");
    let module = output
        .modules
        .iter()
        .find(|module| module.artifact_realm == ArtifactRealm::Server)
        .expect("server artifact");
    let later_pos = module.lua.lua.find("later = 1").expect("later assignment");
    let current_pos = module
        .lua
        .lua
        .find("current = later")
        .expect("current assignment");
    assert!(later_pos < current_pos, "{}", module.lua.lua);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn top_level_local_without_initializer_does_not_emit_empty_assignment() {
    let root = temp_project("empty_local");
    let source_root = root.join("src");
    let source = source_root.join("cl_cache.lux");
    write_lux(
        &source,
        "local cache\nexport client fn getCache() {\n  if cache { return cache }\n  cache = {}\n  cache\n}",
    );

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let output = compile_paths(&config, std::slice::from_ref(&source)).expect("compile");
    let module = output
        .modules
        .iter()
        .find(|module| module.artifact_realm == ArtifactRealm::Client)
        .expect("client artifact");

    assert!(!module.lua.lua.contains("cache = \n"), "{}", module.lua.lua);
    assert!(module.lua.lua.contains("local cache"), "{}", module.lua.lua);
    assert!(module.lua.lua.contains("cache = {}"), "{}", module.lua.lua);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn top_level_destructure_initializes_module_binding_across_parts() {
    let root = temp_project("destructure_module_binding");
    let source_root = root.join("src");
    let entry = source_root.join("inventory/module.lux");
    let state = source_root.join("inventory/state.lux");
    let reader = source_root.join("inventory/read.lux");

    let mut entry_text = String::from("part order { \"module\", \"state\", \"read\" }\n");
    entry_text.push_str("export { readName }\n");
    for index in 0..170 {
        entry_text.push_str(&format!("local binding{index} = {index}\n"));
    }

    write_lux(&entry, &entry_text);
    write_lux(
        &state,
        "local { item: { name }, ignored: _ }, [firstCount = 1] = { item = { name = \"kit\" }, ignored = true }, {}\nconst { worth } = { worth = 12 }",
    );
    write_lux(
        &reader,
        "fn readName() = name .. \":\" .. firstCount .. \":\" .. worth",
    );

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let output =
        compile_paths(&config, &[entry.clone(), state.clone(), reader.clone()]).expect("compile");
    let module = output
        .modules
        .iter()
        .find(|module| module.artifact_realm == ArtifactRealm::Server)
        .expect("server artifact");
    let lua = &module.lua.lua;

    assert!(
        lua.contains("__lux_module_1.name, __lux_module_1.firstCount ="),
        "{lua}"
    );
    assert!(lua.contains("__lux_module_1.worth ="), "{lua}");
    assert!(lua.contains("return __lux_module_1.name"), "{lua}");
    assert!(!lua.contains("local __lux_module_1.name"), "{lua}");
    assert!(!lua.contains("__lux_module_1._"), "{lua}");
    assert!(!lua.contains("local name"), "{lua}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn multi_part_module_requires_entry_part() {
    let root = temp_project("missing_entry");
    let source_root = root.join("src");
    let first = source_root.join("inventory/a.lux");
    let second = source_root.join("inventory/b.lux");
    write_lux(&first, "fn a() = 1");
    write_lux(&second, "fn b() = a()");

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let error = compile_paths(&config, &[first.clone(), second.clone()])
        .expect_err("missing module entry should fail");
    let ProjectError::Diagnostics(diagnostics) = error else {
        panic!("expected diagnostics, got {error:?}");
    };
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("PART007")),
        "{diagnostics:#?}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_part_order_must_live_in_entry_part() {
    let root = temp_project("order_location");
    let source_root = root.join("src");
    let entry = source_root.join("inventory/module.lux");
    let part = source_root.join("inventory/logic.lux");
    write_lux(&entry, "");
    write_lux(
        &part,
        "part order { \"module\", \"logic\" }\nfn logic() = 1",
    );

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let error = compile_paths(&config, &[entry.clone(), part.clone()])
        .expect_err("part order outside entry should fail");
    let ProjectError::Diagnostics(diagnostics) = error else {
        panic!("expected diagnostics, got {error:?}");
    };
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("PART006")),
        "{diagnostics:#?}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn gmod_project_surfaces_unknown_external_warnings() {
    let root = temp_project("unknown_external");
    let source_root = root.join("src");
    let source = source_root.join("inventory/module.lux");
    write_lux(&source, "fn run() = ThirdPartyAddon.DoThing()");

    let config = ProjectConfig::new(&source_root).with_package_id("game");
    let output = compile_paths(&config, std::slice::from_ref(&source)).expect("compile");
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("REALM_UNKNOWN")),
        "{:#?}",
        output.diagnostics
    );

    let config = ProjectConfig::new(&source_root)
        .with_package_id("game")
        .with_resolver_options(ResolverOptions::gmod_default().with_externs([
            ExternSymbol::known("ThirdPartyAddon.DoThing", Realm::Shared),
        ]));
    let output = compile_paths(&config, std::slice::from_ref(&source)).expect("compile");
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn gmod_loader_keeps_runtime_dependencies_before_dependents() {
    let root = temp_project("runtime_loader_order");
    let std_root = test_std_package_root();
    let source_root = root.join("src");
    let output_root = root.join("generated");
    let source = source_root.join("cl_ui_test.lux");
    write_lux(
        &source,
        "import { mount, node } from \"@lux/ui\"\nmount(() => node(\"Label\", {}, {}), (tree) => tree)",
    );

    let mut options = GmodBuildOptions::new(&source_root, &output_root);
    options.bundle_id = Some("runtime_order".into());
    options.package_roots = vec![std_root.clone()];
    let output = build_gmod_project(&options).expect("build gmod project");
    let lua = output
        .build_plan
        .loader
        .client_loader
        .render(&output.build_plan.registry);
    let reactive_pos = lua
        .find("include(\"lux/runtime_order/client/runtime/lux/reactive.lua\")")
        .expect("reactive runtime include");
    let ui_pos = lua
        .find("include(\"lux/runtime_order/client/runtime/lux/ui.lua\")")
        .expect("ui runtime include");
    let entry_pos = lua.find("/ui_test.lua\")").expect("project entry include");

    assert!(reactive_pos < ui_pos, "{lua}");
    assert!(ui_pos < entry_pos, "{lua}");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(std_root);
}

#[test]
fn gmod_build_can_disable_autorun_without_disabling_loader() {
    let root = temp_project("manual_entry");
    let source_root = root.join("src");
    let output_root = root.join("generated");
    let source = source_root.join("cl_ui.lux");
    write_lux(&source, "export client fn main() = true");

    let mut options = GmodBuildOptions::new(&source_root, &output_root);
    options.bundle_id = Some("demo".into());
    options.runtime_base = Some(PathBuf::from("framework/lux/demo"));
    options.autorun = false;
    options.write_files = true;

    let output = build_gmod_project(&options).expect("build gmod project");

    assert!(output.build_plan.autorun.is_none());
    assert!(
        output_root
            .join("framework/lux/demo/loader_shared.lua")
            .is_file()
    );
    assert!(
        output_root
            .join("framework/lux/demo/loader_client.lua")
            .is_file()
    );
    assert!(!output_root.join("autorun/demo.lua").exists());
    assert!(output.artifacts.iter().any(|artifact| {
        artifact
            .lua_path
            .starts_with(output_root.join("framework/lux/demo"))
    }));

    let _ = fs::remove_dir_all(root);
}
