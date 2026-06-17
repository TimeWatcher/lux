use crate::ast::BindingMode;
use crate::ir::{IrExpr, IrExprKind, IrModule, IrStmt, IrStmtKind, Origin, ValueMode};
use crate::lex::Lexer;
use crate::lower::Lowerer;
use crate::parse::Parser;
use crate::resolve::Resolver;
use crate::source::{FileId, SourceFile, SourceSpan};

use super::LuaCodegen;

fn compile(input: &str) -> super::LuaOutput {
    let file = SourceFile::new(0, None, input);
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let resolved = Resolver::resolve(&parsed.module);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:#?}",
        resolved.diagnostics
    );
    let ir = Lowerer::lower(&parsed.module, &resolved).expect("lower");
    LuaCodegen::generate(&ir).expect("codegen")
}

fn compile_with_warnings(input: &str) -> super::LuaOutput {
    let file = SourceFile::new(0, None, input);
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let resolved = Resolver::resolve(&parsed.module);
    assert!(!resolved.has_errors(), "{:#?}", resolved.diagnostics);
    let ir = Lowerer::lower(&parsed.module, &resolved).expect("lower");
    LuaCodegen::generate(&ir).expect("codegen")
}

fn compile_error(input: &str) -> super::CodegenError {
    let file = SourceFile::new(0, None, input);
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let resolved = Resolver::resolve(&parsed.module);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:#?}",
        resolved.diagnostics
    );
    let ir = Lowerer::lower(&parsed.module, &resolved).expect("lower");
    LuaCodegen::generate(&ir).expect_err("codegen should fail")
}

fn synthetic_origin() -> Origin {
    Origin::Synthetic {
        source: SourceSpan::new(FileId(0), 0, 0),
        reason: "test".into(),
    }
}

#[test]
fn emits_private_and_exported_functions() {
    let output = compile("fn helper(x) = x + 1\nexport fn publicApi(x) = helper(x) * 2");
    assert!(output.lua.contains("local helper"));
    assert!(output.lua.contains("local publicApi"));
    assert!(output.lua.contains("__lux_exports.publicApi = publicApi"));
}

#[test]
fn emits_lua_style_function_inputs_through_existing_ir() {
    let output = compile(
        "local function helper(x)\n  if x > 0 then\n    return x\n  elseif x == 0 then\n    return 0\n  else\n    return -x\n  end\nend\nlocal f = function(y)\n  return helper(y)\nend",
    );
    assert!(output.lua.contains("local helper"));
    assert!(output.lua.contains("helper = function(x)"));
    assert!(output.lua.contains("if x > 0 then"));
    assert!(output.lua.contains("return x"));
    assert!(output.lua.contains("else"));
    assert!(output.lua.contains("local f = function(y)"));
    assert!(!output.lua.contains("return nil"));
}

#[test]
fn emits_safe_method_and_nil_coalesce_without_and_or() {
    let output = compile("export fn name(player) = player?:GetName() ?? false");
    assert!(output.lua.contains("local __lux_method_"));
    assert!(output.lua.contains("~= nil"));
    assert!(output.lua.contains("= false"));
}

#[test]
fn emits_conditional_return_as_if() {
    let output = compile("export fn choose(ok) = ok then 1 else 0");
    assert!(output.lua.contains("if ok then"));
    assert!(output.lua.contains("return 1"));
    assert!(output.lua.contains("return 0"));
}

#[test]
fn emits_compound_assignment_with_single_evaluation_place() {
    let output = compile("fn add() { getTable()[nextIndex()] += 1 }");
    assert!(output.lua.contains("local __lux_tbl_"));
    assert!(output.lua.contains("local __lux_key_"));
}

#[test]
fn avoids_place_temps_for_stable_assignments() {
    let output = compile("fn add(tbl, key) { tbl[key] += 1\nbucket.count += 1 }");
    assert!(!output.lua.contains("local __lux_tbl_"));
    assert!(!output.lua.contains("local __lux_key_"));
    assert!(!output.lua.contains("local __lux_obj_"));
    assert!(output.lua.contains("tbl[key] = tbl[key] + 1"));
    assert!(output.lua.contains("bucket.count = bucket.count + 1"));
}

#[test]
fn expression_statement_non_call_is_discarded_safely() {
    let output = compile("fn demo(a, b) { a + b; 1 }");
    assert!(output.lua.contains("local __lux_unused_"));
    assert!(output.lua.contains("return 1"));
}

