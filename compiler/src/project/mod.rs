use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

mod manifest;

use crate::ast::{
    ArrowKind, BindingMode, Block, ChainExpr, ChainSegment, ChainSegmentKind, EnumDecl, EnumRepr,
    EnumVariant, Expr, ExprKind, ExprOrBlock, FunctionBody, FunctionDecl, FunctionExpr,
    FunctionName, ImportPhase, MatchArm, MatchExpr, Module, Realm, Stmt, StmtKind, TableExpr,
    TableField, TableFieldKind, TemplatePart, TemplatePartKind,
};
use crate::codegen::{CodegenError, LuaCodegen, LuaOutput};
use crate::compile_time::{CompileTimeError, CompileTimePackageRegistry};
use crate::diag::{Diagnostic, DiagnosticEmitter, Label};
use crate::gmod::{GmodBackendConfig, GmodBuildPlan, GmodModule, GmodPathError};
use crate::host::HostRegistry;
use crate::ir::{IrBlock, IrFunctionBody, IrModule, IrStmtKind};
use crate::lex::Lexer;
use crate::lower::{LowerError, Lowerer};
use crate::macro_expansion::{MacroRegistry, expand_macros_with_registry};
use crate::module::{
    ArtifactRealm, ModuleExport, ModuleGraph, ModuleGraphConfig, ModuleId, ModuleImport,
    ModuleImportSpecifier, PackageId, RealmSet, normalize_module_path,
    normalize_relative_module_path,
};
use crate::parse::Parser;
use crate::part_order::{PartOrderInput, is_module_entry_path, sort_module_parts};
use crate::resolve::{ResolveOutput, ResolvePart, Resolver, ResolverOptions};
use crate::runtime::{RuntimePackageError, RuntimePackageRegistry};
use crate::source::{SourceFile, SourceSpan};
use crate::sourcemap::{
    SourceCommentMode, SourceMap, map_after_source_comments, with_source_comments,
};

pub use manifest::{ManifestError, ProjectManifest};

#[derive(Debug, Clone)]
pub struct ProjectConfig {
    pub source_root: PathBuf,
    pub package_id: Option<PackageId>,
    pub package_roots: Vec<PathBuf>,
    pub resolver_options: ResolverOptions,
}

impl ProjectConfig {
    pub fn new(source_root: impl Into<PathBuf>) -> Self {
        Self {
            source_root: source_root.into(),
            package_id: None,
            package_roots: Vec::new(),
            resolver_options: ResolverOptions::gmod_default(),
        }
    }

    pub fn with_package_id(mut self, package_id: impl Into<String>) -> Self {
        self.package_id = Some(PackageId::new(package_id.into()));
        self
    }

    pub fn with_package_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.package_roots = roots;
        self
    }

    pub fn with_resolver_options(mut self, options: ResolverOptions) -> Self {
        self.resolver_options = options;
        self
    }

    fn effective_package_id(&self) -> PackageId {
        self.package_id.clone().unwrap_or_else(|| {
            let name = self
                .source_root
                .parent()
                .and_then(Path::file_name)
                .or_else(|| self.source_root.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "app".into());
            PackageId::from_dir_name(&name)
        })
    }
}

