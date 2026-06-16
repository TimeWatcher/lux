use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::ast::*;
use crate::diag::{Diagnostic, Label, Severity};
use crate::package_manager::LUX_STD_REPO;
use crate::source::{SourceFile, SourceSpan};

const MAX_EXPANSION_DEPTH: usize = 32;

#[derive(Debug, Clone)]
pub struct MacroExpandOutput {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

impl MacroExpandOutput {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct MacroKey {
    source: String,
    name: String,
}

#[derive(Debug, Clone)]
struct MacroBinding {
    source: String,
    imported: String,
}

#[derive(Debug, Clone)]
struct MacroNamespace {
    source: String,
}

#[derive(Debug, Clone)]
struct MacroEnv {
    bindings: BTreeMap<String, MacroBinding>,
    namespaces: BTreeMap<String, MacroNamespace>,
    unavailable_bindings: BTreeSet<MacroKey>,
    unavailable_sources: BTreeSet<String>,
}

impl MacroEnv {
    fn from_module(
        module: &Module,
        registry: &MacroRegistry,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Self {
        let mut bindings = BTreeMap::new();
        let mut namespaces = BTreeMap::new();
        let mut unavailable_bindings = BTreeSet::new();
        let mut unavailable_sources = BTreeSet::new();

        for stmt in &module.body {
            let StmtKind::Import(import) = &stmt.kind else {
                continue;
            };
            if import.phase != ImportPhase::Macro {
                continue;
            }
            if import.side_effect_only {
                diagnostics.push(
                    Diagnostic::error("side-effect macro imports are not supported")
                        .with_code("MACRO006")
                        .with_label(Label::primary(stmt.span, "macro import has no binding")),
                );
                continue;
            }

            for specifier in &import.specifiers {
                match specifier {
                    ImportSpecifier::Named { imported, local } => {
                        if !registry.knows_source(&import.source) {
                            let mut diagnostic = Diagnostic::error(format!(
                                "unknown macro package `{}`",
                                import.source
                            ))
                            .with_code("MACRO001")
                            .with_label(Label::primary(stmt.span, "unknown macro package"));
                            if let Some(help) = official_macro_install_help(&import.source) {
                                diagnostic = diagnostic.with_help(help);
                            }
                            diagnostics.push(diagnostic);
                            unavailable_sources.insert(import.source.clone());
                        } else if !registry.contains(&import.source, &imported.name) {
                            diagnostics.push(
                                Diagnostic::error(format!(
                                    "unknown macro `{}` imported from `{}`",
                                    imported.name, import.source
                                ))
                                .with_code("MACRO001")
                                .with_label(Label::primary(imported.span, "unknown macro import")),
                            );
                            unavailable_bindings.insert(MacroKey {
                                source: import.source.clone(),
                                name: imported.name.clone(),
                            });
                        }
                        bindings.insert(
                            local.name.clone(),
                            MacroBinding {
                                source: import.source.clone(),
                                imported: imported.name.clone(),
                            },
                        );
                    }
                    ImportSpecifier::Namespace { local } => {
                        if !registry.knows_source(&import.source) {
                            let mut diagnostic = Diagnostic::error(format!(
                                "unknown macro package `{}`",
                                import.source
                            ))
                            .with_code("MACRO001")
                            .with_label(Label::primary(stmt.span, "unknown macro package"));
                            if let Some(help) = official_macro_install_help(&import.source) {
                                diagnostic = diagnostic.with_help(help);
                            }
                            diagnostics.push(diagnostic);
                            unavailable_sources.insert(import.source.clone());
                        } else if !registry.has_macros_from_source(&import.source) {
                            diagnostics.push(
                                Diagnostic::error(format!(
                                    "macro package `{}` does not export any macros",
                                    import.source
                                ))
                                .with_code("MACRO001")
                                .with_label(Label::primary(local.span, "empty macro namespace")),
                            );
                            unavailable_sources.insert(import.source.clone());
                        }
                        namespaces.insert(
                            local.name.clone(),
                            MacroNamespace {
                                source: import.source.clone(),
                            },
                        );
                    }
                }
            }
        }

        Self {
            bindings,
            namespaces,
            unavailable_bindings,
            unavailable_sources,
        }
    }

    fn direct(&self, name: &str) -> Option<&MacroBinding> {
        self.bindings.get(name)
    }

    fn namespace(&self, name: &str) -> Option<&MacroNamespace> {
        self.namespaces.get(name)
    }

    fn is_compile_time_name(&self, name: &str) -> bool {
        self.bindings.contains_key(name) || self.namespaces.contains_key(name)
    }

    fn unavailable(&self, source: &str, imported: &str) -> bool {
        self.unavailable_sources.contains(source)
            || self.unavailable_bindings.contains(&MacroKey {
                source: source.to_string(),
                name: imported.to_string(),
            })
    }
}

#[derive(Debug, Clone)]
pub struct MacroRegistry {
    macros: BTreeMap<MacroKey, Arc<dyn MacroProvider>>,
    sources: BTreeSet<String>,
}

impl Default for MacroRegistry {
    fn default() -> Self {
        Self::empty()
    }
}

impl MacroRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn empty() -> Self {
        Self {
            macros: BTreeMap::new(),
            sources: BTreeSet::new(),
        }
    }

    pub fn register_source(&mut self, source: &str) {
        self.sources.insert(canonical_import_source(source));
    }

    pub fn register(&mut self, source: &str, name: &str, provider: impl MacroProvider + 'static) {
        self.register_provider(source, name, Arc::new(provider));
    }

    pub fn register_provider(
        &mut self,
        source: &str,
        name: &str,
        provider: Arc<dyn MacroProvider>,
    ) {
        self.register_source(source);
        let source = canonical_import_source(source);
        self.macros.insert(
            MacroKey {
                source,
                name: name.into(),
            },
            provider,
        );
    }

    fn get(&self, source: &str, name: &str) -> Option<Arc<dyn MacroProvider>> {
        let source = canonical_import_source(source);
        self.macros
            .get(&MacroKey {
                source,
                name: name.into(),
            })
            .cloned()
    }

    fn contains(&self, source: &str, name: &str) -> bool {
        let source = canonical_import_source(source);
        self.macros.contains_key(&MacroKey {
            source,
            name: name.into(),
        })
    }

    fn knows_source(&self, source: &str) -> bool {
        self.sources.contains(&canonical_import_source(source))
    }

