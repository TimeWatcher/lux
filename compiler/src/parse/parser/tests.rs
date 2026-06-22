use crate::ast::{
    BinaryOp, BindingMode, ChainSegmentKind, EnumRepr, ExportKind, ExprKind, FunctionBody,
    FunctionName, PartOrderKind, PartOrderRelation, StmtKind,
};
use crate::lex::Lexer;
use crate::source::SourceFile;

use super::Parser;

fn parse(input: &str) -> crate::ast::Module {
    let file = SourceFile::new(0, None, input);
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    parsed.module
}

fn parse_diagnostics(input: &str) -> Vec<crate::diag::Diagnostic> {
    let file = SourceFile::new(0, None, input);
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    Parser::new(&lex.tokens).parse_module().diagnostics
}

#[test]
fn parses_simple_and_method_functions() {
    let module = parse("fn helper(x) = x + 1\nfn PANEL:Paint(w, h) { drawBody(self, w, h) }");
    assert_eq!(module.body.len(), 2);

    match &module.body[0].kind {
        StmtKind::FunctionDecl(decl) => {
            assert!(matches!(decl.name, FunctionName::Simple(_)));
        }
        other => panic!("unexpected stmt: {other:#?}"),
    }

    match &module.body[1].kind {
        StmtKind::FunctionDecl(decl) => {
            assert!(matches!(decl.name, FunctionName::Method { .. }));
        }
        other => panic!("unexpected stmt: {other:#?}"),
    }
}

#[test]
fn parses_lua_style_functions_and_blocks() {
    let module = parse(
        "local function helper(x)\n  if x > 0 then\n    return x\n  elseif x == 0 then\n    return 0\n  else\n    return -x\n  end\nend\n\nfunction PANEL:Paint(w, h)\n  while self:Visible() do\n    break\n  end\nend",
    );
    assert_eq!(module.body.len(), 2);

    let StmtKind::FunctionDecl(helper) = &module.body[0].kind else {
        panic!("expected local function");
    };
    assert!(matches!(helper.name, FunctionName::Simple(_)));
    let FunctionBody::Block(block) = &helper.body else {
        panic!("expected function block");
    };
    assert!(block.tail.is_none());
    assert!(matches!(
        block.statements.first().map(|stmt| &stmt.kind),
        Some(StmtKind::If { .. })
    ));

    let StmtKind::FunctionDecl(method) = &module.body[1].kind else {
        panic!("expected method function");
    };
    assert!(matches!(method.name, FunctionName::Method { .. }));
}

#[test]
fn parses_lua_style_function_expressions() {
    let module = parse("local f = function(x)\n  return x + 1\nend");
    let StmtKind::LocalDecl { values, .. } = &module.body[0].kind else {
        panic!("expected local decl");
    };
    assert!(matches!(
        values.first().map(|expr| &expr.kind),
        Some(ExprKind::Function(_))
    ));
}

#[test]
fn parses_arrow_params_with_trailing_comma() {
    let module = parse("local f = (sender, ) => {}");
    let StmtKind::LocalDecl { values, .. } = &module.body[0].kind else {
        panic!("expected local decl");
    };
    let Some(ExprKind::Function(function)) = values.first().map(|expr| &expr.kind) else {
        panic!("expected arrow function");
    };
    assert_eq!(function.params.len(), 1);
    assert_eq!(function.params[0].name.name, "sender");
}

#[test]
fn parses_table_enum_with_explicit_tag_field() {
    let module = parse("enum Fill repr table(tag = \"__tag\") { Solid(tag = 1, color: Color) }");
    let StmtKind::EnumDecl(decl) = &module.body[0].kind else {
        panic!("expected enum decl");
    };
    assert_eq!(
        decl.repr,
        EnumRepr::Table {
            tag_field: "__tag".into()
        }
    );
}

#[test]
fn rejects_removed_tagged_enum_repr() {
    let diagnostics = parse_diagnostics("enum Fill repr tagged { Solid }");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("PARSE022")),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown enum repr `tagged`")),
        "{diagnostics:#?}"
    );
}

#[test]
fn parses_multi_target_assignments() {
    let module = parse("x, y = panel:LocalToScreen(0, 0)\ntbl.a, tbl[b] = f(), g()");
    assert_eq!(module.body.len(), 2);

    let StmtKind::Assign { targets, values } = &module.body[0].kind else {
        panic!("expected assignment statement");
    };
    assert_eq!(targets.len(), 2);
    assert_eq!(values.len(), 1);

    let StmtKind::Assign { targets, values } = &module.body[1].kind else {
        panic!("expected assignment statement");
    };
    assert_eq!(targets.len(), 2);
    assert_eq!(values.len(), 2);
}