#[derive(Debug, Clone)]
pub struct CompileProjectOutput {
    pub graph: ModuleGraph,
    pub modules: Vec<CompiledModule>,
    pub runtime_externals: Vec<RuntimeExternal>,
    pub diagnostics: Vec<RenderedDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct CompiledModule {
    pub id: ModuleId,
    pub artifact_id: String,
    pub module_path: String,
    pub artifact_realm: ArtifactRealm,
    pub source_files: Vec<SourceFile>,
    pub ir: IrModule,
    pub lua: LuaOutput,
    pub imports: Vec<ModuleImport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RuntimeExternal {
    pub id: ModuleId,
    pub artifact_realm: ArtifactRealm,
}

#[derive(Debug, Clone)]
pub struct GmodProjectOutput {
    pub build_plan: GmodBuildPlan,
    pub artifacts: Vec<GmodCompiledArtifact>,
    pub diagnostics: Vec<RenderedDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct GmodCompiledArtifact {
    pub module_id: ModuleId,
    pub artifact_id: String,
    pub source_path: PathBuf,
    pub source_files: Vec<SourceFile>,
    pub lua_path: PathBuf,
    pub map_path: PathBuf,
    pub lua: String,
    pub source_map: SourceMap,
}

pub(crate) struct ArtifactModulePart<'a> {
    pub module: &'a Module,
    pub default_realm: Realm,
}

#[derive(Debug, Clone)]
pub struct GmodBuildOptions {
    pub source_root: PathBuf,
    pub output_root: PathBuf,
    pub runtime_base: Option<PathBuf>,
    pub autorun: bool,
    pub bundle_id: Option<String>,
    pub package_id: Option<PackageId>,
    pub package_roots: Vec<PathBuf>,
    pub write_files: bool,
    pub source_comments: SourceCommentMode,
    pub resolver_options: ResolverOptions,
}

impl GmodBuildOptions {
    pub fn new(source_root: impl Into<PathBuf>, output_root: impl Into<PathBuf>) -> Self {
        Self {
            source_root: source_root.into(),
            output_root: output_root.into(),
            runtime_base: None,
            autorun: true,
            bundle_id: None,
            package_id: None,
            package_roots: Vec::new(),
            write_files: false,
            source_comments: SourceCommentMode::Readable,
            resolver_options: ResolverOptions::gmod_default(),
        }
    }

    pub fn from_manifest(manifest: ProjectManifest) -> Self {
        let mut options = Self::new(manifest.source_root, manifest.output_root);
        if let Some(source_comments) = manifest.source_comments {
            options.source_comments = source_comments;
        }
        if let Some(runtime_base) = manifest.runtime_base {
            options.runtime_base = Some(runtime_base);
        }
        if let Some(autorun) = manifest.autorun {
            options.autorun = autorun;
        }
        options.package_roots = manifest.package_roots;
        options.package_id = manifest.package_id.map(PackageId::new);
        options.bundle_id = manifest.bundle_id;
        let mut resolver_options = ResolverOptions::gmod_default();
        if let Some(policy) = manifest.gmod_unknown_external {
            resolver_options = resolver_options.with_unknown_external(policy);
        }
        resolver_options = resolver_options.with_externs(manifest.gmod_externs);
        options.resolver_options = resolver_options;
        options
    }
}

#[derive(Debug)]
pub enum ProjectError {
    Io { path: PathBuf, source: io::Error },
    Diagnostics(Vec<RenderedDiagnostic>),
    HostDiagnostics(Vec<RenderedDiagnostic>),
    Runtime(RuntimePackageError),
    CompileTime(CompileTimeError),
    GmodPath(GmodPathError),
    Lower { path: PathBuf, source: LowerError },
    Codegen { path: PathBuf, source: CodegenError },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedDiagnostic {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to read or write {}: {source}", path.display())
            }
            Self::Diagnostics(diagnostics) | Self::HostDiagnostics(diagnostics) => {
                for (index, diagnostic) in diagnostics.iter().enumerate() {
                    if index > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{}", diagnostic.message)?;
                }
                Ok(())
            }
            Self::Runtime(source) => write!(f, "{source}"),
            Self::CompileTime(source) => write!(f, "{source}"),
            Self::GmodPath(source) => write!(f, "{source}"),
            Self::Lower { path, source } => {
                write!(f, "lowering failed for {}: {source}", path.display())
            }
            Self::Codegen { path, source } => {
                write!(f, "codegen failed for {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ProjectError {}

#[derive(Debug, Clone)]
struct ParsedPart {
    path: PathBuf,
    default_realm: Realm,
    module: Module,
    source_file: SourceFile,
}

#[derive(Debug, Clone)]
struct ParsedProjectModule {
    id: ModuleId,
    package_id: PackageId,
    module_path: String,
    parts: Vec<ParsedPart>,
    resolved: ResolveOutput,
}

pub fn compile_project(config: &ProjectConfig) -> Result<CompileProjectOutput, ProjectError> {
    let paths = discover_lux_sources(&config.source_root)?;
    compile_paths(config, &paths)
}

pub fn compile_paths(
    config: &ProjectConfig,
    paths: &[PathBuf],
) -> Result<CompileProjectOutput, ProjectError> {
    let package_id = config.effective_package_id();
    let runtime_registry =
        RuntimePackageRegistry::load_default_with_package_roots(&config.package_roots)
            .map_err(ProjectError::Runtime)?;
    let compile_time_registry =
        CompileTimePackageRegistry::load_default_with_package_roots(&config.package_roots)
            .map_err(ProjectError::CompileTime)?;
    let mut macro_registry = MacroRegistry::empty();
    compile_time_registry
        .register_macros(&mut macro_registry)
        .map_err(ProjectError::CompileTime)?;
    let host_registry = HostRegistry::from_specs(
        compile_time_registry
            .host_transform_specs()
            .map_err(ProjectError::CompileTime)?,
    );

    let mut parsed_modules =
        parse_project_modules(config, package_id.clone(), paths, &macro_registry)?;
    let diagnostics = resolve_project_modules(&mut parsed_modules, &config.resolver_options)?;

    let inputs = parsed_modules
        .values()
        .map(|module| module_input(module))
        .collect::<Vec<_>>();
    let graph_config = ModuleGraphConfig {
        external_modules: runtime_registry.package_ids(),
        external_exports: runtime_registry.export_metadata(),
    };
    let graph = ModuleGraph::build_with_config(inputs, graph_config).map_err(|diagnostics| {
        ProjectError::Diagnostics(render_module_diagnostics(&parsed_modules, &diagnostics))
    })?;

    let edge_link_map = graph
        .edges
        .iter()
        .map(|edge| {
            (
                (edge.from.clone(), edge.raw_source.clone()),
                edge.to.id().clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let external_ids = graph
        .required_externals
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut modules_by_artifact = BTreeMap::<String, CompiledModule>::new();
    let mut runtime_externals = BTreeSet::<RuntimeExternal>::new();
    for module_id in &graph.order {
        let Some(module) = parsed_modules.get(module_id) else {
            continue;
        };
        for artifact_realm in module_artifact_realms(module) {
            let synthetic = build_artifact_module(module, artifact_realm);
            let mut ir = Lowerer::lower_for_artifact(&synthetic, &module.resolved, artifact_realm)
                .map_err(|source| ProjectError::Lower {
                    path: module
                        .parts
                        .first()
                        .map(|part| part.path.clone())
                        .unwrap_or_default(),
                    source,
                })?;
            filter_artifact_exports(&mut ir, &module.resolved, artifact_realm);
            let transformed = host_registry.transform_module(ir, &module.resolved);
            if transformed.has_errors() {
                return Err(ProjectError::HostDiagnostics(render_diagnostics_by_file(
                    module_source_files(module),
                    &transformed.diagnostics,
                )));
            }
            let mut ir = transformed.module;
            rewrite_import_sources(
                &mut ir,
                &module.id,
                artifact_realm,
                &edge_link_map,
                &external_ids,
            );
            collect_runtime_external_imports(
                &ir,
                artifact_realm,
                &external_ids,
                &mut runtime_externals,
            );
            let lua = LuaCodegen::generate(&ir).map_err(|source| ProjectError::Codegen {
                path: module
                    .parts
                    .first()
                    .map(|part| part.path.clone())
                    .unwrap_or_default(),
                source,
            })?;
            let artifact_id = module.id.artifact_id(artifact_realm);
            modules_by_artifact.insert(
                artifact_id.clone(),
                CompiledModule {
                    id: module.id.clone(),
                    artifact_id,
                    module_path: module.module_path.clone(),
                    artifact_realm,
                    source_files: module
                        .parts
                        .iter()
                        .map(|part| part.source_file.clone())
                        .collect(),
                    ir,
                    lua,
                    imports: module_input(module).imports,
                },
            );
        }
    }

    let modules = modules_by_artifact.into_values().collect::<Vec<_>>();
    let runtime_externals = runtime_external_closure(&runtime_registry, runtime_externals)
        .map_err(ProjectError::Runtime)?;

    Ok(CompileProjectOutput {
        graph,
        modules,
        runtime_externals,
        diagnostics,
    })
}

pub fn build_gmod_project(options: &GmodBuildOptions) -> Result<GmodProjectOutput, ProjectError> {
    let project_config = ProjectConfig {
        source_root: options.source_root.clone(),
        package_id: options.package_id.clone(),
        package_roots: options.package_roots.clone(),
        resolver_options: options.resolver_options.clone(),
    };
    let package_id = project_config.effective_package_id();
    let project = compile_project(&project_config)?;
    let runtime_registry =
        RuntimePackageRegistry::load_default_with_package_roots(&options.package_roots)
            .map_err(ProjectError::Runtime)?;
    let mut backend_config = GmodBackendConfig::new(&options.source_root, &options.output_root);
    backend_config.source_comments = options.source_comments;
    backend_config.autorun = options.autorun;
    let bundle_id = options
        .bundle_id
        .clone()
        .unwrap_or_else(|| package_id.as_str().to_string());
    backend_config.set_bundle_id(bundle_id);
    if let Some(runtime_base) = &options.runtime_base {
        backend_config
            .set_runtime_base(runtime_base)
            .map_err(ProjectError::GmodPath)?;
    }
    let mut plan = GmodBuildPlan::from_config(backend_config);
    let mut artifacts = Vec::new();

    for external in &project.runtime_externals {
        if let Some(runtime) =
            runtime_module_artifact(external, &plan.config, &plan.registry, &runtime_registry)?
        {
            plan.modules.push(GmodModule {
                module_id: runtime.artifact_id.clone(),
                lux_path: runtime.source_path.clone(),
                lua_path: runtime.lua_path.clone(),
                realm: to_gmod_realm(external.artifact_realm),
            });
            artifacts.push(runtime);
        }
    }

    for module in &project.modules {
        let lua_path = gmod_lua_path(&plan.config, &module.id, module.artifact_realm);
        let (module_lua, module_map) = apply_source_comments(
            &module.lua,
            &module.source_files,
            plan.config.source_comments,
        );
        let wrapped = plan.registry.wrap_module_lua(&module_lua);
        let source_map = module_map.shifted(1, 2);
        let map_path = lua_path.with_extension("lua.map.json");

        plan.modules.push(GmodModule {
            module_id: module.artifact_id.clone(),
            lux_path: module
                .source_files
                .first()
                .and_then(|file| file.path.clone())
                .unwrap_or_default(),
            lua_path: lua_path.clone(),
            realm: to_gmod_realm(module.artifact_realm),
        });

        artifacts.push(GmodCompiledArtifact {
            module_id: module.id.clone(),
            artifact_id: module.artifact_id.clone(),
            source_path: module
                .source_files
                .first()
                .and_then(|file| file.path.clone())
                .unwrap_or_default(),
            source_files: module.source_files.clone(),
            lua_path,
            map_path,
            lua: wrapped,
            source_map,
        });
    }

    plan.rebuild_loader();

    if options.write_files {
        write_gmod_artifacts(&plan, &artifacts)?;
    }

    Ok(GmodProjectOutput {
        build_plan: plan,
        artifacts,
        diagnostics: project.diagnostics,
    })
}

fn parse_project_modules(
    config: &ProjectConfig,
    package_id: PackageId,
    paths: &[PathBuf],
    macro_registry: &MacroRegistry,
) -> Result<BTreeMap<ModuleId, ParsedProjectModule>, ProjectError> {
    let mut modules = BTreeMap::<ModuleId, ParsedProjectModule>::new();
    let mut diagnostics = Vec::new();

    for (index, path) in paths.iter().enumerate() {
        let file = SourceFile::load(index as u32, path).map_err(|source| ProjectError::Io {
            path: path.clone(),
            source,
        })?;
        let module_path = infer_module_path(&config.source_root, path);
        let module_id = ModuleId::from_package_path(&package_id, &module_path);
        let default_realm = match infer_part_realm(&config.source_root, path, file.id) {
            Ok(realm) => realm,
            Err(diagnostic) => {
                diagnostics.extend(render_diagnostics(path, &file, &[diagnostic]));
                continue;
            }
        };

        let lex = Lexer::new(&file).lex_all();
        if lex.has_errors() {
            diagnostics.extend(render_diagnostics(path, &file, &lex.diagnostics));
            continue;
        }

        let parsed = Parser::new(&lex.tokens).parse_module();
        if parsed.has_errors() {
            diagnostics.extend(render_diagnostics(path, &file, &parsed.diagnostics));
            continue;
        }

        let expanded = expand_macros_with_registry(&file, &parsed.module, macro_registry);
        if expanded.has_errors() {
            diagnostics.extend(render_diagnostics(path, &file, &expanded.diagnostics));
            continue;
        }

        modules
            .entry(module_id.clone())
            .or_insert_with(|| ParsedProjectModule {
                id: module_id.clone(),
                package_id: package_id.clone(),
                module_path: module_path.clone(),
                parts: Vec::new(),
                resolved: Resolver::resolve(&Module {
                    body: Vec::new(),
                    span: expanded.module.span,
                }),
            })
            .parts
            .push(ParsedPart {
                path: path.clone(),
                default_realm,
                module: expanded.module,
                source_file: file,
            });
    }

    if !diagnostics.is_empty() {
        return Err(ProjectError::Diagnostics(diagnostics));
    }

    for module in modules.values_mut() {
        module.parts.sort_by(|a, b| a.path.cmp(&b.path));
        let inputs = module
            .parts
            .iter()
            .map(|part| PartOrderInput {
                path: &part.path,
                module: &part.module,
                is_entry: is_module_entry_path(&part.path),
            })
            .collect::<Vec<_>>();
        match sort_module_parts(&inputs) {
            Ok(order) => {
                module.parts = order
                    .into_iter()
                    .map(|index| module.parts[index].clone())
                    .collect();
            }
            Err(part_order_diagnostics) => {
                diagnostics.extend(render_diagnostics_by_file(
                    module_source_files(module),
                    &part_order_diagnostics,
                ));
            }
        }
    }

    if !diagnostics.is_empty() {
        return Err(ProjectError::Diagnostics(diagnostics));
    }

    Ok(modules)
}

fn resolve_project_modules(
    modules: &mut BTreeMap<ModuleId, ParsedProjectModule>,
    resolver_options: &ResolverOptions,
) -> Result<Vec<RenderedDiagnostic>, ProjectError> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for module in modules.values_mut() {
        let parts = module
            .parts
            .iter()
            .map(|part| ResolvePart {
                module: &part.module,
                default_realm: part.default_realm,
            })
            .collect::<Vec<_>>();
        let resolved = Resolver::resolve_parts_with_options(&parts, resolver_options.clone());
        if resolved.has_errors() {
            errors.extend(render_diagnostics_by_file(
                module_source_files(module),
                &resolved
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.severity == crate::diag::Severity::Error)
                    .cloned()
                    .collect::<Vec<_>>(),
            ));
        }
        warnings.extend(render_diagnostics_by_file(
            module_source_files(module),
            &resolved
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity != crate::diag::Severity::Error)
                .cloned()
                .collect::<Vec<_>>(),
        ));
        module.resolved = resolved;
    }
    if errors.is_empty() {
        Ok(warnings)
    } else {
        errors.extend(warnings);
        Err(ProjectError::Diagnostics(errors))
    }
}

fn module_input(module: &ParsedProjectModule) -> crate::module::ModuleInput {
    crate::module::ModuleInput::new(
        module.package_id.clone(),
        module.module_path.clone(),
        module
            .parts
            .first()
            .map(|part| part.module.span)
            .unwrap_or(SourceSpan::new(crate::source::FileId(0), 0, 0)),
        module_exports(module),
        module_imports(module),
    )
}

fn module_exports(module: &ParsedProjectModule) -> Vec<ModuleExport> {
    module
        .resolved
        .exports
        .iter()
        .filter_map(|export| {
            let binding = module.resolved.bindings.get(export.binding.0)?;
            let realms = export
                .realm
                .map(RealmSet::from_realm)
                .unwrap_or(binding.available_realms);
            Some(ModuleExport {
                name: export.name.clone(),
                realms,
                span: export.span,
            })
        })
        .collect()
}

fn module_imports(module: &ParsedProjectModule) -> Vec<ModuleImport> {
    module
        .resolved
        .module_edges
        .iter()
        .filter(|edge| {
            edge.source.starts_with('@') || edge.source.starts_with('.') || !edge.source.is_empty()
        })
        .map(|edge| {
            let target =
                resolve_import_target(&module.package_id, &module.module_path, &edge.source)
                    .unwrap_or_else(|| ModuleId::new("<error>"));
            let active_realms = if edge.side_effect_only {
                RealmSet::SHARED
            } else {
                RealmSet::NONE
            };
            ModuleImport {
                raw_source: edge.source.clone(),
                target,
                specifiers: edge
                    .specifiers
                    .iter()
                    .map(|specifier| ModuleImportSpecifier {
                        imported: specifier.imported.clone(),
                        local: specifier.local.clone(),
                        namespace: specifier.namespace,
                        active_realms: specifier.active_realms,
                        span: specifier.span,
                    })
                    .collect(),
                side_effect_only: edge.side_effect_only,
                active_realms,
                span: edge.span,
            }
        })
        .collect()
}

pub(crate) fn resolve_import_target(
    package_id: &PackageId,
    module_path: &str,
    raw: &str,
) -> Option<ModuleId> {
    if let Some(external) = raw.strip_prefix('@') {
        return Some(ModuleId::new(external));
    }
    if raw.starts_with("./") || raw.starts_with("../") {
        let path = normalize_relative_module_path(module_path, raw).ok()?;
        return Some(ModuleId::from_package_path(package_id, &path));
    }
    Some(ModuleId::from_package_path(package_id, raw))
}

fn module_artifact_realms(module: &ParsedProjectModule) -> Vec<ArtifactRealm> {
    let mut realms = RealmSet::NONE;
    for binding in &module.resolved.bindings {
        if binding.module_scope {
            realms = realms.union(binding.available_realms);
        }
    }
    for part in &module.parts {
        realms = realms.union(part_runtime_realms(part));
    }
    realms.artifact_realms()
}

fn part_runtime_realms(part: &ParsedPart) -> RealmSet {
    let default_realms = RealmSet::from_realm(part.default_realm);
    part.module
        .body
        .iter()
        .fold(RealmSet::NONE, |realms, stmt| {
            realms.union(stmt_runtime_realms(stmt, default_realms))
        })
}

fn stmt_runtime_realms(stmt: &Stmt, active_realms: RealmSet) -> RealmSet {
    match &stmt.kind {
        StmtKind::PartOrderDecl(_)
        | StmtKind::ExternDecl(_)
        | StmtKind::HostPackageDecl(_)
        | StmtKind::ExportList { .. }
        | StmtKind::ExportAll { .. } => RealmSet::NONE,
        StmtKind::RealmDecl { realm, stmt } => stmt_runtime_realms(
            stmt,
            active_realms.intersection(RealmSet::from_realm(*realm)),
        ),
        StmtKind::RealmBlock { realm, block } => block_runtime_realms(
            block,
            active_realms.intersection(RealmSet::from_realm(*realm)),
        ),
        StmtKind::InitDecl { realm, block } => {
            let realms = realm
                .map(|realm| active_realms.intersection(RealmSet::from_realm(realm)))
                .unwrap_or(active_realms);
            block_runtime_realms(block, realms)
        }
        StmtKind::ExportDecl { realm, stmt, .. } => {
            let realms = realm
                .map(|realm| active_realms.intersection(RealmSet::from_realm(realm)))
                .unwrap_or(active_realms);
            stmt_runtime_realms(stmt, realms)
        }
        _ => active_realms,
    }
}

fn block_runtime_realms(block: &Block, active_realms: RealmSet) -> RealmSet {
    if block.statements.is_empty() && block.tail.is_none() {
        return RealmSet::NONE;
    }
    active_realms
}

pub(crate) fn build_artifact_module_from_parts(
    parts: &[ArtifactModulePart<'_>],
    resolved: &ResolveOutput,
    artifact_realm: ArtifactRealm,
) -> Module {
    let artifact_set = RealmSet::from_artifact(artifact_realm);
    let mut body = Vec::new();
    body.extend(module_predecls_from_resolved(resolved, artifact_set));
    let import_realms = import_realms_by_resolved(resolved);

    for part in parts {
        let part_set = RealmSet::from_realm(part.default_realm);
        let mut statements = Vec::new();
        for stmt in &part.module.body {
            transform_top_level_stmt(
                stmt,
                part_set,
                artifact_set,
                resolved,
                &import_realms,
                &mut statements,
            );
        }
        if !statements.is_empty() {
            body.push(Stmt {
                span: part.module.span,
                kind: StmtKind::Do(Block {
                    statements,
                    tail: None,
                    span: part.module.span,
                }),
            });
        }
    }

    let span = parts
        .first()
        .map(|part| part.module.span)
        .unwrap_or(SourceSpan::new(crate::source::FileId(0), 0, 0));
    Module { body, span }
}

fn build_artifact_module(module: &ParsedProjectModule, artifact_realm: ArtifactRealm) -> Module {
    let parts = module
        .parts
        .iter()
        .map(|part| ArtifactModulePart {
            module: &part.module,
            default_realm: part.default_realm,
        })
        .collect::<Vec<_>>();
    build_artifact_module_from_parts(&parts, &module.resolved, artifact_realm)
}

fn module_predecls_from_resolved(resolved: &ResolveOutput, artifact_set: RealmSet) -> Vec<Stmt> {
    resolved
        .bindings
        .iter()
        .filter(|binding| {
            binding.module_scope
                && !matches!(
                    binding.kind,
                    crate::resolve::BindingKind::Import | crate::resolve::BindingKind::MacroImport
                )
                && binding.available_realms.intersects(artifact_set)
        })
        .map(|binding| Stmt {
            span: binding.span,
            kind: StmtKind::LocalDecl {
                mode: BindingMode::Local,
                names: vec![crate::ast::Identifier {
                    name: binding.name.clone(),
                    span: binding.span,
                }],
                values: Vec::new(),
            },
        })
        .collect()
}

fn import_realms_by_resolved(resolved: &ResolveOutput) -> BTreeMap<SourceSpan, RealmSet> {
    let mut map = BTreeMap::<SourceSpan, RealmSet>::new();
    for edge in &resolved.module_edges {
        let mut realms = if edge.side_effect_only {
            RealmSet::SHARED
        } else {
            RealmSet::NONE
        };
        for specifier in &edge.specifiers {
            realms = realms.union(specifier.active_realms);
        }
        map.insert(edge.span, realms);
    }
    map
}

fn transform_top_level_stmt(
    stmt: &Stmt,
    current_realms: RealmSet,
    artifact_set: RealmSet,
    resolved: &ResolveOutput,
    import_realms: &BTreeMap<SourceSpan, RealmSet>,
    out: &mut Vec<Stmt>,
) {
    match &stmt.kind {
        StmtKind::Import(import) => {
            if import.phase == ImportPhase::Runtime
                && import_realms
                    .get(&stmt.span)
                    .copied()
                    .unwrap_or(current_realms)
                    .intersects(artifact_set)
            {
                out.push(stmt.clone());
            }
        }
        StmtKind::ExportDecl { stmt: inner, .. } | StmtKind::RealmDecl { stmt: inner, .. } => {
            let realms = match &stmt.kind {
                StmtKind::RealmDecl { realm, .. } => RealmSet::from_realm(*realm),
                StmtKind::ExportDecl {
                    realm: Some(realm), ..
                } => RealmSet::from_realm(*realm),
                _ => current_realms,
            };
            if realms.intersects(artifact_set) {
                transform_top_level_stmt(inner, realms, artifact_set, resolved, import_realms, out);
            }
        }
        StmtKind::FunctionDecl(decl) => {
            if function_decl_realms(decl, resolved, current_realms).intersects(artifact_set) {
                out.push(function_decl_assignment(decl, current_realms, artifact_set));
            }
        }
        StmtKind::EnumDecl(decl) if decl.runtime => {
            if enum_decl_realms(decl, resolved, current_realms).intersects(artifact_set) {
                out.push(runtime_enum_metadata_decl(
                    decl,
                    current_realms,
                    artifact_set,
                ));
                out.push(runtime_enum_assignment(decl, current_realms, artifact_set));
            }
        }
        StmtKind::LocalDecl { names, values, .. } => {
            let active_names = names
                .iter()
                .filter(|name| {
                    resolved
                        .binding_by_name(&name.name)
                        .is_some_and(|binding| binding.available_realms.intersects(artifact_set))
                })
                .cloned()
                .collect::<Vec<_>>();
            if !active_names.is_empty() && !values.is_empty() {
                out.push(Stmt {
                    kind: StmtKind::Assign {
                        targets: active_names
                            .iter()
                            .map(|name| Expr {
                                kind: ExprKind::Identifier(name.clone()),
                                span: name.span,
                            })
                            .collect(),
                        values: values
                            .iter()
                            .map(|value| transform_expr(value, current_realms, artifact_set))
                            .collect(),
                    },
                    span: stmt.span,
                });
            }
        }
        StmtKind::RealmBlock { realm, block } => {
            let realms = current_realms.intersection(RealmSet::from_realm(*realm));
            if realms.intersects(artifact_set) {
                out.push(Stmt {
                    kind: StmtKind::Do(transform_block(block, realms, artifact_set)),
                    span: stmt.span,
                });
            }
        }
        StmtKind::InitDecl { realm, block } => {
            let realms = realm
                .map(|realm| current_realms.intersection(RealmSet::from_realm(realm)))
                .unwrap_or(current_realms);
            if realms.intersects(artifact_set) {
                out.push(Stmt {
                    kind: StmtKind::Do(transform_block(block, realms, artifact_set)),
                    span: stmt.span,
                });
            }
        }
        StmtKind::ExternDecl(_)
        | StmtKind::PartOrderDecl(_)
        | StmtKind::HostPackageDecl(_)
        | StmtKind::ExportList { .. }
        | StmtKind::ExportAll { .. } => {}
        StmtKind::LocalDestructure { .. } => {
            if let Some(stmt) = transform_stmt(stmt, current_realms, artifact_set) {
                out.push(stmt);
            }
        }
        _ => {
            if let Some(stmt) = transform_stmt(stmt, current_realms, artifact_set) {
                out.push(stmt);
            }
        }
    }
}

fn function_decl_realms(
    decl: &FunctionDecl,
    resolved: &ResolveOutput,
    fallback: RealmSet,
) -> RealmSet {
    if let FunctionName::Simple(name) = &decl.name {
        return resolved
            .binding_by_name(&name.name)
            .map(|binding| binding.available_realms)
            .unwrap_or(fallback);
    }
    fallback
}

fn enum_decl_realms(decl: &EnumDecl, resolved: &ResolveOutput, fallback: RealmSet) -> RealmSet {
    resolved
        .binding_by_name(&decl.name.name)
        .map(|binding| binding.available_realms)
        .unwrap_or(fallback)
}

fn runtime_enum_metadata_decl(
    decl: &EnumDecl,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> Stmt {
    let mut metadata = transform_enum_decl(decl, current_realms, artifact_set);
    metadata.runtime = false;
    Stmt {
        kind: StmtKind::EnumDecl(metadata),
        span: decl.name.span,
    }
}

fn runtime_enum_assignment(
    decl: &EnumDecl,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> Stmt {
    Stmt {
        span: decl.name.span,
        kind: StmtKind::Assign {
            targets: vec![Expr {
                kind: ExprKind::Identifier(decl.name.clone()),
                span: decl.name.span,
            }],
            values: vec![runtime_enum_table_expr(decl, current_realms, artifact_set)],
        },
    }
}

fn runtime_enum_table_expr(
    decl: &EnumDecl,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> Expr {
    let fields = decl
        .variants
        .iter()
        .enumerate()
        .map(|(index, variant)| TableField {
            span: variant.span,
            kind: TableFieldKind::Named {
                name: variant.name.clone(),
                value: runtime_enum_variant_tag_expr(
                    decl,
                    variant,
                    index,
                    current_realms,
                    artifact_set,
                ),
            },
        })
        .collect();
    Expr {
        span: decl.name.span,
        kind: ExprKind::Table(TableExpr { fields }),
    }
}

fn runtime_enum_variant_tag_expr(
    decl: &EnumDecl,
    variant: &EnumVariant,
    index: usize,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> Expr {
    if let Some(tag) = &variant.tag {
        return transform_expr(tag, current_realms, artifact_set);
    }
    let kind = match &decl.repr {
        EnumRepr::Number => ExprKind::Number(index.to_string()),
        EnumRepr::String | EnumRepr::Table { .. } | EnumRepr::Existing { .. } => {
            ExprKind::String(variant.name.name.clone())
        }
    };
    Expr {
        kind,
        span: variant.span,
    }
}

fn function_decl_assignment(
    decl: &FunctionDecl,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> Stmt {
    let FunctionName::Simple(name) = &decl.name else {
        return Stmt {
            kind: StmtKind::FunctionDecl(transform_function_decl(
                decl,
                current_realms,
                artifact_set,
            )),
            span: SourceSpan::new(crate::source::FileId(0), 0, 0),
        };
    };
    Stmt {
        span: name.span,
        kind: StmtKind::Assign {
            targets: vec![Expr {
                kind: ExprKind::Identifier(name.clone()),
                span: name.span,
            }],
            values: vec![Expr {
                span: name.span,
                kind: ExprKind::Function(FunctionExpr {
                    params: decl.params.clone(),
                    vararg: decl.vararg,
                    body: transform_function_body(&decl.body, current_realms, artifact_set),
                    arrow_kind: ArrowKind::Normal,
                }),
            }],
        },
    }
}

fn transform_stmt(stmt: &Stmt, current_realms: RealmSet, artifact_set: RealmSet) -> Option<Stmt> {
    if !current_realms.intersects(artifact_set) {
        return None;
    }

    let kind = match &stmt.kind {
        StmtKind::LocalDecl {
            mode,
            names,
            values,
        } => StmtKind::LocalDecl {
            mode: *mode,
            names: names.clone(),
            values: values
                .iter()
                .map(|value| transform_expr(value, current_realms, artifact_set))
                .collect(),
        },
        StmtKind::LocalDestructure {
            mode,
            patterns,
            values,
        } => StmtKind::LocalDestructure {
            mode: *mode,
            patterns: patterns.clone(),
            values: values
                .iter()
                .map(|value| transform_expr(value, current_realms, artifact_set))
                .collect(),
        },
        StmtKind::Assign { targets, values } => StmtKind::Assign {
            targets: targets
                .iter()
                .map(|target| transform_expr(target, current_realms, artifact_set))
                .collect(),
            values: values
                .iter()
                .map(|value| transform_expr(value, current_realms, artifact_set))
                .collect(),
        },
        StmtKind::CompoundAssign { target, op, value } => StmtKind::CompoundAssign {
            target: transform_expr(target, current_realms, artifact_set),
            op: *op,
            value: transform_expr(value, current_realms, artifact_set),
        },
        StmtKind::Expr(expr) => StmtKind::Expr(transform_expr(expr, current_realms, artifact_set)),
        StmtKind::Return(values) => StmtKind::Return(
            values
                .iter()
                .map(|value| transform_expr(value, current_realms, artifact_set))
                .collect(),
        ),
        StmtKind::FunctionDecl(decl) => {
            StmtKind::FunctionDecl(transform_function_decl(decl, current_realms, artifact_set))
        }
        StmtKind::EnumDecl(decl) => {
            StmtKind::EnumDecl(transform_enum_decl(decl, current_realms, artifact_set))
        }
        StmtKind::If {
            condition,
            then_block,
            else_block,
        } => StmtKind::If {
            condition: transform_expr(condition, current_realms, artifact_set),
            then_block: transform_block(then_block, current_realms, artifact_set),
            else_block: else_block
                .as_ref()
                .map(|block| transform_block(block, current_realms, artifact_set)),
        },
        StmtKind::While { condition, body } => StmtKind::While {
            condition: transform_expr(condition, current_realms, artifact_set),
            body: transform_block(body, current_realms, artifact_set),
        },
        StmtKind::NumericFor {
            name,
            start,
            end,
            step,
            body,
        } => StmtKind::NumericFor {
            name: name.clone(),
            start: transform_expr(start, current_realms, artifact_set),
            end: transform_expr(end, current_realms, artifact_set),
            step: step
                .as_ref()
                .map(|expr| transform_expr(expr, current_realms, artifact_set)),
            body: transform_block(body, current_realms, artifact_set),
        },
        StmtKind::GenericFor { names, iter, body } => StmtKind::GenericFor {
            names: names.clone(),
            iter: iter
                .iter()
                .map(|expr| transform_expr(expr, current_realms, artifact_set))
                .collect(),
            body: transform_block(body, current_realms, artifact_set),
        },
        StmtKind::RepeatUntil { body, condition } => StmtKind::RepeatUntil {
            body: transform_block(body, current_realms, artifact_set),
            condition: transform_expr(condition, current_realms, artifact_set),
        },
        StmtKind::Do(block) => StmtKind::Do(transform_block(block, current_realms, artifact_set)),
        StmtKind::RealmDecl { realm, stmt: inner } => {
            let realms = current_realms.intersection(RealmSet::from_realm(*realm));
            return transform_stmt(inner, realms, artifact_set);
        }
        StmtKind::RealmBlock { realm, block } => {
            let realms = current_realms.intersection(RealmSet::from_realm(*realm));
            if !realms.intersects(artifact_set) {
                return None;
            }
            StmtKind::Do(transform_block(block, realms, artifact_set))
        }
        StmtKind::InitDecl { realm, block } => {
            let realms = realm
                .map(|realm| current_realms.intersection(RealmSet::from_realm(realm)))
                .unwrap_or(current_realms);
            if !realms.intersects(artifact_set) {
                return None;
            }
            StmtKind::Do(transform_block(block, realms, artifact_set))
        }
        StmtKind::Break
        | StmtKind::Continue
        | StmtKind::Import(_)
        | StmtKind::PartOrderDecl(_)
        | StmtKind::ExternDecl(_)
        | StmtKind::HostPackageDecl(_)
        | StmtKind::ExportDecl { .. }
        | StmtKind::ExportList { .. }
        | StmtKind::ExportAll { .. } => stmt.kind.clone(),
    };

    Some(Stmt {
        kind,
        span: stmt.span,
    })
}

fn transform_block(block: &Block, current_realms: RealmSet, artifact_set: RealmSet) -> Block {
    Block {
        statements: block
            .statements
            .iter()
            .filter_map(|stmt| transform_stmt(stmt, current_realms, artifact_set))
            .collect(),
        tail: block
            .tail
            .as_ref()
            .map(|expr| transform_expr(expr, current_realms, artifact_set)),
        span: block.span,
    }
}

fn transform_function_decl(
    decl: &FunctionDecl,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> FunctionDecl {
    FunctionDecl {
        name: decl.name.clone(),
        params: decl.params.clone(),
        vararg: decl.vararg,
        body: transform_function_body(&decl.body, current_realms, artifact_set),
    }
}

fn transform_enum_decl(
    decl: &EnumDecl,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> EnumDecl {
    EnumDecl {
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
                    .map(|expr| transform_expr(expr, current_realms, artifact_set)),
                span: variant.span,
            })
            .collect(),
    }
}

fn transform_function_body(
    body: &FunctionBody,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> FunctionBody {
    match body {
        FunctionBody::Expr(expr) => {
            FunctionBody::Expr(Box::new(transform_expr(expr, current_realms, artifact_set)))
        }
        FunctionBody::Block(block) => FunctionBody::Block(Box::new(transform_block(
            block,
            current_realms,
            artifact_set,
        ))),
    }
}

fn transform_expr(expr: &Expr, current_realms: RealmSet, artifact_set: RealmSet) -> Expr {
    let kind = match &expr.kind {
        ExprKind::Table(table) => ExprKind::Table(TableExpr {
            fields: table
                .fields
                .iter()
                .map(|field| transform_table_field(field, current_realms, artifact_set))
                .collect(),
        }),
        ExprKind::Paren(inner) => ExprKind::Paren(Box::new(transform_expr(
            inner,
            current_realms,
            artifact_set,
        ))),
        ExprKind::Unary { op, argument } => ExprKind::Unary {
            op: *op,
            argument: Box::new(transform_expr(argument, current_realms, artifact_set)),
        },
        ExprKind::Binary { op, left, right } => ExprKind::Binary {
            op: *op,
            left: Box::new(transform_expr(left, current_realms, artifact_set)),
            right: Box::new(transform_expr(right, current_realms, artifact_set)),
        },
        ExprKind::Conditional {
            condition,
            then_branch,
            else_branch,
            form,
        } => ExprKind::Conditional {
            condition: Box::new(transform_expr(condition, current_realms, artifact_set)),
            then_branch: transform_expr_or_block(then_branch, current_realms, artifact_set),
            else_branch: transform_expr_or_block(else_branch, current_realms, artifact_set),
            form: *form,
        },
        ExprKind::Match(match_expr) => ExprKind::Match(MatchExpr {
            subject: Box::new(transform_expr(
                &match_expr.subject,
                current_realms,
                artifact_set,
            )),
            arms: match_expr
                .arms
                .iter()
                .map(|arm| MatchArm {
                    pattern: arm.pattern.clone(),
                    body: transform_expr_or_block(&arm.body, current_realms, artifact_set),
                    span: arm.span,
                })
                .collect(),
        }),
        ExprKind::Do(block) => ExprKind::Do(Box::new(transform_block(
            block,
            current_realms,
            artifact_set,
        ))),
        ExprKind::Function(func) => ExprKind::Function(FunctionExpr {
            params: func.params.clone(),
            vararg: func.vararg,
            body: transform_function_body(&func.body, current_realms, artifact_set),
            arrow_kind: func.arrow_kind,
        }),
        ExprKind::Chain(chain) => ExprKind::Chain(ChainExpr {
            base: Box::new(transform_expr(&chain.base, current_realms, artifact_set)),
            segments: chain
                .segments
                .iter()
                .map(|segment| transform_chain_segment(segment, current_realms, artifact_set))
                .collect(),
        }),
        ExprKind::TemplateString(parts) => ExprKind::TemplateString(
            parts
                .iter()
                .map(|part| TemplatePart {
                    kind: match &part.kind {
                        TemplatePartKind::Text(text) => TemplatePartKind::Text(text.clone()),
                        TemplatePartKind::Expr(expr) => TemplatePartKind::Expr(transform_expr(
                            expr,
                            current_realms,
                            artifact_set,
                        )),
                    },
                    span: part.span,
                })
                .collect(),
        ),
        ExprKind::Identifier(_)
        | ExprKind::Nil
        | ExprKind::Boolean(_)
        | ExprKind::Number(_)
        | ExprKind::String(_)
        | ExprKind::Vararg
        | ExprKind::PipelinePlaceholder => expr.kind.clone(),
    };

    Expr {
        kind,
        span: expr.span,
    }
}

fn transform_expr_or_block(
    value: &ExprOrBlock,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> ExprOrBlock {
    match value {
        ExprOrBlock::Expr(expr) => {
            ExprOrBlock::Expr(Box::new(transform_expr(expr, current_realms, artifact_set)))
        }
        ExprOrBlock::Block(block) => ExprOrBlock::Block(Box::new(transform_block(
            block,
            current_realms,
            artifact_set,
        ))),
    }
}

fn transform_table_field(
    field: &TableField,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> TableField {
    let kind = match &field.kind {
        TableFieldKind::Array(value) => {
            TableFieldKind::Array(transform_expr(value, current_realms, artifact_set))
        }
        TableFieldKind::Named { name, value } => TableFieldKind::Named {
            name: name.clone(),
            value: transform_expr(value, current_realms, artifact_set),
        },
        TableFieldKind::ExprKey { key, value } => TableFieldKind::ExprKey {
            key: transform_expr(key, current_realms, artifact_set),
            value: transform_expr(value, current_realms, artifact_set),
        },
        TableFieldKind::Spread(value) => {
            TableFieldKind::Spread(transform_expr(value, current_realms, artifact_set))
        }
    };
    TableField {
        kind,
        span: field.span,
    }
}

fn transform_chain_segment(
    segment: &ChainSegment,
    current_realms: RealmSet,
    artifact_set: RealmSet,
) -> ChainSegment {
    let kind = match &segment.kind {
        ChainSegmentKind::Member { name, optional } => ChainSegmentKind::Member {
            name: name.clone(),
            optional: *optional,
        },
        ChainSegmentKind::Index { index, optional } => ChainSegmentKind::Index {
            index: transform_expr(index, current_realms, artifact_set),
            optional: *optional,
        },
        ChainSegmentKind::Call { args, style } => ChainSegmentKind::Call {
            args: args
                .iter()
                .map(|arg| transform_expr(arg, current_realms, artifact_set))
                .collect(),
            style: *style,
        },
        ChainSegmentKind::SafeDotCall { name, args, style } => ChainSegmentKind::SafeDotCall {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| transform_expr(arg, current_realms, artifact_set))
                .collect(),
            style: *style,
        },
        ChainSegmentKind::MethodCall {
            name,
            args,
            optional,
            style,
        } => ChainSegmentKind::MethodCall {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| transform_expr(arg, current_realms, artifact_set))
                .collect(),
            optional: *optional,
            style: *style,
        },
    };
    ChainSegment {
        kind,
        span: segment.span,
    }
}

fn filter_artifact_exports(
    ir: &mut IrModule,
    resolved: &ResolveOutput,
    artifact_realm: ArtifactRealm,
) {
    let artifact_set = RealmSet::from_artifact(artifact_realm);
    ir.exports.retain(|export| {
        let Some(binding) = resolved.bindings.get(export.binding.0) else {
            return false;
        };
        let export_realms = export
            .realm
            .map(RealmSet::from_realm)
            .unwrap_or(binding.available_realms);
        export_realms.intersects(artifact_set)
    });
}

fn rewrite_import_sources(
    module: &mut IrModule,
    module_id: &ModuleId,
    artifact_realm: ArtifactRealm,
    link_map: &BTreeMap<(ModuleId, String), ModuleId>,
    external_ids: &BTreeSet<ModuleId>,
) {
    rewrite_import_sources_in_stmts(
        &mut module.body,
        module_id,
        artifact_realm,
        link_map,
        external_ids,
    );
}

fn rewrite_import_sources_in_stmts(
    stmts: &mut [crate::ir::IrStmt],
    module_id: &ModuleId,
    artifact_realm: ArtifactRealm,
    link_map: &BTreeMap<(ModuleId, String), ModuleId>,
    external_ids: &BTreeSet<ModuleId>,
) {
    for stmt in stmts {
        match &mut stmt.kind {
            IrStmtKind::Import { source, .. } => {
                if let Some(target) = link_map.get(&(module_id.clone(), source.clone())) {
                    *source = target.artifact_id(artifact_realm);
                } else {
                    let canonical = source.strip_prefix('@').unwrap_or(source);
                    let external = ModuleId::new(canonical);
                    if external_ids.contains(&external) {
                        *source = external.artifact_id(artifact_realm);
                    }
                }
            }
            IrStmtKind::FunctionDecl(decl) => {
                rewrite_import_sources_in_function_body(
                    &mut decl.body,
                    module_id,
                    artifact_realm,
                    link_map,
                    external_ids,
                );
            }
            IrStmtKind::If {
                then_block,
                else_block,
                ..
            } => {
                rewrite_import_sources_in_block(
                    then_block,
                    module_id,
                    artifact_realm,
                    link_map,
                    external_ids,
                );
                if let Some(block) = else_block {
                    rewrite_import_sources_in_block(
                        block,
                        module_id,
                        artifact_realm,
                        link_map,
                        external_ids,
                    );
                }
            }
            IrStmtKind::While { body, .. }
            | IrStmtKind::NumericFor { body, .. }
            | IrStmtKind::GenericFor { body, .. }
            | IrStmtKind::RepeatUntil { body, .. }
            | IrStmtKind::Do(body) => {
                rewrite_import_sources_in_block(
                    body,
                    module_id,
                    artifact_realm,
                    link_map,
                    external_ids,
                );
            }
            _ => {}
        }
    }
}

fn rewrite_import_sources_in_block(
    block: &mut IrBlock,
    module_id: &ModuleId,
    artifact_realm: ArtifactRealm,
    link_map: &BTreeMap<(ModuleId, String), ModuleId>,
    external_ids: &BTreeSet<ModuleId>,
) {
    rewrite_import_sources_in_stmts(
        &mut block.statements,
        module_id,
        artifact_realm,
        link_map,
        external_ids,
    );
}

fn rewrite_import_sources_in_function_body(
    body: &mut IrFunctionBody,
    module_id: &ModuleId,
    artifact_realm: ArtifactRealm,
    link_map: &BTreeMap<(ModuleId, String), ModuleId>,
    external_ids: &BTreeSet<ModuleId>,
) {
    if let IrFunctionBody::Block(block) = body {
        rewrite_import_sources_in_block(block, module_id, artifact_realm, link_map, external_ids);
    }
}

fn collect_runtime_external_imports(
    module: &IrModule,
    artifact_realm: ArtifactRealm,
    external_ids: &BTreeSet<ModuleId>,
    out: &mut BTreeSet<RuntimeExternal>,
) {
    collect_runtime_external_imports_in_stmts(&module.body, artifact_realm, external_ids, out);
}

fn collect_runtime_external_imports_in_stmts(
    stmts: &[crate::ir::IrStmt],
    artifact_realm: ArtifactRealm,
    external_ids: &BTreeSet<ModuleId>,
    out: &mut BTreeSet<RuntimeExternal>,
) {
    for stmt in stmts {
        match &stmt.kind {
            IrStmtKind::Import { source, .. } => {
                let base = source
                    .split_once('#')
                    .map(|(base, _)| base)
                    .unwrap_or(source);
                let id = ModuleId::new(base);
                if external_ids.contains(&id) {
                    out.insert(RuntimeExternal { id, artifact_realm });
                }
            }
            IrStmtKind::FunctionDecl(decl) => {
                collect_runtime_external_imports_in_function_body(
                    &decl.body,
                    artifact_realm,
                    external_ids,
                    out,
                );
            }
            IrStmtKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_runtime_external_imports_in_block(
                    then_block,
                    artifact_realm,
                    external_ids,
                    out,
                );
                if let Some(block) = else_block {
                    collect_runtime_external_imports_in_block(
                        block,
                        artifact_realm,
                        external_ids,
                        out,
                    );
                }
            }
            IrStmtKind::While { body, .. }
            | IrStmtKind::NumericFor { body, .. }
            | IrStmtKind::GenericFor { body, .. }
            | IrStmtKind::RepeatUntil { body, .. }
            | IrStmtKind::Do(body) => {
                collect_runtime_external_imports_in_block(body, artifact_realm, external_ids, out);
            }
            _ => {}
        }
    }
}

fn collect_runtime_external_imports_in_block(
    block: &IrBlock,
    artifact_realm: ArtifactRealm,
    external_ids: &BTreeSet<ModuleId>,
    out: &mut BTreeSet<RuntimeExternal>,
) {
    collect_runtime_external_imports_in_stmts(&block.statements, artifact_realm, external_ids, out);
}

fn collect_runtime_external_imports_in_function_body(
    body: &IrFunctionBody,
    artifact_realm: ArtifactRealm,
    external_ids: &BTreeSet<ModuleId>,
    out: &mut BTreeSet<RuntimeExternal>,
) {
    if let IrFunctionBody::Block(block) = body {
        collect_runtime_external_imports_in_block(block, artifact_realm, external_ids, out);
    }
}

fn runtime_external_closure(
    registry: &RuntimePackageRegistry,
    roots: BTreeSet<RuntimeExternal>,
) -> Result<Vec<RuntimeExternal>, RuntimePackageError> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for realm in ArtifactRealm::ALL {
        let ids = roots
            .iter()
            .filter(|external| external.artifact_realm == realm)
            .map(|external| external.id.clone())
            .collect::<Vec<_>>();
        for id in registry.dependency_closure(ids)? {
            let external = RuntimeExternal {
                id,
                artifact_realm: realm,
            };
            if seen.insert(external.clone()) {
                out.push(external);
            }
        }
    }
    Ok(out)
}

fn write_gmod_artifacts(
    plan: &GmodBuildPlan,
    artifacts: &[GmodCompiledArtifact],
) -> Result<(), ProjectError> {
    for artifact in artifacts {
        write_file(&artifact.lua_path, &artifact.lua)?;
        let files = artifact.source_files.iter().collect::<Vec<_>>();
        write_file(&artifact.map_path, &artifact.source_map.to_json(&files))?;
    }

    write_file(
        &plan.loader.shared_loader.path,
        &plan.loader.shared_loader.render(&plan.registry),
    )?;
    write_file(
        &plan.loader.client_loader.path,
        &plan.loader.client_loader.render(&plan.registry),
    )?;
    write_file(
        &plan.loader.server_loader.path,
        &plan.loader.server_loader.render(&plan.registry),
    )?;
    if let Some(autorun) = &plan.autorun {
        write_file(&autorun.path, &autorun.render())?;
    }
    Ok(())
}

fn write_file(path: &Path, contents: &str) -> Result<(), ProjectError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ProjectError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, contents).map_err(|source| ProjectError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn discover_lux_sources(root: &Path) -> Result<Vec<PathBuf>, ProjectError> {
    let mut paths = Vec::new();
    discover_lux_sources_into(root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn discover_lux_sources_into(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), ProjectError> {
    let entries = fs::read_dir(dir).map_err(|source| ProjectError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| ProjectError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| ProjectError::Io {
            path: path.clone(),
            source,
        })?;

        if file_type.is_dir() {
            discover_lux_sources_into(&path, paths)?;
        } else if path.extension().is_some_and(|extension| extension == "lux") {
            paths.push(path);
        }
    }

    Ok(())
}

pub(crate) fn infer_module_path(source_root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(source_root).unwrap_or(path);
    let mut components = rel
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(component_string)
        .filter(|part| !is_realm_dir(part))
        .collect::<Vec<_>>();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = strip_realm_prefix(&stem).1.to_string();
    if stem != "module" && components.is_empty() {
        components.push(stem);
    }
    normalize_module_path(&components.join("/"))
}

pub(crate) fn infer_part_realm(
    source_root: &Path,
    path: &Path,
    file_id: crate::source::FileId,
) -> Result<Realm, Diagnostic> {
    let rel = path.strip_prefix(source_root).unwrap_or(path);
    let dir_realm = rel
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(component_string)
        .rev()
        .find_map(|part| realm_dir(&part));
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let (prefix_realm, _) = strip_realm_prefix(&stem);
    match (prefix_realm, dir_realm) {
        (Some(prefix), Some(dir)) if prefix != dir => Err(Diagnostic::error(format!(
            "realm prefix `{}` conflicts with nearest realm directory `{}`",
            prefix.as_str(),
            dir.as_str()
        ))
        .with_code("REALM003")
        .with_label(Label::primary(
            SourceSpan::new(file_id, 0, 0),
            "conflicting part realm",
        ))),
        (Some(prefix), _) => Ok(prefix),
        (_, Some(dir)) => Ok(dir),
        _ => Ok(Realm::Shared),
    }
}

fn component_string(component: Component<'_>) -> Option<String> {
    match component {
        Component::Normal(part) => Some(part.to_string_lossy().to_string()),
        _ => None,
    }
}

fn is_realm_dir(value: &str) -> bool {
    matches!(value, "shared" | "client" | "server")
}

fn realm_dir(value: &str) -> Option<Realm> {
    match value {
        "shared" => Some(Realm::Shared),
        "client" => Some(Realm::Client),
        "server" => Some(Realm::Server),
        _ => None,
    }
}

fn strip_realm_prefix(stem: &str) -> (Option<Realm>, &str) {
    if let Some(rest) = stem.strip_prefix("cl_") {
        (Some(Realm::Client), rest)
    } else if let Some(rest) = stem.strip_prefix("sv_") {
        (Some(Realm::Server), rest)
    } else if let Some(rest) = stem.strip_prefix("sh_") {
        (Some(Realm::Shared), rest)
    } else {
        (None, stem)
    }
}

fn gmod_lua_path(
    config: &GmodBackendConfig,
    module_id: &ModuleId,
    artifact_realm: ArtifactRealm,
) -> PathBuf {
    let mut path = config.output_root.join(&config.runtime_base);
    path.push(artifact_realm.as_str());
    for part in module_id.as_str().split('/') {
        path.push(part);
    }
    path.set_extension("lua");
    path
}

fn runtime_module_artifact(
    external: &RuntimeExternal,
    config: &GmodBackendConfig,
    registry: &crate::gmod::ModuleRegistryPlan,
    runtime_registry: &RuntimePackageRegistry,
) -> Result<Option<GmodCompiledArtifact>, ProjectError> {
    let Some(package) = runtime_registry.package(&external.id) else {
        return Ok(None);
    };
    let output = package
        .compile_for_realm(external.artifact_realm)
        .map_err(ProjectError::Runtime)?;

    let mut lua_path = config.output_root.join(&config.runtime_base);
    lua_path.push(external.artifact_realm.as_str());
    lua_path.push("runtime");
    for part in external.id.as_str().split('/') {
        lua_path.push(part);
    }
    lua_path.set_extension("lua");
    let map_path = lua_path.with_extension("lua.map.json");
    let artifact_id = external.id.artifact_id(external.artifact_realm);

    Ok(Some(GmodCompiledArtifact {
        module_id: external.id.clone(),
        artifact_id,
        source_path: package.path.clone(),
        source_files: package.source_files(),
        lua_path,
        map_path,
        lua: registry.wrap_module_lua(&output.lua),
        source_map: output.source_map.shifted(1, 2),
    }))
}

fn to_gmod_realm(realm: ArtifactRealm) -> crate::gmod::Realm {
    match realm {
        ArtifactRealm::Client => crate::gmod::Realm::Client,
        ArtifactRealm::Server => crate::gmod::Realm::Server,
    }
}

fn apply_source_comments(
    output: &LuaOutput,
    source_files: &[SourceFile],
    mode: SourceCommentMode,
) -> (String, SourceMap) {
    if mode != SourceCommentMode::None && source_files.len() == 1 {
        let file = &source_files[0];
        (
            with_source_comments(&output.lua, &output.source_map, file, mode),
            map_after_source_comments(&output.lua, &output.source_map, file, mode),
        )
    } else {
        (output.lua.clone(), output.source_map.clone())
    }
}

fn module_source_files(module: &ParsedProjectModule) -> Vec<&SourceFile> {
    module.parts.iter().map(|part| &part.source_file).collect()
}

fn render_module_diagnostics(
    modules: &BTreeMap<ModuleId, ParsedProjectModule>,
    diagnostics: &[Diagnostic],
) -> Vec<RenderedDiagnostic> {
    let files = modules
        .values()
        .flat_map(|module| module.parts.iter().map(|part| &part.source_file))
        .collect::<Vec<_>>();
    render_diagnostics_by_file(files, diagnostics)
}

fn render_diagnostics_by_file(
    files: Vec<&SourceFile>,
    diagnostics: &[Diagnostic],
) -> Vec<RenderedDiagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let file = diagnostic
                .labels
                .first()
                .and_then(|label| files.iter().find(|file| file.id == label.span.file_id))
                .copied()
                .or_else(|| files.first().copied())
                .expect("diagnostics require at least one source file");
            RenderedDiagnostic {
                path: file.path.clone().unwrap_or_default(),
                message: DiagnosticEmitter::render(diagnostic, file),
            }
        })
        .collect()
}

fn render_diagnostics(
    path: &Path,
    file: &SourceFile,
    diagnostics: &[Diagnostic],
) -> Vec<RenderedDiagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| RenderedDiagnostic {
            path: path.to_path_buf(),
            message: DiagnosticEmitter::render(diagnostic, file),
        })
        .collect()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
