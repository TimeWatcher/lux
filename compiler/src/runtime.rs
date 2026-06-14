use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::ast::{Module, Realm};
use crate::compile_time::CompileTimePackageRegistry;
use crate::diag::{Diagnostic, DiagnosticEmitter, Label};
use crate::host::HostRegistry;
use crate::ir::{IrBlock, IrFunctionBody, IrStmtKind};
use crate::lex::Lexer;
use crate::lower::{LowerError, Lowerer};
use crate::macro_expansion::{MacroRegistry, expand_macros_with_registry};
use crate::module::{ArtifactRealm, ModuleExport, ModuleId};
use crate::packages::{
    PackageLoadError, PackagePhase, default_package_root, discover_runtime_phases,
};
use crate::parse::Parser;
use crate::part_order::{PartOrderInput, is_module_entry_path, sort_module_parts};
use crate::project::{ArtifactModulePart, build_artifact_module_from_parts};
use crate::resolve::{ResolveOutput, ResolvePart, Resolver};
use crate::source::{SourceFile, SourceSpan};
use crate::{codegen::LuaCodegen, codegen::LuaOutput};

#[derive(Debug, Clone)]
pub struct RuntimePackageRegistry {
    root: PathBuf,
    packages: BTreeMap<ModuleId, RuntimePackage>,
}

impl RuntimePackageRegistry {
    pub fn load_default() -> Result<Self, RuntimePackageError> {
        Self::load(default_package_root())
    }

