use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::ast::Realm;
use crate::diag::{Diagnostic, Label};
use crate::source::SourceSpan;

pub use crate::ast::Realm as ModuleRealm;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ArtifactRealm {
    Client,
    Server,
}

impl ArtifactRealm {
    pub const ALL: [Self; 2] = [Self::Client, Self::Server];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
        }
    }

    pub const fn as_realm(self) -> Realm {
        match self {
            Self::Client => Realm::Client,
            Self::Server => Realm::Server,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RealmSet(u8);

impl RealmSet {
    pub const NONE: Self = Self(0);
    pub const CLIENT: Self = Self(0b01);
    pub const SERVER: Self = Self(0b10);
    pub const SHARED: Self = Self(0b11);

    pub const fn from_realm(realm: Realm) -> Self {
        match realm {
            Realm::Shared => Self::SHARED,
            Realm::Client => Self::CLIENT,
            Realm::Server => Self::SERVER,
        }
    }

    pub const fn from_artifact(realm: ArtifactRealm) -> Self {
        match realm {
            ArtifactRealm::Client => Self::CLIENT,
            ArtifactRealm::Server => Self::SERVER,
        }
    }

    pub const fn contains_all(self, required: Self) -> bool {
        (self.0 & required.0) == required.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn artifact_realms(self) -> Vec<ArtifactRealm> {
        ArtifactRealm::ALL
            .into_iter()
            .filter(|realm| self.intersects(Self::from_artifact(*realm)))
            .collect()
    }

    pub const fn display_name(self) -> &'static str {
        match self.0 {
            0b01 => "client",
            0b10 => "server",
            0b11 => "shared",
            _ => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealmAvailability {
    Known(RealmSet),
    UnknownExternal,
}

impl RealmAvailability {
    pub const fn known(realm: Realm) -> Self {
        Self::Known(RealmSet::from_realm(realm))
    }

    pub const fn shared() -> Self {
        Self::Known(RealmSet::SHARED)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId(String);

impl PackageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(normalize_module_path(&value.into()))
    }

    pub fn from_dir_name(value: &str) -> Self {
        Self::new(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(String);

impl ModuleId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(normalize_module_path(&value.into()))
    }

    pub fn from_package_path(package_id: &PackageId, module_path: &str) -> Self {
        let module_path = normalize_module_path(module_path);
        if module_path.is_empty() {
            Self::new(package_id.as_str())
        } else {
            Self::new(format!("{}/{}", package_id.as_str(), module_path))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn artifact_id(&self, realm: ArtifactRealm) -> String {
        format!("{}#{}", self.0, realm.as_str())
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInput {
    pub id: ModuleId,
    pub package_id: PackageId,
    pub module_path: String,
    pub span: SourceSpan,
    pub exports: Vec<ModuleExport>,
    pub imports: Vec<ModuleImport>,
}

impl ModuleInput {
    pub fn new(
        package_id: PackageId,
        module_path: impl Into<String>,
        span: SourceSpan,
        exports: Vec<ModuleExport>,
        imports: Vec<ModuleImport>,
    ) -> Self {
        let module_path = normalize_module_path(&module_path.into());
        let id = ModuleId::from_package_path(&package_id, &module_path);
        Self {
            id,
            package_id,
            module_path,
            span,
            exports,
            imports,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleExport {
    pub name: String,
    pub realms: RealmSet,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleImport {
    pub raw_source: String,
    pub target: ModuleId,
    pub specifiers: Vec<ModuleImportSpecifier>,
    pub side_effect_only: bool,
    pub active_realms: RealmSet,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleImportSpecifier {
    pub imported: String,
    pub local: String,
    pub namespace: bool,
    pub active_realms: RealmSet,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleNode {
    pub id: ModuleId,
    pub package_id: PackageId,
    pub module_path: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModuleEdge {
    pub from: ModuleId,
    pub raw_source: String,
    pub to: ModuleRef,
    pub side_effect_only: bool,
    pub active_realms: RealmSet,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModuleRef {
    Project(ModuleId),
    External(ModuleId),
}

impl ModuleRef {
    pub fn id(&self) -> &ModuleId {
        match self {
            Self::Project(id) | Self::External(id) => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleGraphConfig {
    pub external_modules: BTreeSet<ModuleId>,
    pub external_exports: BTreeMap<ModuleId, Vec<ModuleExport>>,
}

impl Default for ModuleGraphConfig {
    fn default() -> Self {
        Self {
            external_modules: BTreeSet::new(),
            external_exports: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleGraph {
    pub nodes: Vec<ModuleNode>,
    pub edges: Vec<ResolvedModuleEdge>,
    pub order: Vec<ModuleId>,
    pub required_externals: Vec<ModuleId>,
}

impl ModuleGraph {
    pub fn build(inputs: Vec<ModuleInput>) -> Result<Self, Vec<Diagnostic>> {
        Self::build_with_config(inputs, ModuleGraphConfig::default())
    }

    pub fn build_with_config(
        inputs: Vec<ModuleInput>,
        config: ModuleGraphConfig,
    ) -> Result<Self, Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();
        let mut nodes_by_id = BTreeMap::<ModuleId, ModuleNode>::new();
        let mut exports_by_id = BTreeMap::<ModuleId, Vec<ModuleExport>>::new();

        for input in &inputs {
            let node = ModuleNode {
                id: input.id.clone(),
                package_id: input.package_id.clone(),
                module_path: input.module_path.clone(),
                span: input.span,
            };

            if let Some(existing) = nodes_by_id.insert(input.id.clone(), node) {
                diagnostics.push(
                    Diagnostic::error(format!("duplicate Lux module id `{}`", input.id))
                        .with_code("MODULE005")
                        .with_label(Label::primary(input.span, "duplicate module"))
                        .with_label(Label::secondary(existing.span, "first module with this id")),
                );
            }

            validate_duplicate_exports(&input.id, &input.exports, &mut diagnostics);
            exports_by_id.insert(input.id.clone(), input.exports.clone());
        }

        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }

        let mut edges = Vec::new();
        for input in &inputs {
            for import in &input.imports {
                let target = if nodes_by_id.contains_key(&import.target) {
                    ModuleRef::Project(import.target.clone())
                } else if config.external_modules.contains(&import.target) {
                    ModuleRef::External(import.target.clone())
                } else {
                    diagnostics.push(missing_module(&import.raw_source, import.span));
                    continue;
                };

                validate_import_specifiers(
                    import,
                    &target,
                    &exports_by_id,
                    &config,
                    &mut diagnostics,
                );
                edges.push(ResolvedModuleEdge {
                    from: input.id.clone(),
                    raw_source: import.raw_source.clone(),
                    to: target,
                    side_effect_only: import.side_effect_only,
                    active_realms: import.active_realms,
                    span: import.span,
                });
            }
        }

        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }

        let order = topo_order(&nodes_by_id, &edges, &mut diagnostics);
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }

        let mut required_externals = edges
            .iter()
            .filter_map(|edge| match &edge.to {
                ModuleRef::External(id) => Some(id.clone()),
                ModuleRef::Project(_) => None,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        required_externals.sort();

        Ok(Self {
            nodes: nodes_by_id.into_values().collect(),
            edges,
            order,
            required_externals,
        })
    }

    pub fn imports_for(&self, id: &ModuleId) -> Vec<&ResolvedModuleEdge> {
        self.edges.iter().filter(|edge| &edge.from == id).collect()
    }

    pub fn node(&self, id: &ModuleId) -> Option<&ModuleNode> {
        self.nodes.iter().find(|node| &node.id == id)
    }
}

fn validate_duplicate_exports(
    id: &ModuleId,
    exports: &[ModuleExport],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen = BTreeMap::<String, &ModuleExport>::new();
    for export in exports {
        if let Some(first) = seen.insert(export.name.clone(), export) {
            diagnostics.push(
                Diagnostic::error(format!(
                    "module `{id}` exports `{}` more than once",
                    export.name
                ))
                .with_code("MODULE009")
                .with_label(Label::primary(export.span, "duplicate export"))
                .with_label(Label::secondary(first.span, "first export with this name")),
            );
        }
    }
}

fn validate_import_specifiers(
    import: &ModuleImport,
    target: &ModuleRef,
    project_exports: &BTreeMap<ModuleId, Vec<ModuleExport>>,
    config: &ModuleGraphConfig,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if import.side_effect_only || import.specifiers.is_empty() {
        return;
    }

    let exports = match target {
        ModuleRef::Project(id) => project_exports.get(id),
        ModuleRef::External(id) => config.external_exports.get(id),
    };

    let Some(exports) = exports else {
        diagnostics.push(
            Diagnostic::error(format!(
                "cannot validate exports for Lux module `{}`",
                target.id()
            ))
            .with_code("MODULE007")
            .with_label(Label::primary(import.span, "module has no export metadata")),
        );
        return;
    };

    for specifier in &import.specifiers {
        if specifier.namespace {
            continue;
        }
        let matching = exports
            .iter()
            .find(|export| export.name == specifier.imported);
        let Some(export) = matching else {
            diagnostics.push(
                Diagnostic::error(format!(
                    "module `{}` does not export `{}`",
                    target.id(),
                    specifier.imported
                ))
                .with_code("MODULE008")
                .with_label(Label::primary(specifier.span, "missing export"))
                .with_help(format!(
                    "export `{}` from `{}` or change this import",
                    specifier.imported,
                    target.id()
                )),
            );
            continue;
        };
        let active_realms = if specifier.active_realms.is_empty() {
            import.active_realms
        } else {
            specifier.active_realms
        };
        if !export.realms.contains_all(active_realms) {
            diagnostics.push(
                Diagnostic::error(format!(
                    "module `{}` export `{}` is not available in {} context",
                    target.id(),
                    specifier.imported,
                    active_realms.display_name()
                ))
                .with_code("MODULE010")
                .with_label(Label::primary(specifier.span, "realm-incompatible import"))
                .with_label(Label::secondary(
                    export.span,
                    format!("export is {}", export.realms.display_name()),
                )),
            );
        }
    }
}

fn topo_order(
    nodes_by_id: &BTreeMap<ModuleId, ModuleNode>,
    edges: &[ResolvedModuleEdge],
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ModuleId> {
    let mut deps = BTreeMap::<ModuleId, Vec<(&ModuleId, SourceSpan)>>::new();
    for edge in edges {
        if let ModuleRef::Project(to) = &edge.to {
            deps.entry(edge.from.clone())
                .or_default()
                .push((to, edge.span));
        }
    }

    for values in deps.values_mut() {
        values.sort_by(|a, b| a.0.cmp(b.0));
    }

    let mut marks = BTreeMap::<ModuleId, VisitMark>::new();
    let mut stack = Vec::<ModuleId>::new();
    let mut order = Vec::<ModuleId>::new();

    for id in nodes_by_id.keys() {
        if !diagnostics.is_empty() {
            break;
        }
        visit(
            id,
            &deps,
            &mut marks,
            &mut stack,
            &mut order,
            diagnostics,
            None,
        );
    }

    order
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitMark {
    Visiting,
    Done,
}

fn visit(
    id: &ModuleId,
    deps: &BTreeMap<ModuleId, Vec<(&ModuleId, SourceSpan)>>,
    marks: &mut BTreeMap<ModuleId, VisitMark>,
    stack: &mut Vec<ModuleId>,
    order: &mut Vec<ModuleId>,
    diagnostics: &mut Vec<Diagnostic>,
    incoming_span: Option<SourceSpan>,
) {
    if !diagnostics.is_empty() {
        return;
    }

    match marks.get(id).copied() {
        Some(VisitMark::Done) => return,
        Some(VisitMark::Visiting) => {
            let cycle = if let Some(index) = stack.iter().position(|stack_id| stack_id == id) {
                stack[index..]
                    .iter()
                    .chain(std::iter::once(id))
                    .map(|module_id| module_id.as_str())
                    .collect::<Vec<_>>()
                    .join(" -> ")
            } else {
                id.as_str().to_string()
            };
            let Some(span) = incoming_span else {
                return;
            };
            diagnostics.push(
                Diagnostic::error(format!("cyclic Lux module dependency: {cycle}"))
                    .with_code("MODULE004")
                    .with_label(Label::primary(span, "this import closes the cycle")),
            );
            return;
        }
        None => {}
    }

    marks.insert(id.clone(), VisitMark::Visiting);
    stack.push(id.clone());

    if let Some(dependencies) = deps.get(id) {
        for (dependency, span) in dependencies {
            visit(
                dependency,
                deps,
                marks,
                stack,
                order,
                diagnostics,
                Some(*span),
            );
            if !diagnostics.is_empty() {
                return;
            }
        }
    }

    stack.pop();
    marks.insert(id.clone(), VisitMark::Done);
    order.push(id.clone());
}

fn missing_module(raw: &str, span: SourceSpan) -> Diagnostic {
    Diagnostic::error(format!("cannot resolve Lux module `{raw}`"))
        .with_code("MODULE001")
        .with_label(Label::primary(span, "unresolved import"))
}

pub fn normalize_module_path(value: &str) -> String {
    let value = value.trim().replace('\\', "/");
    let mut parts = Vec::<String>::new();

    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| last != "..") {
                    parts.pop();
                } else {
                    parts.push(part.to_string());
                }
            }
            other => parts.push(other.to_string()),
        }
    }

    if let Some(last) = parts.last_mut() {
        if let Some(stripped) = last.strip_suffix(".lux") {
            *last = stripped.to_string();
        }
    }

    parts.join("/")
}

pub fn normalize_relative_module_path(base_module_path: &str, raw: &str) -> Result<String, String> {
    let mut parts = normalize_module_path(base_module_path)
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    for part in raw.replace('\\', "/").split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!("relative import `{raw}` escapes the package root"));
                }
            }
            other => parts.push(other.to_string()),
        }
    }

    Ok(parts.join("/"))
}
