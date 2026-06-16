use std::fmt;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode, Stdio};

use serde::Deserialize;
use zip::ZipArchive;

const LUX_HOME_ENV: &str = "LUX_HOME";
const DEFAULT_TOOLCHAIN_FILE: &str = "default-toolchain";
const TOOLCHAIN_MANIFEST: &str = "toolchain.toml";
const LUX_REPO: &str = "TimeWatcher/lux";

#[cfg(windows)]
const LUXC_EXE: &str = "luxc.exe";
#[cfg(not(windows))]
const LUXC_EXE: &str = "luxc";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainLayout {
    pub root: PathBuf,
    pub bin: PathBuf,
    pub toolchains: PathBuf,
    pub default_file: PathBuf,
}

impl ToolchainLayout {
    pub fn discover() -> Result<Self, ToolchainError> {
        let root = if let Some(value) = std::env::var_os(LUX_HOME_ENV) {
            PathBuf::from(value)
        } else {
            user_home_dir()
                .ok_or_else(|| ToolchainError::Invalid("cannot locate user home directory".into()))?
                .join(".lux")
        };
        Ok(Self::new(root))
    }

    pub fn new(root: PathBuf) -> Self {
        Self {
            bin: root.join("bin"),
            toolchains: root.join("toolchains"),
            default_file: root.join(DEFAULT_TOOLCHAIN_FILE),
            root,
        }
    }

    pub fn shim_path(&self) -> PathBuf {
        self.bin.join(LUXC_EXE)
    }

    pub fn toolchain_path(&self, version: &str) -> PathBuf {
        self.toolchains.join(version).join(LUXC_EXE)
    }

