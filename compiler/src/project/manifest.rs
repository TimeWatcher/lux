use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ast::Realm;
use crate::package_manager::lockfile_package_roots;
use crate::resolve::{ExternSymbol, UnknownExternalPolicy};
use crate::sourcemap::SourceCommentMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectManifest {
    pub package_id: Option<String>,
    pub bundle_id: Option<String>,
    pub source_root: PathBuf,
    pub addon_root: PathBuf,
    pub generated_root: Option<PathBuf>,
    pub package_roots: Vec<PathBuf>,
    pub source_comments: Option<SourceCommentMode>,
    pub gmod_unknown_external: Option<UnknownExternalPolicy>,
    pub gmod_externs: Vec<ExternSymbol>,
}

#[derive(Debug)]
pub enum ManifestError {
    Io { path: PathBuf, source: io::Error },
    Parse { path: PathBuf, message: String },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to read manifest {}: {source}", path.display())
            }
            Self::Parse { path, message } => {
                write!(f, "invalid manifest {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ManifestError {}

impl ProjectManifest {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(path, &text)
    }

    pub fn parse(path: &Path, text: &str) -> Result<Self, ManifestError> {
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut section = String::new();
        let mut source_root = None;
        let mut package_id = None;
        let mut bundle_id = None;
        let mut addon_root = None;
        let mut generated_root = None;
        let mut package_roots = Vec::new();
        let mut source_comments = None;
        let mut gmod_unknown_external = None;
        let mut gmod_externs = Vec::new();

        for (line_index, raw_line) in text.lines().enumerate() {
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_string();
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(parse_error(
                    path,
                    line_index,
                    "expected `key = \"value\"` entry",
                ));
            };
            let key = key.trim();
            let raw_value = value.trim();

            match (section.as_str(), key) {
                ("dependencies", _) => {}
                ("", "package_id") | ("project", "package_id") => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    package_id = Some(value);
                }
                ("", "bundle_id") | ("project", "bundle_id") | ("gmod", "bundle_id") => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    bundle_id = Some(value);
                }
                ("", "source_root") | ("project", "source_root") | ("gmod", "source_root") => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    source_root = Some(resolve_path(base, &value));
                }
                ("", "addon_root") | ("project", "addon_root") | ("gmod", "addon_root") => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    addon_root = Some(resolve_path(base, &value));
                }
                ("", "generated_root")
                | ("project", "generated_root")
                | ("gmod", "generated_root") => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    generated_root = Some(resolve_path(base, &value));
                }
                ("", "package_roots")
                | ("project", "package_roots")
                | ("gmod", "package_roots") => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    package_roots = parse_path_list(base, &value);
                }
                ("", "source_comments")
                | ("project", "source_comments")
                | ("gmod", "source_comments") => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    let Some(mode) = SourceCommentMode::parse(&value) else {
                        return Err(parse_error(
                            path,
                            line_index,
                            "source_comments must be \"none\", \"readable\", \"boundary\", or \"dense\"",
                        ));
                    };
                    source_comments = Some(mode);
                }
                ("target.gmod.realm", "unknown_external") => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    let Some(policy) = UnknownExternalPolicy::parse(&value) else {
                        return Err(parse_error(
                            path,
                            line_index,
                            "unknown_external must be \"allow\", \"warn\", or \"error\"",
                        ));
                    };
                    gmod_unknown_external = Some(policy);
                }
                ("target.gmod.extern", extern_path) => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    let Some(realm) = Realm::parse(&value) else {
                        return Err(parse_error(
                            path,
                            line_index,
                            "extern realm must be \"shared\", \"client\", or \"server\"",
                        ));
                    };
                    gmod_externs.push(manifest_extern(extern_path, realm));
                }
                (section_name, "realm") if section_name.starts_with("target.gmod.extern.") => {
                    let value = parse_string_value(path, line_index, raw_value)?;
                    let Some(realm) = Realm::parse(&value) else {
                        return Err(parse_error(
                            path,
                            line_index,
                            "extern realm must be \"shared\", \"client\", or \"server\"",
                        ));
                    };
                    let extern_path = section_name
                        .trim_start_matches("target.gmod.extern.")
                        .trim();
                    gmod_externs.push(manifest_extern(extern_path, realm));
                }
                _ => {
                    return Err(parse_error(
                        path,
                        line_index,
                        format!("unknown manifest key `{key}` in section `[{}]`", section),
                    ));
                }
            }
        }

        let source_root =
            source_root.ok_or_else(|| parse_error(path, 0, "missing `source_root`"))?;
        let addon_root = addon_root.ok_or_else(|| parse_error(path, 0, "missing `addon_root`"))?;

        let mut manifest = Self {
            package_id,
            bundle_id,
            source_root,
            addon_root,
            generated_root,
            package_roots,
            source_comments,
            gmod_unknown_external,
            gmod_externs,
        };
        if let Ok(lock_roots) = lockfile_package_roots(base) {
            for root in lock_roots {
                if !manifest
                    .package_roots
                    .iter()
                    .any(|existing| existing == &root)
                {
                    manifest.package_roots.push(root);
                }
            }
        }
        Ok(manifest)
    }
}

