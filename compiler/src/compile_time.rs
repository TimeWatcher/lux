use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use crate::ast::*;
use crate::diag::DiagnosticEmitter;
use crate::host::{
    HostExprTransformCall, HostExprTransformProvider, HostTransformContext, HostTransformSpec,
};
use crate::ir::{
    IrCallStyle, IrChain, IrChainSegment, IrChainSegmentKind, IrExpr, IrExprKind, IrTableField,
    IrTableFieldKind, Origin, ValueMode,
};
use crate::lex::Lexer;
use crate::macro_expansion::{
    MacroCall, MacroContext, MacroExpansion, MacroProvider, MacroRegistry,
};
use crate::packages::{PackageLoadError, default_package_root, discover_compile_time_phases};
use crate::parse::Parser;
use crate::source::{SourceFile, SourceSpan};

const MAX_COMPILE_TIME_CALL_DEPTH: usize = 16;

#[derive(Debug, Clone)]
pub struct CompileTimePackageRegistry {
    root: PathBuf,
    packages: Arc<BTreeMap<String, Arc<CtModule>>>,
}

impl CompileTimePackageRegistry {
    pub fn load_default() -> Result<Self, CompileTimeError> {
        Self::load(default_package_root())
    }

    pub fn load_default_with_package_roots(
        extra_roots: &[PathBuf],
    ) -> Result<Self, CompileTimeError> {
        let mut roots = Vec::with_capacity(extra_roots.len() + 1);
        roots.push(default_package_root());
        roots.extend(extra_roots.iter().cloned());
        Self::load_roots(roots)
    }

    pub fn load(root: impl Into<PathBuf>) -> Result<Self, CompileTimeError> {
        Self::load_roots(vec![root.into()])
    }

