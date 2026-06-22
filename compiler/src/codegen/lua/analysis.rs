use std::collections::{BTreeMap, BTreeSet};

use crate::ast::FunctionName;
use crate::ir::*;

use super::{FinalCallKind, PendingLine};

pub(super) fn block_has_continue_for_current_loop(block: &IrBlock) -> bool {
    block
        .statements
        .iter()
        .any(stmt_has_continue_for_current_loop)
}

pub(super) fn stmt_has_continue_for_current_loop(stmt: &IrStmt) -> bool {
    match &stmt.kind {
        IrStmtKind::Continue => true,
        IrStmtKind::If {
            then_block,
            else_block,
            ..
        } => {
            block_has_continue_for_current_loop(then_block)
                || else_block
                    .as_ref()
                    .is_some_and(block_has_continue_for_current_loop)
        }
        IrStmtKind::Do(block) => block_has_continue_for_current_loop(block),
        IrStmtKind::While { .. }
        | IrStmtKind::NumericFor { .. }
        | IrStmtKind::GenericFor { .. }
        | IrStmtKind::RepeatUntil { .. }
        | IrStmtKind::FunctionDecl(_) => false,
        _ => false,
    }
}

pub(super) fn block_has_break_for_current_loop(block: &IrBlock) -> bool {
    block.statements.iter().any(stmt_has_break_for_current_loop)
}

pub(super) fn block_ends_with_terminal_stmt(block: &IrBlock) -> bool {
    block
        .statements
        .last()
        .is_some_and(stmt_is_terminal_for_lua_block)
}

fn stmt_is_terminal_for_lua_block(stmt: &IrStmt) -> bool {
    matches!(
        stmt.kind,
        IrStmtKind::Return(_) | IrStmtKind::Break | IrStmtKind::Continue
    )
}

pub(super) fn stmt_has_break_for_current_loop(stmt: &IrStmt) -> bool {
    match &stmt.kind {
        IrStmtKind::Break => true,
        IrStmtKind::If {
            then_block,
            else_block,
            ..
        } => {
            block_has_break_for_current_loop(then_block)
                || else_block
                    .as_ref()
                    .is_some_and(block_has_break_for_current_loop)
        }
        IrStmtKind::Do(block) => block_has_break_for_current_loop(block),
        IrStmtKind::While { .. }
        | IrStmtKind::NumericFor { .. }
        | IrStmtKind::GenericFor { .. }
        | IrStmtKind::RepeatUntil { .. }
        | IrStmtKind::FunctionDecl(_) => false,
        _ => false,
    }
}

pub(super) fn collect_reserved_names(module: &IrModule) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_stmt_names(&module.body, &mut names);
    names
}

pub(super) fn collect_stmt_names(stmts: &[IrStmt], names: &mut BTreeSet<String>) {
    for stmt in stmts {
        match &stmt.kind {
            IrStmtKind::Noop => {}
            IrStmtKind::Continue => {}
            IrStmtKind::EnumDecl(decl) => {
                if decl.runtime {
                    names.insert(decl.name.clone());
                }
            }
            IrStmtKind::LocalDecl {
                names: local_names, ..
            } => {
                names.extend(local_names.iter().cloned());
            }
            IrStmtKind::LocalDestructure {
                patterns, values, ..
            } => {
                names.extend(collect_pattern_names(patterns));
                for value in values {
                    collect_expr_names(value, names);
                }
            }
            IrStmtKind::FunctionDecl(decl) => {
                if let FunctionName::Simple(name) = &decl.name {
                    names.insert(name.name.clone());
                }
                names.extend(decl.params.iter().map(|param| param.name.clone()));
                collect_function_body_names(&decl.body, names);
            }
            IrStmtKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block_names(then_block, names);
                if let Some(block) = else_block {
                    collect_block_names(block, names);
                }
            }
            IrStmtKind::While { body, .. }
            | IrStmtKind::RepeatUntil { body, .. }
            | IrStmtKind::Do(body)
            | IrStmtKind::NumericFor { body, .. }
            | IrStmtKind::GenericFor { body, .. } => collect_block_names(body, names),
            IrStmtKind::Import { specifiers, .. } => {
                names.extend(specifiers.iter().map(|specifier| specifier.local.clone()));
            }
            _ => {}
        }
    }
}

pub(super) fn collect_block_names(block: &IrBlock, names: &mut BTreeSet<String>) {
    collect_stmt_names(&block.statements, names);
}

pub(super) fn collect_pattern_names(patterns: &[IrPattern]) -> Vec<String> {
    let mut names = Vec::new();
    for pattern in patterns {
        collect_pattern_name(pattern, &mut names);
    }
    names
}