    fn has_macros_from_source(&self, source: &str) -> bool {
        let source = canonical_import_source(source);
        self.macros.keys().any(|key| key.source == source)
    }
}

fn canonical_import_source(source: &str) -> String {
    source.strip_prefix('@').unwrap_or(source).to_string()
}

fn official_macro_install_help(source: &str) -> Option<String> {
    let source = canonical_import_source(source);
    if matches!(source.as_str(), "lux/macros" | "lux/gmod/macros") {
        Some(format!(
            "install it with `luxc install @{source} --from github:{LUX_STD_REPO}`"
        ))
    } else {
        None
    }
}

pub trait MacroProvider: std::fmt::Debug + Send + Sync {
    fn expand(&self, ctx: &mut MacroContext<'_>, call: &MacroCall) -> Option<MacroExpansion>;
}

#[derive(Debug, Clone)]
pub enum MacroExpansion {
    Expr(Expr),
    Stmts(Vec<Stmt>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroCallPosition {
    Statement,
    Expression,
}

impl MacroCallPosition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Statement => "statement",
            Self::Expression => "expression",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MacroCall {
    pub source: String,
    pub imported: String,
    pub args: Vec<Expr>,
    pub span: SourceSpan,
    pub position: MacroCallPosition,
    remaining_segments: Vec<ChainSegment>,
}

pub struct MacroContext<'a> {
    file: &'a SourceFile,
    reserved: BTreeSet<String>,
    next_id: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> MacroContext<'a> {
    fn new(file: &'a SourceFile, module: &Module) -> Self {
        let mut reserved = BTreeSet::new();
        collect_module_names(module, &mut reserved);
        Self {
            file,
            reserved,
            next_id: 0,
            diagnostics: Vec::new(),
        }
    }

    pub fn file(&self) -> &SourceFile {
        self.file
    }

    pub fn gensym(&mut self, prefix: &str) -> String {
        loop {
            self.next_id += 1;
            let name = format!("__lux_macro_{prefix}_{}", self.next_id);
            if self.reserved.insert(name.clone()) {
                return name;
            }
        }
    }

    pub fn gensym_string(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        let prefix = sanitize_runtime_prefix(prefix);
        format!("__lux:{prefix}:{}", self.next_id)
    }

    pub fn error(&mut self, code: &str, message: impl Into<String>, span: SourceSpan) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_label(Label::primary(span, "here")),
        );
    }

    fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }
}

pub fn expand_macros(file: &SourceFile, module: &Module) -> MacroExpandOutput {
    expand_macros_with_registry(file, module, &MacroRegistry::default())
}

pub fn expand_macros_with_registry(
    file: &SourceFile,
    module: &Module,
    registry: &MacroRegistry,
) -> MacroExpandOutput {
    let mut diagnostics = Vec::new();
    let env = MacroEnv::from_module(module, registry, &mut diagnostics);
    let mut ctx = MacroContext::new(file, module);
    let mut expander = Expander { registry, env };

    let body = expander.expand_stmt_list(&module.body, &mut ctx, 0);
    expander.validate_no_runtime_macro_refs_in_stmts(&body, &mut ctx);
    diagnostics.extend(ctx.take_diagnostics());

    MacroExpandOutput {
        module: Module {
            body,
            span: module.span,
        },
        diagnostics,
    }
}

struct Expander<'a> {
    registry: &'a MacroRegistry,
    env: MacroEnv,
}