    pub fn load_roots(roots: Vec<PathBuf>) -> Result<Self, CompileTimeError> {
        let root = roots.first().cloned().unwrap_or_else(default_package_root);
        let mut packages = BTreeMap::new();
        for package_root in &roots {
            discover_compile_time_packages(package_root, &mut packages)?;
        }
        Ok(Self {
            root,
            packages: Arc::new(packages),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn register_macros(&self, registry: &mut MacroRegistry) -> Result<(), CompileTimeError> {
        for (source, module) in self.packages.iter() {
            registry.register_source(source);
            for export in module.macro_exports() {
                let provider = Arc::new(LuxMacroProvider {
                    module: module.clone(),
                    packages: self.packages.clone(),
                    export: export.to_string(),
                });
                registry.register_provider(source, export, provider);
            }
        }
        Ok(())
    }

    pub fn host_transform_specs(&self) -> Result<Vec<HostTransformSpec>, CompileTimeError> {
        let mut specs = Vec::new();
        for (source, module) in self.packages.iter() {
            if !module.host_expr_exports.is_empty() && module.host_package().is_none() {
                return Err(CompileTimeError::Eval(format!(
                    "compile-time package `{source}` exports host transforms but does not declare `export host package`"
                )));
            }
            let Some(host_package) = module.host_package() else {
                continue;
            };
            for export in module.host_expr_exports() {
                specs.push(HostTransformSpec {
                    target: host_package.target.clone(),
                    runtime: host_package.runtime.clone(),
                    provider: Arc::new(LuxHostExprTransformProvider {
                        function: CtFunctionRef {
                            module: module.clone(),
                            packages: self.packages.clone(),
                            name: export.to_string(),
                        },
                    }),
                });
            }
        }
        Ok(specs)
    }
}

#[derive(Debug)]
pub enum CompileTimeError {
    Package(PackageLoadError),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Diagnostics(Vec<String>),
    DuplicatePackage {
        id: String,
        first: PathBuf,
        second: PathBuf,
    },
    Eval(String),
}

impl fmt::Display for CompileTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Package(source) => write!(f, "{source}"),
            Self::Io { path, source } => {
                write!(
                    f,
                    "failed to load compile-time package {}: {source}",
                    path.display()
                )
            }
            Self::Diagnostics(diagnostics) => {
                for (index, diagnostic) in diagnostics.iter().enumerate() {
                    if index > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{diagnostic}")?;
                }
                Ok(())
            }
            Self::DuplicatePackage { id, first, second } => write!(
                f,
                "duplicate compile-time package `{id}` at {} and {}",
                first.display(),
                second.display()
            ),
            Self::Eval(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CompileTimeError {}

#[derive(Debug)]
struct LuxMacroProvider {
    module: Arc<CtModule>,
    packages: Arc<BTreeMap<String, Arc<CtModule>>>,
    export: String,
}

impl MacroProvider for LuxMacroProvider {
    fn expand(&self, ctx: &mut MacroContext<'_>, call: &MacroCall) -> Option<MacroExpansion> {
        let mut state = EvalState::for_macro(call.span, ctx, self.packages.clone());
        let value = match self.module.eval_function(
            &self.export,
            vec![ctx_value(), macro_call_value(call)],
            &mut state,
        ) {
            Ok(value) => value,
            Err(err) => {
                state.error(err.message);
                return None;
            }
        };
        match value {
            CtValue::Expansion(expansion) => Some(expansion),
            CtValue::Expr(expr) => Some(MacroExpansion::Expr(expr)),
            CtValue::Stmt(stmt) => Some(MacroExpansion::Stmts(vec![stmt])),
            CtValue::Table(table) => table_to_stmts(&table.borrow()).map(MacroExpansion::Stmts),
            CtValue::Nil => None,
            other => {
                state.error(format!(
                    "compile-time macro `{}` returned {}, expected AST expansion",
                    self.export,
                    other.kind_name()
                ));
                None
            }
        }
    }
}

#[derive(Debug)]
struct LuxHostExprTransformProvider {
    function: CtFunctionRef,
}

impl HostExprTransformProvider for LuxHostExprTransformProvider {
    fn transform(
        &self,
        ctx: &mut HostTransformContext,
        call: &HostExprTransformCall,
    ) -> Option<IrExpr> {
        let mut state = EvalState::for_host_transform(
            call.expr.origin.span(),
            ctx,
            self.function.packages.clone(),
        );
        let value = match self.function.module.eval_function(
            &self.function.name,
            vec![host_ctx_value(), host_call_value(call)],
            &mut state,
        ) {
            Ok(value) => value,
            Err(err) => {
                state.error(err.message);
                return None;
            }
        };
        match value {
            CtValue::Nil => None,
            CtValue::IrExpr(expr) => Some(expr),
            other => {
                state.error(format!(
                    "host transform `{}` returned {}, expected IR expression or nil",
                    self.function.name,
                    other.kind_name()
                ));
                None
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CtModule {
    id: String,
    file: SourceFile,
    body: Vec<Stmt>,
    functions: BTreeMap<String, FunctionDecl>,
    exports: BTreeSet<String>,
    macro_exports: BTreeSet<String>,
    host_expr_exports: BTreeSet<String>,
    host_package: Option<HostPackageDecl>,
}

impl CtModule {
    fn parse(id: String, file: SourceFile) -> Result<Self, CompileTimeError> {
        let lex = Lexer::new(&file).lex_all();
        if lex.has_errors() {
            return Err(CompileTimeError::Diagnostics(
                lex.diagnostics
                    .iter()
                    .map(|diagnostic| DiagnosticEmitter::render(diagnostic, &file))
                    .collect(),
            ));
        }

        let parsed = Parser::new(&lex.tokens).parse_module();
        if parsed.has_errors() {
            return Err(CompileTimeError::Diagnostics(
                parsed
                    .diagnostics
                    .iter()
                    .map(|diagnostic| DiagnosticEmitter::render(diagnostic, &file))
                    .collect(),
            ));
        }

        let mut functions = BTreeMap::new();
        let mut exports = BTreeSet::new();
        let mut macro_exports = BTreeSet::new();
        let mut host_expr_exports = BTreeSet::new();
        let mut host_package = None;
        for stmt in &parsed.module.body {
            match &stmt.kind {
                StmtKind::HostPackageDecl(decl) => {
                    if host_package.is_some() {
                        return Err(CompileTimeError::Eval(format!(
                            "compile-time package `{id}` declares more than one host package"
                        )));
                    }
                    host_package = Some(decl.clone());
                }
                StmtKind::FunctionDecl(decl) => {
                    if let FunctionName::Simple(name) = &decl.name {
                        functions.insert(name.name.clone(), decl.clone());
                    }
                }
                StmtKind::RealmDecl { stmt: inner, .. } => {
                    if let StmtKind::FunctionDecl(decl) = &inner.kind
                        && let FunctionName::Simple(name) = &decl.name
                    {
                        functions.insert(name.name.clone(), decl.clone());
                    }
                }
                StmtKind::ExportDecl {
                    kind, stmt: inner, ..
                } => match &inner.kind {
                    StmtKind::FunctionDecl(decl) => {
                        let FunctionName::Simple(name) = &decl.name else {
                            return Err(CompileTimeError::Eval(format!(
                                "compile-time export declarations in `{id}` require simple function names"
                            )));
                        };

                        functions.insert(name.name.clone(), decl.clone());
                        match kind {
                            ExportKind::Runtime => {
                                exports.insert(name.name.clone());
                            }
                            ExportKind::Macro => {
                                macro_exports.insert(name.name.clone());
                            }
                            ExportKind::HostExpr => {
                                host_expr_exports.insert(name.name.clone());
                            }
                        }
                    }
                    StmtKind::LocalDecl {
                        mode: BindingMode::Const,
                        names,
                        ..
                    } if *kind == ExportKind::Runtime => {
                        for name in names {
                            exports.insert(name.name.clone());
                        }
                    }
                    _ => {}
                },
                StmtKind::ExportList { entries, .. } => {
                    for entry in entries {
                        exports.insert(entry.exported.name.clone());
                    }
                }
                _ => {}
            }
        }

        Ok(Self {
            id,
            file,
            body: parsed.module.body,
            functions,
            exports,
            macro_exports,
            host_expr_exports,
            host_package,
        })
    }

    fn exports(&self) -> impl Iterator<Item = &str> {
        self.exports.iter().map(String::as_str)
    }

    fn macro_exports(&self) -> impl Iterator<Item = &str> {
        self.macro_exports.iter().map(String::as_str)
    }

    fn host_expr_exports(&self) -> impl Iterator<Item = &str> {
        self.host_expr_exports.iter().map(String::as_str)
    }

    fn host_package(&self) -> Option<&HostPackageDecl> {
        self.host_package.as_ref()
    }

    fn eval_function(
        self: &Arc<Self>,
        name: &str,
        args: Vec<CtValue>,
        state: &mut EvalState<'_, '_>,
    ) -> CtResult<CtValue> {
        if state.call_depth >= MAX_COMPILE_TIME_CALL_DEPTH {
            return Err(CtError::new(format!(
                "compile-time function recursion exceeded {MAX_COMPILE_TIME_CALL_DEPTH} calls"
            )));
        }
        state.call_depth += 1;

        let mut function = CtFunctionRef {
            module: self.clone(),
            packages: state.packages.clone(),
            name: name.to_string(),
        };
        let mut args = args;
        let result = loop {
            match eval_function_once(&function, args, state) {
                Ok(FunctionResult::Value(value)) => break Ok(value),
                Ok(FunctionResult::TailCall(next_function, next_args)) => {
                    function = next_function;
                    args = next_args;
                }
                Err(err) => break Err(err),
            }
        };
        state.call_depth -= 1;

        result
    }
}

enum FunctionResult {
    Value(CtValue),
    TailCall(CtFunctionRef, Vec<CtValue>),
}

fn eval_function_once(
    function: &CtFunctionRef,
    args: Vec<CtValue>,
    state: &mut EvalState<'_, '_>,
) -> CtResult<FunctionResult> {
    let module = &function.module;
    let Some(decl) = module.functions.get(&function.name) else {
        return Err(CtError::new(format!(
            "unknown compile-time function `{}.{}`",
            module.id, function.name
        )));
    };

    let mut env = CtEnv::from_module(module.clone(), function.packages.clone())?;
    for (index, param) in decl.params.iter().enumerate() {
        let mut value = args.get(index).cloned().unwrap_or(CtValue::Nil);
        if matches!(value, CtValue::Nil) {
            if let Some(default) = &param.default {
                value = eval_expr(module, default, &mut env, state)?;
            }
        }
        env.set(param.name.name.clone(), value);
    }
    if decl.vararg {
        let rest = args.into_iter().skip(decl.params.len()).collect::<Vec<_>>();
        env.set("...".into(), table_value(CtTable::array(rest)));
    }

    eval_function_body(module, &decl.body, &mut env, state)
}

#[derive(Debug, Clone)]
struct CtEnv {
    values: BTreeMap<String, CtValue>,
}

impl CtEnv {
    fn from_module(
        module: Arc<CtModule>,
        packages: Arc<BTreeMap<String, Arc<CtModule>>>,
    ) -> CtResult<Self> {
        let mut env = Self {
            values: BTreeMap::new(),
        };
        for stmt in &module.body {
            if let StmtKind::Import(import) = &stmt.kind {
                env.bind_compile_time_import(import, packages.clone())?;
            }
        }
        env.set("error".into(), CtValue::Native(CtNative::CompileTimeError));
        for name in module.functions.keys() {
            env.set(
                name.clone(),
                CtValue::Function(CtFunctionRef {
                    module: module.clone(),
                    packages: packages.clone(),
                    name: name.clone(),
                }),
            );
        }
        let mut state = EvalState::for_compile_time_module(
            SourceSpan::new(module.file.id, 0, module.file.text.len()),
            packages,
        );
        for stmt in &module.body {
            match &stmt.kind {
                StmtKind::LocalDecl {
                    mode: BindingMode::Const,
                    ..
                }
                | StmtKind::LocalDestructure {
                    mode: BindingMode::Const,
                    ..
                } => {
                    let _ = eval_stmt(&module, stmt, &mut env, &mut state)?;
                }
                StmtKind::ExportDecl { stmt: inner, .. }
                | StmtKind::RealmDecl { stmt: inner, .. }
                    if matches!(
                        &inner.kind,
                        StmtKind::LocalDecl {
                            mode: BindingMode::Const,
                            ..
                        } | StmtKind::LocalDestructure {
                            mode: BindingMode::Const,
                            ..
                        }
                    ) =>
                {
                    let _ = eval_stmt(&module, inner, &mut env, &mut state)?;
                }
                _ => {}
            }
        }
        Ok(env)
    }

    fn bind_compile_time_import(
        &mut self,
        import: &ImportStmt,
        packages: Arc<BTreeMap<String, Arc<CtModule>>>,
    ) -> CtResult<()> {
        let source = canonical_import_source(&import.source);
        let module = match source.as_str() {
            "lux/compile/ast" => ast_intrinsics(),
            "lux/compile/ir" => ir_intrinsics(),
            source => compile_time_module_exports(packages, source)?,
        };

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Namespace { local } => {
                    self.set(local.name.clone(), module.clone());
                }
                ImportSpecifier::Named { imported, local } => {
                    let Some(value) = module.field(&imported.name) else {
                        return Err(CtError::new(format!(
                            "compile-time import `{}` has no export `{}`",
                            import.source, imported.name
                        )));
                    };
                    self.set(local.name.clone(), value);
                }
            }
        }
        Ok(())
    }

    fn get(&self, name: &str) -> CtResult<CtValue> {
        self.values
            .get(name)
            .cloned()
            .ok_or_else(|| CtError::new(format!("unknown compile-time binding `{name}`")))
    }

    fn set(&mut self, name: String, value: CtValue) {
        self.values.insert(name, value);
    }
}

fn canonical_import_source(source: &str) -> String {
    source.strip_prefix('@').unwrap_or(source).to_string()
}

#[derive(Debug, Clone)]
enum CtValue {
    Nil,
    Bool(bool),
    Number(f64),
    String(String),
    Table(CtTableRef),
    Function(CtFunctionRef),
    Native(CtNative),
    Expr(Expr),
    Stmt(Stmt),
    TableField(TableField),
    Block(Block),
    IrExpr(IrExpr),
    Span(SourceSpan),
    Expansion(MacroExpansion),
}

impl CtValue {
    fn kind_name(&self) -> &'static str {
        match self {
            Self::Nil => "nil",
            Self::Bool(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Table(_) => "table",
            Self::Function(_) => "function",
            Self::Native(_) => "native function",
            Self::Expr(_) => "AST expression",
            Self::Stmt(_) => "AST statement",
            Self::TableField(_) => "AST table field",
            Self::Block(_) => "AST block",
            Self::IrExpr(_) => "IR expression",
            Self::Span(_) => "source span",
            Self::Expansion(_) => "macro expansion",
        }
    }

    fn truthy(&self) -> bool {
        !matches!(self, Self::Nil | Self::Bool(false))
    }

    fn field(&self, name: &str) -> Option<CtValue> {
        match self {
            Self::Table(table) => table.borrow().fields.get(name).cloned(),
            _ => None,
        }
    }
}

type CtTableRef = Rc<RefCell<CtTable>>;

fn table_value(table: CtTable) -> CtValue {
    CtValue::Table(Rc::new(RefCell::new(table)))
}

#[derive(Debug, Clone)]
struct CtTable {
    array: Vec<CtValue>,
    fields: BTreeMap<String, CtValue>,
}

impl CtTable {
    fn new() -> Self {
        Self {
            array: Vec::new(),
            fields: BTreeMap::new(),
        }
    }

    fn array(array: Vec<CtValue>) -> Self {
        Self {
            array,
            fields: BTreeMap::new(),
        }
    }

    fn get_index(&self, index: &CtValue) -> CtValue {
        match index {
            CtValue::Number(number) if *number >= 1.0 => {
                let index = (*number as usize).saturating_sub(1);
                self.array.get(index).cloned().unwrap_or(CtValue::Nil)
            }
            CtValue::String(key) => self.fields.get(key).cloned().unwrap_or(CtValue::Nil),
            _ => CtValue::Nil,
        }
    }

    fn set_index(&mut self, index: CtValue, value: CtValue) -> CtResult<()> {
        match index {
            CtValue::Number(number) if number >= 1.0 => {
                let index = (number as usize).saturating_sub(1);
                while self.array.len() <= index {
                    self.array.push(CtValue::Nil);
                }
                self.array[index] = value;
                Ok(())
            }
            CtValue::String(key) => {
                self.fields.insert(key, value);
                Ok(())
            }
            other => Err(CtError::new(format!(
                "compile-time table index must be number or string, got {}",
                other.kind_name()
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CtNative {
    CtxGensym,
    CtxGensymString,
    CtxLabel,
    CtxError,
    CompileTimeError,
    HostCtxImportRuntime,
    HostCtxError,
    AstIdent,
    AstNil,
    AstData,
    AstNumber,
    AstString,
    AstMember,
    AstIndex,
    AstCall,
    AstTable,
    AstArrayField,
    AstNamedField,
    AstKeyedField,
    AstBlock,
    AstFunc,
    AstFnDecl,
    AstExportRuntime,
    AstAssign,
    AstLocalDecl,
    AstConstDecl,
    AstReturnStmt,
    AstIfStmt,
    AstDoStmt,
    AstDoExpr,
    AstRealmBlock,
    AstExprStmt,
    AstExpr,
    AstStmts,
    IrIdent,
    IrString,
    IrTable,
    IrCall,
    IrChildren,
    IrTailTableParts,
}

#[derive(Debug, Clone)]
struct CtFunctionRef {
    module: Arc<CtModule>,
    packages: Arc<BTreeMap<String, Arc<CtModule>>>,
    name: String,
}

struct EvalState<'a, 'b> {
    current_span: SourceSpan,
    packages: Arc<BTreeMap<String, Arc<CtModule>>>,
    macro_ctx: Option<&'a mut MacroContext<'b>>,
    host_ctx: Option<&'a mut HostTransformContext>,
    pipeline_placeholders: Vec<CtValue>,
    call_depth: usize,
}

impl<'a, 'b> EvalState<'a, 'b> {
    fn for_macro(
        current_span: SourceSpan,
        macro_ctx: &'a mut MacroContext<'b>,
        packages: Arc<BTreeMap<String, Arc<CtModule>>>,
    ) -> Self {
        Self {
            current_span,
            packages,
            macro_ctx: Some(macro_ctx),
            host_ctx: None,
            pipeline_placeholders: Vec::new(),
            call_depth: 0,
        }
    }

    fn for_host_transform(
        current_span: SourceSpan,
        host_ctx: &'a mut HostTransformContext,
        packages: Arc<BTreeMap<String, Arc<CtModule>>>,
    ) -> Self {
        Self {
            current_span,
            packages,
            macro_ctx: None,
            host_ctx: Some(host_ctx),
            pipeline_placeholders: Vec::new(),
            call_depth: 0,
        }
    }

    fn for_compile_time_module(
        current_span: SourceSpan,
        packages: Arc<BTreeMap<String, Arc<CtModule>>>,
    ) -> Self {
        Self {
            current_span,
            packages,
            macro_ctx: None,
            host_ctx: None,
            pipeline_placeholders: Vec::new(),
            call_depth: 0,
        }
    }

    fn error(&mut self, message: String) {
        if let Some(ctx) = self.macro_ctx.as_deref_mut() {
            ctx.error("CT001", message, self.current_span);
        } else if let Some(ctx) = self.host_ctx.as_deref_mut() {
            ctx.error("CT001", message, &Origin::source(self.current_span));
        }
    }
}

#[derive(Debug, Clone)]
struct CtError {
    message: String,
}

impl CtError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<CtError> for CompileTimeError {
    fn from(value: CtError) -> Self {
        CompileTimeError::Eval(value.message)
    }
}

type CtResult<T> = Result<T, CtError>;

enum Flow {
    Continue,
    Return(CtValue),
}

fn eval_function_body(
    module: &Arc<CtModule>,
    body: &FunctionBody,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<FunctionResult> {
    match body {
        FunctionBody::Expr(expr) => eval_function_tail_expr(module, expr, env, state),
        FunctionBody::Block(block) => eval_function_block(module, block, env, state),
    }
}

fn eval_function_block(
    module: &Arc<CtModule>,
    block: &Block,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<FunctionResult> {
    for stmt in &block.statements {
        match &stmt.kind {
            StmtKind::Return(values) => {
                let Some(value) = values.first() else {
                    return Ok(FunctionResult::Value(CtValue::Nil));
                };
                return eval_function_tail_expr(module, value, env, state);
            }
            _ => match eval_stmt(module, stmt, env, state)? {
                Flow::Continue => {}
                Flow::Return(value) => return Ok(FunctionResult::Value(value)),
            },
        }
    }
    if let Some(tail) = &block.tail {
        eval_function_tail_expr(module, tail, env, state)
    } else {
        Ok(FunctionResult::Value(CtValue::Nil))
    }
}

fn eval_function_tail_expr(
    module: &Arc<CtModule>,
    expr: &Expr,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<FunctionResult> {
    if let Some((function, args)) = eval_tail_function_call(module, expr, env, state)? {
        Ok(FunctionResult::TailCall(function, args))
    } else {
        eval_expr(module, expr, env, state).map(FunctionResult::Value)
    }
}

fn eval_tail_function_call(
    module: &Arc<CtModule>,
    expr: &Expr,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<Option<(CtFunctionRef, Vec<CtValue>)>> {
    let ExprKind::Chain(chain) = &expr.kind else {
        return Ok(None);
    };
    let ExprKind::Identifier(base) = &chain.base.kind else {
        return Ok(None);
    };
    let [segment] = chain.segments.as_slice() else {
        return Ok(None);
    };
    let ChainSegmentKind::Call { args, .. } = &segment.kind else {
        return Ok(None);
    };
    let CtValue::Function(function) = env.get(&base.name)? else {
        return Ok(None);
    };
    let args = args
        .iter()
        .map(|arg| eval_expr(module, arg, env, state))
        .collect::<CtResult<Vec<_>>>()?;
    Ok(Some((function, args)))
}

fn eval_block(
    module: &Arc<CtModule>,
    block: &Block,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<CtValue> {
    for stmt in &block.statements {
        match eval_stmt(module, stmt, env, state)? {
            Flow::Continue => {}
            Flow::Return(value) => return Ok(value),
        }
    }
    if let Some(tail) = &block.tail {
        eval_expr(module, tail, env, state)
    } else {
        Ok(CtValue::Nil)
    }
}

fn eval_block_as_statement(
    module: &Arc<CtModule>,
    block: &Block,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<Flow> {
    for stmt in &block.statements {
        match eval_stmt(module, stmt, env, state)? {
            Flow::Continue => {}
            flow @ Flow::Return(_) => return Ok(flow),
        }
    }
    if let Some(tail) = &block.tail {
        let _ = eval_expr(module, tail, env, state)?;
    }
    Ok(Flow::Continue)
}

fn eval_stmt(
    module: &Arc<CtModule>,
    stmt: &Stmt,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<Flow> {
    match &stmt.kind {
        StmtKind::LocalDecl { names, values, .. } => {
            for (index, name) in names.iter().enumerate() {
                let value = values
                    .get(index)
                    .map(|expr| eval_expr(module, expr, env, state))
                    .transpose()?
                    .unwrap_or(CtValue::Nil);
                env.set(name.name.clone(), value);
            }
            Ok(Flow::Continue)
        }
        StmtKind::LocalDestructure {
            patterns, values, ..
        } => {
            let values = values
                .iter()
                .map(|expr| eval_expr(module, expr, env, state))
                .collect::<CtResult<Vec<_>>>()?;
            for (index, pattern) in patterns.iter().enumerate() {
                let value = values.get(index).cloned().unwrap_or(CtValue::Nil);
                bind_pattern(module, pattern, value, env, state)?;
            }
            Ok(Flow::Continue)
        }
        StmtKind::Assign { targets, values } => {
            for (index, target) in targets.iter().enumerate() {
                let value = values
                    .get(index)
                    .map(|expr| eval_expr(module, expr, env, state))
                    .transpose()?
                    .unwrap_or(CtValue::Nil);
                assign_compile_time_target(module, target, value, env, state)?;
            }
            Ok(Flow::Continue)
        }
        StmtKind::Return(values) => {
            let value = values
                .first()
                .map(|expr| eval_expr(module, expr, env, state))
                .transpose()?
                .unwrap_or(CtValue::Nil);
            Ok(Flow::Return(value))
        }
        StmtKind::Expr(expr) => {
            let _ = eval_expr(module, expr, env, state)?;
            Ok(Flow::Continue)
        }
        StmtKind::If {
            condition,
            then_block,
            else_block,
        } => {
            if eval_expr(module, condition, env, state)?.truthy() {
                eval_block_as_statement(module, then_block, env, state)
            } else if let Some(block) = else_block {
                eval_block_as_statement(module, block, env, state)
            } else {
                Ok(Flow::Continue)
            }
        }
        StmtKind::NumericFor {
            name,
            start,
            end,
            step,
            body,
        } => {
            let mut index = expect_compile_time_number(
                eval_expr(module, start, env, state)?,
                "compile-time numeric for start",
            )?;
            let end = expect_compile_time_number(
                eval_expr(module, end, env, state)?,
                "compile-time numeric for end",
            )?;
            let step = step
                .as_ref()
                .map(|expr| {
                    expect_compile_time_number(
                        eval_expr(module, expr, env, state)?,
                        "compile-time numeric for step",
                    )
                })
                .transpose()?
                .unwrap_or(1.0);
            if step.abs() < f64::EPSILON {
                return Err(CtError::new("compile-time numeric for step cannot be zero"));
            }

            while if step > 0.0 {
                index <= end
            } else {
                index >= end
            } {
                env.set(name.name.clone(), CtValue::Number(index));
                match eval_block_as_statement(module, body, env, state)? {
                    Flow::Continue => {}
                    flow @ Flow::Return(_) => return Ok(flow),
                }
                index += step;
            }
            Ok(Flow::Continue)
        }
        StmtKind::FunctionDecl(decl) => {
            if let FunctionName::Simple(name) = &decl.name {
                env.set(
                    name.name.clone(),
                    CtValue::Function(CtFunctionRef {
                        module: module.clone(),
                        packages: state.packages.clone(),
                        name: name.name.clone(),
                    }),
                );
            }
            Ok(Flow::Continue)
        }
        StmtKind::Import(_)
        | StmtKind::HostPackageDecl(_)
        | StmtKind::PartOrderDecl(_)
        | StmtKind::ExternDecl(_)
        | StmtKind::ExportDecl { .. }
        | StmtKind::ExportList { .. }
        | StmtKind::ExportAll { .. } => Ok(Flow::Continue),
        StmtKind::RealmDecl { stmt, .. } => eval_stmt(module, stmt, env, state),
        StmtKind::RealmBlock { block, .. } | StmtKind::InitDecl { block, .. } => {
            eval_block_as_statement(module, block, env, state)
        }
        other => Err(CtError::new(format!(
            "compile-time evaluator does not support statement `{other:?}` yet"
        ))),
    }
}

fn eval_expr(
    module: &Arc<CtModule>,
    expr: &Expr,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<CtValue> {
    match &expr.kind {
        ExprKind::Identifier(name) => env.get(&name.name),
        ExprKind::Nil => Ok(CtValue::Nil),
        ExprKind::Boolean(value) => Ok(CtValue::Bool(*value)),
        ExprKind::Number(value) => Ok(CtValue::Number(value.parse::<f64>().unwrap_or(0.0))),
        ExprKind::String(value) => Ok(CtValue::String(value.clone())),
        ExprKind::Vararg => env.get("..."),
        ExprKind::PipelinePlaceholder => state
            .pipeline_placeholders
            .last()
            .cloned()
            .ok_or_else(|| CtError::new("pipeline placeholder `%` used outside `|>`")),
        ExprKind::Table(table) => eval_table(module, table, env, state),
        ExprKind::Paren(inner) => eval_expr(module, inner, env, state),
        ExprKind::Unary { op, argument } => {
            let value = eval_expr(module, argument, env, state)?;
            match op {
                UnaryOp::Not => Ok(CtValue::Bool(!value.truthy())),
                UnaryOp::Len => match value {
                    CtValue::Table(table) => Ok(CtValue::Number(table.borrow().array.len() as f64)),
                    CtValue::String(value) => Ok(CtValue::Number(value.len() as f64)),
                    _ => Ok(CtValue::Number(0.0)),
                },
                UnaryOp::Neg => match value {
                    CtValue::Number(value) => Ok(CtValue::Number(-value)),
                    _ => Err(CtError::new("compile-time unary minus expects a number")),
                },
            }
        }
        ExprKind::Binary { op, left, right } => eval_binary(module, *op, left, right, env, state),
        ExprKind::Conditional {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            if eval_expr(module, condition, env, state)?.truthy() {
                eval_expr_or_block(module, then_branch, env, state)
            } else {
                eval_expr_or_block(module, else_branch, env, state)
            }
        }
        ExprKind::Do(block) => eval_block(module, block, env, state),
        ExprKind::Chain(chain) => eval_chain(module, chain, env, state),
        other => Err(CtError::new(format!(
            "compile-time evaluator does not support expression `{other:?}` yet"
        ))),
    }
}

fn bind_pattern(
    module: &Arc<CtModule>,
    pattern: &Pattern,
    value: CtValue,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<()> {
    match &pattern.kind {
        PatternKind::Identifier(name) => {
            env.set(name.name.clone(), value);
        }
        PatternKind::Object(fields) => {
            let table = match value {
                CtValue::Table(table) => table.borrow().clone(),
                CtValue::Nil => CtTable::new(),
                other => {
                    return Err(CtError::new(format!(
                        "compile-time object destructuring expected table, got {}",
                        other.kind_name()
                    )));
                }
            };
            for field in fields {
                let mut field_value = table
                    .fields
                    .get(&field.key.name)
                    .cloned()
                    .unwrap_or(CtValue::Nil);
                if matches!(field_value, CtValue::Nil) {
                    if let Some(default) = &field.default {
                        field_value = eval_expr(module, default, env, state)?;
                    }
                }
                bind_pattern(module, &field.pattern, field_value, env, state)?;
            }
        }
        PatternKind::Array(items) => {
            let table = match value {
                CtValue::Table(table) => table.borrow().clone(),
                CtValue::Nil => CtTable::new(),
                other => {
                    return Err(CtError::new(format!(
                        "compile-time array destructuring expected table, got {}",
                        other.kind_name()
                    )));
                }
            };
            for (index, item) in items.iter().enumerate() {
                let mut item_value = table.array.get(index).cloned().unwrap_or(CtValue::Nil);
                if matches!(item_value, CtValue::Nil) {
                    if let Some(default) = &item.default {
                        item_value = eval_expr(module, default, env, state)?;
                    }
                }
                bind_pattern(module, &item.pattern, item_value, env, state)?;
            }
        }
    }
    Ok(())
}

fn assign_compile_time_target(
    module: &Arc<CtModule>,
    target: &Expr,
    value: CtValue,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<()> {
    match &target.kind {
        ExprKind::Identifier(name) => {
            env.set(name.name.clone(), value);
            Ok(())
        }
        ExprKind::Chain(chain) => assign_compile_time_chain(module, chain, value, env, state),
        other => Err(CtError::new(format!(
            "compile-time evaluator cannot assign to `{other:?}`"
        ))),
    }
}

fn assign_compile_time_chain(
    module: &Arc<CtModule>,
    chain: &ChainExpr,
    value: CtValue,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<()> {
    let ExprKind::Identifier(base) = &chain.base.kind else {
        return Err(CtError::new(
            "compile-time table assignment requires an identifier base",
        ));
    };
    if chain.segments.len() != 1 {
        return Err(CtError::new(
            "compile-time table assignment supports one index/member segment",
        ));
    }

    let table = match env.get(&base.name)? {
        CtValue::Table(table) => table,
        other => {
            return Err(CtError::new(format!(
                "compile-time table assignment expected table, got {}",
                other.kind_name()
            )));
        }
    };

    match &chain.segments[0].kind {
        ChainSegmentKind::Index { index, .. } => {
            let key = eval_expr(module, index, env, state)?;
            table.borrow_mut().set_index(key, value)?;
        }
        ChainSegmentKind::Member { name, .. } => {
            table.borrow_mut().fields.insert(name.name.clone(), value);
        }
        other => {
            return Err(CtError::new(format!(
                "compile-time table assignment cannot assign through `{other:?}`"
            )));
        }
    }

    Ok(())
}

fn eval_expr_or_block(
    module: &Arc<CtModule>,
    item: &ExprOrBlock,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<CtValue> {
    match item {
        ExprOrBlock::Expr(expr) => eval_expr(module, expr, env, state),
        ExprOrBlock::Block(block) => eval_block(module, block, env, state),
    }
}

fn eval_binary(
    module: &Arc<CtModule>,
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<CtValue> {
    match op {
        BinaryOp::And => {
            let left = eval_expr(module, left, env, state)?;
            if !left.truthy() {
                return Ok(left);
            }
            eval_expr(module, right, env, state)
        }
        BinaryOp::Or => {
            let left = eval_expr(module, left, env, state)?;
            if left.truthy() {
                return Ok(left);
            }
            eval_expr(module, right, env, state)
        }
        BinaryOp::Eq | BinaryOp::NotEq => {
            let left = eval_expr(module, left, env, state)?;
            let right = eval_expr(module, right, env, state)?;
            let equal = values_equal(&left, &right);
            Ok(CtValue::Bool(if op == BinaryOp::Eq {
                equal
            } else {
                !equal
            }))
        }
        BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
            let left = eval_expr(module, left, env, state)?;
            let right = eval_expr(module, right, env, state)?;
            match (left, right) {
                (CtValue::Number(left), CtValue::Number(right)) => {
                    let result = match op {
                        BinaryOp::Lt => left < right,
                        BinaryOp::LtEq => left <= right,
                        BinaryOp::Gt => left > right,
                        BinaryOp::GtEq => left >= right,
                        _ => unreachable!(),
                    };
                    Ok(CtValue::Bool(result))
                }
                (left, right) => Err(CtError::new(format!(
                    "compile-time comparison expects numbers, got {} and {}",
                    left.kind_name(),
                    right.kind_name()
                ))),
            }
        }
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
            let left = eval_expr(module, left, env, state)?;
            let right = eval_expr(module, right, env, state)?;
            match (left, right) {
                (CtValue::Number(left), CtValue::Number(right)) => {
                    let result = match op {
                        BinaryOp::Add => left + right,
                        BinaryOp::Sub => left - right,
                        BinaryOp::Mul => left * right,
                        BinaryOp::Div => left / right,
                        BinaryOp::Mod => left % right,
                        _ => unreachable!(),
                    };
                    Ok(CtValue::Number(result))
                }
                (left, right) => Err(CtError::new(format!(
                    "compile-time arithmetic expects numbers, got {} and {}",
                    left.kind_name(),
                    right.kind_name()
                ))),
            }
        }
        BinaryOp::Concat => {
            let left = value_to_string(eval_expr(module, left, env, state)?);
            let right = value_to_string(eval_expr(module, right, env, state)?);
            Ok(CtValue::String(format!("{left}{right}")))
        }
        BinaryOp::Coalesce => {
            let left = eval_expr(module, left, env, state)?;
            if matches!(left, CtValue::Nil) {
                eval_expr(module, right, env, state)
            } else {
                Ok(left)
            }
        }
        BinaryOp::Pipe => {
            let left = eval_expr(module, left, env, state)?;
            state.pipeline_placeholders.push(left);
            let result = eval_expr(module, right, env, state);
            state.pipeline_placeholders.pop();
            result
        }
        _ => Err(CtError::new(format!(
            "compile-time evaluator does not support binary operator `{op:?}` yet"
        ))),
    }
}

fn eval_table(
    module: &Arc<CtModule>,
    table: &TableExpr,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<CtValue> {
    let mut out = CtTable::new();
    for field in &table.fields {
        match &field.kind {
            TableFieldKind::Array(expr) => {
                out.array.push(eval_expr(module, expr, env, state)?);
            }
            TableFieldKind::Named { name, value } => {
                out.fields
                    .insert(name.name.clone(), eval_expr(module, value, env, state)?);
            }
            TableFieldKind::ExprKey { key, value } => {
                let key = value_to_string(eval_expr(module, key, env, state)?);
                out.fields
                    .insert(key, eval_expr(module, value, env, state)?);
            }
            TableFieldKind::Spread(value) => match eval_expr(module, value, env, state)? {
                CtValue::Nil => {}
                CtValue::Table(table) => {
                    let table = table.borrow();
                    for item in &table.array {
                        out.array.push(item.clone());
                    }
                    for (key, value) in &table.fields {
                        out.fields.insert(key.clone(), value.clone());
                    }
                }
                other => {
                    return Err(CtError::new(format!(
                        "compile-time table spread expected table, got {}",
                        other.kind_name()
                    )));
                }
            },
        }
    }
    Ok(table_value(out))
}

fn eval_chain(
    module: &Arc<CtModule>,
    chain: &ChainExpr,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<CtValue> {
    let mut value = eval_expr(module, &chain.base, env, state)?;
    for segment in &chain.segments {
        match &segment.kind {
            ChainSegmentKind::Member { name, .. } => {
                value = value.field(&name.name).unwrap_or(CtValue::Nil);
            }
            ChainSegmentKind::Index { index, .. } => {
                let key = eval_expr(module, index, env, state)?;
                value = match value {
                    CtValue::Table(table) => table.borrow().get_index(&key),
                    _ => CtValue::Nil,
                };
            }
            ChainSegmentKind::Call { args, .. } => {
                let args = args
                    .iter()
                    .map(|arg| eval_expr(module, arg, env, state))
                    .collect::<CtResult<Vec<_>>>()?;
                value = call_value(value, args, env, state)?;
            }
            ChainSegmentKind::MethodCall { name, args, .. } => {
                let receiver = value.clone();
                let callee = value.field(&name.name).unwrap_or(CtValue::Nil);
                let mut call_args = vec![receiver];
                call_args.extend(
                    args.iter()
                        .map(|arg| eval_expr(module, arg, env, state))
                        .collect::<CtResult<Vec<_>>>()?,
                );
                value = call_value(callee, call_args, env, state)?;
            }
            ChainSegmentKind::SafeDotCall { name, args, .. } => {
                let callee = value.field(&name.name).unwrap_or(CtValue::Nil);
                let call_args = args
                    .iter()
                    .map(|arg| eval_expr(module, arg, env, state))
                    .collect::<CtResult<Vec<_>>>()?;
                value = call_value(callee, call_args, env, state)?;
            }
        }
    }
    Ok(value)
}

fn call_value(
    callee: CtValue,
    args: Vec<CtValue>,
    env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<CtValue> {
    match callee {
        CtValue::Function(function) => function.module.eval_function(&function.name, args, state),
        CtValue::Native(native) => call_native(native, args, env, state),
        other => Err(CtError::new(format!(
            "attempted to call {}, expected function",
            other.kind_name()
        ))),
    }
}

fn call_native(
    native: CtNative,
    args: Vec<CtValue>,
    _env: &mut CtEnv,
    state: &mut EvalState<'_, '_>,
) -> CtResult<CtValue> {
    match native {
        CtNative::CtxGensym => {
            let prefix = expect_string(args.first(), "ctx.gensym prefix")?;
            let Some(ctx) = state.macro_ctx.as_deref_mut() else {
                return Err(CtError::new(
                    "ctx.gensym is only available during macro expansion",
                ));
            };
            Ok(CtValue::String(ctx.gensym(&prefix)))
        }
        CtNative::CtxGensymString => {
            let prefix = expect_string(args.first(), "ctx.gensymString prefix")?;
            let Some(ctx) = state.macro_ctx.as_deref_mut() else {
                return Err(CtError::new(
                    "ctx.gensymString is only available during macro expansion",
                ));
            };
            Ok(CtValue::String(ctx.gensym_string(&prefix)))
        }
        CtNative::CtxLabel => {
            let span = expect_span(args.first(), state.current_span)?;
            let Some(ctx) = state.macro_ctx.as_deref_mut() else {
                return Err(CtError::new(
                    "ctx.label is only available during macro expansion",
                ));
            };
            let label = format!(
                "{}:{}",
                ctx.file().display_name(),
                ctx.file().line_col(span.byte_start).0
            );
            Ok(CtValue::String(label))
        }
        CtNative::CtxError => {
            let code = expect_string(args.first(), "ctx.error code")?;
            let message = expect_string(args.get(1), "ctx.error message")?;
            let span = expect_span(args.get(2), state.current_span)?;
            let Some(ctx) = state.macro_ctx.as_deref_mut() else {
                return Err(CtError::new(
                    "ctx.error is only available during macro expansion",
                ));
            };
            ctx.error(&code, message, span);
            Ok(CtValue::Nil)
        }
        CtNative::CompileTimeError => {
            let message = expect_string(args.first(), "error message")?;
            Err(CtError::new(message))
        }
        CtNative::HostCtxImportRuntime => {
            let imported = expect_string(args.first(), "host ctx.importRuntime imported")?;
            let local = expect_string(args.get(1), "host ctx.importRuntime local")?;
            let Some(ctx) = state.host_ctx.as_deref_mut() else {
                return Err(CtError::new(
                    "ctx.importRuntime is only available during host transforms",
                ));
            };
            Ok(CtValue::String(ctx.import_runtime(imported, local)))
        }
        CtNative::HostCtxError => {
            let code = expect_string(args.first(), "host ctx.error code")?;
            let message = expect_string(args.get(1), "host ctx.error message")?;
            let Some(ctx) = state.host_ctx.as_deref_mut() else {
                return Err(CtError::new(
                    "ctx.error is only available during host transforms",
                ));
            };
            ctx.error(&code, message, &Origin::source(state.current_span));
            Ok(CtValue::Nil)
        }
        CtNative::AstIdent => {
            let name = expect_string(args.first(), "ast.ident name")?;
            let span = expect_span(args.get(1), state.current_span)?;
            Ok(CtValue::Expr(ident_expr(&name, span)))
        }
        CtNative::AstNil => {
            let span = expect_span(args.first(), state.current_span)?;
            Ok(CtValue::Expr(Expr {
                kind: ExprKind::Nil,
                span,
            }))
        }
        CtNative::AstData => {
            let expr = expect_expr(args.first(), "ast.data expression")?;
            expr_to_declarative_data(&expr)
        }
        CtNative::AstNumber => {
            let value = expect_number_literal(args.first(), "ast.number value")?;
            let span = expect_span(args.get(1), state.current_span)?;
            Ok(CtValue::Expr(Expr {
                kind: ExprKind::Number(value),
                span,
            }))
        }
        CtNative::AstString => {
            let value = expect_string(args.first(), "ast.string value")?;
            let span = expect_span(args.get(1), state.current_span)?;
            Ok(CtValue::Expr(Expr {
                kind: ExprKind::String(value),
                span,
            }))
        }
        CtNative::AstMember => {
            let base = expect_expr(args.first(), "ast.member base")?;
            let name = expect_string(args.get(1), "ast.member name")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Expr(append_chain_segment(
                base,
                ChainSegment {
                    kind: ChainSegmentKind::Member {
                        name: ident(&name, span),
                        optional: false,
                    },
                    span,
                },
                span,
            )))
        }
        CtNative::AstIndex => {
            let base = expect_expr(args.first(), "ast.index base")?;
            let index = expect_expr(args.get(1), "ast.index index")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Expr(append_chain_segment(
                base,
                ChainSegment {
                    kind: ChainSegmentKind::Index {
                        index,
                        optional: false,
                    },
                    span,
                },
                span,
            )))
        }
        CtNative::AstCall => {
            let base = expect_expr(args.first(), "ast.call base")?;
            let call_args = expect_expr_array(args.get(1), "ast.call args")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Expr(append_chain_segment(
                base,
                ChainSegment {
                    kind: ChainSegmentKind::Call {
                        args: call_args,
                        style: CallStyle::Paren,
                    },
                    span,
                },
                span,
            )))
        }
        CtNative::AstTable => {
            let fields = expect_table_field_array(args.first(), "ast.table fields")?;
            let span = expect_span(args.get(1), state.current_span)?;
            Ok(CtValue::Expr(Expr {
                kind: ExprKind::Table(TableExpr { fields }),
                span,
            }))
        }
        CtNative::AstArrayField => {
            let value = expect_expr(args.first(), "ast.arrayField value")?;
            let span = expect_span(args.get(1), state.current_span)?;
            Ok(CtValue::TableField(TableField {
                kind: TableFieldKind::Array(value),
                span,
            }))
        }
        CtNative::AstNamedField => {
            let name = expect_string(args.first(), "ast.namedField name")?;
            let value = expect_expr(args.get(1), "ast.namedField value")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::TableField(TableField {
                kind: TableFieldKind::Named {
                    name: ident(&name, span),
                    value,
                },
                span,
            }))
        }
        CtNative::AstKeyedField => {
            let key = expect_expr(args.first(), "ast.keyedField key")?;
            let value = expect_expr(args.get(1), "ast.keyedField value")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::TableField(TableField {
                kind: TableFieldKind::ExprKey { key, value },
                span,
            }))
        }
        CtNative::AstBlock => {
            let statements = expect_stmt_array(args.first(), "ast.block statements")?;
            let tail = match args.get(1) {
                Some(CtValue::Nil) | None => None,
                Some(value) => Some(expect_expr(Some(value), "ast.block tail")?),
            };
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Block(Block {
                statements,
                tail,
                span,
            }))
        }
        CtNative::AstFunc => {
            let params = expect_string_array(args.first(), "ast.func params")?
                .into_iter()
                .map(|name| Param {
                    name: ident(&name, state.current_span),
                    default: None,
                    span: state.current_span,
                })
                .collect();
            let block = expect_block(args.get(1), "ast.func body")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Expr(Expr {
                kind: ExprKind::Function(FunctionExpr {
                    params,
                    vararg: false,
                    body: FunctionBody::Block(Box::new(block)),
                    arrow_kind: ArrowKind::Normal,
                }),
                span,
            }))
        }
        CtNative::AstFnDecl => {
            let name = expect_string(args.first(), "ast.fnDecl name")?;
            let params = expect_string_array(args.get(1), "ast.fnDecl params")?;
            let body = expect_function_body(args.get(2), "ast.fnDecl body")?;
            let span = expect_span(args.get(3), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::FunctionDecl(FunctionDecl {
                    name: FunctionName::Simple(ident(&name, span)),
                    params: params
                        .into_iter()
                        .map(|name| Param {
                            name: ident(&name, span),
                            default: None,
                            span,
                        })
                        .collect(),
                    vararg: false,
                    body,
                }),
                span,
            }))
        }
        CtNative::AstExportRuntime => {
            let realm = expect_optional_realm(args.first(), "ast.exportRuntime realm")?;
            let stmt = expect_stmt(args.get(1), "ast.exportRuntime statement")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::ExportDecl {
                    kind: ExportKind::Runtime,
                    realm,
                    stmt: Box::new(stmt),
                },
                span,
            }))
        }
        CtNative::AstAssign => {
            let targets = expect_expr_array(args.first(), "ast.assign targets")?;
            let values = expect_expr_array(args.get(1), "ast.assign values")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::Assign { targets, values },
                span,
            }))
        }
        CtNative::AstLocalDecl => {
            let names = expect_string_array(args.first(), "ast.localDecl names")?
                .into_iter()
                .map(|name| ident(&name, state.current_span))
                .collect();
            let values = expect_expr_array(args.get(1), "ast.localDecl values")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::LocalDecl {
                    mode: BindingMode::Local,
                    names,
                    values,
                },
                span,
            }))
        }
        CtNative::AstConstDecl => {
            let names = expect_string_array(args.first(), "ast.constDecl names")?
                .into_iter()
                .map(|name| ident(&name, state.current_span))
                .collect();
            let values = expect_expr_array(args.get(1), "ast.constDecl values")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::LocalDecl {
                    mode: BindingMode::Const,
                    names,
                    values,
                },
                span,
            }))
        }
        CtNative::AstReturnStmt => {
            let values = expect_expr_array(args.first(), "ast.returnStmt values")?;
            let span = expect_span(args.get(1), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::Return(values),
                span,
            }))
        }
        CtNative::AstIfStmt => {
            let condition = expect_expr(args.first(), "ast.ifStmt condition")?;
            let then_statements = expect_stmt_array(args.get(1), "ast.ifStmt then statements")?;
            let else_block = match args.get(2) {
                Some(CtValue::Nil) | None => None,
                Some(value) => Some(Block {
                    statements: expect_stmt_array(Some(value), "ast.ifStmt else statements")?,
                    tail: None,
                    span: expect_span(args.get(3), state.current_span)?,
                }),
            };
            let span = expect_span(args.get(3), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::If {
                    condition,
                    then_block: Block {
                        statements: then_statements,
                        tail: None,
                        span,
                    },
                    else_block,
                },
                span,
            }))
        }
        CtNative::AstDoStmt => {
            let statements = expect_stmt_array(args.first(), "ast.doStmt statements")?;
            let span = expect_span(args.get(1), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::Do(Block {
                    statements,
                    tail: None,
                    span,
                }),
                span,
            }))
        }
        CtNative::AstDoExpr => {
            let statements = expect_stmt_array(args.first(), "ast.doExpr statements")?;
            let tail = expect_expr(args.get(1), "ast.doExpr tail")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Expr(Expr {
                kind: ExprKind::Do(Box::new(Block {
                    statements,
                    tail: Some(tail),
                    span,
                })),
                span,
            }))
        }
        CtNative::AstRealmBlock => {
            let realm_name = expect_string(args.first(), "ast.realmBlock realm")?;
            let Some(realm) = Realm::parse(&realm_name) else {
                return Err(CtError::new(format!(
                    "ast.realmBlock realm must be shared, client, or server, got `{realm_name}`"
                )));
            };
            let statements = expect_stmt_array(args.get(1), "ast.realmBlock statements")?;
            let span = expect_span(args.get(2), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::RealmBlock {
                    realm,
                    block: Block {
                        statements,
                        tail: None,
                        span,
                    },
                },
                span,
            }))
        }
        CtNative::AstExprStmt => {
            let expr = expect_expr(args.first(), "ast.exprStmt expr")?;
            let span = expect_span(args.get(1), state.current_span)?;
            Ok(CtValue::Stmt(Stmt {
                kind: StmtKind::Expr(expr),
                span,
            }))
        }
        CtNative::AstExpr => {
            let expr = expect_expr(args.first(), "ast.expr expr")?;
            Ok(CtValue::Expansion(MacroExpansion::Expr(expr)))
        }
        CtNative::AstStmts => {
            let stmts = expect_stmt_array(args.first(), "ast.stmts statements")?;
            Ok(CtValue::Expansion(MacroExpansion::Stmts(stmts)))
        }
        CtNative::IrIdent => {
            let name = expect_string(args.first(), "ir.ident name")?;
            let origin = expect_origin(args.get(1), state.current_span)?;
            Ok(CtValue::IrExpr(IrExpr {
                kind: IrExprKind::Identifier(name),
                origin,
                value_mode: ValueMode::Single,
                symbol: None,
            }))
        }
        CtNative::IrString => {
            let value = expect_string(args.first(), "ir.string value")?;
            let origin = expect_origin(args.get(1), state.current_span)?;
            Ok(CtValue::IrExpr(IrExpr {
                kind: IrExprKind::String(value),
                origin,
                value_mode: ValueMode::Single,
                symbol: None,
            }))
        }
        CtNative::IrTable => {
            let exprs = expect_ir_expr_array(args.first(), "ir.table fields")?;
            let origin = expect_origin(args.get(1), state.current_span)?;
            Ok(CtValue::IrExpr(IrExpr {
                kind: IrExprKind::Table(
                    exprs
                        .into_iter()
                        .map(|expr| {
                            let origin = expr.origin.clone();
                            IrTableField {
                                kind: IrTableFieldKind::Array(expr),
                                origin,
                            }
                        })
                        .collect(),
                ),
                origin,
                value_mode: ValueMode::Single,
                symbol: None,
            }))
        }
        CtNative::IrCall => {
            let base = expect_ir_expr(args.first(), "ir.call base")?;
            let call_args = expect_ir_expr_array(args.get(1), "ir.call args")?;
            let origin = expect_origin(args.get(2), state.current_span)?;
            Ok(CtValue::IrExpr(append_ir_call(base, call_args, origin)))
        }
        CtNative::IrChildren => {
            let children = expect_ir_expr(args.first(), "ir.children children")?;
            Ok(CtValue::IrExpr(children_array(children)))
        }
        CtNative::IrTailTableParts => {
            let expr = expect_ir_expr(args.first(), "ir.tailTableParts expr")?;
            match tail_table_parts(&expr) {
                Some((props, children)) => Ok(table_value(CtTable {
                    array: Vec::new(),
                    fields: BTreeMap::from([
                        ("props".into(), CtValue::IrExpr(props)),
                        ("children".into(), CtValue::IrExpr(children)),
                    ]),
                })),
                None => Ok(CtValue::Nil),
            }
        }
    }
}