pub(super) fn collect_pattern_name(pattern: &IrPattern, names: &mut Vec<String>) {
    match &pattern.kind {
        IrPatternKind::Identifier(name) => names.push(name.clone()),
        IrPatternKind::Object(fields) => {
            for field in fields {
                collect_pattern_name(&field.pattern, names);
            }
        }
        IrPatternKind::Array(items) => {
            for item in items {
                collect_pattern_name(&item.pattern, names);
            }
        }
    }
}

pub(super) fn collect_expr_names(expr: &IrExpr, names: &mut BTreeSet<String>) {
    match &expr.kind {
        IrExprKind::Function(function) => {
            names.extend(function.params.iter().map(|param| param.name.clone()));
            collect_function_body_names(&function.body, names);
        }
        IrExprKind::Table(fields) => {
            for field in fields {
                match &field.kind {
                    IrTableFieldKind::Array(expr) | IrTableFieldKind::Spread(expr) => {
                        collect_expr_names(expr, names)
                    }
                    IrTableFieldKind::Named { value, .. } => collect_expr_names(value, names),
                    IrTableFieldKind::ExprKey { key, value } => {
                        collect_expr_names(key, names);
                        collect_expr_names(value, names);
                    }
                }
            }
        }
        IrExprKind::Match(match_expr) => {
            collect_expr_names(&match_expr.subject, names);
            for arm in &match_expr.arms {
                collect_match_pattern_names(&arm.pattern, names);
                collect_expr_or_block_names(&arm.body, names);
            }
        }
        _ => {}
    }
}

pub(super) fn collect_expr_or_block_names(item: &IrExprOrBlock, names: &mut BTreeSet<String>) {
    match item {
        IrExprOrBlock::Expr(expr) => collect_expr_names(expr, names),
        IrExprOrBlock::Block(block) => collect_block_names(block, names),
    }
}

pub(super) fn collect_match_pattern_names(pattern: &IrMatchPattern, names: &mut BTreeSet<String>) {
    match &pattern.kind {
        IrMatchPatternKind::Or(patterns) => {
            for pattern in patterns {
                collect_match_pattern_names(pattern, names);
            }
        }
        IrMatchPatternKind::Binding(name) => {
            names.insert(name.clone());
        }
        IrMatchPatternKind::Variant { payload, .. } => {
            if let Some(payload) = payload {
                match payload {
                    IrMatchPatternPayload::Tuple(patterns) => {
                        for pattern in patterns {
                            collect_match_pattern_names(pattern, names);
                        }
                    }
                    IrMatchPatternPayload::Record(fields) => {
                        for field in fields {
                            collect_match_pattern_names(&field.pattern, names);
                        }
                    }
                }
            }
        }
        IrMatchPatternKind::Object(fields) => {
            for field in fields {
                collect_match_pattern_names(&field.pattern, names);
            }
        }
        IrMatchPatternKind::Array(items) => {
            for item in items {
                collect_match_pattern_names(&item.pattern, names);
            }
        }
        IrMatchPatternKind::Wildcard | IrMatchPatternKind::Literal(_) => {}
    }
}

pub(super) fn collect_function_body_names(body: &IrFunctionBody, names: &mut BTreeSet<String>) {
    if let IrFunctionBody::Block(block) = body {
        collect_block_names(block, names);
    }
}

#[derive(Debug, Clone)]
pub(super) struct FunctionHoist {
    pub(super) name: String,
    pub(super) origin: Origin,
}

pub(super) fn collect_simple_function_hoists(stmts: &[IrStmt]) -> Vec<FunctionHoist> {
    let mut hoists = BTreeMap::<String, Origin>::new();
    for stmt in stmts {
        if let IrStmtKind::FunctionDecl(decl) = &stmt.kind {
            if let FunctionName::Simple(name) = &decl.name {
                hoists
                    .entry(name.name.clone())
                    .or_insert_with(|| Origin::Synthetic {
                        source: stmt.origin.span(),
                        reason: "hoisted function declaration".into(),
                    });
            }
        }
    }
    hoists
        .into_iter()
        .map(|(name, origin)| FunctionHoist { name, origin })
        .collect()
}

pub(super) fn final_call_kind(expr: &IrExpr) -> Option<FinalCallKind> {
    let IrExprKind::Chain(chain) = &expr.kind else {
        return None;
    };

    match chain.segments.last().map(|segment| &segment.kind) {
        Some(IrChainSegmentKind::Call { .. }) => Some(FinalCallKind::Inline),
        Some(IrChainSegmentKind::MethodCall {
            optional: false, ..
        }) => Some(FinalCallKind::Inline),
        Some(IrChainSegmentKind::SafeCall { .. })
        | Some(IrChainSegmentKind::SafeDotCall { .. })
        | Some(IrChainSegmentKind::MethodCall { optional: true, .. }) => {
            Some(FinalCallKind::AlreadyEmittedInSetup)
        }
        _ => None,
    }
}