impl Expander<'_> {
    fn expand_stmt_list(
        &mut self,
        stmts: &[Stmt],
        ctx: &mut MacroContext<'_>,
        depth: usize,
    ) -> Vec<Stmt> {
        let mut out = Vec::new();
        for stmt in stmts {
            out.extend(self.expand_stmt(stmt, ctx, depth));
        }
        out
    }

    fn expand_stmt(&mut self, stmt: &Stmt, ctx: &mut MacroContext<'_>, depth: usize) -> Vec<Stmt> {
        if depth > MAX_EXPANSION_DEPTH {
            ctx.error(
                "MACRO005",
                "macro expansion exceeded the maximum recursion depth",
                stmt.span,
            );
            return vec![stmt.clone()];
        }

        match &stmt.kind {
            StmtKind::Expr(expr) => {
                if let Some(call) = self.macro_call(expr, MacroCallPosition::Statement) {
                    match self.expand_call(call, ctx, depth) {
                        Some(MacroExpansion::Expr(expr)) => {
                            return vec![Stmt {
                                kind: StmtKind::Expr(expr),
                                span: stmt.span,
                            }];
                        }
                        Some(MacroExpansion::Stmts(stmts)) => return stmts,
                        None => return vec![stmt.clone()],
                    }
                }

                vec![Stmt {
                    kind: StmtKind::Expr(self.expand_expr(expr, ctx, depth)),
                    span: stmt.span,
                }]
            }
            StmtKind::LocalDecl {
                mode,
                names,
                values,
            } => vec![Stmt {
                kind: StmtKind::LocalDecl {
                    mode: *mode,
                    names: names.clone(),
                    values: values
                        .iter()
                        .map(|expr| self.expand_expr(expr, ctx, depth))
                        .collect(),
                },
                span: stmt.span,
            }],
            StmtKind::LocalDestructure {
                mode,
                patterns,
                values,
            } => vec![Stmt {
                kind: StmtKind::LocalDestructure {
                    mode: *mode,
                    patterns: patterns
                        .iter()
                        .map(|pattern| self.expand_pattern(pattern, ctx, depth))
                        .collect(),
                    values: values
                        .iter()
                        .map(|expr| self.expand_expr(expr, ctx, depth))
                        .collect(),
                },
                span: stmt.span,
            }],
            StmtKind::Assign { targets, values } => vec![Stmt {
                kind: StmtKind::Assign {
                    targets: targets
                        .iter()
                        .map(|expr| self.expand_expr(expr, ctx, depth))
                        .collect(),
                    values: values
                        .iter()
                        .map(|expr| self.expand_expr(expr, ctx, depth))
                        .collect(),
                },
                span: stmt.span,
            }],
            StmtKind::CompoundAssign { target, op, value } => vec![Stmt {
                kind: StmtKind::CompoundAssign {
                    target: self.expand_expr(target, ctx, depth),
                    op: *op,
                    value: self.expand_expr(value, ctx, depth),
                },
                span: stmt.span,
            }],
            StmtKind::Return(values) => vec![Stmt {
                kind: StmtKind::Return(
                    values
                        .iter()
                        .map(|expr| self.expand_expr(expr, ctx, depth))
                        .collect(),
                ),
                span: stmt.span,
            }],
            StmtKind::ExportDecl {
                kind,
                realm,
                stmt: inner,
            } => {
                let expanded = self.expand_stmt(inner, ctx, depth);
                if expanded.len() == 1 {
                    vec![Stmt {
                        kind: StmtKind::ExportDecl {
                            kind: *kind,
                            realm: *realm,
                            stmt: Box::new(expanded[0].clone()),
                        },
                        span: stmt.span,
                    }]
                } else {
                    ctx.error(
                        "MACRO002",
                        "macro expansion cannot replace an export declaration with multiple statements",
                        stmt.span,
                    );
                    vec![stmt.clone()]
                }
            }
            StmtKind::FunctionDecl(decl) => vec![Stmt {
                kind: StmtKind::FunctionDecl(FunctionDecl {
                    name: decl.name.clone(),
                    params: decl.params.clone(),
                    vararg: decl.vararg,
                    body: self.expand_function_body(&decl.body, ctx, depth),
                }),
                span: stmt.span,
            }],
            StmtKind::EnumDecl(decl) => vec![Stmt {
                kind: StmtKind::EnumDecl(EnumDecl {
                    name: decl.name.clone(),
                    repr: decl.repr.clone(),
                    runtime: decl.runtime,
                    variants: decl
                        .variants
                        .iter()
                        .map(|variant| EnumVariant {
                            name: variant.name.clone(),
                            payload: variant.payload.clone(),
                            tag: variant
                                .tag
                                .as_ref()
                                .map(|expr| self.expand_expr(expr, ctx, depth)),
                            span: variant.span,
                        })
                        .collect(),
                }),
                span: stmt.span,
            }],
            StmtKind::RealmDecl { realm, stmt: inner } => {
                let expanded = self.expand_stmt(inner, ctx, depth);
                if expanded.len() == 1 {
                    vec![Stmt {
                        kind: StmtKind::RealmDecl {
                            realm: *realm,
                            stmt: Box::new(expanded[0].clone()),
                        },
                        span: stmt.span,
                    }]
                } else {
                    ctx.error(
                        "MACRO002",
                        "macro expansion cannot replace a realm declaration with multiple statements",
                        stmt.span,
                    );
                    vec![stmt.clone()]
                }
            }
            StmtKind::RealmBlock { realm, block } => vec![Stmt {
                kind: StmtKind::RealmBlock {
                    realm: *realm,
                    block: self.expand_block(block, ctx, depth),
                },
                span: stmt.span,
            }],
            StmtKind::InitDecl { realm, block } => vec![Stmt {
                kind: StmtKind::InitDecl {
                    realm: *realm,
                    block: self.expand_block(block, ctx, depth),
                },
                span: stmt.span,
            }],
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => vec![Stmt {
                kind: StmtKind::If {
                    condition: self.expand_expr(condition, ctx, depth),
                    then_block: self.expand_block(then_block, ctx, depth),
                    else_block: else_block
                        .as_ref()
                        .map(|block| self.expand_block(block, ctx, depth)),
                },
                span: stmt.span,
            }],
            StmtKind::While { condition, body } => vec![Stmt {
                kind: StmtKind::While {
                    condition: self.expand_expr(condition, ctx, depth),
                    body: self.expand_block(body, ctx, depth),
                },
                span: stmt.span,
            }],
            StmtKind::NumericFor {
                name,
                start,
                end,
                step,
                body,
            } => vec![Stmt {
                kind: StmtKind::NumericFor {
                    name: name.clone(),
                    start: self.expand_expr(start, ctx, depth),
                    end: self.expand_expr(end, ctx, depth),
                    step: step.as_ref().map(|expr| self.expand_expr(expr, ctx, depth)),
                    body: self.expand_block(body, ctx, depth),
                },
                span: stmt.span,
            }],
            StmtKind::GenericFor { names, iter, body } => vec![Stmt {
                kind: StmtKind::GenericFor {
                    names: names.clone(),
                    iter: iter
                        .iter()
                        .map(|expr| self.expand_expr(expr, ctx, depth))
                        .collect(),
                    body: self.expand_block(body, ctx, depth),
                },
                span: stmt.span,
            }],
            StmtKind::RepeatUntil { body, condition } => vec![Stmt {
                kind: StmtKind::RepeatUntil {
                    body: self.expand_block(body, ctx, depth),
                    condition: self.expand_expr(condition, ctx, depth),
                },
                span: stmt.span,
            }],
            StmtKind::Do(block) => vec![Stmt {
                kind: StmtKind::Do(self.expand_block(block, ctx, depth)),
                span: stmt.span,
            }],
            StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import(_)
            | StmtKind::PartOrderDecl(_)
            | StmtKind::ExternDecl(_)
            | StmtKind::HostPackageDecl(_)
            | StmtKind::ExportList { .. }
            | StmtKind::ExportAll { .. } => vec![stmt.clone()],
        }
    }

    fn expand_block(&mut self, block: &Block, ctx: &mut MacroContext<'_>, depth: usize) -> Block {
        Block {
            statements: self.expand_stmt_list(&block.statements, ctx, depth),
            tail: block
                .tail
                .as_ref()
                .map(|expr| self.expand_expr(expr, ctx, depth)),
            span: block.span,
        }
    }

    fn expand_function_body(
        &mut self,
        body: &FunctionBody,
        ctx: &mut MacroContext<'_>,
        depth: usize,
    ) -> FunctionBody {
        match body {
            FunctionBody::Expr(expr) => {
                FunctionBody::Expr(Box::new(self.expand_expr(expr, ctx, depth)))
            }
            FunctionBody::Block(block) => {
                FunctionBody::Block(Box::new(self.expand_block(block, ctx, depth)))
            }
        }
    }

    fn expand_expr(&mut self, expr: &Expr, ctx: &mut MacroContext<'_>, depth: usize) -> Expr {
        if depth > MAX_EXPANSION_DEPTH {
            ctx.error(
                "MACRO005",
                "macro expansion exceeded the maximum recursion depth",
                expr.span,
            );
            return expr.clone();
        }

        if let Some(call) = self.macro_call(expr, MacroCallPosition::Expression) {
            return match self.expand_call(call, ctx, depth) {
                Some(MacroExpansion::Expr(expr)) => expr,
                Some(MacroExpansion::Stmts(_)) => {
                    ctx.error(
                        "MACRO002",
                        "statement macro cannot be used in expression position",
                        expr.span,
                    );
                    expr.clone()
                }
                None => expr.clone(),
            };
        }

        match &expr.kind {
            ExprKind::Identifier(_)
            | ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Vararg
            | ExprKind::PipelinePlaceholder => expr.clone(),
            ExprKind::TemplateString(parts) => Expr {
                kind: ExprKind::TemplateString(
                    parts
                        .iter()
                        .map(|part| TemplatePart {
                            kind: match &part.kind {
                                TemplatePartKind::Text(text) => {
                                    TemplatePartKind::Text(text.clone())
                                }
                                TemplatePartKind::Expr(expr) => {
                                    TemplatePartKind::Expr(self.expand_expr(expr, ctx, depth))
                                }
                            },
                            span: part.span,
                        })
                        .collect(),
                ),
                span: expr.span,
            },
            ExprKind::Table(table) => Expr {
                kind: ExprKind::Table(TableExpr {
                    fields: table
                        .fields
                        .iter()
                        .map(|field| TableField {
                            kind: match &field.kind {
                                TableFieldKind::Array(expr) => {
                                    TableFieldKind::Array(self.expand_expr(expr, ctx, depth))
                                }
                                TableFieldKind::Named { name, value } => TableFieldKind::Named {
                                    name: name.clone(),
                                    value: self.expand_expr(value, ctx, depth),
                                },
                                TableFieldKind::ExprKey { key, value } => TableFieldKind::ExprKey {
                                    key: self.expand_expr(key, ctx, depth),
                                    value: self.expand_expr(value, ctx, depth),
                                },
                                TableFieldKind::Spread(value) => {
                                    TableFieldKind::Spread(self.expand_expr(value, ctx, depth))
                                }
                            },
                            span: field.span,
                        })
                        .collect(),
                }),
                span: expr.span,
            },
            ExprKind::Paren(inner) => Expr {
                kind: ExprKind::Paren(Box::new(self.expand_expr(inner, ctx, depth))),
                span: expr.span,
            },
            ExprKind::Unary { op, argument } => Expr {
                kind: ExprKind::Unary {
                    op: *op,
                    argument: Box::new(self.expand_expr(argument, ctx, depth)),
                },
                span: expr.span,
            },
            ExprKind::Binary { op, left, right } => Expr {
                kind: ExprKind::Binary {
                    op: *op,
                    left: Box::new(self.expand_expr(left, ctx, depth)),
                    right: Box::new(self.expand_expr(right, ctx, depth)),
                },
                span: expr.span,
            },
            ExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
                form,
            } => Expr {
                kind: ExprKind::Conditional {
                    condition: Box::new(self.expand_expr(condition, ctx, depth)),
                    then_branch: self.expand_expr_or_block(then_branch, ctx, depth),
                    else_branch: self.expand_expr_or_block(else_branch, ctx, depth),
                    form: *form,
                },
                span: expr.span,
            },
            ExprKind::Match(match_expr) => Expr {
                kind: ExprKind::Match(MatchExpr {
                    subject: Box::new(self.expand_expr(&match_expr.subject, ctx, depth)),
                    arms: match_expr
                        .arms
                        .iter()
                        .map(|arm| MatchArm {
                            pattern: arm.pattern.clone(),
                            body: self.expand_expr_or_block(&arm.body, ctx, depth),
                            span: arm.span,
                        })
                        .collect(),
                }),
                span: expr.span,
            },
            ExprKind::Do(block) => Expr {
                kind: ExprKind::Do(Box::new(self.expand_block(block, ctx, depth))),
                span: expr.span,
            },
            ExprKind::Function(function) => Expr {
                kind: ExprKind::Function(FunctionExpr {
                    params: function.params.clone(),
                    vararg: function.vararg,
                    body: self.expand_function_body(&function.body, ctx, depth),
                    arrow_kind: function.arrow_kind,
                }),
                span: expr.span,
            },
            ExprKind::Chain(chain) => Expr {
                kind: ExprKind::Chain(ChainExpr {
                    base: Box::new(self.expand_expr(&chain.base, ctx, depth)),
                    segments: chain
                        .segments
                        .iter()
                        .map(|segment| self.expand_chain_segment(segment, ctx, depth))
                        .collect(),
                }),
                span: expr.span,
            },
        }
    }

    fn expand_pattern(
        &mut self,
        pattern: &Pattern,
        ctx: &mut MacroContext<'_>,
        depth: usize,
    ) -> Pattern {
        Pattern {
            kind: match &pattern.kind {
                PatternKind::Identifier(name) => PatternKind::Identifier(name.clone()),
                PatternKind::Object(fields) => PatternKind::Object(
                    fields
                        .iter()
                        .map(|field| ObjectPatternField {
                            key: field.key.clone(),
                            pattern: self.expand_pattern(&field.pattern, ctx, depth),
                            default: field
                                .default
                                .as_ref()
                                .map(|expr| self.expand_expr(expr, ctx, depth)),
                            span: field.span,
                        })
                        .collect(),
                ),
                PatternKind::Array(items) => PatternKind::Array(
                    items
                        .iter()
                        .map(|item| ArrayPatternItem {
                            pattern: self.expand_pattern(&item.pattern, ctx, depth),
                            default: item
                                .default
                                .as_ref()
                                .map(|expr| self.expand_expr(expr, ctx, depth)),
                            span: item.span,
                        })
                        .collect(),
                ),
            },
            span: pattern.span,
        }
    }

    fn expand_expr_or_block(
        &mut self,
        item: &ExprOrBlock,
        ctx: &mut MacroContext<'_>,
        depth: usize,
    ) -> ExprOrBlock {
        match item {
            ExprOrBlock::Expr(expr) => {
                ExprOrBlock::Expr(Box::new(self.expand_expr(expr, ctx, depth)))
            }
            ExprOrBlock::Block(block) => {
                ExprOrBlock::Block(Box::new(self.expand_block(block, ctx, depth)))
            }
        }
    }

    fn expand_chain_segment(
        &mut self,
        segment: &ChainSegment,
        ctx: &mut MacroContext<'_>,
        depth: usize,
    ) -> ChainSegment {
        ChainSegment {
            kind: match &segment.kind {
                ChainSegmentKind::Member { name, optional } => ChainSegmentKind::Member {
                    name: name.clone(),
                    optional: *optional,
                },
                ChainSegmentKind::Index { index, optional } => ChainSegmentKind::Index {
                    index: self.expand_expr(index, ctx, depth),
                    optional: *optional,
                },
                ChainSegmentKind::Call { args, style } => ChainSegmentKind::Call {
                    args: args
                        .iter()
                        .map(|expr| self.expand_expr(expr, ctx, depth))
                        .collect(),
                    style: *style,
                },
                ChainSegmentKind::SafeDotCall { name, args, style } => {
                    ChainSegmentKind::SafeDotCall {
                        name: name.clone(),
                        args: args
                            .iter()
                            .map(|expr| self.expand_expr(expr, ctx, depth))
                            .collect(),
                        style: *style,
                    }
                }
                ChainSegmentKind::MethodCall {
                    name,
                    args,
                    optional,
                    style,
                } => ChainSegmentKind::MethodCall {
                    name: name.clone(),
                    args: args
                        .iter()
                        .map(|expr| self.expand_expr(expr, ctx, depth))
                        .collect(),
                    optional: *optional,
                    style: *style,
                },
            },
            span: segment.span,
        }
    }

    fn expand_call(
        &mut self,
        call: MacroCall,
        ctx: &mut MacroContext<'_>,
        depth: usize,
    ) -> Option<MacroExpansion> {
        let Some(provider) = self.registry.get(&call.source, &call.imported) else {
            if !self.env.unavailable(&call.source, &call.imported) {
                ctx.error(
                    "MACRO001",
                    format!(
                        "unknown macro `{}` imported from `{}`",
                        call.imported, call.source
                    ),
                    call.span,
                );
            }
            return None;
        };

        let expansion = provider.expand(ctx, &call)?;

        Some(match expansion {
            MacroExpansion::Expr(expr) => {
                let expr = self.expand_expr(&expr, ctx, depth + 1);
                MacroExpansion::Expr(append_segments(expr, call.remaining_segments))
            }
            MacroExpansion::Stmts(stmts) => {
                MacroExpansion::Stmts(self.expand_stmt_list(&stmts, ctx, depth + 1))
            }
        })
    }

    fn macro_call(&self, expr: &Expr, position: MacroCallPosition) -> Option<MacroCall> {
        let ExprKind::Chain(chain) = &expr.kind else {
            return None;
        };

        let ExprKind::Identifier(base) = &chain.base.kind else {
            return None;
        };

        if let Some(binding) = self.env.direct(&base.name) {
            let Some(first) = chain.segments.first() else {
                return None;
            };
            if let ChainSegmentKind::Call { args, .. } = &first.kind {
                return Some(MacroCall {
                    source: binding.source.clone(),
                    imported: binding.imported.clone(),
                    args: args.clone(),
                    span: first.span,
                    position,
                    remaining_segments: chain.segments[1..].to_vec(),
                });
            }
        }

        let namespace = self.env.namespace(&base.name)?;
        let [first, second, rest @ ..] = chain.segments.as_slice() else {
            return None;
        };
        let ChainSegmentKind::Member {
            name,
            optional: false,
        } = &first.kind
        else {
            return None;
        };
        let ChainSegmentKind::Call { args, .. } = &second.kind else {
            return None;
        };

        Some(MacroCall {
            source: namespace.source.clone(),
            imported: name.name.clone(),
            args: args.clone(),
            span: self.span_from_two(first.span, second.span),
            position,
            remaining_segments: rest.to_vec(),
        })
    }

    fn span_from_two(&self, first: SourceSpan, second: SourceSpan) -> SourceSpan {
        SourceSpan::new(
            first.file_id,
            first.byte_start.min(second.byte_start),
            first.byte_end.max(second.byte_end),
        )
    }

    fn validate_no_runtime_macro_refs_in_stmts(&self, stmts: &[Stmt], ctx: &mut MacroContext<'_>) {
        for stmt in stmts {
            self.validate_no_runtime_macro_refs_in_stmt(stmt, ctx);
        }
    }

    fn validate_no_runtime_macro_refs_in_stmt(&self, stmt: &Stmt, ctx: &mut MacroContext<'_>) {
        match &stmt.kind {
            StmtKind::LocalDecl { values, .. } | StmtKind::Return(values) => {
                for expr in values {
                    self.validate_no_runtime_macro_refs_in_expr(expr, ctx);
                }
            }
            StmtKind::LocalDestructure {
                patterns, values, ..
            } => {
                for pattern in patterns {
                    self.validate_pattern(pattern, ctx);
                }
                for expr in values {
                    self.validate_no_runtime_macro_refs_in_expr(expr, ctx);
                }
            }
            StmtKind::Assign { targets, .. } => {
                for target in targets {
                    self.validate_no_runtime_macro_refs_in_expr(target, ctx);
                }
                if let StmtKind::Assign { values, .. } = &stmt.kind {
                    for value in values {
                        self.validate_no_runtime_macro_refs_in_expr(value, ctx);
                    }
                }
            }
            StmtKind::CompoundAssign { target, value, .. } => {
                self.validate_no_runtime_macro_refs_in_expr(target, ctx);
                self.validate_no_runtime_macro_refs_in_expr(value, ctx);
            }
            StmtKind::Expr(expr) => self.validate_no_runtime_macro_refs_in_expr(expr, ctx),
            StmtKind::ExportDecl { stmt, .. } | StmtKind::RealmDecl { stmt, .. } => {
                self.validate_no_runtime_macro_refs_in_stmt(stmt, ctx)
            }
            StmtKind::RealmBlock { block, .. } | StmtKind::InitDecl { block, .. } => {
                self.validate_block(block, ctx)
            }
            StmtKind::FunctionDecl(decl) => self.validate_function_body(&decl.body, ctx),
            StmtKind::EnumDecl(decl) => {
                for variant in &decl.variants {
                    if let Some(tag) = &variant.tag {
                        self.validate_no_runtime_macro_refs_in_expr(tag, ctx);
                    }
                }
            }
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                self.validate_no_runtime_macro_refs_in_expr(condition, ctx);
                self.validate_block(then_block, ctx);
                if let Some(block) = else_block {
                    self.validate_block(block, ctx);
                }
            }
            StmtKind::While { condition, body } => {
                self.validate_no_runtime_macro_refs_in_expr(condition, ctx);
                self.validate_block(body, ctx);
            }
            StmtKind::NumericFor {
                start,
                end,
                step,
                body,
                ..
            } => {
                self.validate_no_runtime_macro_refs_in_expr(start, ctx);
                self.validate_no_runtime_macro_refs_in_expr(end, ctx);
                if let Some(step) = step {
                    self.validate_no_runtime_macro_refs_in_expr(step, ctx);
                }
                self.validate_block(body, ctx);
            }
            StmtKind::GenericFor { iter, body, .. } => {
                for expr in iter {
                    self.validate_no_runtime_macro_refs_in_expr(expr, ctx);
                }
                self.validate_block(body, ctx);
            }
            StmtKind::RepeatUntil { body, condition } => {
                self.validate_block(body, ctx);
                self.validate_no_runtime_macro_refs_in_expr(condition, ctx);
            }
            StmtKind::Do(block) => self.validate_block(block, ctx),
            StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import(_)
            | StmtKind::PartOrderDecl(_)
            | StmtKind::ExternDecl(_)
            | StmtKind::HostPackageDecl(_)
            | StmtKind::ExportList { .. }
            | StmtKind::ExportAll { .. } => {}
        }
    }

    fn validate_block(&self, block: &Block, ctx: &mut MacroContext<'_>) {
        self.validate_no_runtime_macro_refs_in_stmts(&block.statements, ctx);
        if let Some(tail) = &block.tail {
            self.validate_no_runtime_macro_refs_in_expr(tail, ctx);
        }
    }

    fn validate_function_body(&self, body: &FunctionBody, ctx: &mut MacroContext<'_>) {
        match body {
            FunctionBody::Expr(expr) => self.validate_no_runtime_macro_refs_in_expr(expr, ctx),
            FunctionBody::Block(block) => self.validate_block(block, ctx),
        }
    }

    fn validate_no_runtime_macro_refs_in_expr(&self, expr: &Expr, ctx: &mut MacroContext<'_>) {
        match &expr.kind {
            ExprKind::Identifier(ident) if self.env.is_compile_time_name(&ident.name) => {
                ctx.error(
                    "MACRO004",
                    format!(
                        "compile-time macro `{}` cannot be used as a runtime value",
                        ident.name
                    ),
                    ident.span,
                );
            }
            ExprKind::Identifier(_)
            | ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Vararg
            | ExprKind::PipelinePlaceholder => {}
            ExprKind::TemplateString(parts) => {
                for part in parts {
                    if let TemplatePartKind::Expr(expr) = &part.kind {
                        self.validate_no_runtime_macro_refs_in_expr(expr, ctx);
                    }
                }
            }
            ExprKind::Table(table) => {
                for field in &table.fields {
                    match &field.kind {
                        TableFieldKind::Array(expr) => {
                            self.validate_no_runtime_macro_refs_in_expr(expr, ctx)
                        }
                        TableFieldKind::Named { value, .. } => {
                            self.validate_no_runtime_macro_refs_in_expr(value, ctx)
                        }
                        TableFieldKind::ExprKey { key, value } => {
                            self.validate_no_runtime_macro_refs_in_expr(key, ctx);
                            self.validate_no_runtime_macro_refs_in_expr(value, ctx);
                        }
                        TableFieldKind::Spread(value) => {
                            self.validate_no_runtime_macro_refs_in_expr(value, ctx)
                        }
                    }
                }
            }
            ExprKind::Paren(expr) => self.validate_no_runtime_macro_refs_in_expr(expr, ctx),
            ExprKind::Unary { argument, .. } => {
                self.validate_no_runtime_macro_refs_in_expr(argument, ctx)
            }
            ExprKind::Binary { left, right, .. } => {
                self.validate_no_runtime_macro_refs_in_expr(left, ctx);
                self.validate_no_runtime_macro_refs_in_expr(right, ctx);
            }
            ExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.validate_no_runtime_macro_refs_in_expr(condition, ctx);
                self.validate_expr_or_block(then_branch, ctx);
                self.validate_expr_or_block(else_branch, ctx);
            }
            ExprKind::Match(match_expr) => {
                self.validate_no_runtime_macro_refs_in_expr(&match_expr.subject, ctx);
                for arm in &match_expr.arms {
                    self.validate_expr_or_block(&arm.body, ctx);
                }
            }
            ExprKind::Do(block) => self.validate_block(block, ctx),
            ExprKind::Function(function) => self.validate_function_body(&function.body, ctx),
            ExprKind::Chain(chain) => {
                if let ExprKind::Identifier(ident) = &chain.base.kind {
                    if self.env.is_compile_time_name(&ident.name) {
                        ctx.error(
                            "MACRO004",
                            format!(
                                "compile-time macro `{}` cannot be used as a runtime value",
                                ident.name
                            ),
                            ident.span,
                        );
                    }
                }
                self.validate_no_runtime_macro_refs_in_expr(&chain.base, ctx);
                for segment in &chain.segments {
                    match &segment.kind {
                        ChainSegmentKind::Member { .. } => {}
                        ChainSegmentKind::Index { index, .. } => {
                            self.validate_no_runtime_macro_refs_in_expr(index, ctx)
                        }
                        ChainSegmentKind::Call { args, .. }
                        | ChainSegmentKind::SafeDotCall { args, .. }
                        | ChainSegmentKind::MethodCall { args, .. } => {
                            for arg in args {
                                self.validate_no_runtime_macro_refs_in_expr(arg, ctx);
                            }
                        }
                    }
                }
            }
        }
    }

    fn validate_pattern(&self, pattern: &Pattern, ctx: &mut MacroContext<'_>) {
        match &pattern.kind {
            PatternKind::Identifier(_) => {}
            PatternKind::Object(fields) => {
                for field in fields {
                    self.validate_pattern(&field.pattern, ctx);
                    if let Some(default) = &field.default {
                        self.validate_no_runtime_macro_refs_in_expr(default, ctx);
                    }
                }
            }
            PatternKind::Array(items) => {
                for item in items {
                    self.validate_pattern(&item.pattern, ctx);
                    if let Some(default) = &item.default {
                        self.validate_no_runtime_macro_refs_in_expr(default, ctx);
                    }
                }
            }
        }
    }

    fn validate_expr_or_block(&self, item: &ExprOrBlock, ctx: &mut MacroContext<'_>) {
        match item {
            ExprOrBlock::Expr(expr) => self.validate_no_runtime_macro_refs_in_expr(expr, ctx),
            ExprOrBlock::Block(block) => self.validate_block(block, ctx),
        }
    }
}

