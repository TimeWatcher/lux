use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackagePhaseKind {
    Runtime,
    CompileTime,
    Host,
}

#[derive(Debug, Clone)]
pub struct PackagePhase {
    pub package_id: String,
    pub kind: PackagePhaseKind,
    pub source_dir: PathBuf,
    pub source_path: PathBuf,
    pub source_paths: Vec<PathBuf>,
}

#[derive(Debug)]
pub enum PackageLoadError {
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for PackageLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(
                    f,
                    "failed to discover Lux packages under {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for PackageLoadError {}

pub fn default_package_root() -> PathBuf {
    if let Some(path) = std::env::var_os("LUX_PACKAGE_ROOT").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }

    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        let beside_exe = exe_dir.join("packages");
        if beside_exe.is_dir() {
            return beside_exe;
        }

        let beside_parent = exe_dir.join("../packages");
        if beside_parent.is_dir() {
            return beside_parent;
        }
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../packages")
}

pub fn discover_runtime_phases(root: &Path) -> Result<Vec<PackagePhase>, PackageLoadError> {
    discover_phases(root, PackagePhaseKind::Runtime)
}

pub fn discover_compile_time_phases(root: &Path) -> Result<Vec<PackagePhase>, PackageLoadError> {
    let mut phases = discover_phases(root, PackagePhaseKind::CompileTime)?;
    phases.extend(discover_phases(root, PackagePhaseKind::Host)?);
    phases.sort_by(|a, b| {
        a.package_id
            .cmp(&b.package_id)
            .then(a.source_path.cmp(&b.source_path))
    });
    Ok(phases)
}

fn discover_phases(
    root: &Path,
    kind: PackagePhaseKind,
) -> Result<Vec<PackagePhase>, PackageLoadError> {
    let mut phases = Vec::new();
    discover_phases_into(root, root, kind, &mut phases)?;
    phases.sort_by(|a, b| a.package_id.cmp(&b.package_id));
    Ok(phases)
}

fn discover_phases_into(
    root: &Path,
    dir: &Path,
    kind: PackagePhaseKind,
    out: &mut Vec<PackagePhase>,
) -> Result<(), PackageLoadError> {
    let entries = fs::read_dir(dir).map_err(|source| PackageLoadError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| PackageLoadError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| PackageLoadError::Io {
            path: path.clone(),
            source,
        })?;
        if !file_type.is_dir() {
            continue;
        }

        if let Some((source_dir, source_paths)) = phase_source_paths(&path, kind)? {
            let source_path = source_paths
                .iter()
                .find(|path| path.file_name().is_some_and(|name| name == "module.lux"))
                .cloned()
                .or_else(|| source_paths.first().cloned())
                .unwrap_or_else(|| source_dir.join("module.lux"));
            out.push(PackagePhase {
                package_id: package_id_for_dir(root, &path),
                kind,
                source_dir,
                source_path,
                source_paths,
            });
        }

        discover_phases_into(root, &path, kind, out)?;
    }

    Ok(())
}

fn phase_source_paths(
    package_dir: &Path,
    kind: PackagePhaseKind,
) -> Result<Option<(PathBuf, Vec<PathBuf>)>, PackageLoadError> {
    let source_dir = match kind {
        PackagePhaseKind::Runtime => package_dir.join("src"),
        PackagePhaseKind::CompileTime => package_dir.join("compiletime"),
        PackagePhaseKind::Host => package_dir.join("host"),
    };
    if !source_dir.is_dir() {
        return Ok(None);
    }

    let mut paths = Vec::new();
    discover_lux_sources_into(&source_dir, &mut paths)?;
    paths.sort();
    if paths.is_empty() {
        Ok(None)
    } else {
        Ok(Some((source_dir, paths)))
    }
}

fn discover_lux_sources_into(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), PackageLoadError> {
    let entries = fs::read_dir(dir).map_err(|source| PackageLoadError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| PackageLoadError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| PackageLoadError::Io {
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

fn package_id_for_dir(root: &Path, package_dir: &Path) -> String {
    package_dir
        .strip_prefix(root)
        .unwrap_or(package_dir)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}