#[test]
fn parses_tail_call_chains() {
    let module = parse("Foo { a = 1 } { b = 2 }.baz");
    let StmtKind::Expr(expr) = &module.body[0].kind else {
        panic!("expected expression statement");
    };
    let ExprKind::Chain(chain) = &expr.kind else {
        panic!("expected chain");
    };
    assert_eq!(chain.segments.len(), 3);
}

#[test]
fn newline_table_after_local_initializer_is_block_tail() {
    let module = parse("fn demo() {\n  local resolved = make()\n  { style = resolved }\n}");
    let StmtKind::FunctionDecl(decl) = &module.body[0].kind else {
        panic!("expected function");
    };
    let FunctionBody::Block(block) = &decl.body else {
        panic!("expected block body");
    };
    assert_eq!(block.statements.len(), 1, "{block:#?}");
    assert!(block.tail.is_some(), "{block:#?}");

    let StmtKind::LocalDecl { values, .. } = &block.statements[0].kind else {
        panic!("expected local decl");
    };
    let ExprKind::Chain(chain) = &values[0].kind else {
        panic!("expected call expression");
    };
    assert_eq!(chain.segments.len(), 1, "{chain:#?}");
}

#[test]
fn control_condition_does_not_consume_body_as_tail_table_call() {
    let module = parse(
        "if obj?:ok() { done() }\nwhile player?:Alive() { tick() }\nfor k, v in pairs(t) { use(k, v) }\nfor i = 1, max() { use(i) }",
    );
    assert_eq!(module.body.len(), 4);

    let StmtKind::If { condition, .. } = &module.body[0].kind else {
        panic!("expected if statement");
    };
    let ExprKind::Chain(chain) = &condition.kind else {
        panic!("expected if condition chain");
    };
    assert_eq!(chain.segments.len(), 1);

    let StmtKind::While { condition, .. } = &module.body[1].kind else {
        panic!("expected while statement");
    };
    let ExprKind::Chain(chain) = &condition.kind else {
        panic!("expected while condition chain");
    };
    assert_eq!(chain.segments.len(), 1);

    let StmtKind::GenericFor { iter, .. } = &module.body[2].kind else {
        panic!("expected generic for");
    };
    let ExprKind::Chain(chain) = &iter[0].kind else {
        panic!("expected for iterator chain");
    };
    assert_eq!(chain.segments.len(), 1);

    let StmtKind::NumericFor { end, .. } = &module.body[3].kind else {
        panic!("expected numeric for");
    };
    let ExprKind::Chain(chain) = &end.kind else {
        panic!("expected numeric for end chain");
    };
    assert_eq!(chain.segments.len(), 1);
}

#[test]
fn trailing_semicolon_suppresses_block_tail() {
    let module = parse("fn yes() { 1 }\nfn no() { 1; }");

    let StmtKind::FunctionDecl(yes) = &module.body[0].kind else {
        panic!("expected function");
    };
    let FunctionBody::Block(yes_block) = &yes.body else {
        panic!("expected block body");
    };
    assert!(yes_block.tail.is_some());
    assert!(yes_block.statements.is_empty());

    let StmtKind::FunctionDecl(no) = &module.body[1].kind else {
        panic!("expected function");
    };
    let FunctionBody::Block(no_block) = &no.body else {
        panic!("expected block body");
    };
    assert!(no_block.tail.is_none());
    assert!(matches!(
        no_block.statements.first().map(|stmt| &stmt.kind),
        Some(StmtKind::Expr(_))
    ));
}