#[test]
fn safe_comparison_guards_nil() {
    let output = compile("fn ok(player) = player?:GetExp() > 5");
    assert!(output.lua.contains("local __lux_cmp_"));
    assert!(output.lua.contains("if __lux_val_GetExp_"));
    assert!(output.lua.contains(" ~= nil then"));
    assert!(!output.lua.contains("5 ~= nil"));
}

#[test]
fn logical_and_delays_rhs_setup_until_guard_passes() {
    let output = compile(
        "fn visible(drawStyle) = drawStyle.shadow ~= nil and drawStyle.shadow.color ~= nil and ((drawStyle.shadow.color.a ?? 255) > 0)",
    );
    let guard_pos = output
        .lua
        .find("if __lux_tmp_")
        .expect("short-circuit guard");
    let guarded_access_pos = output
        .lua
        .find("drawStyle.shadow.color.a")
        .expect("guarded access");
    assert!(guard_pos < guarded_access_pos, "{}", output.lua);
}

#[test]
fn condition_context_keeps_simple_short_circuit_inline() {
    let output = compile(
        "fn point(value) { if typeOf(value) == \"table\" and (value.x ~= nil or value[1] ~= nil) and (value.y ~= nil or value[2] ~= nil) { return true } return false }",
    );
    let lua = output.lua;
    assert!(
        lua.contains(
            "if typeOf(value) == \"table\" and (value.x ~= nil or value[1] ~= nil) and (value.y ~= nil or value[2] ~= nil) then"
        ),
        "{lua}"
    );
    assert!(!lua.contains("local __lux_tmp_"), "{lua}");
}

#[test]
fn condition_context_preserves_rhs_setup_short_circuit() {
    let output = compile(
        "fn demo(player) { if player ~= nil and player?:GetName() ~= nil { return true } return false }",
    );
    let lua = output.lua;
    let guard_pos = lua.find("if __lux_tmp_").expect("fallback guard");
    let access_pos = lua.find("GetName").expect("safe call");
    assert!(guard_pos < access_pos, "{lua}");
}

#[test]
fn condition_context_parenthesizes_mixed_short_circuit() {
    let output = compile("fn demo(a, b, c) { if a and (b or c) { return true } return false }");
    let lua = output.lua;
    assert!(lua.contains("if a and (b or c) then"), "{lua}");
}

#[test]
fn setup_free_short_circuit_return_stays_inline() {
    let output = compile(
        "fn point(value) = typeOf(value) == \"table\" and (value.x ~= nil or value[1] ~= nil) and (value.y ~= nil or value[2] ~= nil)",
    );
    let lua = output.lua;
    assert!(
        lua.contains(
            "return typeOf(value) == \"table\" and (value.x ~= nil or value[1] ~= nil) and (value.y ~= nil or value[2] ~= nil)"
        ),
        "{lua}"
    );
    assert!(!lua.contains("local __lux_tmp_"), "{lua}");
}

#[test]
fn short_circuit_value_preserves_rhs_setup_guard() {
    let output = compile("fn demo(player) = player ~= nil and player?:GetName()");
    let lua = output.lua;
    let guard_pos = lua.find("if __lux_tmp_").expect("short-circuit guard");
    let access_pos = lua.find("GetName").expect("safe call");
    assert!(guard_pos < access_pos, "{lua}");
}

#[test]
fn source_map_is_emitted_from_ir_writer() {
    let output = compile("fn one() = 1");
    assert!(!output.source_map.is_empty());
}

#[test]
fn large_module_predecls_are_lifted_into_private_table() {
    let origin = synthetic_origin();
    let mut body = Vec::new();
    for index in 0..170 {
        body.push(IrStmt {
            origin: origin.clone(),
            kind: IrStmtKind::LocalDecl {
                mode: BindingMode::Local,
                names: vec![format!("binding{index}")],
                values: Vec::new(),
            },
        });
    }
    body.push(IrStmt {
        origin: origin.clone(),
        kind: IrStmtKind::Assign {
            targets: vec![crate::ir::IrPlace::Identifier("binding42".into())],
            values: vec![IrExpr {
                kind: IrExprKind::Number("1".into()),
                origin: origin.clone(),
                value_mode: ValueMode::Single,
                symbol: None,
            }],
        },
    });

    let output = LuaCodegen::generate(&IrModule {
        body,
        exports: Vec::new(),
        origin,
    })
    .expect("codegen");

    assert!(output.lua.contains("local __lux_module_"));
    assert!(output.lua.contains("__lux_module_1.binding42 = 1"));
    assert!(!output.lua.contains("local binding42"));
}

