use std::collections::{BTreeSet, HashMap, HashSet};

use gmod_api_db::{ApiIndex, ApiRealm};

use crate::ast::*;
use crate::diag::{Diagnostic, Label, Severity};
use crate::module::{RealmAvailability, RealmSet};
use crate::source::SourceSpan;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindingId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    Local,
    Const,
    Param,
    Function,
    Import,
    MacroImport,
}

impl BindingKind {
    fn is_immutable_binding(self) -> bool {
        matches!(
            self,
            BindingKind::Const | BindingKind::Import | BindingKind::MacroImport
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub id: BindingId,
    pub name: String,
    pub kind: BindingKind,
    pub span: SourceSpan,
    pub source_module: Option<String>,
    pub imported_name: Option<String>,
    pub available_realms: RealmSet,
    pub module_scope: bool,
    pub initialized_at: Option<usize>,
    pub hoisted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSymbol {
    pub binding: BindingId,
    pub local_name: String,
    pub binding_kind: BindingKind,
    pub source_module: Option<String>,
    pub imported_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedExternalSymbol {
    pub path: Vec<String>,
    pub availability: RealmAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    pub local_name: String,
    pub binding: BindingId,
    pub span: SourceSpan,
    pub realm: Option<Realm>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEdge {
    pub source: String,
    pub specifiers: Vec<ModuleEdgeSpecifier>,
    pub side_effect_only: bool,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEdgeSpecifier {
    pub imported: String,
    pub local: String,
    pub namespace: bool,
    pub active_realms: RealmSet,
    pub binding: Option<BindingId>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone)]
pub struct ResolveOutput {
    pub bindings: Vec<Binding>,
    pub exports: Vec<Export>,
    pub module_edges: Vec<ModuleEdge>,
    pub symbols_by_span: HashMap<SourceSpan, ResolvedSymbol>,
    pub external_symbols_by_span: HashMap<SourceSpan, ResolvedExternalSymbol>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvePart<'a> {
    pub module: &'a Module,
    pub default_realm: Realm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownExternalPolicy {
    Allow,
    Warn,
    Error,
}

impl UnknownExternalPolicy {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternSymbol {
    pub path: Vec<String>,
    pub availability: RealmAvailability,
    pub span: Option<SourceSpan>,
}

impl ExternSymbol {
    pub fn known(path: impl AsRef<str>, realm: Realm) -> Self {
        Self {
            path: symbol_path(path.as_ref()),
            availability: RealmAvailability::known(realm),
            span: None,
        }
    }

    pub fn from_decl(decl: &ExternDecl) -> Self {
        Self {
            path: decl.path.iter().map(|part| part.name.clone()).collect(),
            availability: RealmAvailability::known(decl.realm),
            span: decl.path.first().map(|part| part.span),
        }
    }

    pub fn path_string(&self) -> String {
        self.path.join(".")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverOptions {
    pub externs: Vec<ExternSymbol>,
    pub unknown_external: UnknownExternalPolicy,
    pub gmod_api: Option<ApiIndex>,
    pub compile_time_package: bool,
}

impl Default for ResolverOptions {
    fn default() -> Self {
        Self {
            externs: Vec::new(),
            unknown_external: UnknownExternalPolicy::Allow,
            gmod_api: None,
            compile_time_package: false,
        }
    }
}

impl ResolverOptions {
    pub fn gmod_default() -> Self {
        Self {
            externs: Vec::new(),
            unknown_external: UnknownExternalPolicy::Warn,
            gmod_api: Some(ApiIndex::bundled()),
            compile_time_package: false,
        }
    }

    pub fn with_externs(mut self, externs: impl IntoIterator<Item = ExternSymbol>) -> Self {
        self.externs.extend(externs);
        self
    }

    pub fn with_unknown_external(mut self, policy: UnknownExternalPolicy) -> Self {
        self.unknown_external = policy;
        self
    }

    pub fn with_gmod_api(mut self, api: ApiIndex) -> Self {
        self.gmod_api = Some(api);
        self
    }

    pub fn for_compile_time_package(mut self) -> Self {
        self.compile_time_package = true;
        self
    }
}

impl ResolveOutput {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    pub fn binding_by_name(&self, name: &str) -> Option<&Binding> {
        self.bindings.iter().find(|binding| binding.name == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Module,
    Part,
    Function,
    Block,
    Loop,
}

#[derive(Debug)]
struct Scope {
    kind: ScopeKind,
    bindings: HashMap<String, BindingId>,
}

pub struct Resolver {
    options: ResolverOptions,
    scopes: Vec<Scope>,
    bindings: Vec<Binding>,
    exports: Vec<Export>,
    module_edges: Vec<ModuleEdge>,
    symbols_by_span: HashMap<SourceSpan, ResolvedSymbol>,
    external_symbols_by_span: HashMap<SourceSpan, ResolvedExternalSymbol>,
    diagnostics: Vec<Diagnostic>,
    externs: Vec<ExternSymbol>,
    unknown_external_diagnostics: HashSet<ExternalDiagnosticKey>,
    active_realms: Vec<RealmSet>,
    current_decl_realms: RealmSet,
    current_decl_binding: Option<BindingId>,
    current_top_level_order: Option<usize>,
    import_binding_uses: HashMap<BindingId, RealmSet>,
    enum_names: HashSet<String>,
    enum_decls: HashMap<String, EnumDecl>,
}

impl Resolver {
    pub fn resolve(module: &Module) -> ResolveOutput {
        Self::resolve_with_options(module, ResolverOptions::default())
    }

    pub fn resolve_with_options(module: &Module, options: ResolverOptions) -> ResolveOutput {
        let mut resolver = Self::new(options);

        resolver.collect_externs(&module.body);
        resolver.hoist_function_bindings(&module.body);
        resolver.resolve_stmts(&module.body);
        resolver.apply_import_use_realms();

        resolver.finish()
    }

    pub fn resolve_parts(parts: &[ResolvePart<'_>]) -> ResolveOutput {
        Self::resolve_parts_with_options(parts, ResolverOptions::default())
    }

    pub fn resolve_parts_with_options(
        parts: &[ResolvePart<'_>],
        options: ResolverOptions,
    ) -> ResolveOutput {
        let mut resolver = Self::new(options);

        for part in parts {
            resolver.collect_externs(&part.module.body);
        }
        resolver.collect_module_scope_bindings(parts);
        let mut order = 0usize;
        for part in parts {
            resolver.active_realms = vec![RealmSet::from_realm(part.default_realm)];
            resolver.current_decl_realms = RealmSet::from_realm(part.default_realm);
            resolver.with_scope(ScopeKind::Part, |this| {
                for stmt in &part.module.body {
                    let stmt_order = order;
                    order += 1;
                    this.resolve_top_level_stmt(stmt, stmt_order, part.default_realm);
                }
            });
        }
        resolver.apply_import_use_realms();

        resolver.finish()
    }

    fn new(options: ResolverOptions) -> Self {
        let externs = options.externs.clone();
        Self {
            options,
            scopes: vec![Scope {
                kind: ScopeKind::Module,
                bindings: HashMap::new(),
            }],
            bindings: Vec::new(),
            exports: Vec::new(),
            module_edges: Vec::new(),
            symbols_by_span: HashMap::new(),
            external_symbols_by_span: HashMap::new(),
            diagnostics: Vec::new(),
            externs,
            unknown_external_diagnostics: HashSet::new(),
            active_realms: vec![RealmSet::SHARED],
            current_decl_realms: RealmSet::SHARED,
            current_decl_binding: None,
            current_top_level_order: None,
            import_binding_uses: HashMap::new(),
            enum_names: HashSet::new(),
            enum_decls: HashMap::new(),
        }
    }

    fn finish(self) -> ResolveOutput {
        ResolveOutput {
            bindings: self.bindings,
            exports: self.exports,
            module_edges: self.module_edges,
            symbols_by_span: self.symbols_by_span,
            external_symbols_by_span: self.external_symbols_by_span,
            diagnostics: self.diagnostics,
        }
    }

    fn resolve_stmts(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.resolve_stmt(stmt);
        }
    }

    fn collect_externs(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.collect_enum_name(stmt);
            self.collect_extern_stmt(stmt);
        }
    }

    fn collect_enum_name(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::EnumDecl(decl) => {
                self.enum_names.insert(decl.name.name.clone());
                self.enum_decls.insert(decl.name.name.clone(), decl.clone());
            }
            StmtKind::RealmDecl { stmt: inner, .. } | StmtKind::ExportDecl { stmt: inner, .. } => {
                self.collect_enum_name(inner);
            }
            StmtKind::RealmBlock { block, .. } | StmtKind::InitDecl { block, .. } => {
                for stmt in &block.statements {
                    self.collect_enum_name(stmt);
                }
            }
            _ => {}
        }
    }

    fn collect_extern_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::ExternDecl(decl) => {
                self.externs.push(ExternSymbol::from_decl(decl));
            }
            StmtKind::RealmDecl { stmt: inner, .. } | StmtKind::ExportDecl { stmt: inner, .. } => {
                self.collect_extern_stmt(inner);
            }
            _ => {}
        }
    }

    fn collect_module_scope_bindings(&mut self, parts: &[ResolvePart<'_>]) {
        let mut order = 0usize;
        for part in parts {
            let default_realms = RealmSet::from_realm(part.default_realm);
            for stmt in &part.module.body {
                self.collect_top_level_binding(stmt, default_realms, order);
                order += 1;
            }
        }
    }

    fn collect_top_level_binding(&mut self, stmt: &Stmt, default_realms: RealmSet, order: usize) {
        match &stmt.kind {
            StmtKind::FunctionDecl(decl) => {
                if let FunctionName::Simple(name) = &decl.name {
                    self.declare_module_binding(
                        name,
                        BindingKind::Function,
                        default_realms,
                        None,
                        true,
                    );
                }
            }
            StmtKind::EnumDecl(decl) => {
                if decl.runtime {
                    self.declare_module_binding(
                        &decl.name,
                        BindingKind::Const,
                        default_realms,
                        Some(order),
                        false,
                    );
                }
            }
            StmtKind::LocalDecl { mode, names, .. } => {
                let kind = binding_kind_for_mode(*mode);
                for name in names {
                    self.declare_module_binding(name, kind, default_realms, Some(order), false);
                }
            }
            StmtKind::LocalDestructure { mode, patterns, .. } => {
                let kind = binding_kind_for_mode(*mode);
                for name in pattern_identifiers(patterns) {
                    self.declare_module_binding(name, kind, default_realms, Some(order), false);
                }
            }
            StmtKind::ExportDecl {
                realm, stmt: inner, ..
            } => {
                let realms = realm.map(RealmSet::from_realm).unwrap_or(default_realms);
                self.collect_top_level_binding(inner, realms, order);
            }
            StmtKind::RealmDecl { realm, stmt: inner } => {
                self.collect_top_level_binding(inner, RealmSet::from_realm(*realm), order);
            }
            StmtKind::Import(_)
            | StmtKind::PartOrderDecl(_)
            | StmtKind::ExternDecl(_)
            | StmtKind::HostPackageDecl(_)
            | StmtKind::ExportList { .. }
            | StmtKind::ExportAll { .. }
            | StmtKind::RealmBlock { .. }
            | StmtKind::InitDecl { .. }
            | StmtKind::Assign { .. }
            | StmtKind::CompoundAssign { .. }
            | StmtKind::Expr(_)
            | StmtKind::Return(_)
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::If { .. }
            | StmtKind::While { .. }
            | StmtKind::NumericFor { .. }
            | StmtKind::GenericFor { .. }
            | StmtKind::RepeatUntil { .. }
            | StmtKind::Do(_) => {}
        }
    }

    fn resolve_top_level_stmt(&mut self, stmt: &Stmt, order: usize, default_realm: Realm) {
        let previous_order = self.current_top_level_order.replace(order);
        let previous_decl = self.current_decl_realms;
        self.current_decl_realms = RealmSet::from_realm(default_realm);
        self.resolve_top_level_stmt_inner(stmt, order, default_realm);
        self.current_decl_realms = previous_decl;
        self.current_top_level_order = previous_order;
    }

    fn resolve_top_level_stmt_inner(&mut self, stmt: &Stmt, order: usize, _default_realm: Realm) {
        match &stmt.kind {
            StmtKind::Import(import) => self.resolve_import(stmt.span, import),
            StmtKind::FunctionDecl(decl) => self.resolve_function_decl_body_only(decl),
            StmtKind::LocalDecl { values, .. } => {
                for expr in values {
                    self.resolve_expr(expr);
                }
            }
            StmtKind::LocalDestructure {
                patterns, values, ..
            } => {
                for expr in values {
                    self.resolve_expr(expr);
                }
                for pattern in patterns {
                    self.resolve_pattern_defaults(pattern);
                }
            }
            StmtKind::ExportDecl {
                kind,
                realm,
                stmt: inner,
            } => self.resolve_export_decl(stmt.span, *kind, *realm, inner),
            StmtKind::ExportList { realm, entries } => self.resolve_export_list(*realm, entries),
            StmtKind::RealmDecl { realm, stmt: inner } => {
                let previous_active = *self.active_realms.last().unwrap_or(&RealmSet::SHARED);
                let previous_decl = self.current_decl_realms;
                let realms = RealmSet::from_realm(*realm);
                self.active_realms
                    .push(previous_active.intersection(realms));
                self.current_decl_realms = realms;
                self.resolve_top_level_stmt_inner(inner, order, *realm);
                self.current_decl_realms = previous_decl;
                self.active_realms.pop();
            }
            StmtKind::RealmBlock { realm, block } => {
                self.with_realm(*realm, |this| {
                    this.with_scope(ScopeKind::Block, |this| this.resolve_block(block));
                });
            }
            StmtKind::InitDecl { realm, block } => {
                if let Some(realm) = realm {
                    self.with_realm(*realm, |this| {
                        this.with_scope(ScopeKind::Block, |this| this.resolve_block(block));
                    });
                } else {
                    self.with_scope(ScopeKind::Block, |this| this.resolve_block(block));
                }
            }
            StmtKind::PartOrderDecl(_)
            | StmtKind::ExternDecl(_)
            | StmtKind::HostPackageDecl(_)
            | StmtKind::ExportAll { .. } => self.resolve_stmt(stmt),
            _ => self.resolve_stmt(stmt),
        }
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::LocalDecl {
                mode,
                names,
                values,
            } => {
                for expr in values {
                    self.resolve_expr(expr);
                }
                let kind = binding_kind_for_mode(*mode);
                for name in names {
                    self.declare(name, kind, None, None);
                }
            }
            StmtKind::LocalDestructure {
                mode,
                patterns,
                values,
            } => {
                for expr in values {
                    self.resolve_expr(expr);
                }
                let kind = binding_kind_for_mode(*mode);
                for pattern in patterns {
                    self.declare_pattern(pattern, kind);
                }
                for pattern in patterns {
                    self.resolve_pattern_defaults(pattern);
                }
            }
            StmtKind::Assign { targets, values } => {
                for target in targets {
                    self.resolve_expr(target);
                    self.check_assignment_target(target);
                }
                for value in values {
                    self.resolve_expr(value);
                }
            }
            StmtKind::CompoundAssign { target, value, .. } => {
                self.resolve_expr(target);
                self.check_assignment_target(target);
                self.resolve_expr(value);
            }
            StmtKind::Expr(expr) => self.resolve_expr(expr),
            StmtKind::Return(values) => {
                for value in values {
                    self.resolve_expr(value);
                }
            }
            StmtKind::Break => {
                if !self.inside_loop_in_current_function() {
                    self.error(
                        "RESOLVE001",
                        "`break` is only valid inside loops",
                        stmt.span,
                    );
                }
            }
            StmtKind::Continue => {
                if !self.inside_loop_in_current_function() {
                    self.error(
                        "RESOLVE014",
                        "`continue` is only valid inside loops",
                        stmt.span,
                    );
                }
            }
            StmtKind::Import(import) => self.resolve_import(stmt.span, import),
            StmtKind::PartOrderDecl(_) => {}
            StmtKind::ExternDecl(_) => {}
            StmtKind::HostPackageDecl(_) if self.options.compile_time_package => {}
            StmtKind::HostPackageDecl(_) => {
                self.error(
                    "RESOLVE007",
                    "host package declarations are only valid in compile-time packages",
                    stmt.span,
                );
            }
            StmtKind::ExportDecl {
                kind,
                realm,
                stmt: decl,
            } => self.resolve_export_decl(stmt.span, *kind, *realm, decl),
            StmtKind::ExportList { realm, entries } => self.resolve_export_list(*realm, entries),
            StmtKind::ExportAll { .. } => self.error(
                "RESOLVE011",
                "`export all` is only valid during module-level package compilation",
                stmt.span,
            ),
            StmtKind::RealmDecl { realm, stmt: inner } => {
                let previous_decl = self.current_decl_realms;
                self.current_decl_realms = RealmSet::from_realm(*realm);
                self.with_realm(*realm, |this| this.resolve_stmt(inner));
                self.current_decl_realms = previous_decl;
            }
            StmtKind::RealmBlock { realm, block } => {
                self.with_realm(*realm, |this| {
                    this.with_scope(ScopeKind::Block, |this| this.resolve_block(block));
                });
            }
            StmtKind::InitDecl { realm, block } => {
                if let Some(realm) = realm {
                    self.with_realm(*realm, |this| {
                        this.with_scope(ScopeKind::Block, |this| this.resolve_block(block));
                    });
                } else {
                    self.with_scope(ScopeKind::Block, |this| this.resolve_block(block));
                }
            }
            StmtKind::FunctionDecl(decl) => self.resolve_function_decl(decl),
            StmtKind::EnumDecl(decl) => self.resolve_enum_decl(decl),
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                self.resolve_expr(condition);
                self.with_scope(ScopeKind::Block, |this| this.resolve_block(then_block));
                if let Some(block) = else_block {
                    self.with_scope(ScopeKind::Block, |this| this.resolve_block(block));
                }
            }
            StmtKind::While { condition, body } => {
                self.resolve_expr(condition);
                self.with_scope(ScopeKind::Loop, |this| this.resolve_block(body));
            }
            StmtKind::NumericFor {
                name,
                start,
                end,
                step,
                body,
            } => {
                self.resolve_expr(start);
                self.resolve_expr(end);
                if let Some(step) = step {
                    self.resolve_expr(step);
                }
                self.with_scope(ScopeKind::Loop, |this| {
                    this.declare(name, BindingKind::Local, None, None);
                    this.resolve_block(body);
                });
            }
            StmtKind::GenericFor { names, iter, body } => {
                for expr in iter {
                    self.resolve_expr(expr);
                }
                self.with_scope(ScopeKind::Loop, |this| {
                    for name in names {
                        this.declare(name, BindingKind::Local, None, None);
                    }
                    this.resolve_block(body);
                });
            }
            StmtKind::RepeatUntil { body, condition } => {
                self.with_scope(ScopeKind::Loop, |this| {
                    this.resolve_block(body);
                    this.resolve_expr(condition);
                });
            }
            StmtKind::Do(block) => {
                self.with_scope(ScopeKind::Block, |this| this.resolve_block(block));
            }
        }
    }

    fn resolve_block(&mut self, block: &Block) {
        self.hoist_function_bindings(&block.statements);
        self.resolve_stmts(&block.statements);
        if let Some(tail) = &block.tail {
            self.resolve_expr(tail);
        }
    }

    fn resolve_import(&mut self, span: SourceSpan, import: &ImportStmt) {
        let mut edge_specifiers = Vec::new();
        for specifier in &import.specifiers {
            let (local, imported) = import_specifier_binding(specifier);
            let binding = self.declare(
                local,
                if import.phase == ImportPhase::Macro {
                    BindingKind::MacroImport
                } else {
                    BindingKind::Import
                },
                Some(import.source.clone()),
                Some(imported.clone()),
            );
            if import.phase == ImportPhase::Runtime {
                edge_specifiers.push(module_edge_specifier(specifier, binding));
            }
        }

        if import.phase == ImportPhase::Runtime {
            self.module_edges.push(ModuleEdge {
                source: import.source.clone(),
                specifiers: edge_specifiers,
                side_effect_only: import.side_effect_only,
                span,
            });
        }
    }

    fn resolve_export_decl(
        &mut self,
        span: SourceSpan,
        kind: ExportKind,
        realm: Option<Realm>,
        stmt: &Stmt,
    ) {
        if kind != ExportKind::Runtime && !self.options.compile_time_package {
            self.error(
                "RESOLVE006",
                "phase-qualified exports are only valid in compile-time packages",
                span,
            );
            self.resolve_stmt(stmt);
            return;
        }

        match &stmt.kind {
            StmtKind::FunctionDecl(decl) => {
                let FunctionName::Simple(name) = &decl.name else {
                    self.error(
                        "RESOLVE003",
                        "`export fn` only supports simple lexical function names in MVP 0.1",
                        span,
                    );
                    return;
                };

                if self.lookup(&name.name).is_none() {
                    self.declare(name, BindingKind::Function, None, None);
                }

                if self.current_top_level_order.is_some() {
                    self.resolve_function_decl_body_only(decl);
                } else {
                    self.resolve_function_decl(decl);
                }
                self.export_name(name, name, realm);
            }
            StmtKind::LocalDecl {
                mode: BindingMode::Const,
                names,
                values,
                ..
            } => {
                if self.current_top_level_order.is_some() {
                    for expr in values {
                        self.resolve_expr(expr);
                    }
                } else {
                    self.resolve_stmt(stmt);
                }
                for name in names {
                    self.export_name(name, name, realm);
                }
            }
            StmtKind::LocalDestructure {
                mode: BindingMode::Const,
                ..
            } => {
                self.error(
                    "RESOLVE010",
                    "`export const` only supports identifier bindings",
                    span,
                );
                self.resolve_stmt(stmt);
            }
            StmtKind::EnumDecl(decl) if decl.runtime => {
                if self.lookup(&decl.name.name).is_none() {
                    self.declare(&decl.name, BindingKind::Const, None, None);
                }
                self.resolve_enum_decl(decl);
                self.export_name(&decl.name, &decl.name, realm);
            }
            StmtKind::EnumDecl(decl) => {
                self.error(
                    "RESOLVE015",
                    "`export runtime enum` requires runtime enum emission",
                    decl.name.span,
                );
                self.resolve_enum_decl(decl);
            }
            _ => {
                self.error(
                    "RESOLVE002",
                    "only function and const declarations can be exported here",
                    span,
                );
                self.resolve_stmt(stmt);
            }
        }
    }

    fn resolve_export_list(&mut self, realm: Option<Realm>, entries: &[ExportSpecifier]) {
        for entry in entries {
            self.export_name(&entry.exported, &entry.local, realm);
        }
    }

    fn resolve_function_decl(&mut self, decl: &FunctionDecl) {
        match &decl.name {
            FunctionName::Simple(name) => {
                if self.lookup_current_scope(&name.name).is_none() {
                    self.declare(name, BindingKind::Function, None, None);
                }
            }
            FunctionName::Dotted(path) => {
                for ident in path {
                    self.resolve_identifier(ident);
                }
            }
            FunctionName::Method { receiver, .. } => {
                for ident in receiver {
                    self.resolve_identifier(ident);
                }
            }
        }

        self.with_scope(ScopeKind::Function, |this| {
            this.resolve_params(&decl.params);
            this.resolve_function_body(&decl.body);
        });
    }

    fn resolve_function_decl_body_only(&mut self, decl: &FunctionDecl) {
        match &decl.name {
            FunctionName::Simple(_) => {}
            FunctionName::Dotted(path) => {
                for ident in path {
                    self.resolve_identifier(ident);
                }
            }
            FunctionName::Method { receiver, .. } => {
                for ident in receiver {
                    self.resolve_identifier(ident);
                }
            }
        }

        let function_binding = match &decl.name {
            FunctionName::Simple(name) => self.lookup_module(&name.name),
            _ => None,
        };
        let function_realms = function_binding
            .map(|binding_id| self.bindings[binding_id.0].available_realms)
            .unwrap_or(self.current_decl_realms);
        let previous_active = *self.active_realms.last().unwrap_or(&RealmSet::SHARED);
        let previous_decl_binding = self.current_decl_binding;
        self.current_decl_binding = function_binding;
        self.active_realms
            .push(previous_active.intersection(function_realms));
        self.with_scope(ScopeKind::Function, |this| {
            this.resolve_params(&decl.params);
            this.resolve_function_body(&decl.body);
        });
        self.active_realms.pop();
        self.current_decl_binding = previous_decl_binding;
    }

    fn resolve_params(&mut self, params: &[Param]) {
        for param in params {
            self.declare(&param.name, BindingKind::Param, None, None);
            if let Some(default) = &param.default {
                self.resolve_expr(default);
            }
        }
    }

    fn declare_pattern(&mut self, pattern: &Pattern, kind: BindingKind) {
        match &pattern.kind {
            PatternKind::Identifier(name) => {
                self.declare(name, kind, None, None);
            }
            PatternKind::Object(fields) => {
                for field in fields {
                    self.declare_pattern(&field.pattern, kind);
                }
            }
            PatternKind::Array(items) => {
                for item in items {
                    self.declare_pattern(&item.pattern, kind);
                }
            }
        }
    }

    fn resolve_pattern_defaults(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Identifier(_) => {}
            PatternKind::Object(fields) => {
                for field in fields {
                    self.resolve_pattern_defaults(&field.pattern);
                    if let Some(default) = &field.default {
                        self.resolve_expr(default);
                    }
                }
            }
            PatternKind::Array(items) => {
                for item in items {
                    self.resolve_pattern_defaults(&item.pattern);
                    if let Some(default) = &item.default {
                        self.resolve_expr(default);
                    }
                }
            }
        }
    }

    fn resolve_function_body(&mut self, body: &FunctionBody) {
        match body {
            FunctionBody::Expr(expr) => self.resolve_expr(expr),
            FunctionBody::Block(block) => self.resolve_block(block),
        }
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Identifier(ident) => self.resolve_identifier(ident),
            ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Vararg
            | ExprKind::PipelinePlaceholder => {}
            ExprKind::TemplateString(parts) => {
                for part in parts {
                    if let TemplatePartKind::Expr(expr) = &part.kind {
                        self.resolve_expr(expr);
                    }
                }
            }
            ExprKind::Table(table) => {
                for field in &table.fields {
                    match &field.kind {
                        TableFieldKind::Array(value) => self.resolve_expr(value),
                        TableFieldKind::Named { value, .. } => self.resolve_expr(value),
                        TableFieldKind::ExprKey { key, value } => {
                            self.resolve_expr(key);
                            self.resolve_expr(value);
                        }
                        TableFieldKind::Spread(value) => self.resolve_expr(value),
                    }
                }
            }
            ExprKind::Paren(expr) => self.resolve_expr(expr),
            ExprKind::Unary { argument, .. } => self.resolve_expr(argument),
            ExprKind::Binary { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            ExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.resolve_expr(condition);
                self.resolve_expr_or_block(then_branch);
                self.resolve_expr_or_block(else_branch);
            }
            ExprKind::Match(match_expr) => {
                self.resolve_expr(&match_expr.subject);
                for arm in &match_expr.arms {
                    self.validate_match_pattern_variants(&arm.pattern);
                }
                self.analyze_match_coverage(match_expr);
                for arm in &match_expr.arms {
                    self.with_scope(ScopeKind::Block, |this| {
                        this.declare_match_pattern_bindings(&arm.pattern);
                        this.resolve_expr_or_block(&arm.body);
                    });
                }
            }
            ExprKind::Do(block) => {
                self.with_scope(ScopeKind::Block, |this| this.resolve_block(block));
            }
            ExprKind::Function(function) => {
                self.with_scope(ScopeKind::Function, |this| {
                    if function.arrow_kind == ArrowKind::ImplicitSelf {
                        let self_ident = Identifier {
                            name: "self".into(),
                            span: expr.span,
                        };
                        this.declare(&self_ident, BindingKind::Param, None, None);
                    }
                    this.resolve_params(&function.params);
                    this.resolve_function_body(&function.body);
                });
            }
            ExprKind::Chain(chain) => {
                if !self.resolve_external_chain(chain) {
                    self.resolve_expr(&chain.base);
                }
                self.resolve_chain_segments(chain);
            }
        }
    }

    fn resolve_chain_segments(&mut self, chain: &ChainExpr) {
        for segment in &chain.segments {
            match &segment.kind {
                ChainSegmentKind::Member { .. } => {}
                ChainSegmentKind::Index { index, .. } => self.resolve_expr(index),
                ChainSegmentKind::Call { args, .. }
                | ChainSegmentKind::SafeDotCall { args, .. }
                | ChainSegmentKind::MethodCall { args, .. } => {
                    for arg in args {
                        self.resolve_expr(arg);
                    }
                }
            }
        }
    }

    fn resolve_external_chain(&mut self, chain: &ChainExpr) -> bool {
        let Some((path, span)) = self.external_chain_path(chain) else {
            return false;
        };
        let base_name = &path[0];
        if self.enum_names.contains(base_name) {
            return true;
        }
        if self.lookup(base_name).is_some() {
            return false;
        }
        self.check_external_path(&path, span);
        true
    }

    fn external_chain_path(&self, chain: &ChainExpr) -> Option<(Vec<String>, SourceSpan)> {
        let ExprKind::Identifier(base) = &chain.base.kind else {
            return None;
        };
        let mut path = vec![base.name.clone()];
        let mut span = base.span;
        for segment in &chain.segments {
            match &segment.kind {
                ChainSegmentKind::Member { name, .. }
                | ChainSegmentKind::SafeDotCall { name, .. }
                | ChainSegmentKind::MethodCall { name, .. } => {
                    path.push(name.name.clone());
                    span = name.span;
                    if !matches!(&segment.kind, ChainSegmentKind::Member { .. }) {
                        break;
                    }
                }
                ChainSegmentKind::Call { .. } => break,
                ChainSegmentKind::Index { .. } => break,
            }
        }
        Some((path, span))
    }

    fn resolve_expr_or_block(&mut self, item: &ExprOrBlock) {
        match item {
            ExprOrBlock::Expr(expr) => self.resolve_expr(expr),
            ExprOrBlock::Block(block) => self.with_scope(ScopeKind::Block, |this| {
                this.resolve_block(block);
            }),
        }
    }

    fn resolve_enum_decl(&mut self, decl: &EnumDecl) {
        for variant in &decl.variants {
            if let Some(tag) = &variant.tag {
                self.resolve_expr(tag);
            }
        }
        if decl.runtime && self.lookup_current_scope(&decl.name.name).is_none() {
            self.declare(&decl.name, BindingKind::Const, None, None);
        }
    }

    fn declare_match_pattern_bindings(&mut self, pattern: &MatchPattern) {
        let mut names = Vec::new();
        collect_match_pattern_binding_names(pattern, &mut names);
        names.sort_by(|left, right| left.name.cmp(&right.name));
        names.dedup_by(|left, right| left.name == right.name);
        for name in names {
            self.declare(&name, BindingKind::Local, None, None);
        }
    }

    fn analyze_match_coverage(&mut self, match_expr: &MatchExpr) {
        let mut seen_unconditional: Option<SourceSpan> = None;
        let mut seen_literals: HashMap<String, SourceSpan> = HashMap::new();
        let mut enum_name: Option<String> = None;
        let mut covered_variants = BTreeSet::<String>::new();
        let mut covered_variant_spans = HashMap::<String, SourceSpan>::new();
        let mut enum_coverage_complete_span: Option<SourceSpan> = None;
        let mut mixed_or_opaque = false;

        for arm in &match_expr.arms {
            self.validate_or_pattern_bindings(&arm.pattern);

            if let Some(previous) = seen_unconditional {
                self.warn_unreachable_match_arm(
                    arm.span,
                    previous,
                    "this arm is unreachable because an earlier pattern matches everything",
                );
                continue;
            }

            if !mixed_or_opaque
                && let Some(name) = enum_name.as_deref()
                && enum_is_fully_covered(self.enum_decls.get(name), &covered_variants)
            {
                self.warn_unreachable_match_arm(
                    arm.span,
                    enum_coverage_complete_span.unwrap_or(match_expr.subject.span),
                    "this arm is unreachable because all enum variants were already covered",
                );
                continue;
            }

            let units = self.match_pattern_coverage_units(&arm.pattern);
            let mut arm_has_reachable_unit = false;
            let mut arm_unconditional = None;
            let mut arm_reported_unreachable_unit = false;

            for unit in units {
                match unit {
                    MatchCoverageUnit::Unconditional { span } => {
                        arm_has_reachable_unit = true;
                        arm_unconditional = Some(span);
                    }
                    MatchCoverageUnit::Literal { key, span } => {
                        mixed_or_opaque = true;
                        if let Some(previous) = seen_literals.get(&key).copied() {
                            self.warn_unreachable_match_arm(
                                span,
                                previous,
                                "this literal pattern is unreachable because it was already matched",
                            );
                            arm_reported_unreachable_unit = true;
                        } else {
                            arm_has_reachable_unit = true;
                            seen_literals.insert(key, span);
                        }
                    }
                    MatchCoverageUnit::EnumVariant {
                        enum_name: unit_enum,
                        variant_name,
                        total,
                        span,
                    } => {
                        if let Some(current) = enum_name.as_deref() {
                            if current != unit_enum {
                                mixed_or_opaque = true;
                            }
                        } else {
                            enum_name = Some(unit_enum.clone());
                        }

                        if total {
                            if let Some(previous) =
                                covered_variant_spans.get(&variant_name).copied()
                            {
                                self.warn_unreachable_match_arm(
                                    span,
                                    previous,
                                    "this enum variant pattern is unreachable because it was already matched",
                                );
                                arm_reported_unreachable_unit = true;
                            } else {
                                arm_has_reachable_unit = true;
                                covered_variants.insert(variant_name.clone());
                                covered_variant_spans.insert(variant_name, span);
                                if enum_is_fully_covered(
                                    self.enum_decls.get(&unit_enum),
                                    &covered_variants,
                                ) {
                                    enum_coverage_complete_span = Some(span);
                                }
                            }
                        } else {
                            mixed_or_opaque = true;
                            arm_has_reachable_unit = true;
                        }
                    }
                    MatchCoverageUnit::Opaque => {
                        mixed_or_opaque = true;
                        arm_has_reachable_unit = true;
                    }
                }
            }

            if !arm_has_reachable_unit && !arm_reported_unreachable_unit {
                self.warn_unreachable_match_arm(
                    arm.span,
                    match_expr.subject.span,
                    "this arm is unreachable because all of its alternatives were already matched",
                );
            }

            if let Some(span) = arm_unconditional {
                seen_unconditional = Some(span);
            }
        }

        if seen_unconditional.is_some() || mixed_or_opaque {
            return;
        }

        let Some(enum_name) = enum_name else {
            return;
        };
        let Some(enum_decl) = self.enum_decls.get(&enum_name) else {
            return;
        };
        let missing = enum_decl
            .variants
            .iter()
            .filter(|variant| !covered_variants.contains(&variant.name.name))
            .map(|variant| variant.name.name.clone())
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return;
        }

        self.diagnostics.push(
            Diagnostic::error(format!(
                "non-exhaustive match for enum `{enum_name}`; missing {}",
                missing.join(", ")
            ))
            .with_code("MATCH001")
            .with_label(Label::primary(
                match_expr.subject.span,
                "matched value is classified by this enum",
            ))
            .with_help(format!(
                "add arms for {} or add a wildcard `_` arm",
                missing.join(", ")
            )),
        );
    }

    fn match_pattern_coverage_units(&self, pattern: &MatchPattern) -> Vec<MatchCoverageUnit> {
        match &pattern.kind {
            MatchPatternKind::Or(patterns) => patterns
                .iter()
                .flat_map(|pattern| self.match_pattern_coverage_units(pattern))
                .collect(),
            MatchPatternKind::Wildcard | MatchPatternKind::Binding(_) => {
                vec![MatchCoverageUnit::Unconditional { span: pattern.span }]
            }
            MatchPatternKind::Literal(literal) => vec![MatchCoverageUnit::Literal {
                key: match_literal_key(literal),
                span: pattern.span,
            }],
            MatchPatternKind::Variant { path, payload } => {
                let Some((enum_name, variant_name)) = self.lookup_match_variant_path(path) else {
                    return vec![MatchCoverageUnit::Opaque];
                };
                vec![MatchCoverageUnit::EnumVariant {
                    enum_name,
                    variant_name,
                    total: payload
                        .as_ref()
                        .map(match_payload_is_irrefutable)
                        .unwrap_or(true),
                    span: pattern.span,
                }]
            }
            MatchPatternKind::Object(_) | MatchPatternKind::Array(_) => {
                vec![MatchCoverageUnit::Opaque]
            }
        }
    }

    fn lookup_match_variant_path(&self, path: &[Identifier]) -> Option<(String, String)> {
        if path.len() == 2 {
            let enum_name = &path[0].name;
            let variant_name = &path[1].name;
            let enum_decl = self.enum_decls.get(enum_name)?;
            if enum_decl
                .variants
                .iter()
                .any(|variant| variant.name.name == *variant_name)
            {
                return Some((enum_name.clone(), variant_name.clone()));
            }
            return None;
        }

        if path.len() == 1 {
            let variant_name = &path[0].name;
            let mut found = None;
            for (enum_name, enum_decl) in &self.enum_decls {
                if enum_decl
                    .variants
                    .iter()
                    .any(|variant| variant.name.name == *variant_name)
                {
                    if found.is_some() {
                        return None;
                    }
                    found = Some((enum_name.clone(), variant_name.clone()));
                }
            }
            return found;
        }

        None
    }

    fn validate_match_pattern_variants(&mut self, pattern: &MatchPattern) {
        match &pattern.kind {
            MatchPatternKind::Or(patterns) => {
                for pattern in patterns {
                    self.validate_match_pattern_variants(pattern);
                }
            }
            MatchPatternKind::Variant { path, payload } => {
                if self.lookup_match_variant_path(path).is_none() {
                    let path_text = path
                        .iter()
                        .map(|part| part.name.as_str())
                        .collect::<Vec<_>>()
                        .join(".");
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "match pattern `{path_text}` does not resolve to a known enum variant"
                        ))
                        .with_code("RESOLVE015")
                        .with_label(Label::primary(pattern.span, "unresolved match pattern"))
                        .with_help(
                            "match value patterns must name enum variants or use literal patterns",
                        ),
                    );
                }
                if let Some(payload) = payload {
                    self.validate_match_pattern_payload_variants(payload);
                }
            }
            MatchPatternKind::Object(fields) => {
                for field in fields {
                    self.validate_match_pattern_variants(&field.pattern);
                }
            }
            MatchPatternKind::Array(items) => {
                for item in items {
                    self.validate_match_pattern_variants(&item.pattern);
                }
            }
            MatchPatternKind::Wildcard
            | MatchPatternKind::Binding(_)
            | MatchPatternKind::Literal(_) => {}
        }
    }

    fn validate_match_pattern_payload_variants(&mut self, payload: &MatchPatternPayload) {
        match payload {
            MatchPatternPayload::Tuple(patterns) => {
                for pattern in patterns {
                    self.validate_match_pattern_variants(pattern);
                }
            }
            MatchPatternPayload::Record(fields) => {
                for field in fields {
                    self.validate_match_pattern_variants(&field.pattern);
                }
            }
        }
    }

    fn warn_unreachable_match_arm(
        &mut self,
        span: SourceSpan,
        previous: SourceSpan,
        message: &'static str,
    ) {
        self.diagnostics.push(
            Diagnostic::new(Severity::Warning, message)
                .with_code("MATCH002")
                .with_label(Label::primary(span, "unreachable pattern"))
                .with_label(Label::secondary(previous, "covered here")),
        );
    }

    fn validate_or_pattern_bindings(&mut self, pattern: &MatchPattern) {
        match &pattern.kind {
            MatchPatternKind::Or(patterns) => {
                for pattern in patterns {
                    let mut names = Vec::new();
                    collect_match_pattern_binding_names(pattern, &mut names);
                    if !names.is_empty() {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "or-pattern alternatives with bindings are not supported yet",
                            )
                            .with_code("MATCH003")
                            .with_label(Label::primary(
                                pattern.span,
                                "this alternative binds values",
                            ))
                            .with_help(
                                "split this into separate match arms until binding-preserving or-pattern lowering is implemented",
                            ),
                        );
                    }
                    self.validate_or_pattern_bindings(pattern);
                }
            }
            MatchPatternKind::Variant { payload, .. } => {
                if let Some(payload) = payload {
                    match payload {
                        MatchPatternPayload::Tuple(patterns) => {
                            for pattern in patterns {
                                self.validate_or_pattern_bindings(pattern);
                            }
                        }
                        MatchPatternPayload::Record(fields) => {
                            for field in fields {
                                self.validate_or_pattern_bindings(&field.pattern);
                            }
                        }
                    }
                }
            }
            MatchPatternKind::Object(fields) => {
                for field in fields {
                    self.validate_or_pattern_bindings(&field.pattern);
                }
            }
            MatchPatternKind::Array(items) => {
                for item in items {
                    self.validate_or_pattern_bindings(&item.pattern);
                }
            }
            MatchPatternKind::Wildcard
            | MatchPatternKind::Binding(_)
            | MatchPatternKind::Literal(_) => {}
        }
    }

    fn resolve_identifier(&mut self, ident: &Identifier) {
        if let Some(binding_id) = self.lookup(&ident.name) {
            let binding = self.bindings[binding_id.0].clone();
            let active = *self.active_realms.last().unwrap_or(&RealmSet::SHARED);
            if matches!(binding.kind, BindingKind::Import) {
                let entry = self
                    .import_binding_uses
                    .entry(binding_id)
                    .or_insert(RealmSet::NONE);
                *entry = entry.union(active);
            }
            if !binding.available_realms.contains_all(active) {
                self.error(
                    "REALM001",
                    format!(
                        "`{}` is {} but used in {} context",
                        ident.name,
                        binding.available_realms.display_name(),
                        active.display_name()
                    ),
                    ident.span,
                );
            }
            if let Some(current_order) = self.current_top_level_order
                && binding.module_scope
                && !binding.hoisted
                && binding
                    .initialized_at
                    .is_some_and(|init_order| init_order >= current_order)
            {
                self.error(
                    "RESOLVE012",
                    format!(
                        "module binding `{}` is used before initialization",
                        ident.name
                    ),
                    ident.span,
                );
            }
            self.symbols_by_span.insert(
                ident.span,
                ResolvedSymbol {
                    binding: binding_id,
                    local_name: ident.name.clone(),
                    binding_kind: binding.kind.clone(),
                    source_module: binding.source_module.clone(),
                    imported_name: binding.imported_name.clone(),
                },
            );
        } else if ident.name != "_" {
            self.check_external_path(std::slice::from_ref(&ident.name), ident.span);
        }
    }

    fn check_external_path(&mut self, path: &[String], span: SourceSpan) {
        if path.is_empty() {
            return;
        }
        let active = *self.active_realms.last().unwrap_or(&RealmSet::SHARED);
        let availability = self.external_availability(path);
        self.external_symbols_by_span.insert(
            span,
            ResolvedExternalSymbol {
                path: path.to_vec(),
                availability: availability.clone(),
            },
        );
        match availability {
            RealmAvailability::Known(realms) => {
                if !realms.contains_all(active) {
                    let path_name = path.join(".");
                    self.error(
                        "REALM001",
                        format!(
                            "`{}` is {} but used in {} context",
                            path_name,
                            realms.display_name(),
                            active.display_name()
                        ),
                        span,
                    );
                }
            }
            RealmAvailability::UnknownExternal => {
                self.report_unknown_external(path, active, span);
            }
        }
    }

    fn external_availability(&self, path: &[String]) -> RealmAvailability {
        let mut best: Option<&ExternSymbol> = None;
        for symbol in &self.externs {
            if symbol.path.len() > path.len() {
                continue;
            }
            if symbol.path.iter().zip(path.iter()).all(|(a, b)| a == b)
                && best
                    .map(|current| symbol.path.len() >= current.path.len())
                    .unwrap_or(true)
            {
                best = Some(symbol);
            }
        }
        best.map(|symbol| symbol.availability.clone())
            .or_else(|| {
                self.options
                    .gmod_api
                    .as_ref()
                    .and_then(|api| api.longest_match(path))
                    .map(|entry| RealmAvailability::Known(api_realm_set(entry.realm)))
            })
            .unwrap_or(RealmAvailability::UnknownExternal)
    }

    fn report_unknown_external(&mut self, path: &[String], active: RealmSet, span: SourceSpan) {
        if self.options.unknown_external == UnknownExternalPolicy::Allow {
            return;
        }
        let key = ExternalDiagnosticKey {
            path: path.to_vec(),
            active,
            containing_decl: self.current_decl_binding,
        };
        if !self.unknown_external_diagnostics.insert(key) {
            return;
        }

        let path_name = path.join(".");
        let severity = match self.options.unknown_external {
            UnknownExternalPolicy::Allow => return,
            UnknownExternalPolicy::Warn => Severity::Warning,
            UnknownExternalPolicy::Error => Severity::Error,
        };
        self.diagnostics.push(
            Diagnostic::new(
                severity,
                format!("cannot verify realm availability of external symbol `{path_name}`"),
            )
            .with_code("REALM_UNKNOWN")
            .with_label(Label::primary(
                span,
                format!("used in {} context", active.display_name()),
            ))
            .with_help(format!(
                "Add an extern declaration to make this strict:\n  extern shared {path_name}\n  extern client {path_name}\n  extern server {path_name}"
            )),
        );
    }

    fn check_assignment_target(&mut self, target: &Expr) {
        let ExprKind::Identifier(ident) = &unparen_expr(target).kind else {
            return;
        };
        let Some(binding_id) = self.lookup(&ident.name) else {
            return;
        };
        let binding = &self.bindings[binding_id.0];
        if binding.kind.is_immutable_binding() {
            self.error(
                "RESOLVE009",
                format!("cannot assign to immutable binding `{}`", ident.name),
                ident.span,
            );
        }
    }

    fn export_name(&mut self, exported: &Identifier, local: &Identifier, realm: Option<Realm>) {
        if let Some(binding) = self.lookup_module(&local.name) {
            let binding_data = &self.bindings[binding.0];
            if matches!(
                binding_data.kind,
                BindingKind::Import | BindingKind::MacroImport
            ) {
                self.error(
                    "RESOLVE013",
                    format!("cannot export imported binding `{}`", local.name),
                    local.span,
                );
                return;
            }
            let export_realms = realm
                .map(RealmSet::from_realm)
                .unwrap_or(binding_data.available_realms);
            if !binding_data.available_realms.contains_all(export_realms) {
                self.error(
                    "REALM002",
                    format!(
                        "export `{}` widens `{}` from {} to {}",
                        exported.name,
                        local.name,
                        binding_data.available_realms.display_name(),
                        export_realms.display_name()
                    ),
                    exported.span,
                );
                return;
            }
            self.exports.push(Export {
                name: exported.name.clone(),
                local_name: local.name.clone(),
                binding,
                span: exported.span,
                realm,
            });
        } else {
            self.error(
                "RESOLVE004",
                format!("cannot export unknown module binding `{}`", local.name),
                local.span,
            );
        }
    }

    fn hoist_function_bindings(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::FunctionDecl(decl) => {
                    if let FunctionName::Simple(name) = &decl.name {
                        if self.lookup_current_scope(&name.name).is_none() {
                            self.declare(name, BindingKind::Function, None, None);
                        }
                    }
                }
                StmtKind::ExportDecl { stmt: inner, .. }
                | StmtKind::RealmDecl { stmt: inner, .. } => {
                    if let StmtKind::FunctionDecl(decl) = &inner.kind {
                        if let FunctionName::Simple(name) = &decl.name {
                            if self.lookup_current_scope(&name.name).is_none() {
                                self.declare(name, BindingKind::Function, None, None);
                            }
                        }
                    }
                }
                StmtKind::RealmBlock { block, .. } | StmtKind::InitDecl { block, .. } => {
                    self.hoist_function_bindings(&block.statements);
                }
                _ => {}
            }
        }
    }

    fn declare(
        &mut self,
        ident: &Identifier,
        kind: BindingKind,
        source_module: Option<String>,
        imported_name: Option<String>,
    ) -> BindingId {
        if ident.name != "_"
            && let Some(existing) = self.lookup_current_scope(&ident.name)
        {
            self.error(
                "RESOLVE005",
                format!("duplicate binding `{}` in the same scope", ident.name),
                ident.span,
            );
            return existing;
        }

        let id = BindingId(self.bindings.len());
        self.bindings.push(Binding {
            id,
            name: ident.name.clone(),
            kind,
            span: ident.span,
            source_module,
            imported_name,
            available_realms: self.current_decl_realms,
            module_scope: self
                .scopes
                .last()
                .is_some_and(|scope| scope.kind == ScopeKind::Module),
            initialized_at: None,
            hoisted: kind == BindingKind::Function,
        });
        self.scopes
            .last_mut()
            .expect("resolver always has a scope")
            .bindings
            .insert(ident.name.clone(), id);
        id
    }

    fn declare_module_binding(
        &mut self,
        ident: &Identifier,
        kind: BindingKind,
        realms: RealmSet,
        initialized_at: Option<usize>,
        hoisted: bool,
    ) -> BindingId {
        if ident.name != "_"
            && let Some(existing) = self.scopes[0].bindings.get(&ident.name).copied()
        {
            self.error(
                "RESOLVE005",
                format!("duplicate module binding `{}`", ident.name),
                ident.span,
            );
            return existing;
        }

        let id = BindingId(self.bindings.len());
        self.bindings.push(Binding {
            id,
            name: ident.name.clone(),
            kind,
            span: ident.span,
            source_module: None,
            imported_name: None,
            available_realms: realms,
            module_scope: true,
            initialized_at,
            hoisted,
        });
        self.scopes[0].bindings.insert(ident.name.clone(), id);
        id
    }

    fn lookup_current_scope(&self, name: &str) -> Option<BindingId> {
        self.scopes
            .last()
            .and_then(|scope| scope.bindings.get(name).copied())
    }

    fn lookup(&self, name: &str) -> Option<BindingId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.bindings.get(name).copied())
    }

    fn lookup_module(&self, name: &str) -> Option<BindingId> {
        self.scopes
            .first()
            .and_then(|scope| scope.bindings.get(name).copied())
    }

    fn inside_loop_in_current_function(&self) -> bool {
        self.scopes
            .iter()
            .rev()
            .take_while(|scope| scope.kind != ScopeKind::Function)
            .any(|scope| scope.kind == ScopeKind::Loop)
    }

    fn with_scope(&mut self, kind: ScopeKind, f: impl FnOnce(&mut Self)) {
        self.scopes.push(Scope {
            kind,
            bindings: HashMap::new(),
        });
        f(self);
        self.scopes.pop();
    }

    fn with_realm(&mut self, realm: Realm, f: impl FnOnce(&mut Self)) {
        let previous = *self.active_realms.last().unwrap_or(&RealmSet::SHARED);
        self.active_realms
            .push(previous.intersection(RealmSet::from_realm(realm)));
        f(self);
        self.active_realms.pop();
    }

    fn apply_import_use_realms(&mut self) {
        for edge in &mut self.module_edges {
            for specifier in &mut edge.specifiers {
                let Some(binding) = specifier.binding else {
                    continue;
                };
                if let Some(realms) = self.import_binding_uses.get(&binding).copied() {
                    specifier.active_realms = realms;
                }
            }
        }
    }

    fn error(&mut self, code: &str, message: impl Into<String>, span: SourceSpan) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_label(Label::primary(span, "here")),
        );
    }
}

