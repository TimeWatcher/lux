use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::ast::FunctionName;
use crate::diag::{Diagnostic, Label, Severity};
use crate::ir::*;
use crate::resolve::{BindingKind, ResolveOutput};

#[derive(Debug, Clone)]
pub struct HostTransformOutput {
    pub module: IrModule,
    pub diagnostics: Vec<Diagnostic>,
}

impl HostTransformOutput {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Clone)]
pub struct HostTransformSpec {
    pub target: String,
    pub runtime: String,
    pub provider: Arc<dyn HostExprTransformProvider>,
}

pub trait HostExprTransformProvider: std::fmt::Debug + Send + Sync {
    fn transform(
        &self,
        ctx: &mut HostTransformContext,
        call: &HostExprTransformCall,
    ) -> Option<IrExpr>;
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HostRuntimeImport {
    pub imported: String,
    pub local: String,
}

#[derive(Debug, Clone)]
pub struct HostExprTransformCall {
    pub source: String,
    pub runtime: String,
    pub imported: String,
    pub local: String,
    pub expr: IrExpr,
}

#[derive(Debug, Default)]
pub struct HostTransformContext {
    runtime_imports: BTreeSet<HostRuntimeImport>,
    reserved_locals: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
}

impl HostTransformContext {
    fn new(
        reserved_locals: BTreeSet<String>,
        existing_imports: BTreeSet<HostRuntimeImport>,
    ) -> Self {
        let mut reserved_locals = reserved_locals;
        for import in &existing_imports {
            reserved_locals.insert(import.local.clone());
        }
        Self {
            runtime_imports: existing_imports,
            reserved_locals,
            diagnostics: Vec::new(),
        }
    }

    pub fn import_runtime(
        &mut self,
        imported: impl Into<String>,
        local: impl Into<String>,
    ) -> String {
        let imported = imported.into();
        if let Some(existing) = self
            .runtime_imports
            .iter()
            .find(|import| import.imported == imported)
        {
            return existing.local.clone();
        }

        let actual_local = self.reserve_local(local.into());
        self.runtime_imports.insert(HostRuntimeImport {
            imported,
            local: actual_local.clone(),
        });
        actual_local
    }