pub(super) fn scalar_enum_variant_chain_path(chain: &IrChain) -> Option<Vec<String>> {
    let IrExprKind::Identifier(base) = &chain.base.kind else {
        return None;
    };

    let mut path = vec![base.clone()];
    for segment in &chain.segments {
        let IrChainSegmentKind::Member {
            name,
            optional: false,
        } = &segment.kind
        else {
            return None;
        };
        path.push(name.clone());
    }

    (path.len() >= 2).then_some(path)
}

pub(super) fn indent_pending(lines: Vec<PendingLine>, levels: usize) -> Vec<PendingLine> {
    let prefix = "  ".repeat(levels);
    lines
        .into_iter()
        .map(|mut line| {
            if !line.text.is_empty() {
                line.text = format!("{prefix}{}", line.text);
            }
            line
        })
        .collect()
}

pub(super) fn is_stable_place_component(expr: &IrExpr) -> bool {
    match &expr.kind {
        IrExprKind::Identifier(_)
        | IrExprKind::Nil
        | IrExprKind::Boolean(_)
        | IrExprKind::Number(_)
        | IrExprKind::String(_) => true,
        IrExprKind::Chain(chain) => {
            is_stable_place_component(&chain.base)
                && chain.segments.iter().all(|segment| match &segment.kind {
                    IrChainSegmentKind::Member {
                        optional: false, ..
                    } => true,
                    IrChainSegmentKind::Index {
                        index,
                        optional: false,
                    } => is_stable_place_component(index),
                    IrChainSegmentKind::Member { optional: true, .. }
                    | IrChainSegmentKind::Index { optional: true, .. }
                    | IrChainSegmentKind::Call { .. }
                    | IrChainSegmentKind::SafeCall { .. }
                    | IrChainSegmentKind::SafeDotCall { .. }
                    | IrChainSegmentKind::MethodCall { .. } => false,
                })
        }
        IrExprKind::Vararg
        | IrExprKind::PipelinePlaceholder
        | IrExprKind::Template(_)
        | IrExprKind::Table(_)
        | IrExprKind::Unary { .. }
        | IrExprKind::Binary { .. }
        | IrExprKind::Conditional { .. }
        | IrExprKind::Match(_)
        | IrExprKind::Do(_)
        | IrExprKind::Function(_) => false,
    }
}

pub(super) fn exprs_reference_any_names(exprs: &[IrExpr], names: &[String]) -> bool {
    let names = names.iter().map(String::as_str).collect::<BTreeSet<_>>();
    exprs
        .iter()
        .any(|expr| expr_references_any_name(expr, &names))
}

pub(super) fn expr_references_any_name(expr: &IrExpr, names: &BTreeSet<&str>) -> bool {
    match &expr.kind {
        IrExprKind::Identifier(name) => names.contains(name.as_str()),
        IrExprKind::Nil
        | IrExprKind::Boolean(_)
        | IrExprKind::Number(_)
        | IrExprKind::String(_)
        | IrExprKind::Vararg
        | IrExprKind::PipelinePlaceholder => false,
        IrExprKind::Template(parts) => parts.iter().any(|part| match &part.kind {
            IrTemplatePartKind::Text(_) => false,
            IrTemplatePartKind::Expr(expr) => expr_references_any_name(expr, names),
        }),
        IrExprKind::Table(fields) => fields.iter().any(|field| match &field.kind {
            IrTableFieldKind::Array(expr) | IrTableFieldKind::Spread(expr) => {
                expr_references_any_name(expr, names)
            }
            IrTableFieldKind::Named { value, .. } => expr_references_any_name(value, names),
            IrTableFieldKind::ExprKey { key, value } => {
                expr_references_any_name(key, names) || expr_references_any_name(value, names)
            }
        }),
        IrExprKind::Unary { argument, .. } => expr_references_any_name(argument, names),
        IrExprKind::Binary { left, right, .. } => {
            expr_references_any_name(left, names) || expr_references_any_name(right, names)
        }
        IrExprKind::Conditional {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_references_any_name(condition, names)
                || expr_or_block_references_any_name(then_branch, names)
                || expr_or_block_references_any_name(else_branch, names)
        }
        IrExprKind::Match(match_expr) => {
            expr_references_any_name(&match_expr.subject, names)
                || match_expr
                    .arms
                    .iter()
                    .any(|arm| expr_or_block_references_any_name(&arm.body, names))
        }
        IrExprKind::Do(block) => block_references_any_name(block, names),
        IrExprKind::Function(function) => function_body_references_any_name(&function.body, names),
        IrExprKind::Chain(chain) => {
            expr_references_any_name(&chain.base, names)
                || chain.segments.iter().any(|segment| match &segment.kind {
                    IrChainSegmentKind::Member { .. } => false,
                    IrChainSegmentKind::Index { index, .. } => {
                        expr_references_any_name(index, names)
                    }
                    IrChainSegmentKind::Call { args, .. }
                    | IrChainSegmentKind::SafeCall { args, .. }
                    | IrChainSegmentKind::SafeDotCall { args, .. }
                    | IrChainSegmentKind::MethodCall { args, .. } => {
                        args.iter().any(|arg| expr_references_any_name(arg, names))
                    }
                })
        }
    }
}