fn ctx_value() -> CtValue {
    table_value(CtTable {
        array: Vec::new(),
        fields: BTreeMap::from([
            ("gensym".into(), CtValue::Native(CtNative::CtxGensym)),
            (
                "gensymString".into(),
                CtValue::Native(CtNative::CtxGensymString),
            ),
            ("label".into(), CtValue::Native(CtNative::CtxLabel)),
            ("error".into(), CtValue::Native(CtNative::CtxError)),
        ]),
    })
}

fn macro_call_value(call: &MacroCall) -> CtValue {
    table_value(CtTable {
        array: Vec::new(),
        fields: BTreeMap::from([
            ("source".into(), CtValue::String(call.source.clone())),
            ("imported".into(), CtValue::String(call.imported.clone())),
            (
                "position".into(),
                CtValue::String(call.position.as_str().to_string()),
            ),
            ("argc".into(), CtValue::Number(call.args.len() as f64)),
            ("span".into(), CtValue::Span(call.span)),
            (
                "args".into(),
                table_value(CtTable::array(
                    call.args.iter().cloned().map(CtValue::Expr).collect(),
                )),
            ),
        ]),
    })
}

fn host_ctx_value() -> CtValue {
    table_value(CtTable {
        array: Vec::new(),
        fields: BTreeMap::from([
            (
                "importRuntime".into(),
                CtValue::Native(CtNative::HostCtxImportRuntime),
            ),
            ("error".into(), CtValue::Native(CtNative::HostCtxError)),
        ]),
    })
}