fn append_segments(expr: Expr, segments: Vec<ChainSegment>) -> Expr {
    if segments.is_empty() {
        return expr;
    }
    let span = SourceSpan::new(
        expr.span.file_id,
        expr.span.byte_start,
        segments
            .last()
            .map(|segment| segment.span.byte_end)
            .unwrap_or(expr.span.byte_end),
    );
    Expr {
        kind: ExprKind::Chain(ChainExpr {
            base: Box::new(expr),
            segments,
        }),
        span,
    }
}

fn collect_module_names(module: &Module, names: &mut BTreeSet<String>) {
    for stmt in &module.body {
        collect_stmt_names(stmt, names);
    }
}

fn collect_stmt_names(stmt: &Stmt, names: &mut BTreeSet<String>) {
    match &stmt.kind {
        StmtKind::LocalDecl {
            names: local_names, ..
        } => {
            names.extend(local_names.iter().map(|name| name.name.clone()));
        }
        StmtKind::FunctionDecl(decl) => {
            if let FunctionName::Simple(name) = &decl.name {
                names.insert(name.name.clone());
            }
            collect_function_body_names(&decl.body, names);
        }
        StmtKind::EnumDecl(decl) => {
            if decl.runtime {
                names.insert(decl.name.name.clone());
            }
        }
        StmtKind::HostPackageDecl(_) | StmtKind::ExternDecl(_) | StmtKind::PartOrderDecl(_) => {}
        StmtKind::ExportDecl { stmt, .. } | StmtKind::RealmDecl { stmt, .. } => {
            collect_stmt_names(stmt, names)
        }
        StmtKind::RealmBlock { block, .. } | StmtKind::InitDecl { block, .. } => {
            for stmt in &block.statements {
                collect_stmt_names(stmt, names);
            }
        }
        StmtKind::If {
            then_block,
            else_block,
            ..
        } => {
            collect_block_names(then_block, names);
            if let Some(block) = else_block {
                collect_block_names(block, names);
            }
        }
        StmtKind::While { body, .. }
        | StmtKind::NumericFor { body, .. }
        | StmtKind::GenericFor { body, .. }
        | StmtKind::RepeatUntil { body, .. }
        | StmtKind::Do(body) => collect_block_names(body, names),
        StmtKind::Import(import) => {
            for specifier in &import.specifiers {
                match specifier {
                    ImportSpecifier::Named { local, .. } | ImportSpecifier::Namespace { local } => {
                        names.insert(local.name.clone());
                    }
                }
            }
        }
        _ => {}
    }
}