    pub fn load(root: impl Into<PathBuf>) -> Result<Self, RuntimePackageError> {
        let root = root.into();
        let mut packages = BTreeMap::new();
        let mut next_file_id = 10_000u32;
        let mut macro_registry = MacroRegistry::empty();
        CompileTimePackageRegistry::load_default()
            .map_err(|err| RuntimePackageError::Diagnostics(vec![err.to_string()]))?
            .register_macros(&mut macro_registry)
            .map_err(|err| RuntimePackageError::Diagnostics(vec![err.to_string()]))?;
        for phase in discover_runtime_phases(&root).map_err(RuntimePackageError::Package)? {
            let id = ModuleId::new(&phase.package_id);
            let parts = parse_runtime_parts(&phase, &macro_registry, &mut next_file_id)?;
            let resolve_parts = parts
                .iter()
                .map(|part| ResolvePart {
                    module: &part.module,
                    default_realm: part.default_realm,
                })
                .collect::<Vec<_>>();
            let resolved = Resolver::resolve_parts(&resolve_parts);
            if resolved.has_errors() {
                return Err(RuntimePackageError::Diagnostics(
                    render_runtime_diagnostics_by_part(&parts, &resolved.diagnostics),
                ));
            }
            let metadata = runtime_metadata_from_resolved(&resolved);
            let package = RuntimePackage {
                id: id.clone(),
                path: phase.source_path.clone(),
                source_dir: phase.source_dir.clone(),
                parts,
                resolved,
                exports: metadata.exports,
                imports: metadata.imports,
            };

            if let Some(existing) = packages.insert(id.clone(), package) {
                return Err(RuntimePackageError::DuplicatePackage {
                    id: id.as_str().to_string(),
                    first: existing.path,
                    second: phase.source_path,
                });
            }
        }
        Ok(Self { root, packages })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn package_ids(&self) -> BTreeSet<ModuleId> {
        self.packages.keys().cloned().collect()
    }

    pub fn export_metadata(&self) -> BTreeMap<ModuleId, Vec<ModuleExport>> {
        self.packages
            .iter()
            .map(|(id, package)| (id.clone(), package.exports.clone()))
            .collect()
    }

    pub fn package(&self, id: &ModuleId) -> Option<&RuntimePackage> {
        self.packages.get(id)
    }

    pub fn dependency_closure(
        &self,
        roots: impl IntoIterator<Item = ModuleId>,
    ) -> Result<Vec<ModuleId>, RuntimePackageError> {
        let mut ordered = Vec::new();
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();

        for root in roots {
            self.visit_dependency(&root, &mut visiting, &mut visited, &mut ordered)?;
        }

        Ok(ordered)
    }

    fn visit_dependency(
        &self,
        id: &ModuleId,
        visiting: &mut BTreeSet<ModuleId>,
        visited: &mut BTreeSet<ModuleId>,
        ordered: &mut Vec<ModuleId>,
    ) -> Result<(), RuntimePackageError> {
        if visited.contains(id) {
            return Ok(());
        }

        let Some(package) = self.packages.get(id) else {
            return Err(RuntimePackageError::MissingDependency {
                id: id.as_str().to_string(),
            });
        };

        if !visiting.insert(id.clone()) {
            return Err(RuntimePackageError::DependencyCycle {
                id: id.as_str().to_string(),
            });
        }

        for dependency in &package.imports {
            self.visit_dependency(dependency, visiting, visited, ordered)?;
        }

        visiting.remove(id);
        visited.insert(id.clone());
        ordered.push(id.clone());
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RuntimePackage {
    pub id: ModuleId,
    pub path: PathBuf,
    pub source_dir: PathBuf,
    parts: Vec<RuntimeSourcePart>,
    resolved: ResolveOutput,
    pub exports: Vec<ModuleExport>,
    pub imports: Vec<ModuleId>,
}

impl RuntimePackage {
    pub fn compile(&self) -> Result<LuaOutput, RuntimePackageError> {
        self.compile_for_realm(ArtifactRealm::Client)
    }

    pub fn compile_for_realm(
        &self,
        artifact_realm: ArtifactRealm,
    ) -> Result<LuaOutput, RuntimePackageError> {
        let artifact_module = self.artifact_module(artifact_realm);
        let mut ir = Lowerer::lower_for_artifact(&artifact_module, &self.resolved, artifact_realm)
            .map_err(|source| RuntimePackageError::Lower {
                path: self.path.clone(),
                source,
            })?;
        filter_runtime_exports(&mut ir, &self.resolved, artifact_realm);
        rewrite_runtime_imports(&mut ir, artifact_realm);
        let transformed = HostRegistry::empty().transform_module(ir, &self.resolved);
        if transformed.has_errors() {
            return Err(RuntimePackageError::Diagnostics(
                render_runtime_diagnostics_by_part(&self.parts, &transformed.diagnostics),
            ));
        }
        LuaCodegen::generate(&transformed.module).map_err(|source| RuntimePackageError::Codegen {
            path: self.path.clone(),
            source,
        })
    }

    fn artifact_module(&self, artifact_realm: ArtifactRealm) -> Module {
        let parts = self
            .parts
            .iter()
            .map(|part| ArtifactModulePart {
                module: &part.module,
                default_realm: part.default_realm,
            })
            .collect::<Vec<_>>();
        build_artifact_module_from_parts(&parts, &self.resolved, artifact_realm)
    }

    pub fn source_files(&self) -> Vec<SourceFile> {
        self.parts
            .iter()
            .map(|part| part.source_file.clone())
            .collect()
    }
}

#[derive(Debug)]
pub enum RuntimePackageError {
    Package(PackageLoadError),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    DuplicatePackage {
        id: String,
        first: PathBuf,
        second: PathBuf,
    },
    Diagnostics(Vec<String>),
    Lower {
        path: PathBuf,
        source: LowerError,
    },
    Codegen {
        path: PathBuf,
        source: crate::codegen::CodegenError,
    },
    MissingDependency {
        id: String,
    },
    DependencyCycle {
        id: String,
    },
}

impl std::fmt::Display for RuntimePackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Package(source) => write!(f, "{source}"),
            Self::Io { path, source } => write!(f, "failed to load {}: {source}", path.display()),
            Self::DuplicatePackage { id, first, second } => write!(
                f,
                "duplicate runtime package `{id}` at {} and {}",
                first.display(),
                second.display()
            ),
            Self::Diagnostics(diagnostics) => {
                for (index, diagnostic) in diagnostics.iter().enumerate() {
                    if index > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{diagnostic}")?;
                }
                Ok(())
            }
            Self::Lower { path, source } => {
                write!(
                    f,
                    "lowering failed for runtime {}: {source}",
                    path.display()
                )
            }
            Self::Codegen { path, source } => {
                write!(f, "codegen failed for runtime {}: {source}", path.display())
            }
            Self::MissingDependency { id } => {
                write!(f, "runtime package dependency `{id}` was not found")
            }
            Self::DependencyCycle { id } => {
                write!(f, "runtime package dependency cycle involving `{id}`")
            }
        }
    }
}