#[test]
fn hoisted_function_locals_map_to_function_declarations() {
    let output = compile("\n\nfn later() = 1");
    let local_line = output
        .lua
        .lines()
        .position(|line| line == "local later")
        .map(|index| index + 1)
        .expect("hoisted local");
    let mapping = output
        .source_map
        .mappings()
        .iter()
        .find(|mapping| mapping.generated_line == local_line)
        .expect("local mapping");
    let file = SourceFile::new(0, None, "\n\nfn later() = 1");
    assert_eq!(file.line_col(mapping.source.byte_start).0, 3);
}

#[test]
fn imports_use_local_temps() {
    let output = compile("import { arr } from \"lux/std\"");
    assert!(output.lua.contains("local __lux_import_"));
    assert!(output.lua.contains("local arr = __lux_import_"));
}

#[test]
fn imports_reuse_same_module_temp_in_scope() {
    let output = compile("import { arr } from \"lux/std\"\nimport * as std from \"lux/std\"");
    assert_eq!(output.lua.matches("__lux_import(\"lux/std\")").count(), 1);
    assert!(output.lua.contains("local arr = __lux_import_1.arr"));
    assert!(output.lua.contains("local std = __lux_import_1"));
}

#[test]
fn side_effect_import_reuses_existing_module_temp() {
    let output = compile("import \"setup\"\nimport * as setup from \"setup\"");
    assert_eq!(output.lua.matches("__lux_import(\"setup\")").count(), 1);
    assert!(output.lua.contains("local setup = __lux_import_1"));
}

#[test]
fn preserves_multivalue_in_return_and_assignment_tail_positions() {
    let output = compile("fn pass(...) = ...\nfn demo() { local a, b = pass(); return pass() }");
    assert!(output.lua.contains("local a, b = pass()"));
    assert!(output.lua.contains("return pass()"));
}

#[test]
fn collapses_call_multivalue_outside_tail_positions() {
    let output = compile(
        "fn pass(...) = ...\nfn demo() { local a, b = pass(), pass(); return pass(), pass() }",
    );
    assert!(output.lua.contains("local a, b = pass(), pass()"));
    assert!(output.lua.contains("return pass(), pass()"));
}

#[test]
fn preserves_table_constructor_multivalue_only_for_last_array_field() {
    let output = compile("fn pass(...) = ...\nfn demo() = { pass(), 1, pass() }");
    assert!(output.lua.contains("{ pass(), 1, pass() }"));
}

#[test]
fn omits_redundant_call_parentheses_in_table_fields_and_arguments() {
    let output =
        compile("fn demo(node, obj) = { node(\"Label\"), obj?.run(expensiveRadius(), 1) }");
    let lua = output.lua;
    assert!(lua.contains("{ node(\"Label\"), __lux_val_"), "{lua}");
    assert!(lua.contains("= __lux_fn_"), "{lua}");
    assert!(lua.contains("expensiveRadius(), 1"), "{lua}");
    assert!(!lua.contains("(node(\"Label\"))"), "{lua}");
    assert!(!lua.contains("(expensiveRadius())"), "{lua}");
}

#[test]
fn optional_index_delays_key_evaluation_until_receiver_exists() {
    let output = compile("fn demo(tbl) = tbl?.[nextKey()]");
    let lua = output.lua;
    let guard = lua.find("if __lux_obj_").expect("nil guard");
    let key_call = lua.find("nextKey()").expect("key call");
    assert!(guard < key_call, "{lua}");
}

#[test]
fn safe_call_delays_arguments_until_callable_exists() {
    let output = compile("fn demo(obj) = obj?.run(sideEffect())");
    let lua = output.lua;
    let guard = lua.find("if __lux_fn_").expect("function guard");
    let arg = lua.find("sideEffect()").expect("argument call");
    assert!(guard < arg, "{lua}");

    let output = compile("fn demo(obj) = obj?:run(sideEffect())");
    let lua = output.lua;
    let guard = lua.find("if __lux_method_").expect("method guard");
    let arg = lua.find("sideEffect()").expect("argument call");
    assert!(guard < arg, "{lua}");
}