pub(super) fn expr_or_block_references_any_name(
    value: &IrExprOrBlock,
    names: &BTreeSet<&str>,
) -> bool {
    match value {
        IrExprOrBlock::Expr(expr) => expr_references_any_name(expr, names),
        IrExprOrBlock::Block(block) => block_references_any_name(block, names),
    }
}

pub(super) fn block_references_any_name(block: &IrBlock, names: &BTreeSet<&str>) -> bool {
    block
        .statements
        .iter()
        .any(|stmt| stmt_references_any_name(stmt, names))
        || block
            .tail
            .as_ref()
            .is_some_and(|tail| expr_references_any_name(tail, names))
}

pub(super) fn function_body_references_any_name(
    body: &IrFunctionBody,
    names: &BTreeSet<&str>,
) -> bool {
    match body {
        IrFunctionBody::Expr(expr) => expr_references_any_name(expr, names),
        IrFunctionBody::Block(block) => block_references_any_name(block, names),
    }
}

pub(super) fn stmt_references_any_name(stmt: &IrStmt, names: &BTreeSet<&str>) -> bool {
    match &stmt.kind {
        IrStmtKind::Noop
        | IrStmtKind::Break
        | IrStmtKind::Continue
        | IrStmtKind::Import { .. }
        | IrStmtKind::ExportList(_)
        | IrStmtKind::EnumDecl(_) => false,
        IrStmtKind::LocalDecl { values, .. } | IrStmtKind::Return(values) => values
            .iter()
            .any(|expr| expr_references_any_name(expr, names)),
        IrStmtKind::LocalDestructure { values, .. } => values
            .iter()
            .any(|expr| expr_references_any_name(expr, names)),
        IrStmtKind::Assign { targets, values } => {
            targets
                .iter()
                .any(|target| place_references_any_name(target, names))
                || values
                    .iter()
                    .any(|expr| expr_references_any_name(expr, names))
        }
        IrStmtKind::CompoundAssign { target, value, .. } => {
            place_references_any_name(target, names) || expr_references_any_name(value, names)
        }
        IrStmtKind::Expr(expr) => expr_references_any_name(expr, names),
        IrStmtKind::FunctionDecl(decl) => {
            function_name_references_any_name(&decl.name, names)
                || function_body_references_any_name(&decl.body, names)
        }
        IrStmtKind::If {
            condition,
            then_block,
            else_block,
        } => {
            expr_references_any_name(condition, names)
                || block_references_any_name(then_block, names)
                || else_block
                    .as_ref()
                    .is_some_and(|block| block_references_any_name(block, names))
        }
        IrStmtKind::While { condition, body } | IrStmtKind::RepeatUntil { condition, body } => {
            expr_references_any_name(condition, names) || block_references_any_name(body, names)
        }
        IrStmtKind::NumericFor {
            start,
            end,
            step,
            body,
            ..
        } => {
            expr_references_any_name(start, names)
                || expr_references_any_name(end, names)
                || step
                    .as_ref()
                    .is_some_and(|expr| expr_references_any_name(expr, names))
                || block_references_any_name(body, names)
        }
        IrStmtKind::GenericFor { iter, body, .. } => {
            iter.iter()
                .any(|expr| expr_references_any_name(expr, names))
                || block_references_any_name(body, names)
        }
        IrStmtKind::Do(block) => block_references_any_name(block, names),
    }
}

pub(super) fn function_name_references_any_name(
    name: &FunctionName,
    names: &BTreeSet<&str>,
) -> bool {
    match name {
        FunctionName::Simple(name) => names.contains(name.name.as_str()),
        FunctionName::Dotted(path) => path
            .first()
            .is_some_and(|part| names.contains(part.name.as_str())),
        FunctionName::Method { receiver, .. } => receiver
            .first()
            .is_some_and(|part| names.contains(part.name.as_str())),
    }
}

pub(super) fn place_references_any_name(place: &IrPlace, names: &BTreeSet<&str>) -> bool {
    match place {
        IrPlace::Identifier(name) => names.contains(name.as_str()),
        IrPlace::Member { object, .. } => expr_references_any_name(object, names),
        IrPlace::Index { object, index } => {
            expr_references_any_name(object, names) || expr_references_any_name(index, names)
        }
    }
}