#[test]
fn final_if_else_without_semicolon_is_block_tail_expression() {
    let module = parse(
        "fn choose(ok) { if ok { 1 } else { 0 } }\nfn no(ok) { if ok { 1 } else { 0 }; }\nfn stmt(ok) { if ok { 1 } }",
    );

    let StmtKind::FunctionDecl(choose) = &module.body[0].kind else {
        panic!("expected function");
    };
    let FunctionBody::Block(choose_block) = &choose.body else {
        panic!("expected block body");
    };
    assert!(matches!(
        choose_block.tail.as_ref().map(|expr| &expr.kind),
        Some(ExprKind::Conditional { .. })
    ));
    assert!(choose_block.statements.is_empty());

    let StmtKind::FunctionDecl(no) = &module.body[1].kind else {
        panic!("expected function");
    };
    let FunctionBody::Block(no_block) = &no.body else {
        panic!("expected block body");
    };
    assert!(no_block.tail.is_none());
    assert!(matches!(
        no_block.statements.first().map(|stmt| &stmt.kind),
        Some(StmtKind::If { .. })
    ));

    let StmtKind::FunctionDecl(stmt) = &module.body[2].kind else {
        panic!("expected function");
    };
    let FunctionBody::Block(stmt_block) = &stmt.body else {
        panic!("expected block body");
    };
    assert!(stmt_block.tail.is_none());
    assert!(matches!(
        stmt_block.statements.first().map(|stmt| &stmt.kind),
        Some(StmtKind::If {
            else_block: None,
            ..
        })
    ));
}

#[test]
fn distinguishes_safe_dot_call_and_optional_member_call() {
    let module = parse("obj?.name(args); (obj?.name)(args)");
    assert_eq!(module.body.len(), 2);

    let StmtKind::Expr(first) = &module.body[0].kind else {
        panic!("expected expression");
    };
    let ExprKind::Chain(chain) = &first.kind else {
        panic!("expected chain");
    };
    assert!(matches!(
        chain.segments.first().map(|segment| &segment.kind),
        Some(ChainSegmentKind::SafeDotCall { .. })
    ));

    let StmtKind::Expr(second) = &module.body[1].kind else {
        panic!("expected expression");
    };
    let ExprKind::Chain(chain) = &second.kind else {
        panic!("expected chain");
    };
    assert!(matches!(
        chain.segments.last().map(|segment| &segment.kind),
        Some(ChainSegmentKind::Call { .. })
    ));
}

#[test]
fn parses_safe_index_followed_by_normal_call() {
    let module = parse("tbl?.[key](args)");
    let StmtKind::Expr(expr) = &module.body[0].kind else {
        panic!("expected expression");
    };
    let ExprKind::Chain(chain) = &expr.kind else {
        panic!("expected chain");
    };
    assert!(matches!(
        chain.segments.first().map(|segment| &segment.kind),
        Some(ChainSegmentKind::Index { optional: true, .. })
    ));
    assert!(matches!(
        chain.segments.last().map(|segment| &segment.kind),
        Some(ChainSegmentKind::Call { .. })
    ));
}

#[test]
fn parses_safe_function_call() {
    let module = parse("func?(arg)");
    let StmtKind::Expr(expr) = &module.body[0].kind else {
        panic!("expected expression");
    };
    let ExprKind::Chain(chain) = &expr.kind else {
        panic!("expected chain");
    };
    assert!(matches!(
        chain.segments.last().map(|segment| &segment.kind),
        Some(ChainSegmentKind::SafeCall { .. })
    ));
}

#[test]
fn rejects_old_safe_method_spelling() {
    let diagnostics = parse_diagnostics("obj:?call()");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("PARSE004")),
        "{diagnostics:#?}"
    );
}

#[test]
fn parses_import_and_export() {
    let module = parse(
        "import { arr } from \"lux/std\"\nimport \"setup\"\nexport fn foo() = 1\nexport const answer = 42\nexport { foo }",
    );
    assert_eq!(module.body.len(), 5);
    assert!(matches!(
        &module.body[3].kind,
        StmtKind::ExportDecl {
            stmt,
            ..
        } if matches!(
            &stmt.kind,
            StmtKind::LocalDecl {
                mode: BindingMode::Const,
                ..
            }
        )
    ));
}

#[test]
fn parses_const_declarations_and_requires_initializer() {
    let module = parse("const value = 1\nconst { name } = player");
    assert!(matches!(
        &module.body[0].kind,
        StmtKind::LocalDecl {
            mode: BindingMode::Const,
            ..
        }
    ));
    assert!(matches!(
        &module.body[1].kind,
        StmtKind::LocalDestructure {
            mode: BindingMode::Const,
            ..
        }
    ));

    let diagnostics = parse_diagnostics("const missing");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("PARSE016"))
    );
}