fn host_call_value(call: &HostExprTransformCall) -> CtValue {
    table_value(CtTable {
        array: Vec::new(),
        fields: BTreeMap::from([
            ("source".into(), CtValue::String(call.source.clone())),
            ("runtime".into(), CtValue::String(call.runtime.clone())),
            ("imported".into(), CtValue::String(call.imported.clone())),
            ("local".into(), CtValue::String(call.local.clone())),
            ("expr".into(), CtValue::IrExpr(call.expr.clone())),
        ]),
    })
}

fn ast_intrinsics() -> CtValue {
    table_value(CtTable {
        array: Vec::new(),
        fields: BTreeMap::from([
            ("ident".into(), CtValue::Native(CtNative::AstIdent)),
            ("nil".into(), CtValue::Native(CtNative::AstNil)),
            ("data".into(), CtValue::Native(CtNative::AstData)),
            ("number".into(), CtValue::Native(CtNative::AstNumber)),
            ("string".into(), CtValue::Native(CtNative::AstString)),
            ("member".into(), CtValue::Native(CtNative::AstMember)),
            ("index".into(), CtValue::Native(CtNative::AstIndex)),
            ("call".into(), CtValue::Native(CtNative::AstCall)),
            ("table".into(), CtValue::Native(CtNative::AstTable)),
            (
                "arrayField".into(),
                CtValue::Native(CtNative::AstArrayField),
            ),
            (
                "namedField".into(),
                CtValue::Native(CtNative::AstNamedField),
            ),
            (
                "keyedField".into(),
                CtValue::Native(CtNative::AstKeyedField),
            ),
            ("block".into(), CtValue::Native(CtNative::AstBlock)),
            ("func".into(), CtValue::Native(CtNative::AstFunc)),
            ("fnDecl".into(), CtValue::Native(CtNative::AstFnDecl)),
            (
                "exportRuntime".into(),
                CtValue::Native(CtNative::AstExportRuntime),
            ),
            ("assign".into(), CtValue::Native(CtNative::AstAssign)),
            ("localDecl".into(), CtValue::Native(CtNative::AstLocalDecl)),
            ("constDecl".into(), CtValue::Native(CtNative::AstConstDecl)),
            (
                "returnStmt".into(),
                CtValue::Native(CtNative::AstReturnStmt),
            ),
            ("ifStmt".into(), CtValue::Native(CtNative::AstIfStmt)),
            ("doStmt".into(), CtValue::Native(CtNative::AstDoStmt)),
            ("doExpr".into(), CtValue::Native(CtNative::AstDoExpr)),
            (
                "realmBlock".into(),
                CtValue::Native(CtNative::AstRealmBlock),
            ),
            ("exprStmt".into(), CtValue::Native(CtNative::AstExprStmt)),
            ("expr".into(), CtValue::Native(CtNative::AstExpr)),
            ("stmts".into(), CtValue::Native(CtNative::AstStmts)),
        ]),
    })
}