fn binding_kind_for_mode(mode: BindingMode) -> BindingKind {
    match mode {
        BindingMode::Local => BindingKind::Local,
        BindingMode::Const => BindingKind::Const,
    }
}

fn pattern_identifiers(patterns: &[Pattern]) -> Vec<&Identifier> {
    let mut names = Vec::new();
    for pattern in patterns {
        collect_pattern_identifiers(pattern, &mut names);
    }
    names
}

fn collect_pattern_identifiers<'a>(pattern: &'a Pattern, out: &mut Vec<&'a Identifier>) {
    match &pattern.kind {
        PatternKind::Identifier(name) => out.push(name),
        PatternKind::Object(fields) => {
            for field in fields {
                collect_pattern_identifiers(&field.pattern, out);
            }
        }
        PatternKind::Array(items) => {
            for item in items {
                collect_pattern_identifiers(&item.pattern, out);
            }
        }
    }
}

fn unparen_expr(expr: &Expr) -> &Expr {
    match &expr.kind {
        ExprKind::Paren(inner) => unparen_expr(inner),
        _ => expr,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ExternalDiagnosticKey {
    path: Vec<String>,
    active: RealmSet,
    containing_decl: Option<BindingId>,
}

fn collect_match_pattern_binding_names(pattern: &MatchPattern, names: &mut Vec<Identifier>) {
    match &pattern.kind {
        MatchPatternKind::Or(patterns) => {
            for pattern in patterns {
                collect_match_pattern_binding_names(pattern, names);
            }
        }
        MatchPatternKind::Wildcard | MatchPatternKind::Literal(_) => {}
        MatchPatternKind::Binding(name) => {
            if name.name != "_" {
                names.push(name.clone());
            }
        }
        MatchPatternKind::Variant { payload, .. } => {
            if let Some(payload) = payload {
                match payload {
                    MatchPatternPayload::Tuple(patterns) => {
                        for pattern in patterns {
                            collect_match_pattern_binding_names(pattern, names);
                        }
                    }
                    MatchPatternPayload::Record(fields) => {
                        for field in fields {
                            collect_match_pattern_binding_names(&field.pattern, names);
                        }
                    }
                }
            }
        }
        MatchPatternKind::Object(fields) => {
            for field in fields {
                collect_match_pattern_binding_names(&field.pattern, names);
            }
        }
        MatchPatternKind::Array(items) => {
            for item in items {
                collect_match_pattern_binding_names(&item.pattern, names);
            }
        }
    }
}

#[derive(Debug, Clone)]
enum MatchCoverageUnit {
    Unconditional {
        span: SourceSpan,
    },
    Literal {
        key: String,
        span: SourceSpan,
    },
    EnumVariant {
        enum_name: String,
        variant_name: String,
        total: bool,
        span: SourceSpan,
    },
    Opaque,
}

fn enum_is_fully_covered(enum_decl: Option<&EnumDecl>, covered: &BTreeSet<String>) -> bool {
    let Some(enum_decl) = enum_decl else {
        return false;
    };
    if matches!(enum_decl.repr, EnumRepr::Existing { .. }) {
        return false;
    }
    !enum_decl.variants.is_empty()
        && enum_decl
            .variants
            .iter()
            .all(|variant| covered.contains(&variant.name.name))
}

fn match_literal_key(literal: &MatchLiteral) -> String {
    match literal {
        MatchLiteral::Nil => "nil".into(),
        MatchLiteral::Boolean(value) => format!("bool:{value}"),
        MatchLiteral::Number(value) => format!("number:{value}"),
        MatchLiteral::String(value) => format!("string:{value}"),
    }
}

fn match_payload_is_irrefutable(payload: &MatchPatternPayload) -> bool {
    match payload {
        MatchPatternPayload::Tuple(patterns) => patterns.iter().all(match_pattern_is_irrefutable),
        MatchPatternPayload::Record(fields) => fields
            .iter()
            .all(|field| match_pattern_is_irrefutable(&field.pattern)),
    }
}

fn match_pattern_is_irrefutable(pattern: &MatchPattern) -> bool {
    match &pattern.kind {
        MatchPatternKind::Wildcard | MatchPatternKind::Binding(_) => true,
        MatchPatternKind::Or(patterns) => patterns.iter().any(match_pattern_is_irrefutable),
        MatchPatternKind::Variant { payload, .. } => payload
            .as_ref()
            .map(match_payload_is_irrefutable)
            .unwrap_or(true),
        MatchPatternKind::Object(fields) => fields
            .iter()
            .all(|field| match_pattern_is_irrefutable(&field.pattern)),
        MatchPatternKind::Array(items) => items
            .iter()
            .all(|item| match_pattern_is_irrefutable(&item.pattern)),
        MatchPatternKind::Literal(_) => false,
    }
}

fn symbol_path(path: &str) -> Vec<String> {
    path.split('.')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn api_realm_set(realm: ApiRealm) -> RealmSet {
    match realm {
        ApiRealm::Shared => RealmSet::SHARED,
        ApiRealm::Client => RealmSet::CLIENT,
        ApiRealm::Server => RealmSet::SERVER,
        ApiRealm::Menu => RealmSet::NONE,
    }
}

fn module_edge_specifier(specifier: &ImportSpecifier, binding: BindingId) -> ModuleEdgeSpecifier {
    match specifier {
        ImportSpecifier::Named { imported, local } => ModuleEdgeSpecifier {
            imported: imported.name.clone(),
            local: local.name.clone(),
            namespace: false,
            active_realms: RealmSet::NONE,
            binding: Some(binding),
            span: imported.span,
        },
        ImportSpecifier::Namespace { local } => ModuleEdgeSpecifier {
            imported: "*".into(),
            local: local.name.clone(),
            namespace: true,
            active_realms: RealmSet::NONE,
            binding: Some(binding),
            span: local.span,
        },
    }
}

fn import_specifier_binding(specifier: &ImportSpecifier) -> (&Identifier, String) {
    match specifier {
        ImportSpecifier::Named { imported, local } => (local, imported.name.clone()),
        ImportSpecifier::Namespace { local } => (local, "*".into()),
    }
}

#[cfg(test)]
#[path = "resolver/tests.rs"]
mod tests;
