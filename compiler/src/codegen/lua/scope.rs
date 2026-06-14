use std::collections::{BTreeMap, BTreeSet};

use crate::ast::FunctionName;
use crate::ir::*;

use super::analysis::{
    collect_pattern_names, expr_references_any_name, exprs_reference_any_names,
    stmt_references_any_name,
};

pub(super) const MODULE_LIFT_THRESHOLD: usize = 160;
const BLOCK_SCOPE_NARROW_THRESHOLD: usize = 96;
const BLOCK_SCOPE_NARROW_TARGET: usize = 80;

#[derive(Debug, Clone, Default)]
pub(super) struct ModuleLift {
    pub(super) table: Option<String>,
    pub(super) names: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BlockScopeSegment {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) include_tail: bool,
    pub(super) scoped: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ModuleLiftCandidates {
    pub(super) names: BTreeSet<String>,
    import_cache_slots: usize,
}

impl ModuleLiftCandidates {
    pub(super) fn local_pressure(&self) -> usize {
        self.names.len() + self.import_cache_slots
    }
}

pub(super) fn collect_module_lift_candidates(stmts: &[IrStmt]) -> ModuleLiftCandidates {
    let mut occurrences = BTreeMap::<String, usize>::new();
    for stmt in stmts {
        for name in module_lift_decl_group(stmt).unwrap_or_default() {
            *occurrences.entry(name).or_default() += 1;
        }
    }

    let mut out = ModuleLiftCandidates::default();
    let mut import_sources = BTreeSet::new();
    for stmt in stmts {
        if let IrStmtKind::Import { source, .. } = &stmt.kind {
            import_sources.insert(source.clone());
        }

        let Some(names) = module_lift_decl_group(stmt) else {
            continue;
        };
        if names
            .iter()
            .all(|name| occurrences.get(name).copied().unwrap_or(0) == 1)
        {
            out.names.extend(names);
        }
    }
    out.import_cache_slots = import_sources.len();
    out
}

fn module_lift_decl_group(stmt: &IrStmt) -> Option<Vec<String>> {
    match &stmt.kind {
        IrStmtKind::LocalDecl { names, values, .. } => {
            if !values.is_empty() && exprs_reference_any_names(values, names) {
                return None;
            }
            Some(names.clone())
        }
        IrStmtKind::FunctionDecl(decl) => match &decl.name {
            FunctionName::Simple(name) => Some(vec![name.name.clone()]),
            FunctionName::Dotted(_) | FunctionName::Method { .. } => None,
        },
        IrStmtKind::EnumDecl(decl) if decl.runtime => Some(vec![decl.name.clone()]),
        IrStmtKind::Import {
            specifiers,
            side_effect_only,
            ..
        } => {
            if *side_effect_only {
                return Some(Vec::new());
            }
            Some(
                specifiers
                    .iter()
                    .map(|specifier| specifier.local.clone())
                    .collect(),
            )
        }
        _ => None,
    }
}

pub(super) fn plan_block_scope_narrowing(block: &IrBlock) -> Option<Vec<BlockScopeSegment>> {
    let stmt_count = block.statements.len();
    if stmt_count == 0 {
        return None;
    }

    let declarations = collect_current_block_declarations(&block.statements)?;
    let total_decl_slots = declarations
        .iter()
        .map(|decl| decl.names.len())
        .sum::<usize>();
    if total_decl_slots <= BLOCK_SCOPE_NARROW_THRESHOLD {
        return None;
    }

    let mut candidates = BTreeSet::new();
    let mut decl_stmt_by_name = BTreeMap::new();
    for decl in &declarations {
        for name in &decl.names {
            candidates.insert(name.clone());
            decl_stmt_by_name.insert(name.clone(), decl.stmt_index);
        }
    }

    let tail_index = stmt_count;
    let mut last_use = decl_stmt_by_name
        .iter()
        .map(|(name, index)| (name.clone(), *index))
        .collect::<BTreeMap<_, _>>();
    for (index, stmt) in block.statements.iter().enumerate() {
        for name in referenced_decl_names_in_stmt(stmt, &candidates) {
            if let Some(decl_index) = decl_stmt_by_name.get(&name) {
                if index > *decl_index {
                    last_use.insert(name, index);
                }
            }
        }
    }
    if let Some(tail) = &block.tail {
        for name in referenced_decl_names_in_expr(tail, &candidates) {
            if let Some(decl_index) = decl_stmt_by_name.get(&name) {
                if tail_index > *decl_index {
                    last_use.insert(name, tail_index);
                }
            }
        }
    }

    let mut decls_by_stmt = vec![Vec::<String>::new(); stmt_count];
    for decl in declarations {
        decls_by_stmt[decl.stmt_index].extend(decl.names);
    }

    let mut segments = Vec::new();
    let mut index = 0usize;
    let mut emitted_scoped = false;
    while index < stmt_count {
        let start = index;
        let mut end_inclusive = index;
        let mut local_slots = 0usize;
        let mut has_decls = false;

        loop {
            for name in &decls_by_stmt[index] {
                has_decls = true;
                local_slots += 1;
                end_inclusive = end_inclusive.max(*last_use.get(name).unwrap_or(&index));
            }

            if end_inclusive == tail_index {
                break;
            }
            if index >= end_inclusive && has_decls && local_slots >= BLOCK_SCOPE_NARROW_TARGET {
                break;
            }
            if index + 1 >= stmt_count {
                break;
            }
            if index >= end_inclusive
                && has_decls
                && local_slots + decls_by_stmt[index + 1].len() > BLOCK_SCOPE_NARROW_TARGET
            {
                break;
            }
            if index >= end_inclusive && !has_decls {
                break;
            }
            index += 1;
        }

        let include_tail = end_inclusive == tail_index;
        let end = if include_tail {
            stmt_count
        } else {
            end_inclusive + 1
        };
        let scoped = has_decls;
        emitted_scoped |= scoped;
        segments.push(BlockScopeSegment {
            start,
            end,
            include_tail,
            scoped,
        });
        if include_tail {
            break;
        }
        index = end;
    }

    if emitted_scoped { Some(segments) } else { None }
}

#[derive(Debug, Clone)]
struct BlockDeclaration {
    stmt_index: usize,
    names: Vec<String>,
}

fn collect_current_block_declarations(stmts: &[IrStmt]) -> Option<Vec<BlockDeclaration>> {
    let mut declarations = Vec::new();
    let mut seen = BTreeSet::new();
    for (stmt_index, stmt) in stmts.iter().enumerate() {
        let names = current_stmt_declared_names(stmt);
        if names.is_empty() {
            continue;
        }
        for name in &names {
            if !seen.insert(name.clone()) {
                return None;
            }
        }
        declarations.push(BlockDeclaration { stmt_index, names });
    }
    Some(declarations)
}

fn current_stmt_declared_names(stmt: &IrStmt) -> Vec<String> {
    match &stmt.kind {
        IrStmtKind::LocalDecl { names, .. } => names.clone(),
        IrStmtKind::LocalDestructure { patterns, .. } => collect_pattern_names(patterns),
        IrStmtKind::Import {
            specifiers,
            side_effect_only,
            ..
        } => {
            if *side_effect_only {
                Vec::new()
            } else {
                specifiers
                    .iter()
                    .map(|specifier| specifier.local.clone())
                    .collect()
            }
        }
        IrStmtKind::EnumDecl(decl) if decl.runtime => vec![decl.name.clone()],
        IrStmtKind::Noop
        | IrStmtKind::Assign { .. }
        | IrStmtKind::CompoundAssign { .. }
        | IrStmtKind::Expr(_)
        | IrStmtKind::Return(_)
        | IrStmtKind::Break
        | IrStmtKind::Continue
        | IrStmtKind::FunctionDecl(_)
        | IrStmtKind::EnumDecl(_)
        | IrStmtKind::If { .. }
        | IrStmtKind::While { .. }
        | IrStmtKind::NumericFor { .. }
        | IrStmtKind::GenericFor { .. }
        | IrStmtKind::RepeatUntil { .. }
        | IrStmtKind::Do(_)
        | IrStmtKind::ExportList(_) => Vec::new(),
    }
}

fn referenced_decl_names_in_stmt(stmt: &IrStmt, names: &BTreeSet<String>) -> Vec<String> {
    names
        .iter()
        .filter(|name| {
            let mut single = BTreeSet::new();
            single.insert(name.as_str());
            stmt_references_any_name(stmt, &single)
        })
        .cloned()
        .collect()
}

fn referenced_decl_names_in_expr(expr: &IrExpr, names: &BTreeSet<String>) -> Vec<String> {
    names
        .iter()
        .filter(|name| {
            let mut single = BTreeSet::new();
            single.insert(name.as_str());
            expr_references_any_name(expr, &single)
        })
        .cloned()
        .collect()
}
