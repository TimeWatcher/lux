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
    PackageLoadError, PackagePhase, default_package_root, default_package_roots,
    discover_runtime_phases,
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
        Self::load_roots(default_package_roots())
    }

    pub fn load_default_with_package_roots(
        extra_roots: &[PathBuf],
    ) -> Result<Self, RuntimePackageError> {
        let mut roots = default_package_roots();
        roots.extend(extra_roots.iter().cloned());
        Self::load_roots_with_compile_time_extra(roots, extra_roots)
    }

    pub fn load(root: impl Into<PathBuf>) -> Result<Self, RuntimePackageError> {
        Self::load_roots(vec![root.into()])
    }

    pub fn load_roots(roots: Vec<PathBuf>) -> Result<Self, RuntimePackageError> {
        Self::load_roots_with_compile_time_extra(roots, &[])
    }

    fn load_roots_with_compile_time_extra(
        roots: Vec<PathBuf>,
        compile_time_extra_roots: &[PathBuf],
    ) -> Result<Self, RuntimePackageError> {
        let root = roots.first().cloned().unwrap_or_else(default_package_root);
        let mut packages = BTreeMap::new();
        let mut next_file_id = 10_000u32;
        let mut macro_registry = MacroRegistry::empty();
        CompileTimePackageRegistry::load_default_with_package_roots(compile_time_extra_roots)
            .map_err(|err| RuntimePackageError::Diagnostics(vec![err.to_string()]))?
            .register_macros(&mut macro_registry)
            .map_err(|err| RuntimePackageError::Diagnostics(vec![err.to_string()]))?;
        let mut seen_roots = BTreeSet::new();
        for package_root in &roots {
            if !seen_roots.insert(package_root.clone()) {
                continue;
            }
            for phase in
                discover_runtime_phases(package_root).map_err(RuntimePackageError::Package)?
            {
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

    pub fn packages(&self) -> impl Iterator<Item = (&ModuleId, &RuntimePackage)> {
        self.packages.iter()
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
    pub resolved: ResolveOutput,
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

    pub fn source_parts(&self) -> impl Iterator<Item = RuntimePackageSourcePart<'_>> {
        self.parts.iter().map(|part| RuntimePackageSourcePart {
            path: &part.path,
            source_file: &part.source_file,
            module: &part.module,
            default_realm: part.default_realm,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimePackageSourcePart<'a> {
    pub path: &'a Path,
    pub source_file: &'a SourceFile,
    pub module: &'a Module,
    pub default_realm: Realm,
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
    use crate::test_support::test_std_package_root;

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
    fn default_runtime_registry_is_empty_without_package_roots() {
        let registry = RuntimePackageRegistry::load_default().expect("runtime registry");
        assert!(registry.package_ids().is_empty());
    }

    #[test]
    fn loads_std_runtime_packages_and_exports_from_explicit_root() {
        let root = test_std_package_root();
        let registry = RuntimePackageRegistry::load_default_with_package_roots(&[root.clone()])
            .expect("runtime registry");
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
        assert!(
            registry
                .package(&ModuleId::new("lux/ui"))
                .expect("ui package")
                .imports
                .contains(&ModuleId::new("lux/reactive"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compiles_runtime_package_from_lux_source() {
        let root = test_std_package_root();
        let registry = RuntimePackageRegistry::load_default_with_package_roots(&[root.clone()])
            .expect("runtime registry");
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
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compiles_reactive_and_ui_runtime_packages() {
        let root = test_std_package_root();
        let registry = RuntimePackageRegistry::load_default_with_package_roots(&[root.clone()])
            .expect("runtime registry");
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
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_dependency_closure_orders_dependencies_first() {
        let root = test_std_package_root();
        let registry = RuntimePackageRegistry::load_default_with_package_roots(&[root.clone()])
            .expect("runtime registry");
        let closure = registry
            .dependency_closure([ModuleId::new("lux/ui")])
            .expect("runtime closure");
        assert_eq!(
            closure.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            vec!["lux/reactive", "lux/ui"]
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