fn ir_intrinsics() -> CtValue {
    table_value(CtTable {
        array: Vec::new(),
        fields: BTreeMap::from([
            ("ident".into(), CtValue::Native(CtNative::IrIdent)),
            ("string".into(), CtValue::Native(CtNative::IrString)),
            ("table".into(), CtValue::Native(CtNative::IrTable)),
            ("call".into(), CtValue::Native(CtNative::IrCall)),
            ("children".into(), CtValue::Native(CtNative::IrChildren)),
            (
                "tailTableParts".into(),
                CtValue::Native(CtNative::IrTailTableParts),
            ),
        ]),
    })
}

fn table_to_stmts(table: &CtTable) -> Option<Vec<Stmt>> {
    table
        .array
        .iter()
        .map(|value| match value {
            CtValue::Stmt(stmt) => Some(stmt.clone()),
            _ => None,
        })
        .collect()
}

fn compile_time_module_exports(
    packages: Arc<BTreeMap<String, Arc<CtModule>>>,
    source: &str,
) -> CtResult<CtValue> {
    let Some(module) = packages.get(source) else {
        return Err(CtError::new(format!(
            "unknown compile-time import `{source}`"
        )));
    };

    let env = CtEnv::from_module(module.clone(), packages.clone())?;
    let mut fields = BTreeMap::new();
    for export in module.exports() {
        if let Ok(value) = env.get(export) {
            fields.insert(export.to_string(), value);
        }
    }

    Ok(table_value(CtTable {
        array: Vec::new(),
        fields,
    }))
}