#[test]
fn safe_call_inner_function_temp_is_hygienic() {
    let output = compile("fn demo(obj, __lux_fn) = obj?.run(__lux_fn)");
    assert!(output.lua.contains("local __lux_fn_"));
    assert!(output.lua.contains("(__lux_fn)"));

    let output = compile("fn demo(obj, __lux_method) = obj?:run(__lux_method)");
    assert!(output.lua.contains("local __lux_method_"));
    assert!(output.lua.contains(", __lux_method)"));
}

#[test]
fn return_setup_temps_are_inlined_before_return() {
    let output = compile("fn demo(player) = player?:GetName() ?? \"unknown\"");
    assert!(output.lua.contains("  local __lux_obj_"));
    assert!(output.lua.contains("  return __lux_tmp_"));
    assert!(!output.lua.contains("do\n    local __lux_obj_"));
}

#[test]
fn setup_temps_are_scoped_for_local_initializers_when_safe() {
    let output = compile("fn demo(player) { local name = player?:GetName() ?? \"unknown\" }");
    assert!(
        output
            .lua
            .contains("  local name\n  do\n    local __lux_obj_player_")
    );
    assert!(output.lua.contains("    name = __lux_val_GetName_"));
    assert!(output.lua.contains("    if name == nil then"));
    assert!(output.lua.contains("      name = \"unknown\""));
}

#[test]
fn coalesce_local_initializer_uses_target_as_guard_when_safe() {
    let output = compile("fn demo(drawStyle) { local resolved = drawStyle ?? {} }");
    let lua = output.lua;
    assert!(lua.contains("  local resolved = drawStyle"), "{lua}");
    assert!(lua.contains("  if resolved == nil then"), "{lua}");
    assert!(lua.contains("    resolved = {}"), "{lua}");
    assert!(!lua.contains("local __lux_tmp_drawStyle_"), "{lua}");
}

#[test]
fn coalesce_self_assignment_uses_target_as_guard() {
    let output = compile("fn demo(drawStyle) { drawStyle = drawStyle ?? {} }");
    let lua = output.lua;
    assert!(lua.contains("  if drawStyle == nil then"), "{lua}");
    assert!(lua.contains("    drawStyle = {}"), "{lua}");
    assert!(!lua.contains("local __lux_tmp_drawStyle_"), "{lua}");
}

#[test]
fn coalesce_self_assignment_keeps_rhs_setup_under_guard() {
    let output = compile("fn demo(value, player) { value = value ?? player?:GetName() }");
    let lua = output.lua;
    let guard_pos = lua.find("if value == nil then").expect("nil guard");
    let safe_call_pos = lua.find("GetName").expect("safe call");
    assert!(guard_pos < safe_call_pos, "{lua}");
}

#[test]
fn optional_member_coalesce_uses_single_result_temp() {
    let output = compile("fn demo(atlas) = atlas?.w ?? 0");
    let lua = output.lua;
    assert!(lua.contains("local __lux_tmp_w_"), "{lua}");
    assert!(lua.contains("if __lux_tmp_w_"), "{lua}");
    assert!(!lua.contains("local __lux_val_w_"), "{lua}");
}

#[test]
fn coalesce_chain_uses_single_accumulator() {
    let output = compile("fn demo(a, b, c) = a ?? b ?? c");
    let lua = output.lua;
    assert!(lua.contains("local __lux_tmp_a_"), "{lua}");
    assert_eq!(lua.matches("local __lux_tmp_").count(), 1, "{lua}");
    assert!(lua.contains(" = b"), "{lua}");
    assert!(lua.contains(" = c"), "{lua}");
}

#[test]
fn coalesce_chain_keeps_rhs_setup_under_nil_guard() {
    let output = compile("fn demo(a, player) = a ?? player?:GetName() ?? \"unknown\"");
    let lua = output.lua;
    let first_guard = lua.find("if __lux_tmp_a_").expect("nil guard");
    let safe_call = lua.find("GetName").expect("safe call");
    assert!(first_guard < safe_call, "{lua}");
}

#[test]
fn while_condition_setup_runs_each_iteration() {
    let output = compile("fn demo(player) { while (player?:Alive()) { tick() } }");
    let lua = output.lua;
    let loop_start = lua.find("while true do").expect("while true lowering");
    let method = lua.find("local __lux_method_").expect("method lookup");
    let body = lua.find("tick()").expect("loop body");
    assert!(loop_start < method && method < body, "{lua}");
    assert!(lua.contains("if not (__lux_val_"));
    assert!(lua.contains("break"));
}