    pub fn error(&mut self, code: &str, message: impl Into<String>, origin: &Origin) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_label(Label::primary(origin.span(), "here")),
        );
    }

    fn into_parts(self) -> (BTreeSet<HostRuntimeImport>, Vec<Diagnostic>) {
        (self.runtime_imports, self.diagnostics)
    }

    fn reserve_local(&mut self, preferred: String) -> String {
        if self.reserved_locals.insert(preferred.clone()) {
            return preferred;
        }

        let mut index = 1;
        loop {
            let candidate = format!("{preferred}_{index}");
            if self.reserved_locals.insert(candidate.clone()) {
                return candidate;
            }
            index += 1;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HostRegistry {
    providers_by_target: BTreeMap<String, Vec<RegisteredHostTransform>>,
}

#[derive(Debug, Clone)]
struct RegisteredHostTransform {
    runtime: String,
    provider: Arc<dyn HostExprTransformProvider>,
}

impl HostRegistry {
    pub fn empty() -> Self {
        Self {
            providers_by_target: BTreeMap::new(),
        }
    }

    pub fn from_specs(specs: impl IntoIterator<Item = HostTransformSpec>) -> Self {
        let mut registry = Self::empty();
        for spec in specs {
            registry.register(spec);
        }
        registry
    }

    pub fn register(&mut self, spec: HostTransformSpec) {
        self.providers_by_target
            .entry(canonical_import_source(&spec.target))
            .or_default()
            .push(RegisteredHostTransform {
                runtime: spec.runtime,
                provider: spec.provider,
            });
    }

    pub fn with_spec(mut self, spec: HostTransformSpec) -> Self {
        self.register(spec);
        self
    }

    pub fn transform_module(
        &self,
        module: IrModule,
        _resolved: &ResolveOutput,
    ) -> HostTransformOutput {
        let reserved_locals = collect_binding_names(&module.body);
        let mut transformer = HostTransformer {
            registry: self,
            transformed_targets: BTreeSet::new(),
            runtime_imports_by_source: BTreeMap::new(),
            reserved_locals,
            diagnostics: Vec::new(),
        };
        let module = transformer.module(module);
        HostTransformOutput {
            module,
            diagnostics: transformer.diagnostics,
        }
    }
}

struct HostTransformer<'a> {
    registry: &'a HostRegistry,
    transformed_targets: BTreeSet<String>,
    runtime_imports_by_source: BTreeMap<String, BTreeSet<HostRuntimeImport>>,
    reserved_locals: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
}

impl HostTransformer<'_> {
    fn module(&mut self, module: IrModule) -> IrModule {
        let mut transformed = IrModule {
            body: module
                .body
                .into_iter()
                .map(|stmt| self.stmt(stmt))
                .collect(),
            exports: module.exports,
            origin: module.origin,
        };
        transformed.body =
            remove_consumed_runtime_imports(transformed.body, &self.transformed_targets);
        ensure_transform_runtime_imports(
            &mut transformed.body,
            &transformed.origin,
            &self.runtime_imports_by_source,
        );
        transformed
    }

    fn block(&mut self, block: IrBlock) -> IrBlock {
        IrBlock {
            statements: block
                .statements
                .into_iter()
                .map(|stmt| self.stmt(stmt))
                .collect(),
            tail: block.tail.map(|expr| self.expr(expr)),
            origin: block.origin,
        }
    }

    fn stmt(&mut self, stmt: IrStmt) -> IrStmt {
        let origin = stmt.origin;
        let kind = match stmt.kind {
            IrStmtKind::Noop => IrStmtKind::Noop,
            IrStmtKind::LocalDecl {
                mode,
                names,
                values,
            } => IrStmtKind::LocalDecl {
                mode,
                names,
                values: values.into_iter().map(|expr| self.expr(expr)).collect(),
            },
            IrStmtKind::LocalDestructure {
                mode,
                patterns,
                values,
            } => IrStmtKind::LocalDestructure {
                mode,
                patterns: patterns
                    .into_iter()
                    .map(|pattern| self.pattern(pattern))
                    .collect(),
                values: values.into_iter().map(|expr| self.expr(expr)).collect(),
            },
            IrStmtKind::Assign { targets, values } => IrStmtKind::Assign {
                targets: targets.into_iter().map(|place| self.place(place)).collect(),
                values: values.into_iter().map(|expr| self.expr(expr)).collect(),
            },
            IrStmtKind::CompoundAssign { target, op, value } => IrStmtKind::CompoundAssign {
                target: self.place(target),
                op,
                value: self.expr(value),
            },
            IrStmtKind::Expr(expr) => IrStmtKind::Expr(self.expr(expr)),
            IrStmtKind::Return(values) => {
                IrStmtKind::Return(values.into_iter().map(|expr| self.expr(expr)).collect())
            }
            IrStmtKind::Break => IrStmtKind::Break,
            IrStmtKind::Continue => IrStmtKind::Continue,
            IrStmtKind::EnumDecl(mut decl) => {
                decl.variants = decl
                    .variants
                    .into_iter()
                    .map(|mut variant| {
                        variant.tag = self.expr(variant.tag);
                        variant
                    })
                    .collect();
                IrStmtKind::EnumDecl(decl)
            }
            IrStmtKind::FunctionDecl(mut decl) => {
                decl.params = decl
                    .params
                    .into_iter()
                    .map(|param| self.param(param))
                    .collect();
                decl.body = self.function_body(decl.body);
                IrStmtKind::FunctionDecl(decl)
            }
            IrStmtKind::If {
                condition,
                then_block,
                else_block,
            } => IrStmtKind::If {
                condition: self.expr(condition),
                then_block: self.block(then_block),
                else_block: else_block.map(|block| self.block(block)),
            },
            IrStmtKind::While { condition, body } => IrStmtKind::While {
                condition: self.expr(condition),
                body: self.block(body),
            },
            IrStmtKind::NumericFor {
                name,
                start,
                end,
                step,
                body,
            } => IrStmtKind::NumericFor {
                name,
                start: self.expr(start),
                end: self.expr(end),
                step: step.map(|expr| self.expr(expr)),
                body: self.block(body),
            },
            IrStmtKind::GenericFor { names, iter, body } => IrStmtKind::GenericFor {
                names,
                iter: iter.into_iter().map(|expr| self.expr(expr)).collect(),
                body: self.block(body),
            },
            IrStmtKind::RepeatUntil { body, condition } => IrStmtKind::RepeatUntil {
                body: self.block(body),
                condition: self.expr(condition),
            },
            IrStmtKind::Do(block) => IrStmtKind::Do(self.block(block)),
            IrStmtKind::Import {
                source,
                specifiers,
                side_effect_only,
            } => IrStmtKind::Import {
                source,
                specifiers,
                side_effect_only,
            },
            IrStmtKind::ExportList(names) => IrStmtKind::ExportList(names),
        };
        IrStmt { kind, origin }
    }

    fn function_body(&mut self, body: IrFunctionBody) -> IrFunctionBody {
        match body {
            IrFunctionBody::Expr(expr) => IrFunctionBody::Expr(Box::new(self.expr(*expr))),
            IrFunctionBody::Block(block) => IrFunctionBody::Block(Box::new(self.block(*block))),
        }
    }

    fn param(&mut self, mut param: IrParam) -> IrParam {
        param.default = param.default.map(|expr| self.expr(expr));
        param
    }

    fn pattern(&mut self, pattern: IrPattern) -> IrPattern {
        IrPattern {
            kind: match pattern.kind {
                IrPatternKind::Identifier(name) => IrPatternKind::Identifier(name),
                IrPatternKind::Object(fields) => IrPatternKind::Object(
                    fields
                        .into_iter()
                        .map(|field| IrObjectPatternField {
                            key: field.key,
                            pattern: self.pattern(field.pattern),
                            default: field.default.map(|expr| self.expr(expr)),
                            origin: field.origin,
                        })
                        .collect(),
                ),
                IrPatternKind::Array(items) => IrPatternKind::Array(
                    items
                        .into_iter()
                        .map(|item| IrArrayPatternItem {
                            pattern: self.pattern(item.pattern),
                            default: item.default.map(|expr| self.expr(expr)),
                            origin: item.origin,
                        })
                        .collect(),
                ),
            },
            origin: pattern.origin,
        }
    }

    fn place(&mut self, place: IrPlace) -> IrPlace {
        match place {
            IrPlace::Identifier(_) => place,
            IrPlace::Member { object, name } => IrPlace::Member {
                object: self.expr(object),
                name,
            },
            IrPlace::Index { object, index } => IrPlace::Index {
                object: self.expr(object),
                index: self.expr(index),
            },
        }
    }

    fn expr(&mut self, expr: IrExpr) -> IrExpr {
        let transformed = match expr.kind {
            IrExprKind::Unary { op, argument } => IrExpr {
                kind: IrExprKind::Unary {
                    op,
                    argument: Box::new(self.expr(*argument)),
                },
                ..expr
            },
            IrExprKind::Binary { op, left, right } => IrExpr {
                kind: IrExprKind::Binary {
                    op,
                    left: Box::new(self.expr(*left)),
                    right: Box::new(self.expr(*right)),
                },
                ..expr
            },
            IrExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
            } => IrExpr {
                kind: IrExprKind::Conditional {
                    condition: Box::new(self.expr(*condition)),
                    then_branch: self.expr_or_block(then_branch),
                    else_branch: self.expr_or_block(else_branch),
                },
                ..expr
            },
            IrExprKind::Match(match_expr) => IrExpr {
                kind: IrExprKind::Match(IrMatchExpr {
                    subject: Box::new(self.expr(*match_expr.subject)),
                    arms: match_expr
                        .arms
                        .into_iter()
                        .map(|arm| IrMatchArm {
                            pattern: arm.pattern,
                            body: self.expr_or_block(arm.body),
                            origin: arm.origin,
                        })
                        .collect(),
                }),
                ..expr
            },
            IrExprKind::Function(mut function) => {
                function.params = function
                    .params
                    .into_iter()
                    .map(|param| self.param(param))
                    .collect();
                function.body = self.function_body(function.body);
                IrExpr {
                    kind: IrExprKind::Function(function),
                    ..expr
                }
            }
            IrExprKind::Template(parts) => IrExpr {
                kind: IrExprKind::Template(
                    parts
                        .into_iter()
                        .map(|part| IrTemplatePart {
                            kind: match part.kind {
                                IrTemplatePartKind::Text(text) => IrTemplatePartKind::Text(text),
                                IrTemplatePartKind::Expr(expr) => {
                                    IrTemplatePartKind::Expr(self.expr(expr))
                                }
                            },
                            origin: part.origin,
                        })
                        .collect(),
                ),
                ..expr
            },
            IrExprKind::Table(fields) => IrExpr {
                kind: IrExprKind::Table(
                    fields
                        .into_iter()
                        .map(|field| IrTableField {
                            kind: match field.kind {
                                IrTableFieldKind::Array(expr) => {
                                    IrTableFieldKind::Array(self.expr(expr))
                                }
                                IrTableFieldKind::Named { name, value } => {
                                    IrTableFieldKind::Named {
                                        name,
                                        value: self.expr(value),
                                    }
                                }
                                IrTableFieldKind::ExprKey { key, value } => {
                                    IrTableFieldKind::ExprKey {
                                        key: self.expr(key),
                                        value: self.expr(value),
                                    }
                                }
                                IrTableFieldKind::Spread(value) => {
                                    IrTableFieldKind::Spread(self.expr(value))
                                }
                            },
                            origin: field.origin,
                        })
                        .collect(),
                ),
                ..expr
            },
            IrExprKind::Chain(chain) => {
                let expr = IrExpr {
                    kind: IrExprKind::Chain(IrChain {
                        base: Box::new(self.expr(*chain.base)),
                        segments: chain
                            .segments
                            .into_iter()
                            .map(|segment| self.chain_segment(segment))
                            .collect(),
                    }),
                    ..expr
                };
                self.try_host_expr(expr)
            }
            IrExprKind::Do(block) => IrExpr {
                kind: IrExprKind::Do(Box::new(self.block(*block))),
                ..expr
            },
            _ => expr,
        };

        transformed
    }

    fn expr_or_block(&mut self, item: IrExprOrBlock) -> IrExprOrBlock {
        match item {
            IrExprOrBlock::Expr(expr) => IrExprOrBlock::Expr(Box::new(self.expr(*expr))),
            IrExprOrBlock::Block(block) => IrExprOrBlock::Block(Box::new(self.block(*block))),
        }
    }

    fn chain_segment(&mut self, segment: IrChainSegment) -> IrChainSegment {
        IrChainSegment {
            kind: match segment.kind {
                IrChainSegmentKind::Member { name, optional } => {
                    IrChainSegmentKind::Member { name, optional }
                }
                IrChainSegmentKind::Index { index, optional } => IrChainSegmentKind::Index {
                    index: self.expr(index),
                    optional,
                },
                IrChainSegmentKind::Call { args, style } => IrChainSegmentKind::Call {
                    args: args.into_iter().map(|expr| self.expr(expr)).collect(),
                    style,
                },
                IrChainSegmentKind::SafeDotCall { name, args, style } => {
                    IrChainSegmentKind::SafeDotCall {
                        name,
                        args: args.into_iter().map(|expr| self.expr(expr)).collect(),
                        style,
                    }
                }
                IrChainSegmentKind::MethodCall {
                    name,
                    args,
                    optional,
                    style,
                } => IrChainSegmentKind::MethodCall {
                    name,
                    args: args.into_iter().map(|expr| self.expr(expr)).collect(),
                    optional,
                    style,
                },
            },
            origin: segment.origin,
        }
    }

    fn try_host_expr(&mut self, expr: IrExpr) -> IrExpr {
        let IrExprKind::Chain(chain) = &expr.kind else {
            return expr;
        };
        let Some(symbol) = &chain.base.symbol else {
            return expr;
        };
        if symbol.binding_kind != BindingKind::Import {
            return expr;
        }
        let Some(source) = symbol.source_module.as_deref() else {
            return expr;
        };
        let canonical_source = canonical_import_source(source);
        let Some(providers) = self.registry.providers_by_target.get(&canonical_source) else {
            return expr;
        };
        let imported = symbol
            .imported_name
            .clone()
            .unwrap_or_else(|| symbol.local_name.clone());

        for entry in providers {
            let existing_imports = self
                .runtime_imports_by_source
                .get(&entry.runtime)
                .cloned()
                .unwrap_or_default();
            let mut ctx = HostTransformContext::new(self.reserved_locals.clone(), existing_imports);
            let call = HostExprTransformCall {
                source: source.to_string(),
                runtime: entry.runtime.clone(),
                imported: imported.clone(),
                local: symbol.local_name.clone(),
                expr: expr.clone(),
            };
            if let Some(next) = entry.provider.transform(&mut ctx, &call) {
                let (runtime_imports, diagnostics) = ctx.into_parts();
                self.diagnostics.extend(diagnostics);
                self.transformed_targets.insert(source.to_string());
                self.reserved_locals
                    .extend(runtime_imports.iter().map(|import| import.local.clone()));
                self.runtime_imports_by_source
                    .entry(entry.runtime.clone())
                    .or_default()
                    .extend(runtime_imports);
                return next;
            }
            let (_, diagnostics) = ctx.into_parts();
            self.diagnostics.extend(diagnostics);
        }

        expr
    }
}

