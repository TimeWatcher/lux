use crate::ast::{ExprKind, Realm, StmtKind};
use crate::codegen::LuaCodegen;
use crate::host::HostRegistry;
use crate::lex::Lexer;
use crate::lower::Lowerer;
use crate::macro_expansion::expand_macros_with_registry;
use crate::parse::Parser;
use crate::pipeline::parse_expand_resolve;
use crate::source::SourceFile;
use crate::test_support::test_std_package_root;

use super::CompileTimePackageRegistry;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn test_registry() -> (CompileTimePackageRegistry, std::path::PathBuf) {
    let root = test_std_package_root();
    let registry = CompileTimePackageRegistry::load_default_with_package_roots(&[root.clone()])
        .expect("compile-time registry");
    (registry, root)
}

#[test]
fn loads_lux_macro_modules() {
    let (registry, root) = test_registry();
    let mut macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(
        0,
        None,
        "import macro { dbg } from \"lux/macros\"\nlocal x = dbg(1)",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &macros);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:#?}",
        expanded.diagnostics
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compile_time_packages_can_import_lux_helpers() {
    let (registry, root) = test_registry();
    let mut macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(
        0,
        None,
        "import macro { defineNetReceiver } from \"lux/gmod/macros\"\ndefineNetReceiver(\"x\", () => nil)",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &macros);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:#?}",
        expanded.diagnostics
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compile_time_macro_helpers_cache_many_locals() {
    let (registry, root) = test_registry();
    let mut macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(
        0,
        None,
        "import macro { defineServerNetReceiver } from \"lux/gmod/macros\"\ndefineServerNetReceiver(makeName(), makeCallback())",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &macros);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:#?}",
        expanded.diagnostics
    );

    let StmtKind::RealmBlock { realm, .. } = &expanded.module.body[1].kind else {
        panic!(
            "expected server realm block, got {:#?}",
            expanded.module.body[1]
        );
    };
    assert_eq!(*realm, Realm::Server);

    let resolved = crate::resolve::Resolver::resolve(&expanded.module);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:#?}",
        resolved.diagnostics
    );
    let ir = Lowerer::lower(&expanded.module, &resolved).expect("lower");
    let lua = LuaCodegen::generate(&ir).expect("codegen").lua;
    let make_name = lua.find("makeName()").expect("name call");
    let make_callback = lua.find("makeCallback()").expect("callback call");
    let add_string = lua
        .find("util.AddNetworkString(")
        .expect("net string registration");
    let receive = lua.find("net.Receive(").expect("receiver registration");
    assert!(make_name < make_callback, "{lua}");
    assert!(make_callback < add_string, "{lua}");
    assert!(add_string < receive, "{lua}");
    assert!(!lua.contains("if SERVER then"), "{lua}");
    assert!(lua.contains("util.AddNetworkString(__lux_macro_net_name_"));
    assert!(lua.contains("net.Receive(__lux_macro_net_name_"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn macro_helpers_generate_exported_function_declarations() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lux_compile_time_ast_test_{unique}"));
    let package = root.join("project");
    fs::create_dir_all(package.join("compiletime")).expect("create temp package root");
    fs::write(
            package.join("compiletime/module.lux"),
            "import * as m from \"lux/compile/macro\"\n\
             export macro fn generatedCommand(ctx, call) {\n\
               if not m.expectArgCount(ctx, call, 0, \"`generatedCommand` expects no arguments\") {\n\
                 return nil\n\
               }\n\
               local command = m.ident(\"command\", call.span)\n\
               local body = m.block({\n\
                 m.returnOne(m.index(command, m.number(2, call.span), call.span), call.span)\n\
               }, nil, call.span)\n\
               return m.stmts({\n\
                 m.exportRuntime(\"client\", m.fnDecl(\"generated\", { \"command\" }, body, call.span), call.span)\n\
               })\n\
             }\n",
        )
        .expect("write temp compile-time package");

    let std_root = test_std_package_root();
    let registry = CompileTimePackageRegistry::load_default_with_package_roots(&[
        std_root.clone(),
        root.clone(),
    ])
    .expect("compile-time registry");
    let mut macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(
        0,
        None,
        "import macro { generatedCommand } from \"project\"\ngeneratedCommand()",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &macros);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:#?}",
        expanded.diagnostics
    );

    let StmtKind::ExportDecl {
        realm, stmt: inner, ..
    } = &expanded.module.body[1].kind
    else {
        panic!(
            "expected exported function, got {:#?}",
            expanded.module.body[1]
        );
    };
    assert_eq!(*realm, Some(Realm::Client));
    assert!(matches!(inner.kind, StmtKind::FunctionDecl(_)));

    let resolved = crate::resolve::Resolver::resolve(&expanded.module);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:#?}",
        resolved.diagnostics
    );
    let ir = Lowerer::lower(&expanded.module, &resolved).expect("lower");
    let lua = LuaCodegen::generate(&ir).expect("codegen").lua;
    assert!(lua.contains("local generated"), "{lua}");
    assert!(lua.contains("generated = function(command)"), "{lua}");
    assert!(lua.contains("return command[2]"), "{lua}");
    assert!(lua.contains("__lux_exports.generated = generated"), "{lua}");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(std_root);
}

