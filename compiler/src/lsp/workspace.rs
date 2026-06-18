use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::analysis::{AnalysisConfig, AnalysisFile, ProjectAnalysis};
use crate::package_manager::{PACKAGE_SET_MANIFEST, package_set_source_roots};
use crate::packages::{discover_compile_time_phases, discover_runtime_phases};
use crate::project::ProjectManifest;
use lsp_types::{InitializeParams, Uri};
use url::Url;

pub(crate) fn analysis_configs(
    root: &Path,
    documents: &HashMap<Uri, String>,
) -> Vec<AnalysisConfig> {
    let mut configs = Vec::new();
    let mut seen = BTreeSet::<String>::new();

    if let Some(config) = root_analysis_config(root) {
        insert_analysis_config(&mut configs, &mut seen, config);
    }

    for path in documents.keys().filter_map(url_to_path) {
        if let Some(config) = analysis_config_for_path(root, &path) {
            insert_analysis_config(&mut configs, &mut seen, config);
        }
    }

    if configs.is_empty() && !documents.is_empty() {
        insert_analysis_config(&mut configs, &mut seen, AnalysisConfig::standalone(root));
    }

    configs
}

fn root_analysis_config(root: &Path) -> Option<AnalysisConfig> {
    find_package_set_manifest(root)
        .and_then(|path| package_set_analysis_config(root, &path))
        .or_else(|| {
            find_manifest(root)
                .and_then(|path| ProjectManifest::load(path).ok())
                .map(AnalysisConfig::from_manifest)
        })
}

fn analysis_config_for_path(root: &Path, path: &Path) -> Option<AnalysisConfig> {
    let project_manifest = find_named_manifest_for_path(root, path, "lux.toml");
    let package_set_manifest = find_named_manifest_for_path(root, path, PACKAGE_SET_MANIFEST);
    match (project_manifest, package_set_manifest) {
        (Some(project), Some(package_set)) => {
            if manifest_is_deeper(&project, &package_set) {
                ProjectManifest::load(project)
                    .ok()
                    .map(AnalysisConfig::from_manifest)
            } else {
                package_set_analysis_config(root, &package_set)
            }
        }
        (Some(project), None) => ProjectManifest::load(project)
            .ok()
            .map(AnalysisConfig::from_manifest),
        (None, Some(package_set)) => package_set_analysis_config(root, &package_set),
        (None, None) => root_analysis_config(root),
    }
}

fn package_set_analysis_config(root: &Path, package_set_path: &Path) -> Option<AnalysisConfig> {
    let package_root = package_set_path.parent().unwrap_or(root);
    let source_roots = package_set_source_roots(package_root).unwrap_or_default();
    Some(AnalysisConfig::package_set(package_root, source_roots))
}

pub(crate) fn overlays_for_config(
    config: &AnalysisConfig,
    overlays: &[AnalysisFile],
) -> Vec<AnalysisFile> {
    overlays
        .iter()
        .filter(|overlay| analysis_config_contains_path(config, &overlay.path))
        .cloned()
        .collect()
}

fn analysis_config_contains_path(config: &AnalysisConfig, path: &Path) -> bool {
    if config.is_package_set() {
        return package_set_config_contains_path(config, path);
    }
    if config.is_standalone() {
        return path.extension().is_some_and(|extension| extension == "lux")
            && path_is_under(path, &config.source_root);
    }
    path_is_under(path, &config.source_root)
}

fn package_set_config_contains_path(config: &AnalysisConfig, path: &Path) -> bool {
    config.package_roots.iter().any(|root| {
        discover_runtime_phases(root)
            .into_iter()
            .chain(discover_compile_time_phases(root))
            .flatten()
            .flat_map(|phase| phase.source_paths)
            .any(|source_path| same_path(&source_path, path))
    })
}

fn insert_analysis_config(
    configs: &mut Vec<AnalysisConfig>,
    seen: &mut BTreeSet<String>,
    config: AnalysisConfig,
) {
    let key = analysis_config_key(&config);
    if seen.insert(key) {
        configs.push(config);
    }
}