fn canonical_import_source(source: &str) -> String {
    source.strip_prefix('@').unwrap_or(source).to_string()
}

fn remove_consumed_runtime_imports(
    body: Vec<IrStmt>,
    transformed_sources: &BTreeSet<String>,
) -> Vec<IrStmt> {
    if transformed_sources.is_empty() {
        return body;
    }

    let mut referenced_imports = BTreeSet::<(String, String)>::new();
    collect_referenced_imports(&body, &mut referenced_imports);

    body.into_iter()
        .map(|stmt| match stmt.kind {
            IrStmtKind::Import {
                source,
                mut specifiers,
                side_effect_only,
            } if !side_effect_only && transformed_sources.contains(&source) => {
                specifiers.retain(|specifier| {
                    referenced_imports.contains(&(source.clone(), specifier.local.clone()))
                });
                if specifiers.is_empty() {
                    IrStmt {
                        kind: IrStmtKind::Noop,
                        origin: stmt.origin,
                    }
                } else {
                    IrStmt {
                        kind: IrStmtKind::Import {
                            source,
                            specifiers,
                            side_effect_only,
                        },
                        origin: stmt.origin,
                    }
                }
            }
            kind => IrStmt {
                kind,
                origin: stmt.origin,
            },
        })
        .collect()
}