impl std::error::Error for RuntimePackageError {}

#[derive(Debug, Clone)]
struct RuntimeSourcePart {
    path: PathBuf,
    source_file: SourceFile,
    module: Module,
    default_realm: Realm,
}

#[derive(Debug, Clone)]
struct RuntimeMetadata {
    exports: Vec<ModuleExport>,
    imports: Vec<ModuleId>,
}

fn parse_runtime_parts(
    phase: &PackagePhase,
    macro_registry: &MacroRegistry,
    next_file_id: &mut u32,
) -> Result<Vec<RuntimeSourcePart>, RuntimePackageError> {
    let mut parts = Vec::new();
    for path in &phase.source_paths {
        let file =
            SourceFile::load(*next_file_id, path).map_err(|source| RuntimePackageError::Io {
                path: path.clone(),
                source,
            })?;
        *next_file_id += 1;

        let default_realm =
            infer_package_part_realm(&phase.source_dir, path, file.id).map_err(|diagnostic| {
                RuntimePackageError::Diagnostics(render_runtime_diagnostics(
                    path,
                    &file,
                    &[diagnostic],
                ))
            })?;

        let lex = Lexer::new(&file).lex_all();
        if lex.has_errors() {
            return Err(RuntimePackageError::Diagnostics(
                render_runtime_diagnostics(path, &file, &lex.diagnostics),
            ));
        }

        let parsed = Parser::new(&lex.tokens).parse_module();
        if parsed.has_errors() {
            return Err(RuntimePackageError::Diagnostics(
                render_runtime_diagnostics(path, &file, &parsed.diagnostics),
            ));
        }

        let expanded = expand_macros_with_registry(&file, &parsed.module, macro_registry);
        if expanded.has_errors() {
            return Err(RuntimePackageError::Diagnostics(
                render_runtime_diagnostics(path, &file, &expanded.diagnostics),
            ));
        }

        parts.push(RuntimeSourcePart {
            path: path.clone(),
            source_file: file,
            module: expanded.module,
            default_realm,
        });
    }
    parts.sort_by(|a, b| a.path.cmp(&b.path));
    let inputs = parts
        .iter()
        .map(|part| PartOrderInput {
            path: &part.path,
            module: &part.module,
            is_entry: is_module_entry_path(&part.path),
        })
        .collect::<Vec<_>>();
    match sort_module_parts(&inputs) {
        Ok(order) => {
            parts = order
                .into_iter()
                .map(|index| parts[index].clone())
                .collect();
        }
        Err(diagnostics) => {
            return Err(RuntimePackageError::Diagnostics(
                render_runtime_diagnostics_by_part(&parts, &diagnostics),
            ));
        }
    }
    Ok(parts)
}

fn runtime_metadata_from_resolved(resolved: &ResolveOutput) -> RuntimeMetadata {
    RuntimeMetadata {
        exports: resolved
            .exports
            .iter()
            .filter_map(|export| {
                let binding = resolved.bindings.get(export.binding.0)?;
                Some(ModuleExport {
                    name: export.name.clone(),
                    realms: export
                        .realm
                        .map(crate::module::RealmSet::from_realm)
                        .unwrap_or(binding.available_realms),
                    span: export.span,
                })
            })
            .collect(),
        imports: resolved
            .module_edges
            .iter()
            .map(|edge| ModuleId::new(edge.source.strip_prefix('@').unwrap_or(&edge.source)))
            .collect(),
    }
}

