use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

const PROJECT_MANIFEST: &str = "lux.toml";
const PACKAGE_SET_MANIFEST: &str = "lux.package.toml";
const LOCKFILE: &str = "lux.lock";
pub const LUX_STD_REPO: &str = "TimeWatcher/lux-std";
pub const LUX_STD_PACKAGE: &str = "@lux/std";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    pub root: PathBuf,
    pub name: String,
    pub install_std: bool,
    pub output_root: Option<PathBuf>,
    pub runtime_base: Option<PathBuf>,
    pub autorun: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRequest {
    pub project_root: PathBuf,
    pub package: String,
    pub source: DependencySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockRequest {
    pub project_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveRequest {
    pub project_root: PathBuf,
    pub package: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencySource {
    Github {
        repo: String,
        tag: Option<String>,
        branch: Option<String>,
        commit: Option<String>,
    },
    Url(String),
    Path(PathBuf),
}

impl DependencySource {
    pub fn github_ref(&self) -> Option<&str> {
        match self {
            Self::Github {
                tag: Some(value), ..
            }
            | Self::Github {
                branch: Some(value),
                ..
            }
            | Self::Github {
                commit: Some(value),
                ..
            } => Some(value),
            _ => None,
        }
    }

    pub fn stable_key(&self) -> String {
        match self {
            Self::Github {
                repo,
                tag,
                branch,
                commit,
            } => {
                let mut key = format!("github:{repo}");
                if let Some(tag) = tag {
                    key.push_str(":tag:");
                    key.push_str(tag);
                } else if let Some(branch) = branch {
                    key.push_str(":branch:");
                    key.push_str(branch);
                } else if let Some(commit) = commit {
                    key.push_str(":commit:");
                    key.push_str(commit);
                }
                key
            }
            Self::Url(url) => format!("url:{url}"),
            Self::Path(path) => format!("path:{}", path.display()),
        }
    }

    fn manifest_value(&self) -> TomlValue {
        match self {
            Self::Github {
                repo,
                tag,
                branch,
                commit,
            } => {
                let mut fields = BTreeMap::new();
                fields.insert("github".into(), TomlValue::String(repo.clone()));
                if let Some(tag) = tag {
                    fields.insert("tag".into(), TomlValue::String(tag.clone()));
                }
                if let Some(branch) = branch {
                    fields.insert("branch".into(), TomlValue::String(branch.clone()));
                }
                if let Some(commit) = commit {
                    fields.insert("commit".into(), TomlValue::String(commit.clone()));
                }
                TomlValue::InlineTable(fields)
            }
            Self::Url(url) => {
                let mut fields = BTreeMap::new();
                fields.insert("url".into(), TomlValue::String(url.clone()));
                TomlValue::InlineTable(fields)
            }
            Self::Path(path) => {
                let mut fields = BTreeMap::new();
                fields.insert(
                    "path".into(),
                    TomlValue::String(path.to_string_lossy().to_string()),
                );
                TomlValue::InlineTable(fields)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSetManifest {
    pub name: Option<String>,
    #[serde(default)]
    pub package: Vec<PackageEntry>,
    #[serde(default)]
    pub source: Vec<PackageSourceHint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageEntry {
    pub id: String,
    pub version: Option<String>,
    pub path: PathBuf,
    pub license: Option<String>,
    #[serde(default)]
    pub realm: Vec<String>,
    #[serde(default)]
    pub depends: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSourceHint {
    pub package: String,
    pub github: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub url: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Lockfile {
    #[serde(default)]
    pub package: Vec<LockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub id: String,
    pub version: String,
    pub direct: bool,
    pub root: PathBuf,
    pub package_path: PathBuf,
    pub source: LockedSource,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum LockedSource {
    Github {
        repo: String,
        tag: Option<String>,
        branch: Option<String>,
        commit: Option<String>,
        resolved: String,
    },
    Url {
        url: String,
    },
    Path {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutput {
    pub package_id: String,
    pub package_root: PathBuf,
    pub direct_count: usize,
    pub total_count: usize,
    pub lock_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockOutput {
    pub project_root: PathBuf,
    pub direct_count: usize,
    pub total_count: usize,
    pub lock_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutput {
    pub package_id: String,
    pub direct_count: usize,
    pub total_count: usize,
    pub lock_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    pub project_root: PathBuf,
    pub manifest_path: PathBuf,
    pub lock_path: PathBuf,
    pub dependency_count: usize,
    pub locked_count: usize,
    pub package_roots: Vec<PathBuf>,
}

#[derive(Debug)]
pub enum PackageManagerError {
    Io { path: PathBuf, source: io::Error },
    Http(String),
    Zip { path: PathBuf, message: String },
    Parse { path: PathBuf, message: String },
    Invalid(String),
}

impl fmt::Display for PackageManagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Http(message) | Self::Invalid(message) => f.write_str(message),
            Self::Zip { path, message } => {
                write!(f, "failed to unpack {}: {message}", path.display())
            }
            Self::Parse { path, message } => write!(f, "invalid {}: {message}", path.display()),
        }
    }
}

impl std::error::Error for PackageManagerError {}

pub fn init_project(options: &InitOptions) -> Result<(), PackageManagerError> {
    fs::create_dir_all(&options.root).map_err(|source| PackageManagerError::Io {
        path: options.root.clone(),
        source,
    })?;
    let source_root = options.root.join("src");
    fs::create_dir_all(&source_root).map_err(|source| PackageManagerError::Io {
        path: source_root.clone(),
        source,
    })?;
    write_new_file(
        &options.root.join(PROJECT_MANIFEST),
        &project_manifest_template(options),
    )?;
    write_new_file(&source_root.join("module.lux"), "export fn main() = true\n")?;
    if options.install_std {
        install_package(&InstallRequest {
            project_root: options.root.clone(),
            package: LUX_STD_PACKAGE.into(),
            source: lux_std_source(),
        })?;
    }
    Ok(())
}

pub fn lux_std_source() -> DependencySource {
    DependencySource::Github {
        repo: LUX_STD_REPO.into(),
        tag: None,
        branch: None,
        commit: None,
    }
}

pub fn install_package(request: &InstallRequest) -> Result<InstallOutput, PackageManagerError> {
    let project_root = canonical_or_current(&request.project_root)?;
    let manifest_path = project_root.join(PROJECT_MANIFEST);
    let mut manifest = ProjectDependencyManifest::load_or_new(&manifest_path)?;
    manifest.set_dependency(&request.package, &request.source);
    manifest.write(&manifest_path)?;

    let cache = CacheLayout::new()?;
    let package_root = resolve_dependency_source_root(&request.source, &cache, &project_root)?;
    let lock = regenerate_lockfile(&project_root, &manifest)?;
    Ok(InstallOutput {
        package_id: normalize_package_display(&request.package),
        package_root,
        direct_count: lock.direct_count,
        total_count: lock.total_count,
        lock_path: lock.lock_path,
    })
}

pub fn lock_project(request: &LockRequest) -> Result<LockOutput, PackageManagerError> {
    let project_root = canonical_or_current(&request.project_root)?;
    let manifest_path = project_root.join(PROJECT_MANIFEST);
    let manifest = ProjectDependencyManifest::load_or_new(&manifest_path)?;
    regenerate_lockfile(&project_root, &manifest)
}

pub fn remove_package(request: &RemoveRequest) -> Result<RemoveOutput, PackageManagerError> {
    let project_root = canonical_or_current(&request.project_root)?;
    let manifest_path = project_root.join(PROJECT_MANIFEST);
    let mut manifest = ProjectDependencyManifest::load_or_new(&manifest_path)?;
    let package_id = normalize_package_display(&request.package);
    if !manifest.remove_dependency(&package_id) {
        return Err(PackageManagerError::Invalid(format!(
            "package `{package_id}` is not a direct dependency"
        )));
    }
    manifest.write(&manifest_path)?;
    let lock = regenerate_lockfile(&project_root, &manifest)?;
    Ok(RemoveOutput {
        package_id,
        direct_count: lock.direct_count,
        total_count: lock.total_count,
        lock_path: lock.lock_path,
    })
}

pub fn lockfile_package_roots(project_root: &Path) -> Result<Vec<PathBuf>, PackageManagerError> {
    let lock_path = project_root.join(LOCKFILE);
    if !lock_path.is_file() {
        return Ok(Vec::new());
    }
    let lockfile = read_toml::<Lockfile>(&lock_path)?;
    let mut roots = BTreeSet::new();
    for package in lockfile.package {
        roots.insert(package.root);
    }
    Ok(roots.into_iter().collect())
}

pub fn doctor(project_root: &Path) -> Result<DoctorReport, PackageManagerError> {
    let project_root = canonical_or_current(project_root)?;
    let manifest_path = project_root.join(PROJECT_MANIFEST);
    let lock_path = project_root.join(LOCKFILE);
    let manifest = ProjectDependencyManifest::load_or_new(&manifest_path)?;
    let package_roots = lockfile_package_roots(&project_root)?;
    let locked_count = if lock_path.is_file() {
        read_toml::<Lockfile>(&lock_path)?.package.len()
    } else {
        0
    };
    Ok(DoctorReport {
        project_root,
        manifest_path,
        lock_path,
        dependency_count: manifest.dependencies.len(),
        locked_count,
        package_roots,
    })
}

pub fn list_locked(project_root: &Path) -> Result<Vec<LockedPackage>, PackageManagerError> {
    let project_root = canonical_or_current(project_root)?;
    let lock_path = project_root.join(LOCKFILE);
    if !lock_path.is_file() {
        return Ok(Vec::new());
    }
    Ok(read_toml::<Lockfile>(&lock_path)?.package)
}

fn project_manifest_template(options: &InitOptions) -> String {
    let name = escape_toml_string(&options.name);
    let output_root = options
        .output_root
        .as_ref()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| "generated/lua".into());
    let runtime_base = options
        .runtime_base
        .as_ref()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| format!("lux/{}", options.name));
    format!(
        "package_id = \"{name}\"\nbundle_id = \"{name}\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"{}\"\nruntime_base = \"{}\"\nautorun = {}\nsource_comments = \"boundary\"\n\n[dependencies]\n",
        escape_toml_string(&output_root),
        escape_toml_string(&runtime_base),
        options.autorun
    )
}

#[derive(Debug, Clone, Default)]
struct ProjectDependencyManifest {
    pre_dependencies: Vec<String>,
    dependencies: BTreeMap<String, TomlValue>,
    post_dependencies: Vec<String>,
}

impl ProjectDependencyManifest {
    fn load_or_new(path: &Path) -> Result<Self, PackageManagerError> {
        if !path.is_file() {
            return Ok(Self {
                pre_dependencies: vec![
                    "package_id = \"lux-project\"".into(),
                    "bundle_id = \"lux-project\"".into(),
                    "".into(),
                    "[target.gmod]".into(),
                    "source_root = \"src\"".into(),
                    "out = \"generated/lua\"".into(),
                    "runtime_base = \"lux/lux-project\"".into(),
                    "autorun = true".into(),
                    "".into(),
                ],
                dependencies: BTreeMap::new(),
                post_dependencies: Vec::new(),
            });
        }
        let text = fs::read_to_string(path).map_err(|source| PackageManagerError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(parse_project_dependency_manifest(&text))
    }

    fn set_dependency(&mut self, package: &str, source: &DependencySource) {
        self.dependencies
            .insert(normalize_package_display(package), source.manifest_value());
    }

    fn remove_dependency(&mut self, package: &str) -> bool {
        self.dependencies
            .remove(&normalize_package_display(package))
            .is_some()
    }

    fn write(&self, path: &Path) -> Result<(), PackageManagerError> {
        let mut lines = Vec::new();
        lines.extend(self.pre_dependencies.clone());
        if lines.last().is_some_and(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[dependencies]".into());
        for (name, value) in &self.dependencies {
            lines.push(format!("\"{name}\" = {}", value.render()));
        }
        lines.extend(self.post_dependencies.clone());
        write_file(path, &(lines.join("\n") + "\n"))
    }
}

fn regenerate_lockfile(
    project_root: &Path,
    manifest: &ProjectDependencyManifest,
) -> Result<LockOutput, PackageManagerError> {
    let cache = CacheLayout::new()?;
    let resolved = resolve_manifest_dependencies(manifest, project_root, &cache)?;
    let lock_path = project_root.join(LOCKFILE);
    let lockfile = Lockfile { package: resolved };
    let total_count = lockfile.package.len();
    write_toml(&lock_path, &lockfile)?;
    Ok(LockOutput {
        project_root: project_root.to_path_buf(),
        direct_count: manifest.dependencies.len(),
        total_count,
        lock_path,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TomlValue {
    String(String),
    InlineTable(BTreeMap<String, TomlValue>),
    Raw(String),
}

impl TomlValue {
    fn render(&self) -> String {
        match self {
            Self::String(value) => format!("\"{}\"", escape_toml_string(value)),
            Self::Raw(value) => value.clone(),
            Self::InlineTable(fields) => {
                let body = fields
                    .iter()
                    .map(|(key, value)| format!("{key} = {}", value.render()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{ {body} }}")
            }
        }
    }
}

fn parse_project_dependency_manifest(text: &str) -> ProjectDependencyManifest {
    let mut pre_dependencies = Vec::new();
    let mut dependencies = BTreeMap::new();
    let mut post_dependencies = Vec::new();
    let mut in_dependencies = false;
    let mut seen_dependencies = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_dependencies = trimmed == "[dependencies]";
            if in_dependencies {
                seen_dependencies = true;
                continue;
            }
        }
        if in_dependencies {
            if let Some((key, value)) = trimmed.split_once('=') {
                let key = key.trim().trim_matches('"').to_string();
                dependencies.insert(key, TomlValue::Raw(value.trim().to_string()));
            } else if !trimmed.is_empty() {
                post_dependencies.push(line.to_string());
            }
        } else if seen_dependencies {
            post_dependencies.push(line.to_string());
        } else {
            pre_dependencies.push(line.to_string());
        }
    }

    ProjectDependencyManifest {
        pre_dependencies,
        dependencies,
        post_dependencies,
    }
}

fn resolve_manifest_dependencies(
    manifest: &ProjectDependencyManifest,
    project_root: &Path,
    cache: &CacheLayout,
) -> Result<Vec<LockedPackage>, PackageManagerError> {
    let mut source_sets = SourceSetIndex::new(project_root.to_path_buf(), cache.clone());
    let mut locked = Vec::new();
    let mut resolved = BTreeMap::<String, LockedPackage>::new();
    let mut requirements = BTreeMap::<String, Vec<VersionReq>>::new();

    for (package, source_spec) in &manifest.dependencies {
        let package_id = normalize_package_display(package);
        let source = dependency_source_from_toml_value(source_spec, project_root)?;
        let package_set = source_sets.load_source(source)?;
        ensure_source_provides(&package_id, &package_set)?;
        resolve_package_tree(
            &package_id,
            true,
            &package_set,
            &mut source_sets,
            &mut requirements,
            &mut resolved,
        )?;
    }

    locked.extend(resolved.into_values());
    locked.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(locked)
}

fn resolve_package_tree(
    package_id: &str,
    direct: bool,
    package_set: &LoadedPackageSet,
    source_sets: &mut SourceSetIndex,
    requirements: &mut BTreeMap<String, Vec<VersionReq>>,
    resolved: &mut BTreeMap<String, LockedPackage>,
) -> Result<(), PackageManagerError> {
    let package_id = normalize_package_display(package_id);
    if let Some(locked) = resolved.get_mut(&package_id) {
        if direct {
            locked.direct = true;
        }
        return Ok(());
    }

    let package = package_set
        .manifest
        .package
        .iter()
        .find(|package| normalize_package_display(&package.id) == package_id)
        .ok_or_else(|| {
            PackageManagerError::Invalid(format!(
                "package `{package_id}` was not found in {}",
                package_set.root.join(PACKAGE_SET_MANIFEST).display()
            ))
        })?;

    let version = package
        .version
        .as_deref()
        .map(|value| Version::parse(value))
        .transpose()
        .map_err(|err| {
            PackageManagerError::Invalid(format!(
                "invalid version for `{package_id}` in {}: {err}",
                package_set.root.join(PACKAGE_SET_MANIFEST).display()
            ))
        })?;
    if let Some(version) = &version {
        check_package_version(&package_id, version, requirements)?;
    }

    for dep in &package.depends {
        let dep_req = parse_dependency_spec(dep)?;
        let dep_id = dep_req.package.clone();
        requirements
            .entry(dep_id.clone())
            .or_default()
            .push(dep_req.requirement.clone());
        if let Some(locked) = resolved.get(&dep_id) {
            if let Ok(version) = Version::parse(&locked.version) {
                check_package_version(&dep_id, &version, requirements)?;
            }
            continue;
        }
        let dep_set = source_sets.resolve_dependency_source(&dep_req)?;
        resolve_package_tree(
            &dep_id,
            false,
            &dep_set,
            source_sets,
            requirements,
            resolved,
        )?;
    }

    resolved.insert(
        package_id.clone(),
        LockedPackage {
            id: package_id,
            version: version
                .map(|value| value.to_string())
                .unwrap_or_else(|| package.version.clone().unwrap_or_else(|| "0.0.0".into())),
            direct,
            root: package_set.root.clone(),
            package_path: package.path.clone(),
            source: locked_source(&package_set.source),
            sha256: package_set.sha256.clone(),
        },
    );
    Ok(())
}

#[derive(Debug, Clone)]
struct LoadedPackageSet {
    root: PathBuf,
    source: DependencySource,
    manifest: PackageSetManifest,
    sha256: Option<String>,
}

#[derive(Debug, Clone)]
struct SourceSetIndex {
    project_root: PathBuf,
    cache: CacheLayout,
    by_key: BTreeMap<String, LoadedPackageSet>,
    sources_by_package: BTreeMap<String, DependencySource>,
}

impl SourceSetIndex {
    fn new(project_root: PathBuf, cache: CacheLayout) -> Self {
        Self {
            project_root,
            cache,
            by_key: BTreeMap::new(),
            sources_by_package: BTreeMap::new(),
        }
    }

    fn load_source(
        &mut self,
        source: DependencySource,
    ) -> Result<LoadedPackageSet, PackageManagerError> {
        let key = source.stable_key();
        if let Some(package_set) = self.by_key.get(&key) {
            return Ok(package_set.clone());
        }
        let root = fetch_source(&source, &self.cache, &self.project_root)?;
        let manifest = load_package_set(&root)?;
        let sha256 = source_digest(&source, &root)?;
        let package_set = LoadedPackageSet {
            root,
            source: source.clone(),
            manifest,
            sha256,
        };
        for package in &package_set.manifest.package {
            self.sources_by_package
                .insert(normalize_package_display(&package.id), source.clone());
        }
        for hint in &package_set.manifest.source {
            let hinted_source = dependency_source_from_hint(hint, &package_set.root)?;
            self.sources_by_package
                .insert(normalize_package_display(&hint.package), hinted_source);
        }
        self.by_key.insert(key, package_set.clone());
        Ok(package_set)
    }

    fn resolve_dependency_source(
        &mut self,
        dependency: &DependencySpec,
    ) -> Result<LoadedPackageSet, PackageManagerError> {
        let Some(source) = self.sources_by_package.get(&dependency.package).cloned() else {
            return Err(PackageManagerError::Invalid(format!(
                "package `{}` is required but no source is known; add a [[source]] entry in the package set that declared this dependency",
                dependency.package
            )));
        };
        let package_set = self.load_source(source)?;
        ensure_source_provides(&dependency.package, &package_set)?;
        Ok(package_set)
    }
}

#[derive(Debug, Clone)]
struct DependencySpec {
    package: String,
    requirement: VersionReq,
}

fn parse_dependency_spec(value: &str) -> Result<DependencySpec, PackageManagerError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(PackageManagerError::Invalid(
            "dependency entry cannot be empty".into(),
        ));
    }
    let mut parts = trimmed.split_whitespace();
    let Some(package) = parts.next() else {
        return Err(PackageManagerError::Invalid(
            "dependency entry must start with a package id".into(),
        ));
    };
    let requirement_text = parts.collect::<Vec<_>>().join(" ");
    let requirement = if requirement_text.is_empty() {
        VersionReq::STAR
    } else {
        let normalized_requirement = normalize_version_requirement(&requirement_text);
        VersionReq::parse(&normalized_requirement).map_err(|err| {
            PackageManagerError::Invalid(format!(
                "invalid version requirement `{requirement_text}` for `{package}`: {err}"
            ))
        })?
    };
    Ok(DependencySpec {
        package: normalize_package_display(package),
        requirement,
    })
}

fn normalize_version_requirement(value: &str) -> String {
    let parts = value
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() > 1 && !value.contains(',') {
        parts.join(", ")
    } else {
        value.trim().to_string()
    }
}

fn check_package_version(
    package_id: &str,
    version: &Version,
    requirements: &BTreeMap<String, Vec<VersionReq>>,
) -> Result<(), PackageManagerError> {
    if let Some(reqs) = requirements.get(package_id) {
        for req in reqs {
            if !req.matches(version) {
                return Err(PackageManagerError::Invalid(format!(
                    "version conflict for `{package_id}`: locked {version} does not satisfy `{req}`"
                )));
            }
        }
    }
    Ok(())
}

fn ensure_source_provides(
    package_id: &str,
    package_set: &LoadedPackageSet,
) -> Result<(), PackageManagerError> {
    if package_set
        .manifest
        .package
        .iter()
        .any(|package| normalize_package_display(&package.id) == package_id)
    {
        Ok(())
    } else {
        Err(PackageManagerError::Invalid(format!(
            "source {} does not provide requested package `{package_id}`",
            package_set.root.display()
        )))
    }
}

fn resolve_dependency_source_root(
    source: &DependencySource,
    cache: &CacheLayout,
    project_root: &Path,
) -> Result<PathBuf, PackageManagerError> {
    fetch_source(source, cache, project_root)
}

fn dependency_source_from_hint(
    hint: &PackageSourceHint,
    source_root: &Path,
) -> Result<DependencySource, PackageManagerError> {
    let count = [
        hint.github.is_some(),
        hint.url.is_some(),
        hint.path.is_some(),
    ]
    .into_iter()
    .filter(|value| *value)
    .count();
    if count != 1 {
        return Err(PackageManagerError::Invalid(format!(
            "[[source]] for `{}` must set exactly one of github, url, or path",
            hint.package
        )));
    }
    if let Some(repo) = &hint.github {
        return Ok(DependencySource::Github {
            repo: repo.clone(),
            tag: hint.tag.clone(),
            branch: hint.branch.clone(),
            commit: hint.commit.clone(),
        });
    }
    if let Some(url) = &hint.url {
        if hint.tag.is_some() || hint.branch.is_some() || hint.commit.is_some() {
            return Err(PackageManagerError::Invalid(format!(
                "[[source]] url package `{}` cannot set tag, branch, or commit",
                hint.package
            )));
        }
        return Ok(DependencySource::Url(url.clone()));
    }
    let path = hint.path.clone().expect("count checked path");
    if hint.tag.is_some() || hint.branch.is_some() || hint.commit.is_some() {
        return Err(PackageManagerError::Invalid(format!(
            "[[source]] path package `{}` cannot set tag, branch, or commit",
            hint.package
        )));
    }
    let path = if path.is_absolute() {
        path
    } else {
        source_root.join(path)
    };
    Ok(DependencySource::Path(path))
}

fn dependency_source_from_toml_value(
    value: &TomlValue,
    project_root: &Path,
) -> Result<DependencySource, PackageManagerError> {
    match value {
        TomlValue::String(value) => Ok(DependencySource::Path(project_root.join(value))),
        TomlValue::Raw(value) => dependency_source_from_raw_toml(value, project_root),
        TomlValue::InlineTable(fields) => dependency_source_from_toml_fields(fields, project_root),
    }
}

fn dependency_source_from_raw_toml(
    value: &str,
    project_root: &Path,
) -> Result<DependencySource, PackageManagerError> {
    let parsed = parse_toml_value_fragment(value)?;
    match parsed {
        toml::Value::String(value) => Ok(DependencySource::Path(project_root.join(value))),
        toml::Value::Table(table) => {
            let fields = table
                .into_iter()
                .map(|(key, value)| toml_value_to_internal(key, value))
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            dependency_source_from_toml_fields(&fields, project_root)
        }
        _ => Err(PackageManagerError::Invalid(format!(
            "dependency source must be a path string or an inline table, got `{value}`"
        ))),
    }
}

fn parse_toml_value_fragment(value: &str) -> Result<toml::Value, PackageManagerError> {
    let wrapped = format!("source = {value}");
    let mut table =
        toml::from_str::<toml::Table>(&wrapped).map_err(|source| PackageManagerError::Parse {
            path: PathBuf::from(PROJECT_MANIFEST),
            message: source.to_string(),
        })?;
    table.remove("source").ok_or_else(|| {
        PackageManagerError::Invalid(format!("dependency source value is empty: `{value}`"))
    })
}

fn toml_value_to_internal(
    key: String,
    value: toml::Value,
) -> Result<(String, TomlValue), PackageManagerError> {
    match value {
        toml::Value::String(value) => Ok((key, TomlValue::String(value))),
        toml::Value::Boolean(value) => Ok((key, TomlValue::Raw(value.to_string()))),
        other => Err(PackageManagerError::Invalid(format!(
            "dependency source field `{key}` must be a string or boolean, got {other:?}"
        ))),
    }
}

fn dependency_source_from_toml_fields(
    fields: &BTreeMap<String, TomlValue>,
    project_root: &Path,
) -> Result<DependencySource, PackageManagerError> {
    let string_field = |name: &str| match fields.get(name) {
        Some(TomlValue::String(value)) => Ok(Some(value.clone())),
        Some(TomlValue::Raw(value)) if value.starts_with('"') => {
            dependency_source_string_from_raw(value).map(Some)
        }
        Some(_) => Err(PackageManagerError::Invalid(format!(
            "dependency source field `{name}` must be a string"
        ))),
        None => Ok(None),
    };
    let github = string_field("github")?;
    let url = string_field("url")?;
    let path = string_field("path")?;
    let tag = string_field("tag")?;
    let branch = string_field("branch")?;
    let commit = string_field("commit")?;

    let source_count = [github.is_some(), url.is_some(), path.is_some()]
        .into_iter()
        .filter(|value| *value)
        .count();
    if source_count != 1 {
        return Err(PackageManagerError::Invalid(
            "dependency source must set exactly one of github, url, or path".into(),
        ));
    }
    if [tag.as_ref(), branch.as_ref(), commit.as_ref()]
        .into_iter()
        .flatten()
        .count()
        > 1
    {
        return Err(PackageManagerError::Invalid(
            "dependency source may set only one of tag, branch, or commit".into(),
        ));
    }

    if let Some(repo) = github {
        return Ok(DependencySource::Github {
            repo,
            tag,
            branch,
            commit,
        });
    }
    if let Some(url) = url {
        if tag.is_some() || branch.is_some() || commit.is_some() {
            return Err(PackageManagerError::Invalid(
                "url dependency source cannot set tag, branch, or commit".into(),
            ));
        }
        return Ok(DependencySource::Url(url));
    }
    let path = PathBuf::from(path.expect("source_count checked path"));
    if tag.is_some() || branch.is_some() || commit.is_some() {
        return Err(PackageManagerError::Invalid(
            "path dependency source cannot set tag, branch, or commit".into(),
        ));
    }
    Ok(DependencySource::Path(if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    }))
}

fn dependency_source_string_from_raw(value: &str) -> Result<String, PackageManagerError> {
    toml::from_str::<String>(value).map_err(|source| PackageManagerError::Parse {
        path: PathBuf::from(PROJECT_MANIFEST),
        message: source.to_string(),
    })
}

fn locked_source(source: &DependencySource) -> LockedSource {
    match source {
        DependencySource::Github {
            repo,
            tag,
            branch,
            commit,
        } => LockedSource::Github {
            repo: repo.clone(),
            tag: tag.clone(),
            branch: branch.clone(),
            commit: commit.clone(),
            resolved: github_archive_url(
                repo,
                tag.as_deref(),
                branch.as_deref(),
                commit.as_deref(),
            ),
        },
        DependencySource::Url(url) => LockedSource::Url { url: url.clone() },
        DependencySource::Path(path) => LockedSource::Path { path: path.clone() },
    }
}

fn fetch_source(
    source: &DependencySource,
    cache: &CacheLayout,
    project_root: &Path,
) -> Result<PathBuf, PackageManagerError> {
    match source {
        DependencySource::Path(path) => {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                project_root.join(path)
            };
            canonical_or_current(&path)
        }
        DependencySource::Github {
            repo,
            tag,
            branch,
            commit,
        } => {
            let url =
                github_archive_url(repo, tag.as_deref(), branch.as_deref(), commit.as_deref());
            fetch_zip_source(&url, source, cache)
        }
        DependencySource::Url(url) => fetch_zip_source(url, source, cache),
    }
}

fn fetch_zip_source(
    url: &str,
    source: &DependencySource,
    cache: &CacheLayout,
) -> Result<PathBuf, PackageManagerError> {
    let key = cache_key(&source.stable_key());
    let archive_path = cache.sources.join(format!("{key}.zip"));
    let unpack_root = cache.packages.join(&key);
    if unpack_root.join(PACKAGE_SET_MANIFEST).is_file() {
        return Ok(unpack_root);
    }
    fs::create_dir_all(&cache.sources).map_err(|source| PackageManagerError::Io {
        path: cache.sources.clone(),
        source,
    })?;
    fs::create_dir_all(&cache.packages).map_err(|source| PackageManagerError::Io {
        path: cache.packages.clone(),
        source,
    })?;
    let bytes = reqwest::blocking::get(url)
        .map_err(|err| PackageManagerError::Http(format!("failed to download `{url}`: {err}")))?
        .error_for_status()
        .map_err(|err| PackageManagerError::Http(format!("HTTP error for `{url}`: {err}")))?
        .bytes()
        .map_err(|err| PackageManagerError::Http(format!("failed to read `{url}`: {err}")))?;
    fs::write(&archive_path, &bytes).map_err(|source| PackageManagerError::Io {
        path: archive_path.clone(),
        source,
    })?;
    unpack_zip_bytes(&archive_path, bytes.as_ref(), &unpack_root)?;
    Ok(unpack_root)
}

fn unpack_zip_bytes(
    archive_path: &Path,
    bytes: &[u8],
    target: &Path,
) -> Result<(), PackageManagerError> {
    if target.exists() {
        fs::remove_dir_all(target).map_err(|source| PackageManagerError::Io {
            path: target.to_path_buf(),
            source,
        })?;
    }
    fs::create_dir_all(target).map_err(|source| PackageManagerError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|source| PackageManagerError::Zip {
            path: archive_path.to_path_buf(),
            message: source.to_string(),
        })?;
    archive
        .extract(target)
        .map_err(|source| PackageManagerError::Zip {
            path: archive_path.to_path_buf(),
            message: source.to_string(),
        })?;
    flatten_single_archive_root(target)?;
    Ok(())
}

fn flatten_single_archive_root(target: &Path) -> Result<(), PackageManagerError> {
    if target.join(PACKAGE_SET_MANIFEST).is_file() {
        return Ok(());
    }
    let entries = fs::read_dir(target)
        .map_err(|source| PackageManagerError::Io {
            path: target.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| PackageManagerError::Io {
            path: target.to_path_buf(),
            source,
        })?;
    if entries.len() != 1
        || !entries[0]
            .file_type()
            .map(|ty| ty.is_dir())
            .unwrap_or(false)
    {
        return Ok(());
    }
    let inner = entries[0].path();
    if !inner.join(PACKAGE_SET_MANIFEST).is_file() {
        return Ok(());
    }
    let temp = target.with_extension("flattening");
    if temp.exists() {
        fs::remove_dir_all(&temp).map_err(|source| PackageManagerError::Io {
            path: temp.clone(),
            source,
        })?;
    }
    fs::rename(&inner, &temp).map_err(|source| PackageManagerError::Io {
        path: inner.clone(),
        source,
    })?;
    fs::remove_dir_all(target).map_err(|source| PackageManagerError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    fs::rename(&temp, target).map_err(|source| PackageManagerError::Io { path: temp, source })?;
    Ok(())
}

fn source_digest(
    source: &DependencySource,
    source_root: &Path,
) -> Result<Option<String>, PackageManagerError> {
    match source {
        DependencySource::Path(_) => Ok(None),
        DependencySource::Github { .. } | DependencySource::Url(_) => {
            let cache = CacheLayout::new()?;
            let archive = cache
                .sources
                .join(format!("{}.zip", cache_key(&source.stable_key())));
            if archive.is_file() {
                let bytes = fs::read(&archive).map_err(|source| PackageManagerError::Io {
                    path: archive.clone(),
                    source,
                })?;
                Ok(Some(hex_sha256(&bytes)))
            } else {
                let manifest =
                    fs::read(source_root.join(PACKAGE_SET_MANIFEST)).map_err(|source| {
                        PackageManagerError::Io {
                            path: source_root.join(PACKAGE_SET_MANIFEST),
                            source,
                        }
                    })?;
                Ok(Some(hex_sha256(&manifest)))
            }
        }
    }
}

fn load_package_set(root: &Path) -> Result<PackageSetManifest, PackageManagerError> {
    let path = root.join(PACKAGE_SET_MANIFEST);
    read_toml(&path)
}

fn github_archive_url(
    repo: &str,
    tag: Option<&str>,
    branch: Option<&str>,
    commit: Option<&str>,
) -> String {
    let repo = repo.trim_matches('/');
    if let Some(tag) = tag {
        return format!("https://github.com/{repo}/archive/refs/tags/{tag}.zip");
    }
    if let Some(branch) = branch {
        return format!("https://github.com/{repo}/archive/refs/heads/{branch}.zip");
    }
    if let Some(commit) = commit {
        return format!("https://github.com/{repo}/archive/{commit}.zip");
    }
    format!("https://github.com/{repo}/archive/refs/heads/main.zip")
}

#[derive(Debug, Clone)]
struct CacheLayout {
    sources: PathBuf,
    packages: PathBuf,
}

impl CacheLayout {
    fn new() -> Result<Self, PackageManagerError> {
        let root = if let Some(path) = std::env::var_os("LUX_HOME") {
            PathBuf::from(path)
        } else if let Some(home) =
            std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        {
            PathBuf::from(home).join(".lux")
        } else {
            std::env::current_dir()
                .map_err(|source| PackageManagerError::Io {
                    path: PathBuf::from("."),
                    source,
                })?
                .join(".lux")
        };
        Ok(Self {
            sources: root.join("cache").join("sources"),
            packages: root.join("cache").join("packages"),
        })
    }
}

fn write_new_file(path: &Path, contents: &str) -> Result<(), PackageManagerError> {
    if path.exists() {
        return Err(PackageManagerError::Invalid(format!(
            "{} already exists",
            path.display()
        )));
    }
    write_file(path, contents)
}

fn write_file(path: &Path, contents: &str) -> Result<(), PackageManagerError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| PackageManagerError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, contents).map_err(|source| PackageManagerError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write_toml(path: &Path, value: &impl Serialize) -> Result<(), PackageManagerError> {
    let text = toml::to_string_pretty(value).map_err(|source| PackageManagerError::Parse {
        path: path.to_path_buf(),
        message: source.to_string(),
    })?;
    write_file(path, &text)
}

fn read_toml<T>(path: &Path) -> Result<T, PackageManagerError>
where
    T: for<'de> Deserialize<'de>,
{
    let text = fs::read_to_string(path).map_err(|source| PackageManagerError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| PackageManagerError::Parse {
        path: path.to_path_buf(),
        message: source.to_string(),
    })
}

fn canonical_or_current(path: &Path) -> Result<PathBuf, PackageManagerError> {
    let canonical = path
        .canonicalize()
        .map_err(|source| PackageManagerError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(strip_windows_verbatim_prefix(canonical))
}

fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

fn cache_key(value: &str) -> String {
    hex_sha256(value.as_bytes())[..16].to_string()
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalize_package_display(value: &str) -> String {
    let normalized = value
        .trim()
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if normalized.starts_with('@') {
        normalized
    } else {
        format!("@{normalized}")
    }
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "lux_pkg_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        root
    }

    fn write_project_manifest(project: &Path) {
        fs::create_dir_all(project).expect("project");
        fs::write(
            project.join("lux.toml"),
            "package_id = \"demo\"\nbundle_id = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n[dependencies]\n",
        )
        .expect("project manifest");
    }

    fn write_ui_package_set(source: &Path) {
        fs::create_dir_all(source.join("packages/core/src")).expect("core package");
        fs::create_dir_all(source.join("packages/ui/src")).expect("ui package");
        fs::write(
            source.join("lux.package.toml"),
            r#"
name = "ui-set"

[[package]]
id = "@vendor/core"
version = "0.1.0"
path = "packages/core"

[[package]]
id = "@vendor/ui"
version = "0.1.0"
path = "packages/ui"
depends = ["@vendor/core >=0.1 <0.2"]
"#,
        )
        .expect("source manifest");
        fs::write(
            source.join("packages/core/src/module.lux"),
            "export fn core() = true\n",
        )
        .expect("core module");
        fs::write(
            source.join("packages/ui/src/module.lux"),
            "import { core } from \"@vendor/core\"\nexport fn ui() = core()\n",
        )
        .expect("ui module");
    }

    #[test]
    fn init_project_manifest_is_valid_for_gmod_build() {
        let root = temp_root("init_manifest");
        init_project(&InitOptions {
            root: root.clone(),
            name: "demo".into(),
            install_std: false,
            output_root: None,
            runtime_base: None,
            autorun: true,
        })
        .expect("init project");

        let text = fs::read_to_string(root.join("lux.toml")).expect("manifest");
        assert!(text.contains("package_id = \"demo\""), "{text}");
        assert!(text.contains("bundle_id = \"demo\""), "{text}");
        assert!(!text.contains("name ="), "{text}");

        crate::project::ProjectManifest::load(root.join("lux.toml"))
            .expect("generated manifest should parse");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_manifest_dependency_section_is_command_written() {
        let mut manifest = parse_project_dependency_manifest(
            "package_id = \"demo\"\nbundle_id = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n",
        );
        manifest.set_dependency(
            "@vendor/ui-ext",
            &DependencySource::Github {
                repo: "vendor/ui-ext".into(),
                tag: Some("v0.1.0".into()),
                branch: None,
                commit: None,
            },
        );

        let mut path = temp_root("manifest");
        fs::create_dir_all(&path).expect("temp");
        path.push("lux.toml");
        manifest.write(&path).expect("write manifest");
        let text = fs::read_to_string(&path).expect("manifest text");
        assert!(text.contains("[dependencies]"), "{text}");
        assert!(text.contains("\"@vendor/ui-ext\""), "{text}");
        assert!(text.contains("github = \"vendor/ui-ext\""), "{text}");
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn std_dependency_source_targets_lux_std_repo() {
        let source = lux_std_source();
        assert_eq!(source.stable_key(), "github:TimeWatcher/lux-std");

        let mut manifest = ProjectDependencyManifest::default();
        manifest.set_dependency(LUX_STD_PACKAGE, &source);
        let path = temp_root("std_manifest").join("lux.toml");
        fs::create_dir_all(path.parent().expect("parent")).expect("temp");
        manifest.write(&path).expect("write manifest");
        let text = fs::read_to_string(&path).expect("manifest text");
        assert!(
            text.contains("\"@lux/std\" = { github = \"TimeWatcher/lux-std\" }"),
            "{text}"
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn install_path_dependency_writes_transitive_lock() {
        let source = temp_root("source");
        fs::create_dir_all(source.join("packages/core/src")).expect("source package");
        fs::create_dir_all(source.join("packages/ui/src")).expect("ui package");
        fs::write(
            source.join("lux.package.toml"),
            format!(
                r#"
name = "ui-ext"

[[package]]
id = "@vendor/core"
version = "0.1.0"
path = "packages/core"

[[package]]
id = "@vendor/ui-ext"
version = "0.1.0"
path = "packages/ui"
depends = [
  "@vendor/core 0.1.0",
  "@vendor/reactive >=0.1 <0.2",
]

[[package]]
id = "@vendor/reactive"
version = "0.1.0"
path = "packages/reactive"

[[source]]
package = "@vendor/reactive"
path = "{}"
"#,
                source.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .expect("source manifest");
        fs::create_dir_all(source.join("packages/reactive/src")).expect("reactive package");
        fs::write(
            source.join("packages/core/src/module.lux"),
            "export fn draw() = true\n",
        )
        .expect("core module");
        fs::write(
            source.join("packages/ui/src/module.lux"),
            "import { draw } from \"@vendor/core\"\nexport fn mount() = draw()\n",
        )
        .expect("ui module");
        fs::write(
            source.join("packages/reactive/src/module.lux"),
            "export fn signal(value) = value\n",
        )
        .expect("reactive module");

        let project = temp_root("project");
        fs::create_dir_all(&project).expect("project");
        fs::write(
            project.join("lux.toml"),
            "name = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n",
        )
        .expect("project manifest");

        let output = install_package(&InstallRequest {
            project_root: project.clone(),
            package: "@vendor/ui-ext".into(),
            source: DependencySource::Path(source.clone()),
        })
        .expect("install");

        assert_eq!(output.total_count, 3);
        let lock = fs::read_to_string(project.join("lux.lock")).expect("lock");
        assert!(lock.contains("@vendor/ui-ext"), "{lock}");
        assert!(lock.contains("@vendor/core"), "{lock}");
        assert!(lock.contains("@vendor/reactive"), "{lock}");
        let roots = lockfile_package_roots(&project).expect("roots");
        assert!(
            roots
                .iter()
                .any(|root| root == &strip_windows_verbatim_prefix(source.canonicalize().unwrap()))
        );

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn lock_project_rewrites_lock_from_manifest() {
        let source = temp_root("lock_source");
        write_ui_package_set(&source);
        let project = temp_root("lock_project");
        write_project_manifest(&project);
        fs::write(
            project.join("lux.toml"),
            format!(
                "package_id = \"demo\"\nbundle_id = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n[dependencies]\n\"@vendor/ui\" = {{ path = \"{}\" }}\n",
                source.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .expect("project manifest");
        fs::write(
            project.join("lux.lock"),
            "[[package]]\nid = \"@stale/pkg\"\n",
        )
        .expect("stale lock");

        let output = lock_project(&LockRequest {
            project_root: project.clone(),
        })
        .expect("lock project");

        assert_eq!(output.direct_count, 1);
        assert_eq!(output.total_count, 2);
        let locked = list_locked(&project).expect("locked");
        assert_eq!(locked.len(), 2);
        assert!(locked.iter().any(|package| package.id == "@vendor/ui"));
        assert!(locked.iter().any(|package| package.id == "@vendor/core"));
        assert!(!locked.iter().any(|package| package.id == "@stale/pkg"));

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn remove_package_updates_manifest_and_prunes_transitives() {
        let source = temp_root("remove_source");
        write_ui_package_set(&source);
        let project = temp_root("remove_project");
        write_project_manifest(&project);

        let installed = install_package(&InstallRequest {
            project_root: project.clone(),
            package: "@vendor/ui".into(),
            source: DependencySource::Path(source.clone()),
        })
        .expect("install");
        assert_eq!(installed.total_count, 2);

        let output = remove_package(&RemoveRequest {
            project_root: project.clone(),
            package: "@vendor/ui".into(),
        })
        .expect("remove");

        assert_eq!(output.package_id, "@vendor/ui");
        assert_eq!(output.direct_count, 0);
        assert_eq!(output.total_count, 0);
        let manifest = fs::read_to_string(project.join("lux.toml")).expect("manifest");
        assert!(!manifest.contains("@vendor/ui"), "{manifest}");
        let locked = list_locked(&project).expect("locked");
        assert!(locked.is_empty(), "{locked:?}");

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn remove_package_rejects_transitive_dependency() {
        let source = temp_root("remove_transitive_source");
        write_ui_package_set(&source);
        let project = temp_root("remove_transitive_project");
        write_project_manifest(&project);

        install_package(&InstallRequest {
            project_root: project.clone(),
            package: "@vendor/ui".into(),
            source: DependencySource::Path(source.clone()),
        })
        .expect("install");

        let err = remove_package(&RemoveRequest {
            project_root: project.clone(),
            package: "@vendor/core".into(),
        })
        .expect_err("transitive remove should fail");
        assert!(
            err.to_string()
                .contains("package `@vendor/core` is not a direct dependency"),
            "{err}"
        );

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn package_source_hints_resolve_external_transitive_dependencies() {
        let base = temp_root("external_source");
        let core_source = base.join("core-set");
        let ui_source = base.join("ui-set");
        fs::create_dir_all(core_source.join("packages/core/src")).expect("core package");
        fs::create_dir_all(ui_source.join("packages/ui/src")).expect("ui package");
        fs::write(
            core_source.join("lux.package.toml"),
            r#"
name = "core-set"

[[package]]
id = "@vendor/core"
version = "0.1.0"
path = "packages/core"
"#,
        )
        .expect("core manifest");
        fs::write(
            ui_source.join("lux.package.toml"),
            format!(
                r#"
name = "ui-set"

[[package]]
id = "@vendor/ui-ext"
version = "0.1.0"
path = "packages/ui"
depends = [
  "@vendor/core >=0.1 <0.2",
]

[[source]]
package = "@vendor/core"
path = "{}"
"#,
                core_source.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .expect("ui manifest");
        fs::write(
            core_source.join("packages/core/src/module.lux"),
            "export fn draw() = true\n",
        )
        .expect("core module");
        fs::write(
            ui_source.join("packages/ui/src/module.lux"),
            "import { draw } from \"@vendor/core\"\nexport fn mount() = draw()\n",
        )
        .expect("ui module");

        let project = temp_root("source_hint_project");
        fs::create_dir_all(&project).expect("project");
        fs::write(
            project.join("lux.toml"),
            "name = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n",
        )
        .expect("project manifest");

        let output = install_package(&InstallRequest {
            project_root: project.clone(),
            package: "@vendor/ui-ext".into(),
            source: DependencySource::Path(ui_source.clone()),
        })
        .expect("install");

        assert_eq!(output.total_count, 2);
        let locked = list_locked(&project).expect("locked");
        assert!(
            locked
                .iter()
                .any(|package| package.id == "@vendor/ui-ext" && package.direct)
        );
        assert!(
            locked
                .iter()
                .any(|package| package.id == "@vendor/core" && !package.direct)
        );
        let roots = lockfile_package_roots(&project).expect("roots");
        assert!(
            roots
                .iter()
                .any(|root| root
                    == &strip_windows_verbatim_prefix(ui_source.canonicalize().unwrap()))
        );
        assert!(roots.iter().any(
            |root| root == &strip_windows_verbatim_prefix(core_source.canonicalize().unwrap())
        ));

        let _ = fs::remove_dir_all(base);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn inline_table_dependency_sources_parse_github() {
        let manifest = parse_project_dependency_manifest(
            r#"
name = "demo"

[dependencies]
"@vendor/core" = { github = "vendor/core", tag = "v0.1.0" }
"#,
        );

        let core = dependency_source_from_toml_value(
            manifest
                .dependencies
                .get("@vendor/core")
                .expect("core source"),
            Path::new("."),
        )
        .expect("core source parsed");
        assert!(matches!(
            core,
            DependencySource::Github {
                ref repo,
                tag: Some(ref tag),
                branch: None,
                commit: None
            } if repo == "vendor/core" && tag == "v0.1.0"
        ));
    }

    #[test]
    fn github_archive_urls_use_correct_ref_forms() {
        assert_eq!(
            github_archive_url("vendor/core", Some("v0.1.0"), None, None),
            "https://github.com/vendor/core/archive/refs/tags/v0.1.0.zip"
        );
        assert_eq!(
            github_archive_url("vendor/core", None, Some("develop"), None),
            "https://github.com/vendor/core/archive/refs/heads/develop.zip"
        );
        assert_eq!(
            github_archive_url("vendor/core", None, None, Some("abc123")),
            "https://github.com/vendor/core/archive/abc123.zip"
        );
        assert_eq!(
            github_archive_url("vendor/core", None, None, None),
            "https://github.com/vendor/core/archive/refs/heads/main.zip"
        );
    }

    #[test]
    fn transitive_version_conflicts_are_errors() {
        let source = temp_root("conflict_source");
        fs::create_dir_all(source.join("packages/app/src")).expect("app package");
        fs::create_dir_all(source.join("packages/core/src")).expect("core package");
        fs::write(
            source.join("lux.package.toml"),
            r#"
name = "conflict"

[[package]]
id = "@vendor/app"
version = "0.1.0"
path = "packages/app"
depends = ["@vendor/core >=2.0 <3.0"]

[[package]]
id = "@vendor/core"
version = "1.0.0"
path = "packages/core"
"#,
        )
        .expect("source manifest");
        fs::write(
            source.join("packages/app/src/module.lux"),
            "export fn app() = true\n",
        )
        .expect("app module");
        fs::write(
            source.join("packages/core/src/module.lux"),
            "export fn core() = true\n",
        )
        .expect("core module");

        let project = temp_root("conflict_project");
        fs::create_dir_all(&project).expect("project");
        fs::write(
            project.join("lux.toml"),
            "name = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n",
        )
        .expect("project manifest");

        let err = install_package(&InstallRequest {
            project_root: project.clone(),
            package: "@vendor/app".into(),
            source: DependencySource::Path(source.clone()),
        })
        .expect_err("conflict should fail");
        assert!(
            err.to_string()
                .contains("version conflict for `@vendor/core`"),
            "{err}"
        );

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn source_hints_must_use_one_source_kind() {
        let base = temp_root("bad_source_hint");
        let dep_source = base.join("dep");
        let app_source = base.join("app");
        fs::create_dir_all(dep_source.join("packages/dep/src")).expect("dep package");
        fs::create_dir_all(app_source.join("packages/app/src")).expect("app package");
        fs::write(
            dep_source.join("lux.package.toml"),
            r#"
name = "dep"

[[package]]
id = "@vendor/dep"
version = "0.1.0"
path = "packages/dep"
"#,
        )
        .expect("dep manifest");
        fs::write(
            app_source.join("lux.package.toml"),
            format!(
                r#"
name = "app"

[[package]]
id = "@vendor/app"
version = "0.1.0"
path = "packages/app"
depends = ["@vendor/dep >=0.1 <0.2"]

[[source]]
package = "@vendor/dep"
github = "vendor/dep"
path = "{}"
"#,
                dep_source.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .expect("app manifest");
        fs::write(
            dep_source.join("packages/dep/src/module.lux"),
            "export fn dep() = true\n",
        )
        .expect("dep module");
        fs::write(
            app_source.join("packages/app/src/module.lux"),
            "export fn app() = true\n",
        )
        .expect("app module");

        let project = temp_root("bad_source_hint_project");
        fs::create_dir_all(&project).expect("project");
        fs::write(
            project.join("lux.toml"),
            "name = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n",
        )
        .expect("project manifest");

        let err = install_package(&InstallRequest {
            project_root: project.clone(),
            package: "@vendor/app".into(),
            source: DependencySource::Path(app_source.clone()),
        })
        .expect_err("bad source hint should fail");
        assert!(
            err.to_string()
                .contains("[[source]] for `@vendor/dep` must set exactly one"),
            "{err}"
        );

        let _ = fs::remove_dir_all(base);
        let _ = fs::remove_dir_all(project);
    }
}