#[test]
fn macro_helpers_read_declarative_data_tables() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lux_compile_time_data_test_{unique}"));
    let package = root.join("project");
    fs::create_dir_all(package.join("compiletime")).expect("create temp package root");
    fs::write(
            package.join("compiletime/module.lux"),
            "import * as m from \"lux/compile/macro\"\n\
             export macro fn fromData(ctx, call) {\n\
               if not m.expectArgCount(ctx, call, 1, \"`fromData` expects a declaration table\") {\n\
                 return nil\n\
               }\n\
               local spec = m.data(call.args[1])\n\
               local body = m.block({\n\
                 m.returnOne(m.string(spec.fields[2], call.span), call.span)\n\
               }, nil, call.span)\n\
               return m.stmts({\n\
                 m.exportRuntime(\"client\", m.fnDecl(spec.name, { \"command\" }, body, call.span), call.span)\n\
               })\n\
             }\n",
        )
        .expect("write temp compile-time package");

    let std_root = test_std_package_root();
    let registry = CompileTimePackageRegistry::load_default_with_package_roots(&[
        std_root.clone(),
        root.clone(),
    ])
    .expect("compile-time registry");
    let mut macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(
        0,
        None,
        "import macro { fromData } from \"project\"\nfromData { name = generated, fields = { x, y } }",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &macros);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:#?}",
        expanded.diagnostics
    );

    let resolved = crate::resolve::Resolver::resolve(&expanded.module);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:#?}",
        resolved.diagnostics
    );
    let ir = Lowerer::lower(&expanded.module, &resolved).expect("lower");
    let lua = LuaCodegen::generate(&ir).expect("codegen").lua;
    assert!(lua.contains("generated = function(command)"), "{lua}");
    assert!(lua.contains("return \"y\""), "{lua}");
    assert!(lua.contains("__lux_exports.generated = generated"), "{lua}");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(std_root);
}

#[test]
fn compile_time_table_mutation_through_function_parameters_is_visible() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lux_compile_time_table_ref_test_{unique}"));
    let package = root.join("project");
    fs::create_dir_all(package.join("compiletime")).expect("create temp package root");
    fs::write(
        package.join("compiletime/module.lux"),
        "import * as m from \"lux/compile/macro\"\n\
             fn push(out, span) {\n\
               out[#out + 1] = m.localOne(\"generated\", m.string(\"ok\", span), span)\n\
               return out\n\
             }\n\
             export macro fn build(ctx, call) {\n\
               local out = {}\n\
               push(out, call.span)\n\
               return m.stmts(out)\n\
             }\n",
    )
    .expect("write temp compile-time package");

    let std_root = test_std_package_root();
    let registry = CompileTimePackageRegistry::load_default_with_package_roots(&[
        std_root.clone(),
        root.clone(),
    ])
    .expect("compile-time registry");
    let mut macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(0, None, "import macro { build } from \"project\"\nbuild()");
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &macros);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:#?}",
        expanded.diagnostics
    );

    let StmtKind::LocalDecl { names, values, .. } = &expanded.module.body[1].kind else {
        panic!(
            "expected helper-generated local declaration, got {:#?}",
            expanded.module.body
        );
    };
    assert_eq!(names[0].name, "generated");
    assert!(matches!(values[0].kind, ExprKind::String(ref value) if value == "ok"));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(std_root);
}

#[test]
fn compile_time_tail_recursive_helpers_do_not_grow_the_rust_stack() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lux_compile_time_tail_rec_test_{unique}"));
    let package = root.join("project");
    fs::create_dir_all(package.join("compiletime")).expect("create temp package root");
    fs::write(
        package.join("compiletime/module.lux"),
        "import * as m from \"lux/compile/macro\"\n\
             fn append(out, index, limit, span) {\n\
               if index > limit {\n\
                 return out\n\
               }\n\
               out[#out + 1] = m.localOne(\"generated\" .. index, m.number(index, span), span)\n\
               return append(out, index + 1, limit, span)\n\
             }\n\
             export macro fn build(ctx, call) {\n\
               return m.stmts(append({}, 1, 128, call.span))\n\
             }\n",
    )
    .expect("write temp compile-time package");

    let std_root = test_std_package_root();
    let registry = CompileTimePackageRegistry::load_default_with_package_roots(&[
        std_root.clone(),
        root.clone(),
    ])
    .expect("compile-time registry");
    let mut macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(0, None, "import macro { build } from \"project\"\nbuild()");
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &macros);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:#?}",
        expanded.diagnostics
    );

    assert_eq!(expanded.module.body.len(), 129);
    let StmtKind::LocalDecl { names, values, .. } = &expanded.module.body[128].kind else {
        panic!(
            "expected helper-generated local declaration, got {:#?}",
            expanded.module.body[128]
        );
    };
    assert_eq!(names[0].name, "generated128");
    assert!(matches!(values[0].kind, ExprKind::Number(ref value) if value == "128"));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(std_root);
}