    fn version_root(&self, version: &str) -> PathBuf {
        self.toolchains.join(version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainCommand {
    Install {
        version: Option<String>,
        source: Option<String>,
        make_default: bool,
    },
    Update,
    Default {
        version: String,
    },
    List,
    Which {
        project_root: PathBuf,
    },
    Pin {
        version: String,
        project_root: PathBuf,
    },
    Unpin {
        project_root: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledToolchain {
    pub version: String,
    pub path: PathBuf,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallToolchainOutput {
    pub version: String,
    pub executable: PathBuf,
    pub shim: PathBuf,
    pub default_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedToolchain {
    pub version: String,
    pub executable: PathBuf,
    pub source: ToolchainSelectionSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainSelectionSource {
    ProjectPin(PathBuf),
    GlobalDefault(PathBuf),
}

#[derive(Debug)]
pub enum ToolchainError {
    Io { path: PathBuf, source: io::Error },
    Http(String),
    Zip { path: PathBuf, message: String },
    Invalid(String),
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Http(message) | Self::Invalid(message) => f.write_str(message),
            Self::Zip { path, message } => {
                write!(f, "failed to unpack {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ToolchainError {}

#[derive(Debug, Clone)]
pub struct InstallToolchainRequest {
    pub version: Option<String>,
    pub source: Option<String>,
    pub make_default: bool,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn install_toolchain(
    layout: &ToolchainLayout,
    request: &InstallToolchainRequest,
) -> Result<InstallToolchainOutput, ToolchainError> {
    let version = request
        .version
        .clone()
        .unwrap_or_else(|| current_version().to_string());
    validate_version_name(&version)?;

    let bytes = match &request.source {
        Some(source) => read_toolchain_source(source)?,
        None if version == current_version() => read_current_executable()?,
        None => download_release_asset(&version)?,
    };

    let executable = install_toolchain_bytes(layout, &version, &bytes)?;
    let shim = install_shim(layout)?;
    let default_version = if request.make_default || !layout.default_file.is_file() {
        set_default_toolchain(layout, &version)?;
        Some(version.clone())
    } else {
        read_default_toolchain(layout)?
    };

    Ok(InstallToolchainOutput {
        version,
        executable,
        shim,
        default_version,
    })
}

pub fn update_toolchain(
    layout: &ToolchainLayout,
) -> Result<InstallToolchainOutput, ToolchainError> {
    let version = latest_release_version()?;
    install_toolchain(
        layout,
        &InstallToolchainRequest {
            version: Some(version),
            source: None,
            make_default: true,
        },
    )
}

pub fn set_default_toolchain(
    layout: &ToolchainLayout,
    version: &str,
) -> Result<(), ToolchainError> {
    validate_version_name(version)?;
    let executable = layout.toolchain_path(version);
    if !executable.is_file() {
        return Err(ToolchainError::Invalid(format!(
            "toolchain `{version}` is not installed"
        )));
    }
    fs::create_dir_all(&layout.root).map_err(|source| ToolchainError::Io {
        path: layout.root.clone(),
        source,
    })?;
    write_text_file(&layout.default_file, &(version.to_string() + "\n"))
}

pub fn list_toolchains(
    layout: &ToolchainLayout,
) -> Result<Vec<InstalledToolchain>, ToolchainError> {
    let default = read_default_toolchain(layout)?;
    if !layout.toolchains.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&layout.toolchains).map_err(|source| ToolchainError::Io {
        path: layout.toolchains.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| ToolchainError::Io {
            path: layout.toolchains.clone(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let version = entry.file_name().to_string_lossy().to_string();
        let executable = path.join(LUXC_EXE);
        if executable.is_file() {
            entries.push(InstalledToolchain {
                is_default: default.as_deref() == Some(version.as_str()),
                version,
                path: executable,
            });
        }
    }
    entries.sort_by(|left, right| left.version.cmp(&right.version));
    Ok(entries)
}

pub fn select_toolchain(
    layout: &ToolchainLayout,
    project_root: &Path,
) -> Result<Option<SelectedToolchain>, ToolchainError> {
    if let Some((manifest, version)) = find_project_toolchain_pin(project_root)? {
        let executable = layout.toolchain_path(&version);
        if !executable.is_file() {
            return Err(ToolchainError::Invalid(format!(
                "project pins Lux compiler `{version}`, but it is not installed; run `luxc self install {version}`"
            )));
        }
        return Ok(Some(SelectedToolchain {
            version,
            executable,
            source: ToolchainSelectionSource::ProjectPin(manifest),
        }));
    }

    let Some(version) = read_default_toolchain(layout)? else {
        return Ok(None);
    };
    let executable = layout.toolchain_path(&version);
    if !executable.is_file() {
        return Err(ToolchainError::Invalid(format!(
            "default Lux compiler `{version}` is not installed; run `luxc self install {version} --default`"
        )));
    }
    Ok(Some(SelectedToolchain {
        version,
        executable,
        source: ToolchainSelectionSource::GlobalDefault(layout.default_file.clone()),
    }))
}

pub fn pin_toolchain(project_root: &Path, version: &str) -> Result<PathBuf, ToolchainError> {
    validate_version_name(version)?;
    let dir = project_root.join(".lux");
    fs::create_dir_all(&dir).map_err(|source| ToolchainError::Io {
        path: dir.clone(),
        source,
    })?;
    let path = dir.join(TOOLCHAIN_MANIFEST);
    write_text_file(&path, &format!("luxc = \"{}\"\n", escape_toml(version)))?;
    Ok(path)
}

pub fn unpin_toolchain(project_root: &Path) -> Result<Option<PathBuf>, ToolchainError> {
    let path = project_root.join(".lux").join(TOOLCHAIN_MANIFEST);
    if !path.exists() {
        return Ok(None);
    }
    fs::remove_file(&path).map_err(|source| ToolchainError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(Some(path))
}

pub fn is_shim_executable(layout: &ToolchainLayout, current_exe: &Path) -> bool {
    same_path(current_exe, &layout.shim_path())
}

pub fn dispatch_from_shim_if_needed(
    args: &[std::ffi::OsString],
) -> Result<Option<ExitCode>, String> {
    if args
        .first()
        .is_some_and(|arg| arg.to_string_lossy().as_ref() == "self")
    {
        return Ok(None);
    }
    let layout = ToolchainLayout::discover().map_err(|err| err.to_string())?;
    let current_exe = std::env::current_exe().map_err(|err| err.to_string())?;
    if !is_shim_executable(&layout, &current_exe) {
        return Ok(None);
    }
    let project_root = std::env::current_dir().map_err(|err| err.to_string())?;
    let Some(selected) = select_toolchain(&layout, &project_root).map_err(|err| err.to_string())?
    else {
        return Ok(None);
    };
    if same_path(&current_exe, &selected.executable) {
        return Ok(None);
    }
    let status = ProcessCommand::new(&selected.executable)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| format!("failed to run {}: {err}", selected.executable.display()))?;
    Ok(Some(exit_code_from_status(status)))
}

fn install_toolchain_bytes(
    layout: &ToolchainLayout,
    version: &str,
    bytes: &[u8],
) -> Result<PathBuf, ToolchainError> {
    let target_root = layout.version_root(version);
    if target_root.exists() {
        fs::remove_dir_all(&target_root).map_err(|source| ToolchainError::Io {
            path: target_root.clone(),
            source,
        })?;
    }
    fs::create_dir_all(&target_root).map_err(|source| ToolchainError::Io {
        path: target_root.clone(),
        source,
    })?;

    if looks_like_zip(bytes) {
        unpack_zip_bytes(bytes, &target_root)?;
        let found = find_executable_in_tree(&target_root).ok_or_else(|| {
            ToolchainError::Invalid(format!("downloaded toolchain did not contain `{LUXC_EXE}`"))
        })?;
        let target = layout.toolchain_path(version);
        if !same_path(&found, &target) {
            copy_executable(&found, &target)?;
        }
    } else {
        let target = layout.toolchain_path(version);
        write_binary_file(&target, bytes)?;
        make_executable(&target)?;
    }

    let executable = layout.toolchain_path(version);
    if !executable.is_file() {
        return Err(ToolchainError::Invalid(format!(
            "failed to install toolchain executable at {}",
            executable.display()
        )));
    }
    Ok(executable)
}

fn install_shim(layout: &ToolchainLayout) -> Result<PathBuf, ToolchainError> {
    let shim = layout.shim_path();
    if shim.is_file() {
        return Ok(shim);
    }
    let current = std::env::current_exe().map_err(|source| ToolchainError::Io {
        path: PathBuf::from("current executable"),
        source,
    })?;
    copy_executable(&current, &shim)?;
    Ok(shim)
}

fn read_toolchain_source(source: &str) -> Result<Vec<u8>, ToolchainError> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return download_url(source);
    }
    let path = PathBuf::from(source);
    fs::read(&path).map_err(|source| ToolchainError::Io { path, source })
}

fn read_current_executable() -> Result<Vec<u8>, ToolchainError> {
    let path = std::env::current_exe().map_err(|source| ToolchainError::Io {
        path: PathBuf::from("current executable"),
        source,
    })?;
    fs::read(&path).map_err(|source| ToolchainError::Io { path, source })
}

fn download_release_asset(version: &str) -> Result<Vec<u8>, ToolchainError> {
    let asset = release_asset_name();
    let url = format!("https://github.com/{LUX_REPO}/releases/download/{version}/{asset}");
    download_url(&url)
}

fn latest_release_version() -> Result<String, ToolchainError> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }

    let client = reqwest::blocking::Client::new();
    let release = client
        .get(format!(
            "https://api.github.com/repos/{LUX_REPO}/releases/latest"
        ))
        .header(reqwest::header::USER_AGENT, "luxc")
        .send()
        .map_err(|err| ToolchainError::Http(format!("failed to query latest Lux release: {err}")))?
        .error_for_status()
        .map_err(|err| ToolchainError::Http(format!("HTTP error for latest Lux release: {err}")))?
        .json::<Release>()
        .map_err(|err| {
            ToolchainError::Http(format!("failed to parse latest Lux release: {err}"))
        })?;
    if release.tag_name.trim().is_empty() {
        return Err(ToolchainError::Invalid(
            "latest Lux release did not include a tag name".into(),
        ));
    }
    Ok(release.tag_name)
}

fn download_url(url: &str) -> Result<Vec<u8>, ToolchainError> {
    reqwest::blocking::get(url)
        .map_err(|err| ToolchainError::Http(format!("failed to download `{url}`: {err}")))?
        .error_for_status()
        .map_err(|err| ToolchainError::Http(format!("HTTP error for `{url}`: {err}")))?
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|err| ToolchainError::Http(format!("failed to read `{url}`: {err}")))
}

fn release_asset_name() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "luxc-windows-x64.zip",
        ("linux", "x86_64") => "luxc-linux-x64.zip",
        ("macos", "x86_64") => "luxc-macos-x64.zip",
        ("macos", "aarch64") => "luxc-macos-arm64.zip",
        _ => "luxc.zip",
    }
}

fn unpack_zip_bytes(bytes: &[u8], target: &Path) -> Result<(), ToolchainError> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|source| ToolchainError::Zip {
            path: target.to_path_buf(),
            message: source.to_string(),
        })?;
    archive
        .extract(target)
        .map_err(|source| ToolchainError::Zip {
            path: target.to_path_buf(),
            message: source.to_string(),
        })?;
    Ok(())
}

fn find_executable_in_tree(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let entries = fs::read_dir(&path).ok()?;
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                stack.push(child);
            } else if child
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(LUXC_EXE))
            {
                return Some(child);
            }
        }
    }
    None
}

fn find_project_toolchain_pin(start: &Path) -> Result<Option<(PathBuf, String)>, ToolchainError> {
    let start = absolute_path(start)?;
    let start = if start.is_file() {
        start.parent().unwrap_or(&start).to_path_buf()
    } else {
        start
    };
    let mut current = start;
    loop {
        let path = current.join(".lux").join(TOOLCHAIN_MANIFEST);
        if path.is_file() {
            let manifest = read_toolchain_manifest(&path)?;
            return Ok(Some((path, manifest.luxc)));
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, ToolchainError> {
    if let Ok(path) = path.canonicalize() {
        return Ok(path);
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let current = std::env::current_dir().map_err(|source| ToolchainError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    Ok(current.join(path))
}

#[derive(Debug, Deserialize)]
struct ToolchainManifest {
    luxc: String,
}

fn read_toolchain_manifest(path: &Path) -> Result<ToolchainManifest, ToolchainError> {
    let text = fs::read_to_string(path).map_err(|source| ToolchainError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let manifest = toml::from_str::<ToolchainManifest>(&text).map_err(|source| {
        ToolchainError::Invalid(format!("invalid {}: {source}", path.display()))
    })?;
    validate_version_name(&manifest.luxc)?;
    Ok(manifest)
}

fn read_default_toolchain(layout: &ToolchainLayout) -> Result<Option<String>, ToolchainError> {
    if !layout.default_file.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&layout.default_file).map_err(|source| ToolchainError::Io {
        path: layout.default_file.clone(),
        source,
    })?;
    let version = text.trim().to_string();
    if version.is_empty() {
        return Ok(None);
    }
    validate_version_name(&version)?;
    Ok(Some(version))
}

fn validate_version_name(version: &str) -> Result<(), ToolchainError> {
    let trimmed = version.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err(ToolchainError::Invalid(format!(
            "invalid toolchain version `{version}`"
        )));
    }
    Ok(())
}

fn copy_executable(from: &Path, to: &Path) -> Result<(), ToolchainError> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|source| ToolchainError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::copy(from, to).map_err(|source| ToolchainError::Io {
        path: to.to_path_buf(),
        source,
    })?;
    make_executable(to)?;
    Ok(())
}

fn write_binary_file(path: &Path, bytes: &[u8]) -> Result<(), ToolchainError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ToolchainError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, bytes).map_err(|source| ToolchainError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    make_executable(path)
}

fn write_text_file(path: &Path, text: &str) -> Result<(), ToolchainError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ToolchainError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut file = fs::File::create(path).map_err(|source| ToolchainError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(text.as_bytes())
        .map_err(|source| ToolchainError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), ToolchainError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|source| ToolchainError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|source| ToolchainError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), ToolchainError> {
    Ok(())
}

fn looks_like_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

fn same_path(left: &Path, right: &Path) -> bool {
    if let (Ok(left), Ok(right)) = (left.canonicalize(), right.canonicalize()) {
        return left == right;
    }
    left == right
}

fn exit_code_from_status(status: std::process::ExitStatus) -> ExitCode {
    match status.code() {
        Some(code) if (0..=255).contains(&code) => ExitCode::from(code as u8),
        Some(_) | None => ExitCode::from(1),
    }
}

fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn escape_toml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "lux_toolchain_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        root
    }

    #[test]
    fn install_from_raw_executable_bytes_writes_toolchain_and_default() {
        let root = temp_root("install");
        let layout = ToolchainLayout::new(root.clone());
        let output =
            install_toolchain_bytes(&layout, "0.1.0-alpha.1", b"fake exe").expect("install");
        assert_eq!(output, layout.toolchain_path("0.1.0-alpha.1"));
        set_default_toolchain(&layout, "0.1.0-alpha.1").expect("default");

        assert_eq!(
            fs::read_to_string(&layout.default_file).expect("default text"),
            "0.1.0-alpha.1\n"
        );
        assert_eq!(
            fs::read(layout.toolchain_path("0.1.0-alpha.1")).expect("exe"),
            b"fake exe"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn list_toolchains_marks_default() {
        let root = temp_root("list");
        let layout = ToolchainLayout::new(root.clone());
        install_toolchain_bytes(&layout, "0.1.0-alpha.1", b"one").expect("one");
        install_toolchain_bytes(&layout, "0.1.0-alpha.2", b"two").expect("two");
        set_default_toolchain(&layout, "0.1.0-alpha.2").expect("default");

        let versions = list_toolchains(&layout).expect("list");
        assert_eq!(versions.len(), 2);
        assert!(!versions[0].is_default);
        assert!(versions[1].is_default);
        assert_eq!(versions[1].version, "0.1.0-alpha.2");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_pin_is_selected_before_global_default() {
        let root = temp_root("select");
        let layout = ToolchainLayout::new(root.join("home"));
        install_toolchain_bytes(&layout, "global", b"global").expect("global");
        install_toolchain_bytes(&layout, "pinned", b"pinned").expect("pinned");
        set_default_toolchain(&layout, "global").expect("default");

        let project = root.join("project");
        let nested = project.join("src").join("ui");
        fs::create_dir_all(&nested).expect("nested");
        pin_toolchain(&project, "pinned").expect("pin");

        let selected = select_toolchain(&layout, &nested)
            .expect("select")
            .expect("selected");
        assert_eq!(selected.version, "pinned");
        assert!(matches!(
            selected.source,
            ToolchainSelectionSource::ProjectPin(_)
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn version_names_cannot_escape_toolchain_root() {
        assert!(validate_version_name("0.1.0").is_ok());
        assert!(validate_version_name("../0.1.0").is_err());
        assert!(validate_version_name("0.1.0\\evil").is_err());
    }
}