fn values_equal(left: &CtValue, right: &CtValue) -> bool {
    match (left, right) {
        (CtValue::Nil, CtValue::Nil) => true,
        (CtValue::Bool(left), CtValue::Bool(right)) => left == right,
        (CtValue::Number(left), CtValue::Number(right)) => (*left - *right).abs() < f64::EPSILON,
        (CtValue::String(left), CtValue::String(right)) => left == right,
        _ => false,
    }
}

fn value_to_string(value: CtValue) -> String {
    match value {
        CtValue::Nil => "nil".into(),
        CtValue::Bool(value) => value.to_string(),
        CtValue::Number(value) => value.to_string(),
        CtValue::String(value) => value,
        other => other.kind_name().into(),
    }
}

fn expr_to_declarative_data(expr: &Expr) -> CtResult<CtValue> {
    match &expr.kind {
        ExprKind::Identifier(name) => Ok(CtValue::String(name.name.clone())),
        ExprKind::Nil => Ok(CtValue::Nil),
        ExprKind::Boolean(value) => Ok(CtValue::Bool(*value)),
        ExprKind::Number(value) => Ok(CtValue::Number(value.parse::<f64>().unwrap_or(0.0))),
        ExprKind::String(value) => Ok(CtValue::String(value.clone())),
        ExprKind::Paren(inner) => expr_to_declarative_data(inner),
        ExprKind::Table(table) => {
            let mut out = CtTable::new();
            for field in &table.fields {
                match &field.kind {
                    TableFieldKind::Array(value) => {
                        out.array.push(expr_to_declarative_data(value)?);
                    }
                    TableFieldKind::Named { name, value } => {
                        out.fields
                            .insert(name.name.clone(), expr_to_declarative_data(value)?);
                    }
                    TableFieldKind::ExprKey { key, value } => {
                        let key = data_key_to_string(expr_to_declarative_data(key)?)?;
                        out.fields.insert(key, expr_to_declarative_data(value)?);
                    }
                    TableFieldKind::Spread(_) => {
                        return Err(CtError::new(
                            "ast.data does not support table spread in declarative data",
                        ));
                    }
                }
            }
            Ok(table_value(out))
        }
        other => Err(CtError::new(format!(
            "ast.data does not support expression `{other:?}`"
        ))),
    }
}