#[test]
fn parses_phase_qualified_exports() {
    let module = parse(
        "export host package { target = \"lux/ui\", runtime = \"lux/ui\" }\nexport macro fn dbg(ctx, call) = nil\nexport host expr fn fold(ctx, call) = nil",
    );
    assert_eq!(module.body.len(), 3);
    assert!(matches!(module.body[0].kind, StmtKind::HostPackageDecl(_)));
    assert!(matches!(
        module.body[1].kind,
        StmtKind::ExportDecl {
            kind: ExportKind::Macro,
            ..
        }
    ));
    assert!(matches!(
        module.body[2].kind,
        StmtKind::ExportDecl {
            kind: ExportKind::HostExpr,
            ..
        }
    ));
}

#[test]
fn parses_part_order_declarations() {
    let module = parse(
        "part order { \"cl_base\", \"client/install\" }\npart before \"cl_install\"\npart after \"cl_base\"",
    );
    assert_eq!(module.body.len(), 3);

    let StmtKind::PartOrderDecl(order) = &module.body[0].kind else {
        panic!("expected part order decl");
    };
    assert!(matches!(
        &order.kind,
        PartOrderKind::Order { targets }
            if targets == &vec!["cl_base".to_string(), "client/install".to_string()]
    ));

    let StmtKind::PartOrderDecl(before) = &module.body[1].kind else {
        panic!("expected part before decl");
    };
    assert!(matches!(
        &before.kind,
        PartOrderKind::Relative {
            relation: PartOrderRelation::Before,
            target,
        } if target == "cl_install"
    ));
}

#[test]
fn rejects_unparenthesized_coalesce_mixed_with_comparison() {
    let diagnostics = parse_diagnostics("fn choose(player) = player?:GetExp() ?? 0 > 5");
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("PARSE009")
            && diagnostic
                .message
                .contains("ambiguous use of `??` with a comparison")
    }));
}

#[test]
fn allows_parenthesized_coalesce_mixed_with_comparison() {
    let module = parse("fn choose(player) = (player?:GetExp() ?? 0) > 5");
    assert_eq!(module.body.len(), 1);
}

#[test]
fn allows_coalesce_with_parenthesized_comparison() {
    let module = parse("fn choose(player) = player?:GetExp() ?? (0 > 5)");
    assert_eq!(module.body.len(), 1);
}

#[test]
fn parses_pipeline_placeholder_and_modulo_distinctly() {
    let module = parse("fn pipe(x) = x |> clamp(0, %, 100)\nfn modulo(a, b) = a % b");
    assert_eq!(module.body.len(), 2);

    let StmtKind::FunctionDecl(pipe) = &module.body[0].kind else {
        panic!("expected function");
    };
    let FunctionBody::Expr(expr) = &pipe.body else {
        panic!("expected expression body");
    };
    assert!(matches!(
        &expr.kind,
        ExprKind::Binary {
            op: BinaryOp::Pipe,
            ..
        }
    ));

    let StmtKind::FunctionDecl(modulo) = &module.body[1].kind else {
        panic!("expected function");
    };
    let FunctionBody::Expr(expr) = &modulo.body else {
        panic!("expected expression body");
    };
    assert!(matches!(
        &expr.kind,
        ExprKind::Binary {
            op: BinaryOp::Mod,
            ..
        }
    ));
}

#[test]
fn rejects_pipeline_without_placeholder() {
    let diagnostics = parse_diagnostics("fn pipe(x) = x |> f(1)");
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("PARSE014")
            && diagnostic.message.contains("pipeline RHS must contain `%`")
    }));
}

#[test]
fn rejects_pipeline_placeholder_inside_arrow_expr_body() {
    let diagnostics = parse_diagnostics("fn bad(xs) = xs |> arr.map(%, (x) => x + %)");
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("PARSE015")
            && diagnostic
                .message
                .contains("`%` pipeline placeholder is only valid")
    }));
}

#[test]
fn rejects_pipeline_placeholder_inside_arrow_block_body() {
    let diagnostics = parse_diagnostics("fn bad(xs) = xs |> arr.map(%, (x) => { x + % })");
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("PARSE015")
            && diagnostic
                .message
                .contains("`%` pipeline placeholder is only valid")
    }));
}

#[test]
fn recovers_to_next_statement_after_parse_error() {
    let file = SourceFile::new(0, None, "local = broken\nfn ok() = 1");
    let lex = Lexer::new(&file).lex_all();
    assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
    let parsed = Parser::new(&lex.tokens).parse_module();

    assert!(parsed.has_errors());
    assert!(
        parsed
            .module
            .body
            .iter()
            .any(|stmt| matches!(stmt.kind, StmtKind::FunctionDecl(_)))
    );
}