fn ensure_transform_runtime_imports(
    body: &mut Vec<IrStmt>,
    origin: &Origin,
    runtime_imports_by_source: &BTreeMap<String, BTreeSet<HostRuntimeImport>>,
) {
    for (source, imports) in runtime_imports_by_source {
        for import in imports {
            ensure_runtime_import(body, origin, source, &import.imported, &import.local);
        }
    }
}

fn ensure_runtime_import(
    body: &mut Vec<IrStmt>,
    origin: &Origin,
    runtime_source: &str,
    runtime_import: &str,
    runtime_local: &str,
) {
    let mut has_import = false;
    for stmt in body.iter_mut() {
        let IrStmtKind::Import {
            source,
            specifiers,
            side_effect_only,
        } = &mut stmt.kind
        else {
            continue;
        };
        if source != runtime_source || *side_effect_only {
            continue;
        }
        has_import = true;
        if !specifiers
            .iter()
            .any(|specifier| specifier.local == runtime_local)
        {
            specifiers.push(IrImportSpecifier {
                imported: runtime_import.into(),
                local: runtime_local.into(),
                namespace: false,
            });
        }
    }

    if !has_import {
        body.insert(
            0,
            IrStmt {
                kind: IrStmtKind::Import {
                    source: runtime_source.into(),
                    specifiers: vec![IrImportSpecifier {
                        imported: runtime_import.into(),
                        local: runtime_local.into(),
                        namespace: false,
                    }],
                    side_effect_only: false,
                },
                origin: origin.clone(),
            },
        );
    }
}

