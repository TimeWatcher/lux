use crate::ast::{Module, Realm};
use crate::lex::Lexer;
use crate::parse::Parser;
use crate::source::SourceFile;

use super::{BindingKind, ResolvePart, Resolver, ResolverOptions, UnknownExternalPolicy};

fn parse(input: &str) -> Module {
    parse_with_id(0, input)
}

fn parse_with_id(file_id: u32, input: &str) -> Module {
    let file = SourceFile::new(file_id, None, input);
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    parsed.module
}

fn has_diagnostic(output: &super::ResolveOutput, code: &str) -> bool {
    output
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code.as_deref() == Some(code))
}

#[test]
fn hoists_simple_functions_and_exports_only_requested_names() {
    let module = parse("fn helper(x) = public(x)\nexport fn public(x) = helper(x)");
    let output = Resolver::resolve(&module);
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert_eq!(output.exports.len(), 1);
    assert_eq!(output.exports[0].name, "public");
    assert_eq!(
        output
            .binding_by_name("helper")
            .map(|binding| &binding.kind),
        Some(&BindingKind::Function)
    );
}

#[test]
fn records_import_provenance() {
    let module = parse("import { Column } from \"lux/ui\"\nColumn { gap = 1 }");
    let output = Resolver::resolve(&module);
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let binding = output.binding_by_name("Column").expect("Column binding");
    assert_eq!(binding.kind, BindingKind::Import);
    assert_eq!(binding.source_module.as_deref(), Some("lux/ui"));
    assert_eq!(binding.imported_name.as_deref(), Some("Column"));
}

#[test]
fn const_bindings_are_immutable_but_fields_are_mutable() {
    let module = parse("const state = { count = 0 }\nstate.count += 1\nstate = {}");
    let output = Resolver::resolve(&module);
    assert!(output.has_errors());
    assert_eq!(
        output.binding_by_name("state").map(|binding| binding.kind),
        Some(BindingKind::Const)
    );
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code.as_deref() == Some("RESOLVE009"))
            .count(),
        1
    );
}

#[test]
fn imports_are_immutable_bindings() {
    let module = parse("import { arr } from \"lux/std\"\narr = nil");
    let output = Resolver::resolve(&module);
    assert!(output.has_errors());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("RESOLVE009"))
    );
}