fn data_key_to_string(value: CtValue) -> CtResult<String> {
    match value {
        CtValue::Nil | CtValue::Bool(_) | CtValue::Number(_) | CtValue::String(_) => {
            Ok(value_to_string(value))
        }
        other => Err(CtError::new(format!(
            "ast.data table key must be scalar, got {}",
            other.kind_name()
        ))),
    }
}

fn expect_string(value: Option<&CtValue>, label: &str) -> CtResult<String> {
    match value {
        Some(CtValue::String(value)) => Ok(value.clone()),
        Some(other) => Err(CtError::new(format!(
            "{label} must be string, got {}",
            other.kind_name()
        ))),
        None => Err(CtError::new(format!("{label} is missing"))),
    }
}

fn expect_number_literal(value: Option<&CtValue>, label: &str) -> CtResult<String> {
    match value {
        Some(CtValue::Number(value)) if value.fract().abs() < f64::EPSILON => {
            Ok(format!("{value:.0}"))
        }
        Some(CtValue::Number(value)) => Ok(value.to_string()),
        Some(CtValue::String(value)) => Ok(value.clone()),
        Some(other) => Err(CtError::new(format!(
            "{label} must be number or numeric string, got {}",
            other.kind_name()
        ))),
        None => Err(CtError::new(format!("{label} is missing"))),
    }
}

fn expect_compile_time_number(value: CtValue, label: &str) -> CtResult<f64> {
    match value {
        CtValue::Number(value) => Ok(value),
        other => Err(CtError::new(format!(
            "{label} must be number, got {}",
            other.kind_name()
        ))),
    }
}

fn expect_span(value: Option<&CtValue>, fallback: SourceSpan) -> CtResult<SourceSpan> {
    match value {
        Some(CtValue::Span(span)) => Ok(*span),
        Some(CtValue::Nil) | None => Ok(fallback),
        Some(other) => Err(CtError::new(format!(
            "span argument must be source span, got {}",
            other.kind_name()
        ))),
    }
}