fn collect_binding_names(body: &[IrStmt]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_stmt_binding_names(body, &mut names);
    names
}

fn collect_stmt_binding_names(stmts: &[IrStmt], names: &mut BTreeSet<String>) {
    for stmt in stmts {
        match &stmt.kind {
            IrStmtKind::Noop
            | IrStmtKind::Break
            | IrStmtKind::Continue
            | IrStmtKind::ExportList(_) => {}
            IrStmtKind::EnumDecl(decl) => {
                if decl.runtime {
                    names.insert(decl.name.clone());
                }
                for variant in &decl.variants {
                    collect_expr_binding_names(&variant.tag, names);
                }
            }
            IrStmtKind::LocalDecl {
                names: local_names,
                values,
                ..
            } => {
                names.extend(local_names.iter().cloned());
                for value in values {
                    collect_expr_binding_names(value, names);
                }
            }
            IrStmtKind::LocalDestructure {
                patterns, values, ..
            } => {
                names.extend(collect_pattern_binding_names(patterns));
                for value in values {
                    collect_expr_binding_names(value, names);
                }
            }
            IrStmtKind::Assign { targets, values } => {
                for target in targets {
                    collect_place_binding_names(target, names);
                }
                for value in values {
                    collect_expr_binding_names(value, names);
                }
            }
            IrStmtKind::CompoundAssign { target, value, .. } => {
                collect_place_binding_names(target, names);
                collect_expr_binding_names(value, names);
            }
            IrStmtKind::Expr(expr) => collect_expr_binding_names(expr, names),
            IrStmtKind::Return(values) => {
                for value in values {
                    collect_expr_binding_names(value, names);
                }
            }
            IrStmtKind::FunctionDecl(decl) => {
                if let FunctionName::Simple(name) = &decl.name {
                    names.insert(name.name.clone());
                }
                names.extend(decl.params.iter().map(|param| param.name.clone()));
                for param in &decl.params {
                    if let Some(default) = &param.default {
                        collect_expr_binding_names(default, names);
                    }
                }
                collect_function_body_binding_names(&decl.body, names);
            }
            IrStmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                collect_expr_binding_names(condition, names);
                collect_block_binding_names(then_block, names);
                if let Some(block) = else_block {
                    collect_block_binding_names(block, names);
                }
            }
            IrStmtKind::While { condition, body } => {
                collect_expr_binding_names(condition, names);
                collect_block_binding_names(body, names);
            }
            IrStmtKind::NumericFor {
                name,
                start,
                end,
                step,
                body,
            } => {
                names.insert(name.clone());
                collect_expr_binding_names(start, names);
                collect_expr_binding_names(end, names);
                if let Some(step) = step {
                    collect_expr_binding_names(step, names);
                }
                collect_block_binding_names(body, names);
            }
            IrStmtKind::GenericFor {
                names: vars,
                iter,
                body,
            } => {
                names.extend(vars.iter().cloned());
                for value in iter {
                    collect_expr_binding_names(value, names);
                }
                collect_block_binding_names(body, names);
            }
            IrStmtKind::RepeatUntil { body, condition } => {
                collect_block_binding_names(body, names);
                collect_expr_binding_names(condition, names);
            }
            IrStmtKind::Do(block) => collect_block_binding_names(block, names),
            IrStmtKind::Import { specifiers, .. } => {
                names.extend(specifiers.iter().map(|specifier| specifier.local.clone()));
            }
        }
    }
}

fn collect_block_binding_names(block: &IrBlock, names: &mut BTreeSet<String>) {
    collect_stmt_binding_names(&block.statements, names);
    if let Some(tail) = &block.tail {
        collect_expr_binding_names(tail, names);
    }
}

fn collect_pattern_binding_names(patterns: &[IrPattern]) -> Vec<String> {
    let mut names = Vec::new();
    for pattern in patterns {
        collect_pattern_binding_name(pattern, &mut names);
    }
    names
}

fn collect_pattern_binding_name(pattern: &IrPattern, names: &mut Vec<String>) {
    match &pattern.kind {
        IrPatternKind::Identifier(name) => names.push(name.clone()),
        IrPatternKind::Object(fields) => {
            for field in fields {
                collect_pattern_binding_name(&field.pattern, names);
            }
        }
        IrPatternKind::Array(items) => {
            for item in items {
                collect_pattern_binding_name(&item.pattern, names);
            }
        }
    }
}

fn collect_match_pattern_binding_names(pattern: &IrMatchPattern, names: &mut BTreeSet<String>) {
    match &pattern.kind {
        IrMatchPatternKind::Or(patterns) => {
            for pattern in patterns {
                collect_match_pattern_binding_names(pattern, names);
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
                            collect_match_pattern_binding_names(pattern, names);
                        }
                    }
                    IrMatchPatternPayload::Record(fields) => {
                        for field in fields {
                            collect_match_pattern_binding_names(&field.pattern, names);
                        }
                    }
                }
            }
        }
        IrMatchPatternKind::Object(fields) => {
            for field in fields {
                collect_match_pattern_binding_names(&field.pattern, names);
            }
        }
        IrMatchPatternKind::Array(items) => {
            for item in items {
                collect_match_pattern_binding_names(&item.pattern, names);
            }
        }
        IrMatchPatternKind::Wildcard | IrMatchPatternKind::Literal(_) => {}
    }
}

