use crate::ast::{BinaryOp, CompoundAssignOp, UnaryOp};
use crate::ir::{IrExpr, IrExprKind, IrParam};

use super::{BinaryOperandSide, LuaPrecedence};

const READABLE_LINE_WIDTH: usize = 88;

pub(super) fn dotted_name(path: &[crate::ast::Identifier]) -> String {
    path.iter()
        .map(|part| part.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

pub(super) fn param_list(params: &[IrParam], vararg: bool) -> String {
    let mut parts = params
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    if vararg {
        parts.push("...".into());
    }
    parts.join(", ")
}

pub(super) fn lua_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

pub(super) fn callable_expr(value: &str) -> String {
    if value.starts_with("function(") {
        format!("({value})")
    } else {
        value.to_string()
    }
}

pub(super) fn chain_prefix_expr(value: &str, precedence: LuaPrecedence) -> String {
    if precedence < LuaPrecedence::Primary
        || value.starts_with("function(")
        || value.starts_with('{')
    {
        format!("({value})")
    } else {
        value.to_string()
    }
}

pub(super) fn format_lua_table(fields: &[String]) -> String {
    if fields.is_empty() {
        return "{}".into();
    }

    let inline = format!("{{ {} }}", fields.join(", "));
    if inline.len() <= READABLE_LINE_WIDTH && !fields.iter().any(|field| field.contains('\n')) {
        return inline;
    }

    let body = fields
        .iter()
        .map(|field| format!("{},", indent_multiline(field, 1)))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{{\n{body}\n}}")
}

pub(super) fn format_lua_call(callee: &str, args: &[String]) -> String {
    if args.is_empty() {
        return format!("{callee}()");
    }

    let inline = format!("{callee}({})", args.join(", "));
    if inline.len() <= READABLE_LINE_WIDTH && !args.iter().any(|arg| arg.contains('\n')) {
        return inline;
    }

    let body = args
        .iter()
        .map(|arg| indent_multiline(arg, 1))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{callee}(\n{body}\n)")
}

pub(super) fn indent_multiline(value: &str, levels: usize) -> String {
    let prefix = "  ".repeat(levels);
    value
        .lines()
        .map(|line| {
            if line.is_empty() {
                line.to_string()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn binary_precedence(op: BinaryOp) -> LuaPrecedence {
    match op {
        BinaryOp::Or => LuaPrecedence::Or,
        BinaryOp::And => LuaPrecedence::And,
        BinaryOp::Eq
        | BinaryOp::NotEq
        | BinaryOp::Lt
        | BinaryOp::LtEq
        | BinaryOp::Gt
        | BinaryOp::GtEq => LuaPrecedence::Compare,
        BinaryOp::Concat => LuaPrecedence::Concat,
        BinaryOp::Add | BinaryOp::Sub => LuaPrecedence::Add,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => LuaPrecedence::Mul,
        BinaryOp::Pow => LuaPrecedence::Power,
        BinaryOp::Coalesce | BinaryOp::Pipe => {
            unreachable!("custom-lowered operators do not have Lua precedence")
        }
    }
}

pub(super) fn format_lua_binary(op: BinaryOp, left: &str, right: &str) -> String {
    let operator = lua_binary_op(op);
    let inline = format!("{left} {operator} {right}");
    if op != BinaryOp::Concat || inline.len() <= READABLE_LINE_WIDTH {
        return inline;
    }

    format!("{left} {operator}\n{}", indent_multiline(right, 1))
}

pub(super) fn format_lua_concat_parts(parts: &[String]) -> String {
    let inline = parts.join(" .. ");
    if inline.len() <= READABLE_LINE_WIDTH {
        return inline;
    }

    let Some((first, rest)) = parts.split_first() else {
        return inline;
    };

    let mut out = first.clone();
    for part in rest {
        out.push_str(" ..\n");
        out.push_str(&indent_multiline(part, 1));
    }
    out
}

pub(super) fn parenthesize_unary_operand(
    op: UnaryOp,
    value: String,
    precedence: LuaPrecedence,
) -> String {
    if precedence < LuaPrecedence::Unary || (op == UnaryOp::Neg && value.starts_with('-')) {
        format!("({value})")
    } else {
        value
    }
}

pub(super) fn parenthesize_binary_operand(
    value: String,
    precedence: LuaPrecedence,
    parent_op: BinaryOp,
    side: BinaryOperandSide,
) -> String {
    let parent_precedence = binary_precedence(parent_op);
    let same_precedence_needs_parens = match side {
        BinaryOperandSide::Left => matches!(parent_op, BinaryOp::Pow | BinaryOp::Concat),
        BinaryOperandSide::Right => !matches!(parent_op, BinaryOp::Pow | BinaryOp::Concat),
    };

    if precedence < parent_precedence
        || (precedence == parent_precedence && same_precedence_needs_parens)
    {
        format!("({value})")
    } else {
        value
    }
}

pub(super) fn lua_binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Pow => "^",
        BinaryOp::Concat => "..",
        BinaryOp::Eq => "==",
        BinaryOp::NotEq => "~=",
        BinaryOp::Lt => "<",
        BinaryOp::LtEq => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::GtEq => ">=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        BinaryOp::Coalesce => unreachable!("coalesce uses custom lowering"),
        BinaryOp::Pipe => unreachable!("pipeline uses custom lowering"),
    }
}

pub(super) fn is_ordering_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq
    )
}

pub(super) fn lua_compound_op(op: CompoundAssignOp) -> &'static str {
    match op {
        CompoundAssignOp::Add => "+",
        CompoundAssignOp::Sub => "-",
        CompoundAssignOp::Mul => "*",
        CompoundAssignOp::Div => "/",
        CompoundAssignOp::Mod => "%",
        CompoundAssignOp::Pow => "^",
        CompoundAssignOp::Concat => "..",
    }
}

pub(super) fn force_tail_single_value_expr(expr: &IrExpr, value: String) -> String {
    if matches!(expr.kind, IrExprKind::Vararg) {
        "(...)".into()
    } else {
        value
    }
}