#[test]
fn compile_time_non_tail_recursion_reports_depth_error() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lux_compile_time_rec_limit_test_{unique}"));
    let package = root.join("project");
    fs::create_dir_all(package.join("compiletime")).expect("create temp package root");
    fs::write(
        package.join("compiletime/module.lux"),
        "fn recurse(index) {\n\
               return recurse(index + 1) + 1\n\
             }\n\
             export macro fn build(ctx, call) {\n\
               return recurse(1)\n\
             }\n",
    )
    .expect("write temp compile-time package");

    let std_root = test_std_package_root();
    let registry = CompileTimePackageRegistry::load_default_with_package_roots(&[
        std_root.clone(),
        root.clone(),
    ])
    .expect("compile-time registry");
    let mut macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(0, None, "import macro { build } from \"project\"\nbuild()");
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &macros);
    assert!(expanded.has_errors());
    assert!(
        expanded.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("compile-time function recursion exceeded")),
        "{:#?}",
        expanded.diagnostics
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(std_root);
}

#[test]
fn phase_qualified_macro_exports_are_registered() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lux_compile_time_test_{unique}"));
    let package = root.join("project");
    fs::create_dir_all(package.join("compiletime")).expect("create temp package root");
    fs::write(
        package.join("compiletime/module.lux"),
        "import * as m from \"lux/compile/macro\"\n\
             export fn helper() = 1\n\
             export macro fn literal(ctx, call) {\n\
               return m.expr(m.string(\"ok\", call.span))\n\
             }\n",
    )
    .expect("write temp compile-time package");

    let std_root = test_std_package_root();
    let registry = CompileTimePackageRegistry::load_default_with_package_roots(&[
        std_root.clone(),
        root.clone(),
    ])
    .expect("compile-time registry");
    let mut macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(
        0,
        None,
        "import macro { literal } from \"project\"\nlocal x = literal()",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &macros);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:#?}",
        expanded.diagnostics
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(std_root);
}

#[test]
fn compile_time_packages_export_const_values() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lux_compile_time_const_test_{unique}"));
    let constants = root.join("constants");
    let macros = root.join("const-macros");
    fs::create_dir_all(constants.join("compiletime")).expect("create constants package");
    fs::create_dir_all(macros.join("compiletime")).expect("create macros package");
    fs::write(
        constants.join("compiletime/module.lux"),
        "export const CODE = \"ok\"\n",
    )
    .expect("write constants source");
    fs::write(
        macros.join("compiletime/module.lux"),
        "import { CODE } from \"constants\"\n\
             import * as m from \"lux/compile/macro\"\n\
             export macro fn literal(ctx, call) {\n\
               return m.expr(m.string(CODE, call.span))\n\
             }\n",
    )
    .expect("write macros source");

    let std_root = test_std_package_root();
    let registry = CompileTimePackageRegistry::load_default_with_package_roots(&[
        std_root.clone(),
        root.clone(),
    ])
    .expect("compile-time registry");
    let mut registry_macros = crate::macro_expansion::MacroRegistry::empty();
    registry
        .register_macros(&mut registry_macros)
        .expect("register compile-time macros");

    let file = SourceFile::new(
        0,
        None,
        "import macro { literal } from \"const-macros\"\nlocal x = literal()",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let expanded = expand_macros_with_registry(&file, &parsed.module, &registry_macros);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:#?}",
        expanded.diagnostics
    );
    let StmtKind::LocalDecl { values, .. } = &expanded.module.body[1].kind else {
        panic!("expected local declaration");
    };
    let ExprKind::String(value) = &values[0].kind else {
        panic!("expected string literal");
    };
    assert_eq!(value, "ok");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(std_root);
}