fn collect_function_body_binding_names(body: &IrFunctionBody, names: &mut BTreeSet<String>) {
    match body {
        IrFunctionBody::Expr(expr) => collect_expr_binding_names(expr, names),
        IrFunctionBody::Block(block) => collect_block_binding_names(block, names),
    }
}

fn collect_place_binding_names(place: &IrPlace, names: &mut BTreeSet<String>) {
    match place {
        IrPlace::Identifier(_) => {}
        IrPlace::Member { object, .. } => collect_expr_binding_names(object, names),
        IrPlace::Index { object, index } => {
            collect_expr_binding_names(object, names);
            collect_expr_binding_names(index, names);
        }
    }
}

fn collect_expr_or_block_binding_names(item: &IrExprOrBlock, names: &mut BTreeSet<String>) {
    match item {
        IrExprOrBlock::Expr(expr) => collect_expr_binding_names(expr, names),
        IrExprOrBlock::Block(block) => collect_block_binding_names(block, names),
    }
}

fn collect_expr_binding_names(expr: &IrExpr, names: &mut BTreeSet<String>) {
    match &expr.kind {
        IrExprKind::Identifier(_)
        | IrExprKind::Nil
        | IrExprKind::Boolean(_)
        | IrExprKind::Number(_)
        | IrExprKind::String(_)
        | IrExprKind::Vararg
        | IrExprKind::PipelinePlaceholder => {}
        IrExprKind::Template(parts) => {
            for part in parts {
                if let IrTemplatePartKind::Expr(expr) = &part.kind {
                    collect_expr_binding_names(expr, names);
                }
            }
        }
        IrExprKind::Table(fields) => {
            for field in fields {
                match &field.kind {
                    IrTableFieldKind::Array(expr) | IrTableFieldKind::Spread(expr) => {
                        collect_expr_binding_names(expr, names)
                    }
                    IrTableFieldKind::Named { value, .. } => {
                        collect_expr_binding_names(value, names);
                    }
                    IrTableFieldKind::ExprKey { key, value } => {
                        collect_expr_binding_names(key, names);
                        collect_expr_binding_names(value, names);
                    }
                }
            }
        }
        IrExprKind::Unary { argument, .. } => collect_expr_binding_names(argument, names),
        IrExprKind::Binary { left, right, .. } => {
            collect_expr_binding_names(left, names);
            collect_expr_binding_names(right, names);
        }
        IrExprKind::Conditional {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_binding_names(condition, names);
            collect_expr_or_block_binding_names(then_branch, names);
            collect_expr_or_block_binding_names(else_branch, names);
        }
        IrExprKind::Match(match_expr) => {
            collect_expr_binding_names(&match_expr.subject, names);
            for arm in &match_expr.arms {
                collect_match_pattern_binding_names(&arm.pattern, names);
                collect_expr_or_block_binding_names(&arm.body, names);
            }
        }
        IrExprKind::Do(block) => collect_block_binding_names(block, names),
        IrExprKind::Function(function) => {
            names.extend(function.params.iter().map(|param| param.name.clone()));
            for param in &function.params {
                if let Some(default) = &param.default {
                    collect_expr_binding_names(default, names);
                }
            }
            collect_function_body_binding_names(&function.body, names);
        }
        IrExprKind::Chain(chain) => {
            collect_expr_binding_names(&chain.base, names);
            for segment in &chain.segments {
                match &segment.kind {
                    IrChainSegmentKind::Member { .. } => {}
                    IrChainSegmentKind::Index { index, .. } => {
                        collect_expr_binding_names(index, names);
                    }
                    IrChainSegmentKind::Call { args, .. }
                    | IrChainSegmentKind::SafeDotCall { args, .. }
                    | IrChainSegmentKind::MethodCall { args, .. } => {
                        for arg in args {
                            collect_expr_binding_names(arg, names);
                        }
                    }
                }
            }
        }
    }
}