fn strip_comment(line: &str) -> &str {
    line.split_once('#')
        .map(|(before, _)| before)
        .unwrap_or(line)
}

fn parse_string_value(
    path: &Path,
    line_index: usize,
    value: &str,
) -> Result<String, ManifestError> {
    let Some(stripped) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return Err(parse_error(
            path,
            line_index,
            "manifest values must be quoted strings",
        ));
    };

    Ok(stripped.replace("\\\"", "\"").replace("\\\\", "\\"))
}

fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn parse_path_list(base: &Path, value: &str) -> Vec<PathBuf> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| resolve_path(base, item))
        .collect()
}

fn manifest_extern(path: &str, realm: Realm) -> ExternSymbol {
    let path = path.trim().trim_matches('"');
    ExternSymbol::known(path, realm)
}

fn parse_error(path: &Path, line_index: usize, message: impl Into<String>) -> ManifestError {
    ManifestError::Parse {
        path: path.to_path_buf(),
        message: format!("line {}: {}", line_index + 1, message.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::ProjectManifest;

    #[test]
    fn parses_gmod_manifest_paths_relative_to_manifest_file() {
        let manifest = ProjectManifest::parse(
            Path::new("C:/game/addons/lux/lux.toml"),
            r#"
            [gmod]
            source_root = "src"
            addon_root = "."
            generated_root = "generated"
            package_roots = "packages, vendor/lux-packages"
            source_comments = "boundary"
            bundle_id = "docs_bundle"
            "#,
        )
        .expect("manifest");

        assert_eq!(manifest.bundle_id.as_deref(), Some("docs_bundle"));
        assert!(manifest.source_root.ends_with("src"));
        assert!(manifest.addon_root.ends_with("lux"));
        assert!(
            manifest
                .generated_root
                .expect("generated")
                .ends_with("generated")
        );
        assert_eq!(manifest.package_roots.len(), 2);
        assert!(manifest.package_roots[0].ends_with("packages"));
        assert!(manifest.package_roots[1].ends_with("vendor/lux-packages"));
        assert_eq!(
            manifest.source_comments,
            Some(crate::sourcemap::SourceCommentMode::Boundary)
        );
    }

    #[test]
    fn rejects_unknown_manifest_keys() {
        let error = ProjectManifest::parse(
            Path::new("lux.toml"),
            r#"
            source_root = "src"
            addon_root = "addon"
            nope = "x"
            "#,
        )
        .expect_err("invalid manifest");

        assert!(error.to_string().contains("unknown manifest key"));
    }

    #[test]
    fn parses_gmod_realm_policy_and_externs() {
        let manifest = ProjectManifest::parse(
            Path::new("lux.toml"),
            r#"
            source_root = "src"
            addon_root = "addon"

            [target.gmod.realm]
            unknown_external = "error"

            [target.gmod.extern]
            ThirdPartyAddon = "server"
            net.Start = "server"

            [target.gmod.extern."FancyHud.Open"]
            realm = "client"
            "#,
        )
        .expect("manifest");

        assert_eq!(
            manifest.gmod_unknown_external,
            Some(crate::resolve::UnknownExternalPolicy::Error)
        );
        assert!(
            manifest
                .gmod_externs
                .iter()
                .any(|symbol| symbol.path == vec!["ThirdPartyAddon".to_string()])
        );
        assert!(
            manifest
                .gmod_externs
                .iter()
                .any(|symbol| symbol.path == vec!["net".to_string(), "Start".to_string()])
        );
        assert!(
            manifest
                .gmod_externs
                .iter()
                .any(|symbol| symbol.path == vec!["FancyHud".to_string(), "Open".to_string()])
        );
    }
}