pub(crate) fn analysis_config_key(config: &AnalysisConfig) -> String {
    let mode = if config.is_package_set() {
        "package-set"
    } else if config.is_standalone() {
        "standalone"
    } else {
        "project"
    };
    let package_id = config
        .package_id
        .as_ref()
        .map(|id| id.as_str())
        .unwrap_or_default();
    let package_roots = config
        .package_roots
        .iter()
        .map(|path| normalized_path(path))
        .collect::<Vec<_>>()
        .join("|");
    let externs = config
        .resolver_options
        .externs
        .iter()
        .map(|symbol| {
            format!(
                "{}:{:?}:{}",
                symbol.path_string(),
                symbol.availability,
                symbol.span.is_some()
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    let unknown_external = match config.resolver_options.unknown_external {
        crate::resolve::UnknownExternalPolicy::Allow => "allow",
        crate::resolve::UnknownExternalPolicy::Warn => "warn",
        crate::resolve::UnknownExternalPolicy::Error => "error",
    };
    let gmod_api = if config.resolver_options.gmod_api.is_some() {
        "api"
    } else {
        "no-api"
    };
    format!(
        "{mode}:{}:{package_id}:{package_roots}:{unknown_external}:{}:{gmod_api}:{externs}",
        normalized_path(&config.source_root),
        config.resolver_options.compile_time_package
    )
}

pub(crate) fn analysis_path_score(analysis: &ProjectAnalysis, path: &Path) -> (bool, usize) {
    (
        !analysis.config.is_package_set(),
        common_path_prefix_len(&analysis.config.source_root, path),
    )
}

fn common_path_prefix_len(left: &Path, right: &Path) -> usize {
    left.components()
        .zip(right.components())
        .take_while(|(left, right)| left == right)
        .count()
}

fn path_is_under(path: &Path, root: &Path) -> bool {
    let path = normalized_path(path);
    let root = normalized_path(root).trim_end_matches('/').to_string();
    path == root || path.starts_with(&(root + "/"))
}

pub(crate) fn analysis_config_summary(configs: &[AnalysisConfig]) -> String {
    configs
        .iter()
        .map(analysis_config_label)
        .collect::<Vec<_>>()
        .join("; ")
}

pub(crate) fn analysis_config_label_for_analysis(analysis: &ProjectAnalysis) -> String {
    analysis_config_label(&analysis.config)
}

pub(crate) fn analysis_config_label(config: &AnalysisConfig) -> String {
    let mode = if config.is_package_set() {
        "package-set"
    } else if config.is_standalone() {
        "standalone"
    } else {
        "project"
    };
    format!(
        "{}:{}:{}",
        mode,
        normalized_path(&config.source_root),
        config
            .package_id
            .as_ref()
            .map(|id| id.as_str())
            .unwrap_or("<none>")
    )
}

pub(crate) fn overlays_summary(overlays: &[AnalysisFile]) -> String {
    overlays
        .iter()
        .map(|overlay| normalized_path(&overlay.path))
        .collect::<Vec<_>>()
        .join("; ")
}

fn manifest_is_deeper(left: &Path, right: &Path) -> bool {
    left.components().count() >= right.components().count()
}

#[allow(deprecated)]
pub(crate) fn workspace_root(initialize: &InitializeParams) -> PathBuf {
    initialize
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .and_then(|folder| url_to_path(&folder.uri))
        .or_else(|| initialize.root_uri.as_ref().and_then(url_to_path))
        .or_else(|| initialize.root_path.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn find_manifest(root: &Path) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    loop {
        let candidate = current.join("lux.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn find_package_set_manifest(root: &Path) -> Option<PathBuf> {
    find_named_manifest(root, PACKAGE_SET_MANIFEST)
}

pub(crate) fn find_manifest_for_path(root: &Path, path: &Path) -> Option<PathBuf> {
    find_named_manifest_for_path(root, path, "lux.toml").or_else(|| find_manifest(root))
}

fn find_named_manifest(root: &Path, name: &str) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    loop {
        let candidate = current.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn find_named_manifest_for_path(root: &Path, path: &Path, name: &str) -> Option<PathBuf> {
    let mut current = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| path.to_path_buf());
    loop {
        let candidate = current.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        if current == root {
            break;
        }
        if !current.pop() {
            break;
        }
    }
    None
}

pub(crate) fn is_lux_analysis_watched_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "lux.toml" | "lux.lock" | "lux.package.toml"))
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    normalized_path(a) == normalized_path(b)
}

pub(crate) fn normalized_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

fn url_to_path(uri: &Uri) -> Option<PathBuf> {
    let parsed = Url::parse(uri.as_str()).ok()?;
    parsed.to_file_path().ok()
}