#[test]
fn loads_lux_host_transform_specs() {
    let (registry, root) = test_registry();
    let specs = registry.host_transform_specs().expect("host specs");
    assert!(
        specs
            .iter()
            .any(|spec| spec.target == "lux/ui" && spec.runtime == "lux/ui")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn lux_host_transform_rewrites_ui_calls() {
    let (registry, root) = test_registry();
    let host_registry =
        HostRegistry::from_specs(registry.host_transform_specs().expect("host specs"));
    let file = SourceFile::new(
        0,
        None,
        "import { Column, Label } from \"lux/ui\"\nlocal view = Column { gap = 1 } { Label { text = \"Hi\" } }",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = parse_expand_resolve(&file, &lex.tokens);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let ir = Lowerer::lower(&parsed.module, &parsed.resolved).expect("lower");
    let transformed = host_registry.transform_module(ir, &parsed.resolved);
    assert!(transformed.diagnostics.is_empty());
    let lua = LuaCodegen::generate(&transformed.module)
        .expect("codegen")
        .lua;
    assert!(lua.contains("__lux_ui_node(\"Column\""));
    assert!(lua.contains("__lux_ui_node(\"Label\""));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn lux_host_transform_uses_original_import_name_for_aliases() {
    let (registry, root) = test_registry();
    let host_registry =
        HostRegistry::from_specs(registry.host_transform_specs().expect("host specs"));
    let file = SourceFile::new(
        0,
        None,
        "import { Column as Stack, Label } from \"lux/ui\"\nlocal view = Stack { gap = 1 } { Label { text = \"Hi\" } }",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = parse_expand_resolve(&file, &lex.tokens);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let ir = Lowerer::lower(&parsed.module, &parsed.resolved).expect("lower");
    let transformed = host_registry.transform_module(ir, &parsed.resolved);
    assert!(transformed.diagnostics.is_empty());
    let lua = LuaCodegen::generate(&transformed.module)
        .expect("codegen")
        .lua;
    assert!(lua.contains("__lux_ui_node(\"Column\""), "{lua}");
    assert!(lua.contains("__lux_ui_node(\"Label\""), "{lua}");
    assert!(!lua.contains("__lux_ui_node(\"Stack\""), "{lua}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn lux_host_transform_ignores_non_dsl_ui_call_shapes() {
    let (registry, root) = test_registry();
    let host_registry =
        HostRegistry::from_specs(registry.host_transform_specs().expect("host specs"));
    let file = SourceFile::new(
        0,
        None,
        "import { Column } from \"lux/ui\"\nlocal dynamic = 1\nlocal a = Column(1, 2)\nlocal b = Column { dynamic }",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = parse_expand_resolve(&file, &lex.tokens);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let ir = Lowerer::lower(&parsed.module, &parsed.resolved).expect("lower");
    let transformed = host_registry.transform_module(ir, &parsed.resolved);
    assert!(transformed.diagnostics.is_empty());
    let lua = LuaCodegen::generate(&transformed.module)
        .expect("codegen")
        .lua;
    assert!(!lua.contains("__lux_ui_node"), "{lua}");
    assert!(lua.contains("local Column = __lux_import_"), "{lua}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn project_host_package_can_target_external_runtime_source() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lux_host_package_test_{unique}"));
    let package = root.join("project");
    fs::create_dir_all(package.join("host")).expect("create temp package root");
    fs::write(
        package.join("host/module.lux"),
        "import * as ir from \"lux/compile/ir\"\n\
             export host package {\n\
               target = \"my/ui\",\n\
               runtime = \"my/runtime\"\n\
             }\n\
             export host expr fn foldWidget(ctx, call) {\n\
               if call.imported ~= \"Widget\" { return nil }\n\
               local makeWidget = ctx.importRuntime(\"makeWidget\", \"__my_makeWidget\")\n\
               return ir.call(ir.ident(makeWidget, call.expr), {\n\
                 ir.string(call.runtime, call.expr)\n\
               }, call.expr)\n\
             }\n",
    )
    .expect("write host package");

    let std_root = test_std_package_root();
    let registry = CompileTimePackageRegistry::load_default_with_package_roots(&[
        std_root.clone(),
        root.clone(),
    ])
    .expect("compile-time registry");
    let host_registry =
        HostRegistry::from_specs(registry.host_transform_specs().expect("host specs"));
    let file = SourceFile::new(
        0,
        None,
        "import { Widget } from \"my/ui\"\nlocal __my_makeWidget = \"occupied\"\nlocal view = Widget { id = \"inventory\" }",
    );
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = parse_expand_resolve(&file, &lex.tokens);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let ir = Lowerer::lower(&parsed.module, &parsed.resolved).expect("lower");
    let transformed = host_registry.transform_module(ir, &parsed.resolved);
    assert!(transformed.diagnostics.is_empty());
    let lua = LuaCodegen::generate(&transformed.module)
        .expect("codegen")
        .lua;
    assert!(lua.contains("__lux_import(\"my/runtime\")"), "{lua}");
    assert!(
        lua.contains("local __my_makeWidget_1 = __lux_import_"),
        "{lua}"
    );
    assert!(lua.contains("__my_makeWidget_1(\"my/runtime\")"), "{lua}");
    assert!(!lua.contains("__lux_import(\"my/ui\")"), "{lua}");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(std_root);
}