#[test]
fn local_initializer_scope_preserves_self_reference_semantics() {
    let output = compile("fn demo(x) { do { local x = x ?? 1 } }");
    assert!(output.lua.contains("local __lux_tmp_"));
    assert!(output.lua.contains("local x = __lux_tmp_"));
    assert!(!output.lua.contains("local x\n  do"));

    let output =
        compile("fn demo(player) { local f = 1; do { local f = player?:GetName() ?? (() => f) } }");
    assert!(output.lua.contains("local f = __lux_tmp_"));
    assert!(!output.lua.contains("local f\n    do"));
}

#[test]
fn trailing_semicolon_suppresses_implicit_return() {
    let output = compile("fn yes() { 1 }\nfn no() { 1; }");
    assert!(output.lua.contains("yes = function()\n  return 1\nend"));
    assert!(
        output
            .lua
            .contains("no = function()\n  local __lux_unused_")
    );
    assert!(!output.lua.contains("no = function()\n  return 1\nend"));
}

#[test]
fn emits_default_parameters_as_nil_guards() {
    let output = compile("fn demo(a = 1, b = a + 1) = b");
    assert!(output.lua.contains("if a == nil then"));
    assert!(output.lua.contains("a = 1"));
    assert!(output.lua.contains("if b == nil then"));
    assert!(output.lua.contains("b = a + 1"));
}

#[test]
fn omits_default_parameter_guard_when_default_is_nil() {
    let output = compile("fn demo(a = nil, b = 1) = b");
    assert!(!output.lua.contains("if a == nil then"), "{}", output.lua);
    assert!(output.lua.contains("if b == nil then"), "{}", output.lua);
}

#[test]
fn emits_empty_table_without_internal_padding() {
    let output = compile("fn demo() = {}");
    assert!(output.lua.contains("return {}"), "{}", output.lua);
    assert!(!output.lua.contains("{  }"), "{}", output.lua);
}

#[test]
fn removes_redundant_expression_parentheses_but_preserves_required_ones() {
    let output = compile("fn flat(x) = x + 1\nfn grouped(a, b, c, d) = (a + b) * (c - d)");
    let lua = output.lua;
    assert!(lua.contains("return x + 1"), "{lua}");
    assert!(lua.contains("return (a + b) * (c - d)"), "{lua}");
}

#[test]
fn wraps_long_tables_and_calls_readably() {
    let output = compile(
        "fn tableDemo() = { alpha = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\", beta = \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\", gamma = \"cccccccccccccccccccccccccccccccc\" }\nfn callDemo() = draw(\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\", \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\", \"cccccccccccccccccccccccccccccccc\")\nfn templateDemo(veryLongIndexName, veryLongPlayerName, veryLongHealthValue) = `#${veryLongIndexName}: ${veryLongPlayerName} (${veryLongHealthValue} hp)`",
    );
    let lua = output.lua;
    assert!(lua.contains("return {\n"), "{lua}");
    assert!(lua.contains("  alpha = "), "{lua}");
    assert!(lua.contains("return draw(\n"), "{lua}");
    assert!(
        lua.contains("  \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\""),
        "{lua}"
    );
    assert!(lua.contains("return \"#\" ..\n"), "{lua}");
}

#[test]
fn emits_const_bindings_as_plain_lua_locals_and_exports() {
    let output = compile("export const answer = 42\nfn demo() = answer");
    assert!(output.lua.contains("local answer = 42"));
    assert!(output.lua.contains("__lux_exports.answer = answer"));
    assert!(output.lua.contains("return answer"));
}

#[test]
fn arrow_block_can_return_table_literal_without_closing_outer_function() {
    let output = compile(
        "fn test(owner) {\n  owner.shaderStatus = () => {\n    {\n      ok = false,\n      reason = \"x\"\n    }\n  }\n\n  owner\n}",
    );
    let lua = output.lua;
    assert!(
            lua.contains(
                "test = function(owner)\n  owner.shaderStatus = function()\n    return { ok = false, reason = \"x\" }\n  end\n  return owner\nend"
            ),
            "{lua}"
        );
}