fn collect_block_names(block: &Block, names: &mut BTreeSet<String>) {
    for stmt in &block.statements {
        collect_stmt_names(stmt, names);
    }
}

fn collect_function_body_names(body: &FunctionBody, names: &mut BTreeSet<String>) {
    if let FunctionBody::Block(block) = body {
        collect_block_names(block, names);
    }
}

fn sanitize_runtime_prefix(prefix: &str) -> String {
    let mut out = String::new();
    for ch in prefix.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "id".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::{Block, Expr, ExprKind, Realm, Stmt, StmtKind};
    use crate::codegen::LuaCodegen;
    use crate::compile_time::CompileTimePackageRegistry;
    use crate::lex::Lexer;
    use crate::lower::Lowerer;
    use crate::parse::Parser;
    use crate::resolve::Resolver;
    use crate::source::SourceFile;
    use crate::test_support::test_std_package_root;

    use super::{MacroExpansion, MacroProvider, expand_macros_with_registry};

    fn expand(input: &str) -> super::MacroExpandOutput {
        let file = SourceFile::new(0, None, input);
        let lex = Lexer::new(&file).lex_all();
        assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
        let parsed = Parser::new(&lex.tokens).parse_module();
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let std_root = test_std_package_root();
        let compile_time =
            CompileTimePackageRegistry::load_default_with_package_roots(&[std_root.clone()])
                .expect("compile-time registry");
        let mut registry = super::MacroRegistry::empty();
        compile_time
            .register_macros(&mut registry)
            .expect("register compile-time macros");
        let output = expand_macros_with_registry(&file, &parsed.module, &registry);
        let _ = std::fs::remove_dir_all(std_root);
        output
    }

    fn lua_for_expanded(output: &super::MacroExpandOutput) -> String {
        let resolved = Resolver::resolve(&output.module);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:#?}",
            resolved.diagnostics
        );
        let ir = Lowerer::lower(&output.module, &resolved).expect("lower");
        LuaCodegen::generate(&ir).expect("codegen").lua
    }

    fn expect_realm_block(stmt: &Stmt, expected: Realm) -> &Block {
        let StmtKind::RealmBlock { realm, block } = &stmt.kind else {
            panic!("expected {expected:?} realm block, got {stmt:#?}");
        };
        assert_eq!(*realm, expected);
        block
    }

    #[test]
    fn expands_expression_macros() {
        let output = expand("import macro { dbg } from \"lux/macros\"\nlocal x = dbg(1)");
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        let StmtKind::LocalDecl { values, .. } = &output.module.body[1].kind else {
            panic!("expected local declaration");
        };
        let ExprKind::Do(block) = &values[0].kind else {
            panic!(
                "expected dbg to expand to setup expression, got {:#?}",
                values[0]
            );
        };
        assert_eq!(block.statements.len(), 2);
        assert!(block.tail.is_some());
    }

    #[test]
    fn expands_statement_macros_to_scoped_statements() {
        let output = expand(
            "import macro { defineNetReceiver } from \"lux/gmod/macros\"\ndefineNetReceiver(\"x\", () => nil)",
        );
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert_eq!(output.module.body.len(), 2);
        let shared = expect_realm_block(&output.module.body[1], Realm::Shared);
        assert_eq!(shared.statements.len(), 4);
        expect_realm_block(&shared.statements[2], Realm::Server);

        let resolved = Resolver::resolve(&output.module);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:#?}",
            resolved.diagnostics
        );
        let ir = Lowerer::lower(&output.module, &resolved).expect("lower");
        let lua = LuaCodegen::generate(&ir).expect("codegen").lua;
        assert!(lua.contains("do\n  local __lux_macro_net_name_"));
        assert!(!lua.contains("if SERVER then"), "{lua}");
        assert!(lua.contains("util.AddNetworkString(__lux_macro_net_name_"));
        assert!(lua.contains("net.Receive(__lux_macro_net_name_"));
    }

    #[test]
    fn gmod_realm_specific_macros_guard_argument_evaluation() {
        let output = expand(
            "import macro { defineNetString, defineServerNetReceiver, defineClientNetReceiver } from \"lux/gmod/macros\"\ndefineNetString(makeString())\ndefineServerNetReceiver(makeServerName(), makeServerCallback())\ndefineClientNetReceiver(makeClientName(), makeClientCallback())",
        );
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        expect_realm_block(&output.module.body[1], Realm::Server);
        expect_realm_block(&output.module.body[2], Realm::Server);
        expect_realm_block(&output.module.body[3], Realm::Client);

        let lua = lua_for_expanded(&output);
        let string_arg = lua.find("makeString()").expect("net string argument");
        let string_register = lua[string_arg..]
            .find("util.AddNetworkString(")
            .map(|offset| offset + string_arg)
            .expect("net string registration");
        assert!(string_arg < string_register, "{lua}");

        let server_name = lua.find("makeServerName()").expect("server name argument");
        let server_callback = lua
            .find("makeServerCallback()")
            .expect("server callback argument");
        let server_register = lua[server_callback..]
            .find("util.AddNetworkString(")
            .map(|offset| offset + server_callback)
            .expect("server net string registration");
        let server_receive = lua[server_callback..]
            .find("net.Receive(")
            .map(|offset| offset + server_callback)
            .expect("server receiver registration");
        assert!(server_name < server_callback, "{lua}");
        assert!(server_callback < server_register, "{lua}");
        assert!(server_register < server_receive, "{lua}");
        assert!(server_callback < server_receive, "{lua}");

        let client_name = lua.find("makeClientName()").expect("client name argument");
        let client_callback = lua
            .find("makeClientCallback()")
            .expect("client callback argument");
        let client_receive = lua[client_callback..]
            .find("net.Receive(")
            .map(|offset| offset + client_callback)
            .expect("client receiver registration");
        assert!(client_name < client_callback, "{lua}");
        assert!(client_callback < client_receive, "{lua}");
        assert!(!lua.contains("if SERVER then"), "{lua}");
        assert!(!lua.contains("if CLIENT then"), "{lua}");
    }

    #[test]
    fn gmod_shared_net_receiver_keeps_shared_argument_evaluation() {
        let output = expand(
            "import macro { defineSharedNetReceiver } from \"lux/gmod/macros\"\ndefineSharedNetReceiver(makeName(), makeCallback())",
        );
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        let shared = expect_realm_block(&output.module.body[1], Realm::Shared);
        assert_eq!(shared.statements.len(), 4);
        expect_realm_block(&shared.statements[2], Realm::Server);

        let lua = lua_for_expanded(&output);
        let do_scope = lua
            .find("do\n  local __lux_macro_net_name_")
            .expect("do scope");
        let name = lua.find("makeName()").expect("name argument");
        let callback = lua.find("makeCallback()").expect("callback argument");
        let add_string = lua.find("util.AddNetworkString(").expect("add net string");
        let receive = lua.find("net.Receive(").expect("receive call");
        assert!(do_scope < name, "{lua}");
        assert!(name < callback, "{lua}");
        assert!(callback < add_string, "{lua}");
        assert!(add_string < receive, "{lua}");
        assert!(!lua.contains("if SERVER then"), "{lua}");
    }

    #[test]
    fn gmod_net_receiver_aliases_report_their_own_names() {
        let shared = expand(
            "import macro { defineSharedNetReceiver } from \"lux/gmod/macros\"\ndefineSharedNetReceiver(\"x\")",
        );
        assert!(shared.has_errors());
        assert!(
            shared
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("`defineSharedNetReceiver`")),
            "{:#?}",
            shared.diagnostics
        );

        let alias = expand(
            "import macro { defineNetReceiver } from \"lux/gmod/macros\"\ndefineNetReceiver(\"x\")",
        );
        assert!(alias.has_errors());
        assert!(
            alias
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("`defineNetReceiver`")),
            "{:#?}",
            alias.diagnostics
        );
    }

    #[test]
    fn statement_hook_macro_preserves_argument_order_in_scope() {
        let output = expand(
            "import macro { defineHook } from \"lux/gmod/macros\"\ndefineHook(makeEvent(), makeId(), makeCallback())",
        );
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert_eq!(output.module.body.len(), 2);
        assert!(matches!(output.module.body[1].kind, StmtKind::Do(_)));

        let resolved = Resolver::resolve(&output.module);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:#?}",
            resolved.diagnostics
        );
        let ir = Lowerer::lower(&output.module, &resolved).expect("lower");
        let lua = LuaCodegen::generate(&ir).expect("codegen").lua;
        let event = lua.find("makeEvent()").expect("event call");
        let id = lua.find("makeId()").expect("id call");
        let callback = lua.find("makeCallback()").expect("callback call");
        let hook_add = lua.find("hook.Add").expect("hook add call");

        assert!(lua.contains("do\n  local __lux_macro_hook_event_"), "{lua}");
        assert!(event < id, "{lua}");
        assert!(id < callback, "{lua}");
        assert!(callback < hook_add, "{lua}");
    }

    #[test]
    fn statement_only_macros_report_precise_expression_position_error() {
        let output = expand(
            "import macro { defineNetReceiver } from \"lux/gmod/macros\"\nlocal receiver = defineNetReceiver(\"x\", () => nil)",
        );
        assert!(output.has_errors());
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("MACRO002")
                    && diagnostic
                        .message
                        .contains("statement macro cannot be used in expression position")
            }),
            "{:#?}",
            output.diagnostics
        );
    }

    #[test]
    fn supports_namespace_macro_imports() {
        let output =
            expand("import macro * as macros from \"lux/macros\"\nlocal x = macros.dbg(1)");
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    }

    #[test]
    fn expression_macros_expand_in_nested_value_positions() {
        let output = expand(
            "import macro { dbg } from \"lux/macros\"\nfn use(value) = value\nfn demo(x) {\n  local a = dbg(x)\n  local b = { dbg(a), named = dbg(a + 1) }\n  return use(dbg(b[1]))\n}",
        );
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);

        let resolved = Resolver::resolve(&output.module);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:#?}",
            resolved.diagnostics
        );
        let ir = Lowerer::lower(&output.module, &resolved).expect("lower");
        let lua = LuaCodegen::generate(&ir).expect("codegen").lua;
        assert!(!lua.contains("(function(__lux_macro_dbg_"), "{lua}");
        assert!(lua.contains("local __lux_macro_dbg_"));
        assert!(lua.contains("print("));
    }

    #[test]
    fn expression_macro_position_is_visible_to_lux_code() {
        let output = expand(
            "import macro { defineHook } from \"lux/gmod/macros\"\nlocal hookId = defineHook(\"HUDPaint\", () => drawLuxHud())",
        );
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);

        let resolved = Resolver::resolve(&output.module);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:#?}",
            resolved.diagnostics
        );
        let ir = Lowerer::lower(&output.module, &resolved).expect("lower");
        let lua = LuaCodegen::generate(&ir).expect("codegen").lua;
        assert!(!lua.contains("local hookId = (function()"), "{lua}");
        assert!(lua.contains("local hookId"));
        assert!(lua.contains("hook.Add("));
        assert!(lua.contains("\"HUDPaint\""));
        assert!(lua.contains("\"__lux:hook:"));
        assert!(lua.contains("hookId = __lux_macro_hook_id_"));
    }

    #[test]
    fn expression_macros_can_preserve_argument_evaluation_order() {
        let output = expand(
            "import macro { defineHook } from \"lux/gmod/macros\"\nlocal hookId = defineHook(makeEvent(), makeId(), makeCallback())",
        );
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);

        let resolved = Resolver::resolve(&output.module);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:#?}",
            resolved.diagnostics
        );
        let ir = Lowerer::lower(&output.module, &resolved).expect("lower");
        let lua = LuaCodegen::generate(&ir).expect("codegen").lua;
        let event = lua.find("makeEvent()").expect("event call");
        let id = lua.find("makeId()").expect("id call");
        let callback = lua.find("makeCallback()").expect("callback call");
        let hook_add = lua.find("hook.Add").expect("hook add call");

        assert!(event < id, "{lua}");
        assert!(id < callback, "{lua}");
        assert!(callback < hook_add, "{lua}");
    }

    #[test]
    fn rejects_macro_as_runtime_value() {
        let output = expand("import macro { dbg } from \"lux/macros\"\nlocal x = dbg");
        assert!(output.has_errors());
        assert_eq!(output.diagnostics[0].code.as_deref(), Some("MACRO004"));
    }

    #[test]
    fn helper_exports_are_not_registered_as_user_macros() {
        let output =
            expand("import macro { localExpr } from \"lux/compile/macro\"\nlocalExpr(nil)");
        assert!(output.has_errors());
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("MACRO001")
                    && diagnostic
                        .message
                        .contains("unknown macro `localExpr` imported from `lux/compile/macro`")
            }),
            "{:#?}",
            output.diagnostics
        );

        let output = expand("import macro { serverOnly } from \"lux/gmod/realm\"\nserverOnly(nil)");
        assert!(output.has_errors());
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("MACRO001")
                    && diagnostic
                        .message
                        .contains("unknown macro package `lux/gmod/realm`")
            }),
            "{:#?}",
            output.diagnostics
        );
    }

    #[test]
    fn invalid_macro_imports_are_reported_even_when_unused() {
        let output = expand("import macro { nope } from \"missing/macros\"");
        assert!(output.has_errors());
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("MACRO001")
                    && diagnostic
                        .message
                        .contains("unknown macro package `missing/macros`")
            }),
            "{:#?}",
            output.diagnostics
        );

        let output = expand("import macro { localExpr } from \"lux/compile/macro\"");
        assert!(output.has_errors());
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("MACRO001")
                    && diagnostic
                        .message
                        .contains("unknown macro `localExpr` imported from `lux/compile/macro`")
            }),
            "{:#?}",
            output.diagnostics
        );

        let output = expand("import macro * as missing from \"missing/macros\"");
        assert!(output.has_errors());
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("MACRO001")
                    && diagnostic
                        .message
                        .contains("unknown macro package `missing/macros`")
            }),
            "{:#?}",
            output.diagnostics
        );

        let output = expand("import macro * as realm from \"lux/gmod/realm\"");
        assert!(output.has_errors());
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("MACRO001")
                    && diagnostic
                        .message
                        .contains("unknown macro package `lux/gmod/realm`")
            }),
            "{:#?}",
            output.diagnostics
        );
    }

    #[test]
    fn missing_official_macro_package_suggests_install_command() {
        let file = SourceFile::new(0, None, "import macro { dbg } from \"@lux/macros\"\ndbg(1)");
        let lex = Lexer::new(&file).lex_all();
        assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
        let parsed = Parser::new(&lex.tokens).parse_module();
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);

        let output =
            expand_macros_with_registry(&file, &parsed.module, &super::MacroRegistry::empty());
        assert!(output.has_errors());
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("MACRO001")
                    && diagnostic.help.as_deref().is_some_and(|help| {
                        help.contains("luxc install @lux/macros --from github:TimeWatcher/lux-packages")
                    })
            }),
            "{:#?}",
            output.diagnostics
        );
    }

    #[test]
    fn namespace_macro_member_errors_stay_at_call_site() {
        let output =
            expand("import macro * as macros from \"lux/macros\"\nlocal x = macros.nope(1)");
        assert!(output.has_errors());
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("MACRO001")
                    && diagnostic
                        .message
                        .contains("unknown macro `nope` imported from `lux/macros`")
            }),
            "{:#?}",
            output.diagnostics
        );
    }

    #[derive(Debug)]
    struct LiteralMacro;

    impl MacroProvider for LiteralMacro {
        fn expand(
            &self,
            _ctx: &mut super::MacroContext<'_>,
            call: &super::MacroCall,
        ) -> Option<MacroExpansion> {
            Some(MacroExpansion::Expr(Expr {
                kind: ExprKind::String(format!("expanded:{}", call.imported)),
                span: call.span,
            }))
        }
    }

    #[test]
    fn supports_custom_macro_providers() {
        let file = SourceFile::new(
            0,
            None,
            "import macro { literal } from \"project/macros\"\nlocal x = literal()",
        );
        let lex = Lexer::new(&file).lex_all();
        assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
        let parsed = Parser::new(&lex.tokens).parse_module();
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);

        let mut registry = super::MacroRegistry::new();
        registry.register("project/macros", "literal", LiteralMacro);
        let output = expand_macros_with_registry(&file, &parsed.module, &registry);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);

        let StmtKind::LocalDecl { values, .. } = &output.module.body[1].kind else {
            panic!("expected local declaration");
        };
        assert!(matches!(&values[0].kind, ExprKind::String(value) if value == "expanded:literal"));
    }
}