fn infer_package_part_realm(
    source_dir: &Path,
    path: &Path,
    file_id: crate::source::FileId,
) -> Result<Realm, Diagnostic> {
    let rel = path.strip_prefix(source_dir).unwrap_or(path);
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
            "conflicting package part realm",
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

fn rewrite_runtime_imports(module: &mut crate::ir::IrModule, artifact_realm: ArtifactRealm) {
    rewrite_runtime_imports_in_stmts(&mut module.body, artifact_realm);
}

fn filter_runtime_exports(
    ir: &mut crate::ir::IrModule,
    resolved: &crate::resolve::ResolveOutput,
    artifact_realm: ArtifactRealm,
) {
    let artifact_set = crate::module::RealmSet::from_artifact(artifact_realm);
    ir.exports.retain(|export| {
        let Some(binding) = resolved.bindings.get(export.binding.0) else {
            return false;
        };
        let export_realms = export
            .realm
            .map(crate::module::RealmSet::from_realm)
            .unwrap_or(binding.available_realms);
        export_realms.intersects(artifact_set)
    });
}

fn rewrite_runtime_imports_in_stmts(
    stmts: &mut [crate::ir::IrStmt],
    artifact_realm: ArtifactRealm,
) {
    for stmt in stmts {
        match &mut stmt.kind {
            IrStmtKind::Import { source, .. } => {
                let canonical = source.strip_prefix('@').unwrap_or(source).to_string();
                *source = ModuleId::new(canonical).artifact_id(artifact_realm);
            }
            IrStmtKind::FunctionDecl(decl) => {
                rewrite_runtime_imports_in_function_body(&mut decl.body, artifact_realm);
            }
            IrStmtKind::If {
                then_block,
                else_block,
                ..
            } => {
                rewrite_runtime_imports_in_block(then_block, artifact_realm);
                if let Some(block) = else_block {
                    rewrite_runtime_imports_in_block(block, artifact_realm);
                }
            }
            IrStmtKind::While { body, .. }
            | IrStmtKind::NumericFor { body, .. }
            | IrStmtKind::GenericFor { body, .. }
            | IrStmtKind::RepeatUntil { body, .. }
            | IrStmtKind::Do(body) => rewrite_runtime_imports_in_block(body, artifact_realm),
            _ => {}
        }
    }
}

fn rewrite_runtime_imports_in_block(block: &mut IrBlock, artifact_realm: ArtifactRealm) {
    rewrite_runtime_imports_in_stmts(&mut block.statements, artifact_realm);
}

fn rewrite_runtime_imports_in_function_body(
    body: &mut IrFunctionBody,
    artifact_realm: ArtifactRealm,
) {
    if let IrFunctionBody::Block(block) = body {
        rewrite_runtime_imports_in_block(block, artifact_realm);
    }
}

fn render_runtime_diagnostics(
    _path: &Path,
    file: &SourceFile,
    diagnostics: &[Diagnostic],
) -> Vec<String> {
    diagnostics
        .iter()
        .map(|diagnostic| DiagnosticEmitter::render(diagnostic, file))
        .collect()
}

fn render_runtime_diagnostics_by_part(
    parts: &[RuntimeSourcePart],
    diagnostics: &[Diagnostic],
) -> Vec<String> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let file_id = diagnostic
                .labels
                .first()
                .map(|label| label.span.file_id)
                .or_else(|| diagnostic.notes.iter().find_map(|_| None));
            let file = file_id
                .and_then(|file_id| {
                    parts
                        .iter()
                        .find(|part| part.source_file.id == file_id)
                        .map(|part| &part.source_file)
                })
                .or_else(|| parts.first().map(|part| &part.source_file));
            match file {
                Some(file) => DiagnosticEmitter::render(diagnostic, file),
                None => diagnostic.message.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::RuntimePackageRegistry;
    use crate::module::ModuleId;

    fn has_export(
        exports: &std::collections::BTreeMap<ModuleId, Vec<crate::module::ModuleExport>>,
        id: &str,
        name: &str,
    ) -> bool {
        exports
            .get(&ModuleId::new(id))
            .is_some_and(|items| items.iter().any(|export| export.name == name))
    }

    #[test]
    fn loads_default_runtime_packages_and_exports() {
        let registry = RuntimePackageRegistry::load_default().expect("runtime registry");
        let exports = registry.export_metadata();
        assert!(has_export(&exports, "lux/std", "arr"));
        assert!(has_export(&exports, "lux/std", "dict"));
        assert!(has_export(&exports, "lux/std", "pool"));
        assert!(has_export(&exports, "lux/gmod", "valid"));
        assert!(has_export(&exports, "lux/gmod", "hookx"));
        assert!(has_export(&exports, "lux/gmod", "netx"));
        assert!(has_export(&exports, "lux/reactive", "signal"));
        assert!(has_export(&exports, "lux/reactive", "effect"));
        assert!(has_export(&exports, "lux/ui", "node"));
        assert!(has_export(&exports, "lux/ui", "Column"));
        assert!(has_export(&exports, "lux/mgfx", "paint"));
        assert!(has_export(&exports, "lux/mgfx", "style"));
        assert!(has_export(&exports, "lux/mgfx", "frame"));
        assert!(has_export(&exports, "lux/mgfx", "geometry"));
        assert!(has_export(&exports, "lux/mgfx", "text"));
        assert!(has_export(&exports, "lux/mgfx", "shaderpack"));
        assert!(has_export(&exports, "lux/mgfx", "materials"));
        assert!(has_export(&exports, "lux/mgfx", "profiler"));
        assert!(has_export(&exports, "lux/mgfx", "roundrect"));
        assert!(has_export(&exports, "lux/mgfx", "primitives"));
        assert!(has_export(&exports, "lux/mgfx", "widgets"));
        assert!(!has_export(&exports, "lux/mgfx", "console"));
        assert!(!has_export(&exports, "lux/mgfx", "demo"));
        assert!(!has_export(&exports, "lux/mgfx", "wheelDemo"));
        assert!(has_export(&exports, "lux/mgfx/frame", "startPanel"));
        assert!(has_export(
            &exports,
            "lux/mgfx/geometry",
            "drawTexturedRectUV"
        ));
        assert!(has_export(&exports, "lux/mgfx/geometry", "pushTransform"));
        assert!(has_export(&exports, "lux/mgfx/paint", "roundedBoxEx"));
        assert!(has_export(&exports, "lux/mgfx/text", "drawEx"));
        assert!(has_export(&exports, "lux/mgfx/shaderpack", "VERSION"));
        assert!(has_export(&exports, "lux/mgfx/shaderpack", "gma"));
        assert!(has_export(&exports, "lux/mgfx/shaderpack", "pack"));
        assert!(has_export(&exports, "lux/mgfx/shaderpack", "current"));
        assert!(has_export(&exports, "lux/mgfx/shaderpack", "installGlobal"));
        assert!(has_export(&exports, "lux/mgfx/capabilities", "TARGET"));
        assert!(has_export(&exports, "lux/mgfx/capabilities", "TARGET_NAME"));
        assert!(has_export(
            &exports,
            "lux/mgfx/capabilities",
            "normalizeStyle"
        ));
        assert!(has_export(&exports, "lux/mgfx/capabilities", "install"));
        assert!(has_export(&exports, "lux/mgfx/console", "install"));
        assert!(has_export(&exports, "lux/mgfx/console", "selftest"));
        assert!(has_export(&exports, "lux/mgfx/demo", "install"));
        assert!(has_export(&exports, "lux/mgfx/demo", "open"));
        assert!(has_export(&exports, "lux/mgfx/wheel_demo", "install"));
        assert!(has_export(&exports, "lux/mgfx/wheel_demo", "open"));
        assert!(
            registry
                .package(&ModuleId::new("lux/ui"))
                .expect("ui package")
                .imports
                .contains(&ModuleId::new("lux/reactive"))
        );
    }

    #[test]
    fn compiles_runtime_package_from_lux_source() {
        let registry = RuntimePackageRegistry::load_default().expect("runtime registry");
        let package = registry
            .package(&ModuleId::new("lux/std"))
            .expect("std package");
        let output = package.compile().expect("compile runtime");
        assert!(output.lua.contains("local arr"));
        assert!(output.lua.contains("arr = {}"));
        assert!(output.lua.contains("__lux_exports.arr = arr"));

        let gmod = registry
            .package(&ModuleId::new("lux/gmod"))
            .expect("gmod package")
            .compile()
            .expect("compile gmod runtime");
        assert!(gmod.lua.contains("__lux_exports.valid = valid"));
        assert!(gmod.lua.contains("__lux_exports.netx = netx"));

        let mgfx = registry
            .package(&ModuleId::new("lux/mgfx"))
            .expect("mgfx package")
            .compile()
            .expect("compile mgfx runtime");
        assert!(mgfx.lua.contains("__lux_exports.paint = paint"));
        assert!(mgfx.lua.contains("__lux_exports.frame = frame"));
        assert!(mgfx.lua.contains("__lux_exports.geometry = geometry"));
        assert!(!mgfx.lua.contains("__lux_exports.console = console"));
        assert!(!mgfx.lua.contains("__lux_exports.demo = demo"));
        assert!(!mgfx.lua.contains("__lux_exports.wheelDemo = wheelDemo"));
        assert!(mgfx.lua.contains("__lux_import(\"lux/mgfx/paint#client\")"));
        assert!(
            mgfx.lua
                .contains("__lux_import(\"lux/mgfx/geometry#client\")")
        );
        assert!(
            !mgfx
                .lua
                .contains("__lux_import(\"lux/mgfx/console#client\")")
        );
        assert!(!mgfx.lua.contains("__lux_import(\"lux/mgfx/demo#client\")"));
        assert!(
            !mgfx
                .lua
                .contains("__lux_import(\"lux/mgfx/wheel_demo#client\")")
        );

        let mgfx_frame = registry
            .package(&ModuleId::new("lux/mgfx/frame"))
            .expect("mgfx frame package")
            .compile()
            .expect("compile mgfx frame runtime");
        assert!(mgfx_frame.lua.contains("startPanel = function"));

        let mgfx_paint = registry
            .package(&ModuleId::new("lux/mgfx/paint"))
            .expect("mgfx paint package")
            .compile()
            .expect("compile mgfx paint runtime");
        assert!(mgfx_paint.lua.contains("roundedBoxEx = function"));
        assert!(
            mgfx_paint
                .lua
                .contains("__lux_import(\"lux/mgfx/geometry#client\")")
        );

        let mgfx_commands = registry
            .package(&ModuleId::new("lux/mgfx/commands"))
            .expect("mgfx commands package")
            .compile()
            .expect("compile mgfx commands runtime");
        assert!(
            mgfx_commands
                .lua
                .contains("progressBar = function(command)")
        );
        assert!(
            mgfx_commands
                .lua
                .contains("return makeProgress(\n      command[1],"),
            "{}",
            mgfx_commands.lua
        );
        assert!(mgfx_commands.lua.contains("ringValues = function(command)"));
        assert!(
            mgfx_commands.lua.contains("field(command, \"cx\", 2)"),
            "{}",
            mgfx_commands.lua
        );
        assert!(
            mgfx_commands
                .lua
                .contains("textBoxValues = function(command)")
        );

        let mgfx_capabilities = registry
            .package(&ModuleId::new("lux/mgfx/capabilities"))
            .expect("mgfx capabilities package")
            .compile()
            .expect("compile mgfx capabilities runtime");
        assert!(mgfx_capabilities.lua.contains("TARGET.ROUNDED_BOX = 1"));
        assert!(
            mgfx_capabilities
                .lua
                .contains("TARGET_NAME[1] = \"MGFX.TARGET.ROUNDED_BOX\"")
        );
        assert!(mgfx_capabilities.lua.contains("matrix[14] = {"));
        assert!(mgfx_capabilities.lua.contains("coverage = \"sector\""));
        let supports = mgfx_capabilities
            .lua
            .find("owner.Supports = supports")
            .expect("supports assignment");
        let is_pattern = mgfx_capabilities
            .lua
            .find("owner.IsPattern = isPattern")
            .expect("isPattern assignment");
        let shader_status = mgfx_capabilities
            .lua
            .find("owner.shaderStatus = function()")
            .expect("shaderStatus assignment");
        let shader_return = mgfx_capabilities
            .lua
            .find("return { ok = false, shaderVersion = nil, reason = \"lux/mgfx fallback renderer active\" }")
            .expect("shaderStatus return");
        let install_return = mgfx_capabilities
            .lua
            .find("return owner")
            .expect("install return");
        let export_flush = mgfx_capabilities
            .lua
            .find("__lux_exports.TARGET = TARGET")
            .expect("export flush");
        assert!(supports < is_pattern, "{}", mgfx_capabilities.lua);
        assert!(is_pattern < shader_status, "{}", mgfx_capabilities.lua);
        assert!(shader_status < shader_return, "{}", mgfx_capabilities.lua);
        assert!(shader_return < install_return, "{}", mgfx_capabilities.lua);
        assert!(install_return < export_flush, "{}", mgfx_capabilities.lua);
        assert!(
            mgfx_capabilities
                .lua
                .contains("__lux_exports.TARGET = TARGET")
        );
        assert!(
            mgfx_capabilities
                .lua
                .contains("__lux_exports.normalizeStyle = normalizeStyle")
        );
        assert_eq!(
            mgfx_capabilities
                .lua
                .matches("__lux_exports.TARGET = TARGET")
                .count(),
            1,
            "{}",
            mgfx_capabilities.lua
        );
        assert_eq!(
            mgfx_capabilities
                .lua
                .matches("__lux_exports.TARGET_NAME = TARGET_NAME")
                .count(),
            1,
            "{}",
            mgfx_capabilities.lua
        );
        assert_eq!(
            mgfx_capabilities
                .lua
                .matches("__lux_exports.get = get")
                .count(),
            1,
            "{}",
            mgfx_capabilities.lua
        );

        let mgfx_geometry = registry
            .package(&ModuleId::new("lux/mgfx/geometry"))
            .expect("mgfx geometry package")
            .compile()
            .expect("compile mgfx geometry runtime");
        assert!(mgfx_geometry.lua.contains("drawTexturedRectUV = function"));
        assert!(mgfx_geometry.lua.contains("pushTransform = function"));

        let mgfx_console = registry
            .package(&ModuleId::new("lux/mgfx/console"))
            .expect("mgfx console package")
            .compile()
            .expect("compile mgfx console runtime");
        assert!(mgfx_console.lua.contains("selftest = function"));
        assert!(mgfx_console.lua.contains("__lux_exports.install = install"));

        let mgfx_demo = registry
            .package(&ModuleId::new("lux/mgfx/demo"))
            .expect("mgfx demo package")
            .compile()
            .expect("compile mgfx demo runtime");
        assert!(mgfx_demo.lua.contains("function PANEL:Paint"));
        assert!(mgfx_demo.lua.contains("__lux_exports.open = open"));

        let mgfx_wheel_demo = registry
            .package(&ModuleId::new("lux/mgfx/wheel_demo"))
            .expect("mgfx wheel demo package")
            .compile()
            .expect("compile mgfx wheel demo runtime");
        assert!(mgfx_wheel_demo.lua.contains("function PANEL:Paint"));
        assert!(mgfx_wheel_demo.lua.contains("__lux_exports.open = open"));
    }

    #[test]
    fn compiles_reactive_and_ui_runtime_packages() {
        let registry = RuntimePackageRegistry::load_default().expect("runtime registry");
        let reactive = registry
            .package(&ModuleId::new("lux/reactive"))
            .expect("reactive package")
            .compile()
            .expect("compile reactive");
        assert!(reactive.lua.contains("__lux_exports.signal = signal"));
        assert!(reactive.lua.contains("__lux_exports.effect = effect"));

        let ui = registry
            .package(&ModuleId::new("lux/ui"))
            .expect("ui package")
            .compile()
            .expect("compile ui");
        assert!(
            ui.lua.contains("__lux_import(\"lux/reactive#client\")"),
            "{}",
            ui.lua
        );
        assert!(ui.lua.contains("__lux_exports.mount = mount"));
    }

    #[test]
    fn runtime_dependency_closure_orders_dependencies_first() {
        let registry = RuntimePackageRegistry::load_default().expect("runtime registry");
        let closure = registry
            .dependency_closure([ModuleId::new("lux/ui")])
            .expect("runtime closure");
        assert_eq!(
            closure.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            vec!["lux/reactive", "lux/ui"]
        );
    }
}