#[test]
fn rejects_function_over_lua_local_slot_limit() {
    let mut input = String::from("fn demo() {\n");
    for index in 0..201 {
        input.push_str(&format!("  local value{index} = {index}\n"));
    }
    input.push_str("  return value0\n}\n");

    let error = compile_error(&input);
    assert!(
        error.message.contains("uses 201 local slots"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("Lua 5.1 limit is 200"),
        "{}",
        error.message
    );
}

#[test]
fn narrows_large_independent_function_local_ranges() {
    let mut input = String::from("fn demo() {\n");
    for index in 0..220 {
        input.push_str(&format!("  local value{index} = {index}\n"));
    }
    input.push_str("  return 1\n}\n");

    let output = compile(&input);
    assert!(
        output.lua.contains("  do\n    local value0 = 0"),
        "{}",
        output.lua
    );
    assert!(
        output.local_budget.max_slots <= super::super::LUA_LOCAL_SLOT_LIMIT,
        "{:#?}",
        output.local_budget
    );
}

#[test]
fn lifts_large_module_import_cache_slots() {
    let mut input = String::new();
    for index in 0..170 {
        input.push_str(&format!("import * as mod{index} from \"pkg/{index}\"\n"));
    }
    input.push_str("export fn demo() = mod42\n");

    let output = compile(&input);
    assert!(output.lua.contains("local __lux_module_"), "{}", output.lua);
    assert!(
        output
            .lua
            .contains("__lux_module_1.mod0 = __lux_import(\"pkg/0\")"),
        "{}",
        output.lua
    );
    assert!(
        !output.lua.contains("local __lux_import_"),
        "{}",
        output.lua
    );
    assert!(
        output.lua.contains("__lux_module_1.mod42"),
        "{}",
        output.lua
    );
    assert!(
        output.local_budget.max_slots <= super::super::LUA_LOCAL_SLOT_LIMIT,
        "{:#?}",
        output.local_budget
    );
}

#[test]
fn emits_destructuring_with_single_value_cache_and_defaults() {
    let output = compile(
        "fn demo(player, point) {\n  local { name, hp, armor = 0 } = player\n  local [x, y] = point\n  return name, hp, armor, x, y\n}",
    );
    let lua = output.lua;
    assert!(lua.contains("local name, hp, armor"));
    assert!(lua.contains("local x, y"));
    assert!(lua.contains("local __lux_destructure_"));
    assert!(lua.contains("if __lux_field_"));
    assert!(lua.contains("armor = __lux_field_"));
    assert!(lua.contains("__lux_destructure_"));
}

#[test]
fn destructuring_does_not_nil_protect_source() {
    let output = compile("fn demo(player) { local { name } = player\n return name }");
    let lua = output.lua;
    assert!(lua.contains(" = __lux_destructure_"));
    assert!(lua.contains(".name"));
    assert!(!lua.contains("if __lux_destructure_"));
}

#[test]
fn emits_table_spread_as_ordered_merge() {
    let output = compile("fn demo(base) = { ...base, text = \"ok\" }");
    let lua = output.lua;
    assert!(lua.contains("local __lux_table_"));
    assert!(lua.contains("if __lux_spread_"));
    assert!(lua.contains("for __lux_k_"));
    assert!(lua.contains("pairs(__lux_spread_"));
    assert!(lua.contains(".text = \"ok\""));
}

#[test]
fn emits_pipeline_placeholder_at_explicit_position() {
    let output = compile("fn demo(x) = x |> clamp(0, %, 100)");
    let lua = output.lua;
    assert!(lua.contains("local __lux_pipe_"));
    assert!(lua.contains("return clamp(0, __lux_pipe_"));
    assert!(!lua.contains("return clamp(__lux_pipe_"));

    let output = compile("fn demo(xs, f) = xs |> arr.map(%, f)");
    let lua = output.lua;
    assert!(lua.contains("return arr.map(__lux_pipe_"));
}

#[test]
fn emits_do_expression_into_value_context() {
    let output = compile("fn demo() { local x = do { local y = 1\n y + 1 }\n return x }");
    let lua = output.lua;
    assert!(lua.contains("local x"));
    assert!(lua.contains("local y = 1"));
    assert!(lua.contains("x = y + 1"));
}

#[test]
fn emits_conditional_control_shortcuts() {
    let output = compile(
        "fn demo(x, ok) {\n  stopif x == nil\n  stopif x < 0, false, \"bad\"\n  stopifn ok, nil\n  return true\n}",
    );
    let lua = output.lua;
    assert!(lua.contains("if x == nil then"));
    assert!(lua.contains("return\n"));
    assert!(lua.contains("if x < 0 then"));
    assert!(lua.contains("return false, \"bad\""));
    assert!(lua.contains("if not ok then"));
    assert!(lua.contains("return nil"));
}

#[test]
fn emits_continueif_with_break_preserving_loop_break() {
    let output = compile(
        "fn demo() {\n  local total = 0\n  for i = 1, 10 {\n    continueif i < 3\n    breakif i > 8\n    total += i\n  }\n  return total\n}",
    );
    let lua = output.lua;
    assert!(lua.contains("local __lux_break_"));
    assert!(lua.contains("repeat"));
    assert!(lua.contains("if i < 3 then"));
    assert!(lua.contains("break"));
    assert!(lua.contains("__lux_break_"));
    assert!(lua.contains("if __lux_break_"));
}

#[test]
fn scalar_enum_match_is_zero_runtime() {
    let output = compile(
        "enum FillKind repr number { Solid = 0, Linear = 1 }\nfn demo(kind, a, b) = match kind { FillKind.Solid => a FillKind.Linear => b }",
    );
    let lua = output.lua;
    assert!(!lua.contains("FillKind ="));
    assert!(!lua.contains("local FillKind"));
    assert!(lua.contains("local __lux_match_"));
    assert!(lua.contains("if __lux_match_1 == 0 then"));
    assert!(lua.contains("elseif __lux_match_1 == 1 then"));
    assert!(lua.contains("return a"));
    assert!(lua.contains("return b"));
    assert!(!lua.contains("function()"));
}

#[test]
fn scalar_enum_variant_expression_is_zero_runtime() {
    let output = compile(
        "enum RingMode repr number { Full = 0, Arc = 1 }\nfn demo(mode) {\n  if mode == RingMode.Arc { return RingMode.Arc }\n  return RingMode.Full\n}",
    );
    let lua = output.lua;
    assert!(!lua.contains("RingMode ="), "{lua}");
    assert!(!lua.contains("RingMode."), "{lua}");
    assert!(lua.contains("if mode == 1 then"), "{lua}");
    assert!(lua.contains("return 1"), "{lua}");
    assert!(lua.contains("return 0"), "{lua}");

    let output = compile("enum Kind repr string { Solid = \"solid\" }\nfn demo() = Kind.Solid");
    let lua = output.lua;
    assert!(!lua.contains("Kind."), "{lua}");
    assert!(lua.contains("return \"solid\""), "{lua}");
}

#[test]
fn match_local_initializer_assigns_target_directly() {
    let output = compile(
        "enum Mode repr string { A = \"a\", B = \"b\" }\nfn demo(kind) {\n  local mode = match kind { Mode.A => \"alpha\" _ => \"other\" }\n  return mode\n}",
    );
    let lua = output.lua;
    assert!(lua.contains("local mode"));
    assert!(lua.contains("mode = \"alpha\""));
    assert!(lua.contains("mode = \"other\""));
    assert!(!lua.contains("local __lux_tmp_"));
}

#[test]
fn existing_enum_match_hoists_tag_and_reads_fields() {
    let output = compile_with_warnings(
        "enum Fill repr existing(kind = \"kind\") { Solid(kind = FILL_SOLID, color: Color), Linear(kind = FILL_LINEAR, x1: number) }\nfn demo(fill) = match fill { Fill.Solid { color } => color Fill.Linear { x1 } => x1 _ => nil }",
    );
    let lua = output.lua;
    assert!(lua.contains("local __lux_match_"));
    assert!(lua.contains("local __lux_tag_"));
    assert_eq!(lua.matches(".kind").count(), 1, "{lua}");
    assert!(lua.contains("if __lux_tag_2 == FILL_SOLID then"));
    assert!(lua.contains("local color = __lux_match_1.color"));
    assert!(lua.contains("elseif __lux_tag_2 == FILL_LINEAR then"));
    assert!(lua.contains("local x1 = __lux_match_1.x1"));
}

#[test]
fn match_pattern_alternatives_share_tag_read() {
    let output = compile_with_warnings(
        "enum Fill repr existing(kind = \"kind\") { Linear(kind = FILL_LINEAR), Radial(kind = FILL_RADIAL), Conic(kind = FILL_CONIC) }\nfn demo(fill) = match fill { Fill.Linear | Fill.Radial | Fill.Conic => bindGradientLut(fill) _ => nil }",
    );
    let lua = output.lua;
    assert_eq!(lua.matches(".kind").count(), 1, "{lua}");
    assert!(lua.contains(
        "__lux_tag_2 == FILL_LINEAR or __lux_tag_2 == FILL_RADIAL or __lux_tag_2 == FILL_CONIC"
    ));
}

#[test]
fn match_codegen_skips_unreachable_arms() {
    let output = compile_with_warnings(
        "enum Mode repr number { A = 0, B = 1 }\nfn demo(mode) = match mode { Mode.A => one() Mode.A => two() _ => fallback() Mode.B => three() }",
    );
    let lua = output.lua;
    assert!(lua.contains("one()"), "{lua}");
    assert!(lua.contains("fallback()"), "{lua}");
    assert!(!lua.contains("two()"), "{lua}");
    assert!(!lua.contains("three()"), "{lua}");
}

#[test]
fn match_codegen_skips_wildcard_after_full_enum_coverage() {
    let output = compile_with_warnings(
        "enum Mode repr number { A = 0, B = 1 }\nfn demo(mode) = match mode { Mode.A => one() Mode.B => two() _ => fallback() }",
    );
    let lua = output.lua;
    assert!(lua.contains("one()"), "{lua}");
    assert!(lua.contains("two()"), "{lua}");
    assert!(!lua.contains("fallback()"), "{lua}");
}

#[test]
fn table_enum_uses_explicit_tag_field_for_construction_and_match() {
    let output = compile_with_warnings(
        "enum Fill repr table(tag = \"__tag\") { Solid(tag = 1, color: Color), Linear(tag = 2, color: Color) }\nfn make(color) = Fill.Solid(color)\nfn read(fill) = match fill { Fill.Solid { color } => color _ => nil }",
    );
    let lua = output.lua;
    assert!(lua.contains("__tag = 1"), "{lua}");
    assert!(lua.contains("color = color"), "{lua}");
    assert_eq!(lua.matches(".__tag").count(), 1, "{lua}");
    assert!(lua.contains("== 1"), "{lua}");
}

#[test]
fn match_return_arm_block_with_explicit_return_does_not_append_return_nil() {
    let output = compile(
        "enum Op repr string { A = \"A\", B = \"B\" }\nfn bounds(op) {\n  match op {\n    Op.A => { return 1, 2, 3, 4 }\n    _ => { return 0, 0, 0, 0 }\n  }\n}",
    );
    let lua = output.lua;
    assert!(lua.contains("return 1, 2, 3, 4"), "{lua}");
    assert!(lua.contains("return 0, 0, 0, 0"), "{lua}");
    assert!(
        !lua.contains("return 1, 2, 3, 4\n      return nil"),
        "{lua}"
    );
    assert!(
        !lua.contains("return 0, 0, 0, 0\n      return nil"),
        "{lua}"
    );
}

#[test]
fn match_tag_hoist_ignores_unreachable_arms() {
    let output = compile_with_warnings(
        "enum Fill repr table(tag = \"kind\") { Solid(kind = FILL_SOLID) }\nenum Other repr existing(kind = \"type\") { Missing(kind = OTHER_MISSING) }\nfn demo(fill) = match fill { Fill.Solid => one() _ => fallback() Other.Missing => other() }",
    );
    let lua = output.lua;
    assert_eq!(lua.matches(".kind").count(), 1, "{lua}");
    assert_eq!(lua.matches(".type").count(), 0, "{lua}");
    assert!(lua.contains("one()"), "{lua}");
    assert!(!lua.contains("fallback()"), "{lua}");
    assert!(!lua.contains("other()"), "{lua}");
}

#[test]
fn existing_enum_match_with_fallback_reads_tag_nil_safely() {
    let output = compile(
        "enum Fill repr existing(kind = \"kind\") { Solid(kind = FILL_SOLID) }\nfn demo(fill) = match fill { Fill.Solid => one() _ => fallback() }",
    );
    let lua = output.lua;
    assert!(
        lua.contains("local __lux_tag_2\n  if __lux_match_1 ~= nil then"),
        "{lua}"
    );
    assert!(lua.contains("__lux_tag_2 = __lux_match_1.kind"), "{lua}");
    assert!(lua.contains("else\n    return fallback()"), "{lua}");

    let output = compile(
        "enum Fill repr existing(kind = \"kind\") { Solid(kind = FILL_SOLID) }\nfn demo(fill) = match fill { Fill.Solid => one() }",
    );
    let lua = output.lua;
    assert!(
        lua.contains("local __lux_tag_2 = __lux_match_1.kind"),
        "{lua}"
    );
    assert!(!lua.contains("if __lux_match_1 ~= nil then"), "{lua}");
}