#[test]
fn match_variant_patterns_must_resolve_to_enum_variants() {
    let module = parse(
        "local ShopPanelKey = core.ShopPanelKey\nfn demo(mode) = match mode { ShopPanelKey.Arsenal => true _ => false }",
    );
    let output = Resolver::resolve(&module);
    assert!(output.has_errors(), "{:#?}", output.diagnostics);
    assert!(
        has_diagnostic(&output, "RESOLVE015"),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn exports_const_bindings() {
    let module = parse("export const answer = 42");
    let output = Resolver::resolve(&module);
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert_eq!(output.exports.len(), 1);
    assert_eq!(output.exports[0].name, "answer");
    assert_eq!(
        output.binding_by_name("answer").map(|binding| binding.kind),
        Some(BindingKind::Const)
    );
}

#[test]
fn const_shadowing_follows_lexical_scope_rules() {
    let same_scope = parse("local name = \"old\"\nconst { name } = player");
    let same_scope_output = Resolver::resolve(&same_scope);
    assert!(
        same_scope_output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("RESOLVE005"))
    );

    let inner_scope = parse("local name = \"old\"\ndo { const { name } = player }");
    let inner_scope_output = Resolver::resolve(&inner_scope);
    assert!(
        inner_scope_output.diagnostics.is_empty(),
        "{:#?}",
        inner_scope_output.diagnostics
    );
}

#[test]
fn underscore_can_be_reused_as_discard_binding() {
    let module = parse("local _, _, x = values()\nfn f(_, _) = x");
    let output = Resolver::resolve(&module);
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
}

#[test]
fn reports_non_exhaustive_enum_match() {
    let module = parse(
        "enum Mode repr number { A = 0, B = 1, C = 2 }\nfn f(mode) = match mode { Mode.A => 1 Mode.C => 3 }",
    );
    let output = Resolver::resolve(&module);
    assert!(output.has_errors(), "{:#?}", output.diagnostics);
    assert!(
        has_diagnostic(&output, "MATCH001"),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn warns_unreachable_match_arms() {
    let module = parse(
        "enum Mode repr number { A = 0, B = 1 }\nfn f(mode) = match mode { Mode.A => 1 Mode.A => 2 _ => 0 Mode.B => 3 }",
    );
    let output = Resolver::resolve(&module);
    assert!(!output.has_errors(), "{:#?}", output.diagnostics);
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code.as_deref() == Some("MATCH002"))
            .count(),
        2,
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn warns_wildcard_after_full_enum_coverage() {
    let module = parse(
        "enum Mode repr number { A = 0, B = 1 }\nfn f(mode) = match mode { Mode.A => 1 Mode.B => 2 _ => 0 }",
    );
    let output = Resolver::resolve(&module);
    assert!(!output.has_errors(), "{:#?}", output.diagnostics);
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code.as_deref() == Some("MATCH002"))
            .count(),
        1,
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn existing_enum_wildcard_after_known_variants_stays_reachable() {
    let module = parse(
        "enum Fill repr existing(kind = \"kind\") { Solid(kind = FILL_SOLID) }\nfn f(fill) = match fill { Fill.Solid => 1 _ => 0 }",
    );
    let output = Resolver::resolve(&module);
    assert!(!output.has_errors(), "{:#?}", output.diagnostics);
    assert!(
        output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() != Some("MATCH002")),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn rejects_binding_or_patterns_for_now() {
    let module = parse(
        "enum Fill repr existing(kind = \"kind\") { Solid(kind = FILL_SOLID, color: Color), Linear(kind = FILL_LINEAR, color: Color) }\nfn f(fill) = match fill { Fill.Solid { color } | Fill.Linear { color } => color _ => nil }",
    );
    let output = Resolver::resolve(&module);
    assert!(output.has_errors(), "{:#?}", output.diagnostics);
    assert!(
        has_diagnostic(&output, "MATCH003"),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn side_effect_import_creates_module_edge_without_binding() {
    let module = parse("import \"setup\"");
    let output = Resolver::resolve(&module);
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert_eq!(output.module_edges.len(), 1);
    assert!(output.module_edges[0].side_effect_only);
    assert!(output.bindings.is_empty());
}

#[test]
fn rejects_exporting_unknown_bindings() {
    let module = parse("export { missing }");
    let output = Resolver::resolve(&module);
    assert!(output.has_errors());
}

#[test]
fn rejects_export_fn_method() {
    let module = parse("export fn PANEL:Paint(w, h) { draw(self, w, h) }");
    let output = Resolver::resolve(&module);
    assert!(output.has_errors());
}

#[test]
fn rejects_phase_qualified_exports_in_runtime_modules() {
    let module = parse("export macro fn dbg(ctx, call) = nil");
    let output = Resolver::resolve(&module);
    assert!(output.has_errors());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("RESOLVE006"))
    );
}

#[test]
fn module_parts_share_module_scope_bindings() {
    let state = parse_with_id(1, "local base = 1\nfn helper() = base");
    let api = parse_with_id(2, "export fn run() = helper()");
    let output = Resolver::resolve_parts(&[
        ResolvePart {
            module: &state,
            default_realm: Realm::Shared,
        },
        ResolvePart {
            module: &api,
            default_realm: Realm::Shared,
        },
    ]);

    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert_eq!(
        output
            .binding_by_name("base")
            .map(|binding| binding.module_scope),
        Some(true)
    );
    assert_eq!(
        output
            .binding_by_name("helper")
            .map(|binding| binding.module_scope),
        Some(true)
    );
    assert_eq!(output.exports.len(), 1);
    assert_eq!(output.exports[0].name, "run");
}

#[test]
fn top_level_imports_are_part_local() {
    let importer = parse_with_id(1, "import { arr } from \"lux/std\"");
    let other_part = parse_with_id(2, "fn run() = arr");
    let output = Resolver::resolve_parts(&[
        ResolvePart {
            module: &importer,
            default_realm: Realm::Shared,
        },
        ResolvePart {
            module: &other_part,
            default_realm: Realm::Shared,
        },
    ]);

    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert_eq!(output.module_edges.len(), 1);
    assert_eq!(output.module_edges[0].specifiers[0].local, "arr");
    assert!(
        output.module_edges[0].specifiers[0]
            .active_realms
            .is_empty()
    );
}

#[test]
fn simple_functions_are_hoisted_across_parts() {
    let api = parse_with_id(1, "export fn run() = helper()");
    let helper = parse_with_id(2, "fn helper() = 1");
    let output = Resolver::resolve_parts(&[
        ResolvePart {
            module: &api,
            default_realm: Realm::Shared,
        },
        ResolvePart {
            module: &helper,
            default_realm: Realm::Shared,
        },
    ]);

    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert_eq!(
        output
            .binding_by_name("helper")
            .map(|binding| binding.hoisted),
        Some(true)
    );
}

#[test]
fn non_function_module_locals_follow_part_order() {
    let first = parse_with_id(1, "local a = b");
    let second = parse_with_id(2, "local b = 1");
    let output = Resolver::resolve_parts(&[
        ResolvePart {
            module: &first,
            default_realm: Realm::Shared,
        },
        ResolvePart {
            module: &second,
            default_realm: Realm::Shared,
        },
    ]);

    assert!(
        has_diagnostic(&output, "RESOLVE012"),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn duplicate_module_bindings_across_parts_and_realms_are_errors() {
    let server_part = parse_with_id(1, "server fn sync() = 1");
    let client_part = parse_with_id(2, "client fn sync() = 2");
    let output = Resolver::resolve_parts(&[
        ResolvePart {
            module: &server_part,
            default_realm: Realm::Shared,
        },
        ResolvePart {
            module: &client_part,
            default_realm: Realm::Shared,
        },
    ]);

    assert!(
        has_diagnostic(&output, "RESOLVE005"),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn export_alias_maps_public_name_to_module_binding() {
    let module = parse_with_id(
        1,
        "local player_inventory = {}\nexport { p_inv = player_inventory }",
    );
    let output = Resolver::resolve_parts(&[ResolvePart {
        module: &module,
        default_realm: Realm::Shared,
    }]);

    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert_eq!(output.exports.len(), 1);
    assert_eq!(output.exports[0].name, "p_inv");
    assert_eq!(output.exports[0].local_name, "player_inventory");
}

#[test]
fn explicit_realm_functions_allow_realm_specific_shared_part_code() {
    let server_api = parse_with_id(1, "server fn grant() = 1");
    let shared_part = parse_with_id(2, "server fn run() = grant()");
    let output = Resolver::resolve_parts(&[
        ResolvePart {
            module: &server_api,
            default_realm: Realm::Shared,
        },
        ResolvePart {
            module: &shared_part,
            default_realm: Realm::Shared,
        },
    ]);

    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);

    let invalid_shared_use = parse_with_id(3, "fn leak() = grant()");
    let output = Resolver::resolve_parts(&[
        ResolvePart {
            module: &server_api,
            default_realm: Realm::Shared,
        },
        ResolvePart {
            module: &invalid_shared_use,
            default_realm: Realm::Shared,
        },
    ]);

    assert!(
        has_diagnostic(&output, "REALM001"),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn gmod_known_externals_are_checked_strictly() {
    let module = parse("fn bad() = vgui.Create(\"DFrame\")");
    let output = Resolver::resolve_with_options(&module, ResolverOptions::gmod_default());

    assert!(
        has_diagnostic(&output, "REALM001"),
        "{:#?}",
        output.diagnostics
    );

    let module = parse("client fn good() = vgui.Create(\"DFrame\")");
    let output = Resolver::resolve_with_options(&module, ResolverOptions::gmod_default());

    assert!(!output.has_errors(), "{:#?}", output.diagnostics);
}

#[test]
fn gmod_api_database_uses_path_level_realm_data() {
    let module = parse("fn bad() = net.Broadcast()");
    let output = Resolver::resolve_with_options(&module, ResolverOptions::gmod_default());

    assert!(
        has_diagnostic(&output, "REALM001"),
        "{:#?}",
        output.diagnostics
    );

    let module = parse("fn ok() = net.Start(\"LuxMessage\")");
    let output = Resolver::resolve_with_options(&module, ResolverOptions::gmod_default());

    assert!(
        !output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("REALM_UNKNOWN")),
        "{:#?}",
        output.diagnostics
    );
    assert!(!output.has_errors(), "{:#?}", output.diagnostics);
}

#[test]
fn unknown_externals_warn_by_default_in_gmod_mode() {
    let module = parse("fn run() {\n  ThirdPartyAddon.DoThing()\n  ThirdPartyAddon.DoThing()\n}");
    let output = Resolver::resolve_with_options(&module, ResolverOptions::gmod_default());

    assert!(!output.has_errors(), "{:#?}", output.diagnostics);
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code.as_deref() == Some("REALM_UNKNOWN"))
            .count(),
        1
    );
}

#[test]
fn unknown_external_policy_can_be_error() {
    let module = parse("fn run() = ThirdPartyAddon.DoThing()");
    let output = Resolver::resolve_with_options(
        &module,
        ResolverOptions::gmod_default().with_unknown_external(UnknownExternalPolicy::Error),
    );

    assert!(
        has_diagnostic(&output, "REALM_UNKNOWN"),
        "{:#?}",
        output.diagnostics
    );
    assert!(output.has_errors(), "{:#?}", output.diagnostics);
}

#[test]
fn extern_declarations_use_longest_path_match() {
    let module = parse(
        "extern shared ThirdPartyAddon\n\
             extern server ThirdPartyAddon.DoThing\n\
             server fn ok() = ThirdPartyAddon.DoThing()\n\
             fn other() = ThirdPartyAddon.Other()\n\
             fn bad() = ThirdPartyAddon.DoThing()",
    );
    let output = Resolver::resolve_with_options(&module, ResolverOptions::gmod_default());

    assert!(
        has_diagnostic(&output, "REALM001"),
        "{:#?}",
        output.diagnostics
    );
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code.as_deref() == Some("REALM_UNKNOWN"))
            .count(),
        0,
        "{:#?}",
        output.diagnostics
    );
}