fn collect_referenced_imports(stmts: &[IrStmt], out: &mut BTreeSet<(String, String)>) {
    for stmt in stmts {
        match &stmt.kind {
            IrStmtKind::Noop
            | IrStmtKind::Break
            | IrStmtKind::Continue
            | IrStmtKind::Import { .. }
            | IrStmtKind::ExportList(_) => {}
            IrStmtKind::EnumDecl(decl) => {
                for variant in &decl.variants {
                    collect_expr_imports(&variant.tag, out);
                }
            }
            IrStmtKind::LocalDecl { values, .. } | IrStmtKind::Return(values) => {
                for expr in values {
                    collect_expr_imports(expr, out);
                }
            }
            IrStmtKind::LocalDestructure { values, .. } => {
                for expr in values {
                    collect_expr_imports(expr, out);
                }
            }
            IrStmtKind::Assign { targets, values } => {
                for target in targets {
                    collect_place_imports(target, out);
                }
                for value in values {
                    collect_expr_imports(value, out);
                }
            }
            IrStmtKind::CompoundAssign { target, value, .. } => {
                collect_place_imports(target, out);
                collect_expr_imports(value, out);
            }
            IrStmtKind::Expr(expr) => collect_expr_imports(expr, out),
            IrStmtKind::FunctionDecl(decl) => {
                for param in &decl.params {
                    if let Some(default) = &param.default {
                        collect_expr_imports(default, out);
                    }
                }
                collect_function_body_imports(&decl.body, out);
            }
            IrStmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                collect_expr_imports(condition, out);
                collect_block_imports(then_block, out);
                if let Some(block) = else_block {
                    collect_block_imports(block, out);
                }
            }
            IrStmtKind::While { condition, body } => {
                collect_expr_imports(condition, out);
                collect_block_imports(body, out);
            }
            IrStmtKind::NumericFor {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_expr_imports(start, out);
                collect_expr_imports(end, out);
                if let Some(step) = step {
                    collect_expr_imports(step, out);
                }
                collect_block_imports(body, out);
            }
            IrStmtKind::GenericFor { iter, body, .. } => {
                for expr in iter {
                    collect_expr_imports(expr, out);
                }
                collect_block_imports(body, out);
            }
            IrStmtKind::RepeatUntil { body, condition } => {
                collect_block_imports(body, out);
                collect_expr_imports(condition, out);
            }
            IrStmtKind::Do(block) => collect_block_imports(block, out),
        }
    }
}

fn collect_block_imports(block: &IrBlock, out: &mut BTreeSet<(String, String)>) {
    collect_referenced_imports(&block.statements, out);
    if let Some(tail) = &block.tail {
        collect_expr_imports(tail, out);
    }
}

fn collect_function_body_imports(body: &IrFunctionBody, out: &mut BTreeSet<(String, String)>) {
    match body {
        IrFunctionBody::Expr(expr) => collect_expr_imports(expr, out),
        IrFunctionBody::Block(block) => collect_block_imports(block, out),
    }
}

fn collect_place_imports(place: &IrPlace, out: &mut BTreeSet<(String, String)>) {
    match place {
        IrPlace::Identifier(_) => {}
        IrPlace::Member { object, .. } => collect_expr_imports(object, out),
        IrPlace::Index { object, index } => {
            collect_expr_imports(object, out);
            collect_expr_imports(index, out);
        }
    }
}

fn collect_expr_imports(expr: &IrExpr, out: &mut BTreeSet<(String, String)>) {
    if let Some(symbol) = &expr.symbol {
        if symbol.binding_kind == BindingKind::Import {
            if let Some(source) = &symbol.source_module {
                out.insert((source.clone(), symbol.local_name.clone()));
            }
        }
    }

    match &expr.kind {
        IrExprKind::Identifier(_)
        | IrExprKind::Nil
        | IrExprKind::Boolean(_)
        | IrExprKind::Number(_)
        | IrExprKind::String(_)
        | IrExprKind::Vararg
        | IrExprKind::PipelinePlaceholder => {}
        IrExprKind::Template(parts) => {
            for part in parts {
                if let IrTemplatePartKind::Expr(expr) = &part.kind {
                    collect_expr_imports(expr, out);
                }
            }
        }
        IrExprKind::Table(fields) => {
            for field in fields {
                match &field.kind {
                    IrTableFieldKind::Array(expr) | IrTableFieldKind::Spread(expr) => {
                        collect_expr_imports(expr, out)
                    }
                    IrTableFieldKind::Named { value, .. } => collect_expr_imports(value, out),
                    IrTableFieldKind::ExprKey { key, value } => {
                        collect_expr_imports(key, out);
                        collect_expr_imports(value, out);
                    }
                }
            }
        }
        IrExprKind::Unary { argument, .. } => collect_expr_imports(argument, out),
        IrExprKind::Binary { left, right, .. } => {
            collect_expr_imports(left, out);
            collect_expr_imports(right, out);
        }
        IrExprKind::Conditional {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_imports(condition, out);
            collect_expr_or_block_imports(then_branch, out);
            collect_expr_or_block_imports(else_branch, out);
        }
        IrExprKind::Match(match_expr) => {
            collect_expr_imports(&match_expr.subject, out);
            for arm in &match_expr.arms {
                collect_expr_or_block_imports(&arm.body, out);
            }
        }
        IrExprKind::Function(function) => {
            for param in &function.params {
                if let Some(default) = &param.default {
                    collect_expr_imports(default, out);
                }
            }
            collect_function_body_imports(&function.body, out);
        }
        IrExprKind::Do(block) => collect_block_imports(block, out),
        IrExprKind::Chain(chain) => {
            collect_expr_imports(&chain.base, out);
            for segment in &chain.segments {
                match &segment.kind {
                    IrChainSegmentKind::Member { .. } => {}
                    IrChainSegmentKind::Index { index, .. } => collect_expr_imports(index, out),
                    IrChainSegmentKind::Call { args, .. }
                    | IrChainSegmentKind::SafeDotCall { args, .. }
                    | IrChainSegmentKind::MethodCall { args, .. } => {
                        for arg in args {
                            collect_expr_imports(arg, out);
                        }
                    }
                }
            }
        }
    }
}