fn expect_origin(value: Option<&CtValue>, fallback: SourceSpan) -> CtResult<Origin> {
    match value {
        Some(CtValue::Span(span)) => Ok(Origin::source(*span)),
        Some(CtValue::IrExpr(expr)) => Ok(expr.origin.clone()),
        Some(CtValue::Nil) | None => Ok(Origin::source(fallback)),
        Some(other) => Err(CtError::new(format!(
            "origin argument must be source span or IR expression, got {}",
            other.kind_name()
        ))),
    }
}

fn expect_expr(value: Option<&CtValue>, label: &str) -> CtResult<Expr> {
    match value {
        Some(CtValue::Expr(expr)) => Ok(expr.clone()),
        Some(other) => Err(CtError::new(format!(
            "{label} must be AST expression, got {}",
            other.kind_name()
        ))),
        None => Err(CtError::new(format!("{label} is missing"))),
    }
}

fn expect_stmt(value: Option<&CtValue>, label: &str) -> CtResult<Stmt> {
    match value {
        Some(CtValue::Stmt(stmt)) => Ok(stmt.clone()),
        Some(other) => Err(CtError::new(format!(
            "{label} must be AST statement, got {}",
            other.kind_name()
        ))),
        None => Err(CtError::new(format!("{label} is missing"))),
    }
}

fn expect_block(value: Option<&CtValue>, label: &str) -> CtResult<Block> {
    match value {
        Some(CtValue::Block(block)) => Ok(block.clone()),
        Some(other) => Err(CtError::new(format!(
            "{label} must be AST block, got {}",
            other.kind_name()
        ))),
        None => Err(CtError::new(format!("{label} is missing"))),
    }
}

fn expect_function_body(value: Option<&CtValue>, label: &str) -> CtResult<FunctionBody> {
    match value {
        Some(CtValue::Block(block)) => Ok(FunctionBody::Block(Box::new(block.clone()))),
        Some(CtValue::Expr(expr)) => Ok(FunctionBody::Expr(Box::new(expr.clone()))),
        Some(other) => Err(CtError::new(format!(
            "{label} must be AST block or expression, got {}",
            other.kind_name()
        ))),
        None => Err(CtError::new(format!("{label} is missing"))),
    }
}

fn expect_ir_expr(value: Option<&CtValue>, label: &str) -> CtResult<IrExpr> {
    match value {
        Some(CtValue::IrExpr(expr)) => Ok(expr.clone()),
        Some(other) => Err(CtError::new(format!(
            "{label} must be IR expression, got {}",
            other.kind_name()
        ))),
        None => Err(CtError::new(format!("{label} is missing"))),
    }
}

fn expect_expr_array(value: Option<&CtValue>, label: &str) -> CtResult<Vec<Expr>> {
    let table = expect_table(value, label)?;
    table
        .array
        .iter()
        .map(|value| expect_expr(Some(value), label))
        .collect()
}

fn expect_ir_expr_array(value: Option<&CtValue>, label: &str) -> CtResult<Vec<IrExpr>> {
    let table = expect_table(value, label)?;
    table
        .array
        .iter()
        .map(|value| expect_ir_expr(Some(value), label))
        .collect()
}

fn expect_stmt_array(value: Option<&CtValue>, label: &str) -> CtResult<Vec<Stmt>> {
    let table = expect_table(value, label)?;
    table
        .array
        .iter()
        .map(|value| match value {
            CtValue::Stmt(stmt) => Ok(stmt.clone()),
            other => Err(CtError::new(format!(
                "{label} must contain AST statements, got {}",
                other.kind_name()
            ))),
        })
        .collect()
}

fn expect_table_field_array(value: Option<&CtValue>, label: &str) -> CtResult<Vec<TableField>> {
    let table = expect_table(value, label)?;
    table
        .array
        .iter()
        .map(|value| match value {
            CtValue::TableField(field) => Ok(field.clone()),
            other => Err(CtError::new(format!(
                "{label} must contain AST table fields, got {}",
                other.kind_name()
            ))),
        })
        .collect()
}

fn expect_string_array(value: Option<&CtValue>, label: &str) -> CtResult<Vec<String>> {
    let table = expect_table(value, label)?;
    table
        .array
        .iter()
        .map(|value| expect_string(Some(value), label))
        .collect()
}

fn expect_optional_realm(value: Option<&CtValue>, label: &str) -> CtResult<Option<Realm>> {
    match value {
        Some(CtValue::Nil) | None => Ok(None),
        Some(CtValue::String(value)) => Realm::parse(value)
            .map(Some)
            .ok_or_else(|| CtError::new(format!("{label} must be shared, client, or server"))),
        Some(other) => Err(CtError::new(format!(
            "{label} must be string or nil, got {}",
            other.kind_name()
        ))),
    }
}

fn expect_table(value: Option<&CtValue>, label: &str) -> CtResult<CtTable> {
    match value {
        Some(CtValue::Table(table)) => Ok(table.borrow().clone()),
        Some(other) => Err(CtError::new(format!(
            "{label} must be table, got {}",
            other.kind_name()
        ))),
        None => Err(CtError::new(format!("{label} is missing"))),
    }
}

fn ident(name: &str, span: SourceSpan) -> Identifier {
    Identifier {
        name: name.into(),
        span,
    }
}

fn ident_expr(name: &str, span: SourceSpan) -> Expr {
    Expr {
        kind: ExprKind::Identifier(ident(name, span)),
        span,
    }
}

fn append_chain_segment(mut expr: Expr, segment: ChainSegment, span: SourceSpan) -> Expr {
    match &mut expr.kind {
        ExprKind::Chain(chain) => {
            chain.segments.push(segment);
            expr.span = SourceSpan::new(expr.span.file_id, expr.span.byte_start, span.byte_end);
            expr
        }
        _ => Expr {
            kind: ExprKind::Chain(ChainExpr {
                base: Box::new(expr),
                segments: vec![segment],
            }),
            span,
        },
    }
}

fn append_ir_call(mut expr: IrExpr, args: Vec<IrExpr>, origin: Origin) -> IrExpr {
    let segment = IrChainSegment {
        kind: IrChainSegmentKind::Call {
            args,
            style: IrCallStyle::Paren,
        },
        origin: origin.clone(),
    };
    match &mut expr.kind {
        IrExprKind::Chain(chain) => {
            chain.segments.push(segment);
            expr.origin = origin;
            expr
        }
        _ => IrExpr {
            kind: IrExprKind::Chain(IrChain {
                base: Box::new(expr),
                segments: vec![segment],
            }),
            origin,
            value_mode: ValueMode::Single,
            symbol: None,
        },
    }
}

fn tail_table_parts(expr: &IrExpr) -> Option<(IrExpr, IrExpr)> {
    let IrExprKind::Chain(chain) = &expr.kind else {
        return None;
    };

    match chain.segments.as_slice() {
        [first] => {
            let props = single_props_tail_arg(first)?;
            let children = IrExpr {
                kind: IrExprKind::Table(Vec::new()),
                origin: expr.origin.clone(),
                value_mode: ValueMode::Single,
                symbol: None,
            };
            Some((props, children))
        }
        [first, second] => {
            let props = single_props_tail_arg(first)?;
            let children = children_array(single_table_tail_arg(second)?);
            Some((props, children))
        }
        _ => None,
    }
}

fn single_table_tail_arg(segment: &IrChainSegment) -> Option<IrExpr> {
    let IrChainSegmentKind::Call { args, style } = &segment.kind else {
        return None;
    };
    if *style != IrCallStyle::TailTable || args.len() != 1 {
        return None;
    }
    let arg = args[0].clone();
    if matches!(arg.kind, IrExprKind::Table(_)) {
        Some(arg)
    } else {
        None
    }
}

fn single_props_tail_arg(segment: &IrChainSegment) -> Option<IrExpr> {
    let props = single_table_tail_arg(segment)?;
    let IrExprKind::Table(fields) = &props.kind else {
        return None;
    };
    if fields
        .iter()
        .any(|field| matches!(field.kind, IrTableFieldKind::Array(_)))
    {
        return None;
    }
    Some(props)
}

fn children_array(children: IrExpr) -> IrExpr {
    let origin = children.origin.clone();
    match children.kind {
        IrExprKind::Table(fields) => IrExpr {
            kind: IrExprKind::Table(fields),
            origin,
            value_mode: ValueMode::Single,
            symbol: None,
        },
        _ => IrExpr {
            kind: IrExprKind::Table(vec![IrTableField {
                kind: IrTableFieldKind::Array(children),
                origin: origin.clone(),
            }]),
            origin,
            value_mode: ValueMode::Single,
            symbol: None,
        },
    }
}

fn discover_compile_time_packages(
    root: &Path,
    out: &mut BTreeMap<String, Arc<CtModule>>,
) -> Result<(), CompileTimeError> {
    for phase in discover_compile_time_phases(root).map_err(CompileTimeError::Package)? {
        let id = phase.package_id;
        let path = phase.source_path;
        let file = SourceFile::load((out.len() + 20_000) as u32, &path).map_err(|source| {
            CompileTimeError::Io {
                path: path.clone(),
                source,
            }
        })?;
        let module = CtModule::parse(id.clone(), file)?;
        if let Some(existing) = out.get(&id) {
            return Err(CompileTimeError::DuplicatePackage {
                id,
                first: existing.file.path.clone().unwrap_or_default(),
                second: path,
            });
        }
        out.insert(id, Arc::new(module));
    }
    Ok(())
}

#[cfg(test)]
#[path = "compile_time/tests.rs"]
mod tests;