fn collect_expr_or_block_imports(item: &IrExprOrBlock, out: &mut BTreeSet<(String, String)>) {
    match item {
        IrExprOrBlock::Expr(expr) => collect_expr_imports(expr, out),
        IrExprOrBlock::Block(block) => collect_block_imports(block, out),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::codegen::LuaCodegen;
    use crate::host::{
        HostExprTransformCall, HostExprTransformProvider, HostTransformContext, HostTransformSpec,
    };
    use crate::ir::{
        IrCallStyle, IrChain, IrChainSegment, IrChainSegmentKind, IrExpr, IrExprKind, ValueMode,
    };
    use crate::lex::Lexer;
    use crate::lower::Lowerer;
    use crate::pipeline::parse_expand_resolve;
    use crate::source::SourceFile;

    use super::HostRegistry;

    #[derive(Debug)]
    struct TestUiProvider;

    impl HostExprTransformProvider for TestUiProvider {
        fn transform(
            &self,
            ctx: &mut HostTransformContext,
            call: &HostExprTransformCall,
        ) -> Option<IrExpr> {
            if call.imported != "Column" {
                return None;
            }
            let node = ctx.import_runtime("node", "__lux_ui_node");
            Some(IrExpr {
                kind: IrExprKind::Chain(IrChain {
                    base: Box::new(IrExpr {
                        kind: IrExprKind::Identifier(node),
                        origin: call.expr.origin.clone(),
                        value_mode: ValueMode::Single,
                        symbol: None,
                    }),
                    segments: vec![IrChainSegment {
                        kind: IrChainSegmentKind::Call {
                            args: vec![
                                IrExpr {
                                    kind: IrExprKind::String(call.imported.clone()),
                                    origin: call.expr.origin.clone(),
                                    value_mode: ValueMode::Single,
                                    symbol: None,
                                },
                                IrExpr {
                                    kind: IrExprKind::Table(Vec::new()),
                                    origin: call.expr.origin.clone(),
                                    value_mode: ValueMode::Single,
                                    symbol: None,
                                },
                                IrExpr {
                                    kind: IrExprKind::Table(Vec::new()),
                                    origin: call.expr.origin.clone(),
                                    value_mode: ValueMode::Single,
                                    symbol: None,
                                },
                            ],
                            style: IrCallStyle::Paren,
                        },
                        origin: call.expr.origin.clone(),
                    }],
                }),
                origin: call.expr.origin.clone(),
                value_mode: call.expr.value_mode,
                symbol: None,
            })
        }
    }

    fn registry() -> HostRegistry {
        HostRegistry::from_specs([HostTransformSpec {
            target: "lux/ui".into(),
            runtime: "lux/ui".into(),
            provider: Arc::new(TestUiProvider),
        }])
    }

    #[test]
    fn folds_ui_import_calls_by_symbol_origin() {
        let file = SourceFile::new(
            0,
            None,
            "import { Column } from \"lux/ui\"\nlocal view = Column { gap = 1 } { }",
        );
        let lex = Lexer::new(&file).lex_all();
        assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
        let parsed = parse_expand_resolve(&file, &lex.tokens);
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let ir = Lowerer::lower(&parsed.module, &parsed.resolved).expect("lower");
        let transformed = registry().transform_module(ir, &parsed.resolved);
        assert!(transformed.diagnostics.is_empty());
        let lua = LuaCodegen::generate(&transformed.module)
            .expect("codegen")
            .lua;
        assert!(lua.contains("local __lux_ui_node = __lux_import_"));
        assert!(lua.contains("__lux_ui_node(\"Column\""));
        assert!(lua.contains("__lux_import(\"lux/ui\")"));
    }

    #[test]
    fn does_not_fold_user_local_same_name() {
        let file = SourceFile::new(0, None, "local Column = makeColumn\nColumn { gap = 1 }");
        let lex = Lexer::new(&file).lex_all();
        assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
        let parsed = parse_expand_resolve(&file, &lex.tokens);
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let ir = Lowerer::lower(&parsed.module, &parsed.resolved).expect("lower");
        let transformed = registry().transform_module(ir, &parsed.resolved);
        assert!(transformed.diagnostics.is_empty());
        let lua = LuaCodegen::generate(&transformed.module)
            .expect("codegen")
            .lua;
        assert!(!lua.contains("__lux_ui_node"));
    }

    #[test]
    fn preserves_runtime_ui_import_specifiers_that_are_still_referenced() {
        let file = SourceFile::new(
            0,
            None,
            "import { Column, node } from \"lux/ui\"\nlocal view = Column { gap = 1 }\nlocal x = node(\"Raw\", {}, {})",
        );
        let lex = Lexer::new(&file).lex_all();
        assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
        let parsed = parse_expand_resolve(&file, &lex.tokens);
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let ir = Lowerer::lower(&parsed.module, &parsed.resolved).expect("lower");
        let transformed = registry().transform_module(ir, &parsed.resolved);
        assert!(transformed.diagnostics.is_empty());
        let lua = LuaCodegen::generate(&transformed.module)
            .expect("codegen")
            .lua;
        assert!(lua.contains("local node = __lux_import_"));
        assert!(!lua.contains("local Column = __lux_import_"));
    }
}
