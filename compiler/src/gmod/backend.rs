use std::ffi::OsString;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::sourcemap::SourceCommentMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Realm {
    Shared,
    Client,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmodModule {
    pub module_id: String,
    pub lux_path: PathBuf,
    pub lua_path: PathBuf,
    pub realm: Realm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmodBackendConfig {
    pub source_root: PathBuf,
    pub output_root: PathBuf,
    pub bundle_id: String,
    pub runtime_base: PathBuf,
    pub source_comments: SourceCommentMode,
    pub autorun: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmodBuildPlan {
    pub config: GmodBackendConfig,
    pub modules: Vec<GmodModule>,
    pub loader: LoaderPlan,
    pub autorun: Option<AutorunForwarder>,
    pub registry: ModuleRegistryPlan,
    pub packaging: Option<GmaPackagePlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderPlan {
    pub shared_loader: LoaderFile,
    pub client_loader: LoaderFile,
    pub server_loader: LoaderFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderFile {
    pub path: PathBuf,
    pub runtime_path: PathBuf,
    pub operations: Vec<LoaderOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoaderOperation {
    AddCsLuaFile(PathBuf),
    Include(PathBuf),
    RegisterModule { module_id: String, path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutorunForwarder {
    pub path: PathBuf,
    pub runtime_path: PathBuf,
    pub shared_loader: PathBuf,
    pub client_loader: PathBuf,
    pub server_loader: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRegistryPlan {
    pub bundle_id: String,
    pub global_name: String,
    pub local_name: String,
    pub import_function_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmaPackagePlan {
    pub gmad_path: PathBuf,
    pub addon_json: PathBuf,
    pub output_gma: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPlan {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmodPathError {
    path: PathBuf,
}

impl GmodPathError {
    fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }
}

impl fmt::Display for GmodPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GMod runtime path `{}` must be a non-empty relative path without `..`",
            self.path.display()
        )
    }
}

impl std::error::Error for GmodPathError {}

impl Realm {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Client => "client",
            Self::Server => "server",
        }
    }

    pub fn from_source_path(path: impl AsRef<Path>) -> Option<Self> {
        let mut saw_src = false;
        for component in path.as_ref().components() {
            let Component::Normal(part) = component else {
                continue;
            };
            let part = part.to_string_lossy();
            if saw_src {
                return match part.as_ref() {
                    "shared" => Some(Self::Shared),
                    "client" => Some(Self::Client),
                    "server" => Some(Self::Server),
                    _ => None,
                };
            }
            saw_src = part == "src";
        }
        None
    }
}

impl GmodBackendConfig {
    pub fn new(source_root: impl Into<PathBuf>, output_root: impl Into<PathBuf>) -> Self {
        let source_root = source_root.into();
        let output_root = output_root.into();
        let bundle_id = sanitize_bundle_id(&bundle_id_from_path(&output_root));
        let runtime_base = default_runtime_base_for_bundle(&bundle_id);
        Self {
            source_root,
            output_root,
            bundle_id,
            runtime_base,
            source_comments: SourceCommentMode::Readable,
            autorun: true,
        }
    }

    pub fn set_bundle_id(&mut self, bundle_id: impl AsRef<str>) {
        self.bundle_id = sanitize_bundle_id(bundle_id.as_ref());
        self.runtime_base = default_runtime_base_for_bundle(&self.bundle_id);
    }

    pub fn set_runtime_base(
        &mut self,
        runtime_base: impl AsRef<Path>,
    ) -> Result<(), GmodPathError> {
        self.runtime_base = safe_gmod_relative_path(runtime_base.as_ref())
            .ok_or_else(|| GmodPathError::new(runtime_base.as_ref()))?;
        Ok(())
    }
}

impl GmodBuildPlan {
    pub fn new(output_root: impl Into<PathBuf>) -> Self {
        let output_root = output_root.into();
        let config = GmodBackendConfig::new("src", output_root);
        Self::from_config(config)
    }

    pub fn from_config(config: GmodBackendConfig) -> Self {
        Self {
            loader: LoaderPlan::empty(&config.output_root, &config.runtime_base),
            autorun: None,
            registry: ModuleRegistryPlan::for_bundle(&config.bundle_id),
            config,
            modules: Vec::new(),
            packaging: None,
        }
    }

    pub fn with_gma_packaging(
        mut self,
        gmad_path: impl Into<PathBuf>,
        addon_json: impl Into<PathBuf>,
        output_gma: impl Into<PathBuf>,
    ) -> Self {
        self.packaging = Some(GmaPackagePlan {
            gmad_path: gmad_path.into(),
            addon_json: addon_json.into(),
            output_gma: output_gma.into(),
        });
        self
    }

    pub fn add_module(
        mut self,
        lux_path: impl Into<PathBuf>,
        lua_path: impl Into<PathBuf>,
        realm: Realm,
    ) -> Self {
        let lux_path = lux_path.into();
        let lua_path = lua_path.into();
        let module_id = module_id_from_lua_path(&lua_path, realm);
        self.modules.push(GmodModule {
            module_id,
            lux_path,
            lua_path,
            realm,
        });
        self.rebuild_loader();
        self
    }

    pub fn add_source_module(mut self, lux_path: impl Into<PathBuf>) -> Self {
        let lux_path = lux_path.into();
        let realm = Realm::from_source_path(&lux_path).unwrap_or(Realm::Shared);
        let lua_path = lua_path_for_source(&self.config, &lux_path, realm);
        let module_id = module_id_for_source(&self.config, &lux_path, realm);
        self.modules.push(GmodModule {
            module_id,
            lux_path,
            lua_path,
            realm,
        });
        self.rebuild_loader();
        self
    }

    pub fn sorted_modules(&self) -> Vec<&GmodModule> {
        self.modules.iter().collect()
    }

    pub fn gma_command(&self, package_root: impl AsRef<Path>) -> Option<CommandPlan> {
        let packaging = self.packaging.as_ref()?;
        Some(CommandPlan {
            program: packaging.gmad_path.clone(),
            args: vec![
                OsString::from("create"),
                OsString::from("-folder"),
                package_root.as_ref().as_os_str().to_os_string(),
                OsString::from("-out"),
                packaging.output_gma.as_os_str().to_os_string(),
            ],
        })
    }

    pub fn rebuild_loader(&mut self) {
        let shared_loader_path =
            loader_relative_path(&self.config.runtime_base, LoaderKind::Shared);
        let client_loader_path =
            loader_relative_path(&self.config.runtime_base, LoaderKind::Client);
        let server_loader_path =
            loader_relative_path(&self.config.runtime_base, LoaderKind::Server);
        let mut shared_ops = vec![
            LoaderOperation::AddCsLuaFile(shared_loader_path.clone()),
            LoaderOperation::AddCsLuaFile(client_loader_path.clone()),
        ];
        let mut client_ops = Vec::new();
        let mut server_ops = Vec::new();

        for module in self.sorted_modules() {
            let lua_path = runtime_path_for_output_path(&self.config, &module.lua_path);
            match module.realm {
                Realm::Shared => {
                    shared_ops.push(LoaderOperation::AddCsLuaFile(lua_path.clone()));
                    shared_ops.push(LoaderOperation::RegisterModule {
                        module_id: module.module_id.clone(),
                        path: lua_path.clone(),
                    });
                    client_ops.push(LoaderOperation::RegisterModule {
                        module_id: module.module_id.clone(),
                        path: lua_path.clone(),
                    });
                    server_ops.push(LoaderOperation::RegisterModule {
                        module_id: module.module_id.clone(),
                        path: lua_path,
                    });
                }
                Realm::Client => {
                    shared_ops.push(LoaderOperation::AddCsLuaFile(lua_path.clone()));
                    client_ops.push(LoaderOperation::RegisterModule {
                        module_id: module.module_id.clone(),
                        path: lua_path,
                    });
                }
                Realm::Server => {
                    server_ops.push(LoaderOperation::RegisterModule {
                        module_id: module.module_id.clone(),
                        path: lua_path,
                    });
                }
            }
        }

        self.loader = LoaderPlan {
            shared_loader: LoaderFile {
                path: self.config.output_root.join(&shared_loader_path),
                runtime_path: shared_loader_path,
                operations: shared_ops,
            },
            client_loader: LoaderFile {
                path: self.config.output_root.join(&client_loader_path),
                runtime_path: client_loader_path,
                operations: client_ops,
            },
            server_loader: LoaderFile {
                path: self.config.output_root.join(&server_loader_path),
                runtime_path: server_loader_path,
                operations: server_ops,
            },
        };
        self.autorun = self.config.autorun.then(|| {
            let runtime_path = autorun_relative_path(&self.config.bundle_id);
            AutorunForwarder {
                path: self.config.output_root.join(&runtime_path),
                runtime_path,
                shared_loader: self.loader.shared_loader.runtime_path.clone(),
                client_loader: self.loader.client_loader.runtime_path.clone(),
                server_loader: self.loader.server_loader.runtime_path.clone(),
            }
        });
    }
}

impl LoaderPlan {
    pub fn empty(root: impl AsRef<Path>, runtime_base: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        let runtime_base = safe_gmod_relative_path(runtime_base.as_ref())
            .unwrap_or_else(|| PathBuf::from("lux/app"));
        let shared_loader_path = loader_relative_path(&runtime_base, LoaderKind::Shared);
        let client_loader_path = loader_relative_path(&runtime_base, LoaderKind::Client);
        let server_loader_path = loader_relative_path(&runtime_base, LoaderKind::Server);
        Self {
            shared_loader: LoaderFile {
                path: root.join(&shared_loader_path),
                runtime_path: shared_loader_path,
                operations: Vec::new(),
            },
            client_loader: LoaderFile {
                path: root.join(&client_loader_path),
                runtime_path: client_loader_path,
                operations: Vec::new(),
            },
            server_loader: LoaderFile {
                path: root.join(&server_loader_path),
                runtime_path: server_loader_path,
                operations: Vec::new(),
            },
        }
    }
}

impl AutorunForwarder {
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("-- Generated by luxc. Do not edit by hand.\n");
        out.push_str("if SERVER then\n");
        out.push_str(&format!(
            "  AddCSLuaFile({})\n",
            lua_string_gmod_path(&self.runtime_path)
        ));
        out.push_str("end\n");
        out.push_str(&format!(
            "include({})\n",
            lua_string_gmod_path(&self.shared_loader)
        ));
        out.push_str("if SERVER then\n");
        out.push_str(&format!(
            "  include({})\n",
            lua_string_gmod_path(&self.server_loader)
        ));
        out.push_str("end\n");
        out.push_str("if CLIENT then\n");
        out.push_str(&format!(
            "  include({})\n",
            lua_string_gmod_path(&self.client_loader)
        ));
        out.push_str("end\n");
        out
    }
}

impl LoaderFile {
    pub fn render(&self, registry: &ModuleRegistryPlan) -> String {
        let mut out = String::new();
        out.push_str("-- Generated by luxc. Do not edit by hand.\n");
        out.push_str(&registry.render_bootstrap());
        let mut index = 0;
        while index < self.operations.len() {
            match &self.operations[index] {
                LoaderOperation::AddCsLuaFile(path) => {
                    out.push_str("if SERVER then\n");
                    out.push_str(&format!("  AddCSLuaFile({})\n", lua_string_gmod_path(path)));
                    index += 1;
                    while let Some(LoaderOperation::AddCsLuaFile(path)) = self.operations.get(index)
                    {
                        out.push_str(&format!("  AddCSLuaFile({})\n", lua_string_gmod_path(path)));
                        index += 1;
                    }
                    out.push_str("end\n");
                    continue;
                }
                LoaderOperation::Include(path) => {
                    out.push_str(&format!("include({})\n", lua_string_gmod_path(path)));
                }
                LoaderOperation::RegisterModule { module_id, path } => {
                    out.push_str("do\n");
                    out.push_str(&format!(
                        "  if {}[{}] == nil then\n",
                        registry.local_name,
                        lua_string(module_id)
                    ));
                    out.push_str(&format!(
                        "    local __lux_factory = include({})\n",
                        lua_string_gmod_path(path)
                    ));
                    out.push_str(&format!(
                        "    {}[{}] = __lux_factory({}) or {{}}\n",
                        registry.local_name,
                        lua_string(module_id),
                        registry.import_function_name
                    ));
                    out.push_str("  end\n");
                    out.push_str("end\n");
                }
            }
            index += 1;
        }
        out
    }
}

impl Default for ModuleRegistryPlan {
    fn default() -> Self {
        Self::for_bundle("app")
    }
}

impl ModuleRegistryPlan {
    pub fn for_bundle(bundle_id: impl AsRef<str>) -> Self {
        let bundle_id = sanitize_bundle_id(bundle_id.as_ref());
        Self {
            global_name: format!("__lux_bundle_{bundle_id}_modules"),
            local_name: "__lux_registry".into(),
            import_function_name: "__lux_import".into(),
            bundle_id,
        }
    }

    pub fn render_bootstrap(&self) -> String {
        format!(
            "{global} = {global} or {{}}\nlocal {local_name} = {global}\nlocal function {import}(id)\n  local module = {local_name}[id]\n  if module == nil then\n    error(\"Lux module not loaded in bundle {bundle}: \" .. tostring(id), 2)\n  end\n  return module\nend\n",
            global = self.global_name,
            local_name = self.local_name,
            bundle = lua_string_content(&self.bundle_id),
            import = self.import_function_name
        )
    }

    pub fn wrap_module_lua(&self, lua: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("return function({})\n", self.import_function_name));
        for line in lua.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("end\n");
        out
    }
}

impl GmaPackagePlan {
    pub fn command(&self, package_root: impl AsRef<Path>) -> CommandPlan {
        CommandPlan {
            program: self.gmad_path.clone(),
            args: vec![
                OsString::from("create"),
                OsString::from("-folder"),
                package_root.as_ref().as_os_str().to_os_string(),
                OsString::from("-out"),
                self.output_gma.as_os_str().to_os_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LoaderKind {
    Shared,
    Client,
    Server,
}

fn loader_relative_path(runtime_base: &Path, kind: LoaderKind) -> PathBuf {
    let mut path =
        safe_gmod_relative_path(runtime_base).unwrap_or_else(|| PathBuf::from("lux/app"));
    match kind {
        LoaderKind::Shared => path.push("loader_shared.lua"),
        LoaderKind::Client => path.push("loader_client.lua"),
        LoaderKind::Server => path.push("loader_server.lua"),
    }
    path
}

fn autorun_relative_path(bundle_id: &str) -> PathBuf {
    PathBuf::from("autorun").join(format!("{}.lua", sanitize_bundle_id(bundle_id)))
}

fn bundle_id_from_path(path: &Path) -> String {
    let raw = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "app".into());
    sanitize_bundle_id(&raw)
}

fn sanitize_bundle_id(raw: &str) -> String {
    sanitize_lua_path_segment(raw)
}

fn sanitize_lua_path_segment(raw: &str) -> String {
    let mut out = String::new();
    let mut last_was_separator = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator && !out.is_empty() {
            out.push('_');
            last_was_separator = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() { "app".into() } else { out }
}

pub fn default_runtime_base_for_bundle(bundle_id: &str) -> PathBuf {
    PathBuf::from("lux").join(sanitize_bundle_id(bundle_id))
}

pub fn safe_gmod_relative_path(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

fn module_id_for_source(config: &GmodBackendConfig, lux_path: &Path, realm: Realm) -> String {
    let rel = strip_source_realm_prefix(&config.source_root, lux_path, realm).unwrap_or(lux_path);
    let mut parts = Vec::new();
    parts.push(realm_dir(realm).to_string());
    for component in rel.components() {
        if let Component::Normal(part) = component {
            parts.push(part.to_string_lossy().to_string());
        }
    }
    if let Some(last) = parts.last_mut() {
        if let Some(stripped) = last.strip_suffix(".lux") {
            *last = stripped.to_string();
        }
    }
    parts.join("/")
}

fn lua_path_for_source(config: &GmodBackendConfig, lux_path: &Path, realm: Realm) -> PathBuf {
    let mut path = config.output_root.join(&config.runtime_base);
    path.push(realm_dir(realm));
    let rel = strip_source_realm_prefix(&config.source_root, lux_path, realm).unwrap_or(lux_path);
    for component in rel.components() {
        if let Component::Normal(part) = component {
            path.push(part);
        }
    }
    path.set_extension("lua");
    path
}

fn strip_source_realm_prefix<'a>(
    source_root: &Path,
    lux_path: &'a Path,
    realm: Realm,
) -> Option<&'a Path> {
    let with_realm = source_root.join(realm_dir(realm));
    lux_path
        .strip_prefix(&with_realm)
        .ok()
        .or_else(|| lux_path.strip_prefix(source_root).ok())
}

fn module_id_from_lua_path(lua_path: &Path, realm: Realm) -> String {
    let mut parts = Vec::new();
    for component in lua_path.components() {
        if let Component::Normal(part) = component {
            parts.push(part.to_string_lossy().to_string());
        }
    }

    if let Some(realm_index) = parts
        .iter()
        .enumerate()
        .find_map(|(index, part)| (part == realm_dir(realm)).then_some(index))
    {
        parts.drain(0..realm_index);
    }

    if let Some(last) = parts.last_mut() {
        if let Some(stripped) = last.strip_suffix(".lua") {
            *last = stripped.to_string();
        }
    }
    parts.join("/")
}

fn realm_dir(realm: Realm) -> &'static str {
    match realm {
        Realm::Shared => "shared",
        Realm::Client => "client",
        Realm::Server => "server",
    }
}

fn runtime_path_for_output_path(config: &GmodBackendConfig, path: &Path) -> PathBuf {
    path.strip_prefix(&config.output_root)
        .map(clean_gmod_path)
        .unwrap_or_else(|_| clean_gmod_path(path))
}

fn clean_gmod_path(path: &Path) -> PathBuf {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_os_string()),
            _ => None,
        })
        .collect()
}

fn lua_string_gmod_path(path: &Path) -> String {
    lua_string_path(&clean_gmod_path(path))
}

fn lua_string_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn lua_string(value: &str) -> String {
    let mut out = String::from("\"");
    out.push_str(&lua_string_content(value));
    out.push('"');
    out
}

fn lua_string_content(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::{GmaPackagePlan, GmodBackendConfig, GmodBuildPlan, LoaderOperation, Realm};

    #[test]
    fn creates_loader_paths_under_output_root() {
        let plan = GmodBuildPlan::new("addon").add_module(
            "src/shared/foo.lux",
            "addon/lux/addon/shared/foo.lua",
            Realm::Shared,
        );

        assert_eq!(plan.modules.len(), 1);
        assert!(
            plan.loader
                .shared_loader
                .path
                .ends_with("lux/addon/loader_shared.lua")
        );
        assert!(
            plan.autorun
                .as_ref()
                .expect("autorun")
                .path
                .ends_with("autorun/addon.lua")
        );
    }

    #[test]
    fn infers_realm_from_explicit_source_folders() {
        assert_eq!(
            Realm::from_source_path("src/shared/foo.lux"),
            Some(Realm::Shared)
        );
        assert_eq!(
            Realm::from_source_path("src/client/foo.lux"),
            Some(Realm::Client)
        );
        assert_eq!(
            Realm::from_source_path("src/server/foo.lux"),
            Some(Realm::Server)
        );
        assert_eq!(Realm::from_source_path("src/unknown/foo.lux"), None);
    }

    #[test]
    fn builds_loader_operations_for_gmod_realms() {
        let config = GmodBackendConfig::new("src", "generated");
        let plan = GmodBuildPlan::from_config(config)
            .add_source_module("src/shared/core.lux")
            .add_source_module("src/client/ui.lux")
            .add_source_module("src/server/init.lux");

        assert!(
            plan.loader
                .shared_loader
                .operations
                .contains(&LoaderOperation::AddCsLuaFile(PathBuf::from(
                    "lux/generated/shared/core.lua"
                )))
        );
        assert!(
            plan.loader
                .shared_loader
                .operations
                .contains(&LoaderOperation::AddCsLuaFile(PathBuf::from(
                    "lux/generated/client/ui.lua"
                )))
        );
        assert!(
            plan.loader
                .client_loader
                .operations
                .contains(&LoaderOperation::RegisterModule {
                    module_id: "client/ui".into(),
                    path: PathBuf::from("lux/generated/client/ui.lua")
                })
        );
        assert!(
            plan.loader
                .server_loader
                .operations
                .contains(&LoaderOperation::RegisterModule {
                    module_id: "server/init".into(),
                    path: PathBuf::from("lux/generated/server/init.lua")
                })
        );

        assert!(
            plan.loader
                .client_loader
                .operations
                .contains(&LoaderOperation::RegisterModule {
                    module_id: "shared/core".into(),
                    path: PathBuf::from("lux/generated/shared/core.lua")
                })
        );
        assert!(
            plan.loader
                .server_loader
                .operations
                .contains(&LoaderOperation::RegisterModule {
                    module_id: "shared/core".into(),
                    path: PathBuf::from("lux/generated/shared/core.lua")
                })
        );
    }

    #[test]
    fn renders_loader_lua_with_forward_slashes() {
        let plan = GmodBuildPlan::new("addon").add_module(
            "src/client/ui.lux",
            "addon\\lux\\addon\\client\\ui.lua",
            Realm::Client,
        );
        let lua = plan.loader.shared_loader.render(&plan.registry);
        assert!(lua.contains(
            "if SERVER then\n  AddCSLuaFile(\"lux/addon/loader_shared.lua\")\n  AddCSLuaFile(\"lux/addon/loader_client.lua\")\n  AddCSLuaFile(\"lux/addon/client/ui.lua\")\nend\n"
        ));
        assert!(!lua.contains("if SERVER then AddCSLuaFile"));
    }

    #[test]
    fn loader_registers_modules_in_private_registry() {
        let plan = GmodBuildPlan::new("addon").add_module(
            "src/shared/foo.lux",
            "addon/lux/addon/shared/foo.lua",
            Realm::Shared,
        );
        let lua = plan.loader.shared_loader.render(&plan.registry);
        assert!(lua.contains("__lux_bundle_addon_modules = __lux_bundle_addon_modules or {}"));
        assert!(lua.contains("local __lux_registry = __lux_bundle_addon_modules"));
        assert!(lua.contains("if __lux_registry[\"shared/foo\"] == nil then"));
        assert!(lua.contains("local __lux_factory = include(\"lux/addon/shared/foo.lua\")"));
        assert!(lua.contains("__lux_registry[\"shared/foo\"] = __lux_factory(__lux_import) or {}"));
    }

    #[test]
    fn autorun_forwarder_includes_generated_loaders() {
        let plan = GmodBuildPlan::new("addon").add_module(
            "src/shared/foo.lux",
            "addon/lux/addon/shared/foo.lua",
            Realm::Shared,
        );
        let lua = plan.autorun.expect("autorun").render();

        assert!(lua.contains("AddCSLuaFile(\"autorun/addon.lua\")"));
        assert!(lua.contains("include(\"lux/addon/loader_shared.lua\")"));
        assert!(lua.contains("include(\"lux/addon/loader_server.lua\")"));
        assert!(lua.contains("include(\"lux/addon/loader_client.lua\")"));
    }

    #[test]
    fn wraps_module_lua_as_importable_factory() {
        let registry = super::ModuleRegistryPlan::default();
        let wrapped = registry.wrap_module_lua("local __lux_exports = {}\nreturn __lux_exports\n");

        assert!(wrapped.starts_with("return function(__lux_import)\n"));
        assert!(wrapped.contains("  local __lux_exports = {}"));
        assert!(wrapped.contains("  return __lux_exports"));
        assert!(wrapped.ends_with("end\n"));
    }

    #[test]
    fn packaging_plan_is_command_only() {
        let package = GmaPackagePlan {
            gmad_path: PathBuf::from("gmad.exe"),
            addon_json: PathBuf::from("addon/addon.json"),
            output_gma: PathBuf::from("dist/lux.gma"),
        };
        let command = package.command("addon");
        assert_eq!(command.program, PathBuf::from("gmad.exe"));
        assert_eq!(command.args[0], OsString::from("create"));
        assert!(command.args.contains(&OsString::from("-folder")));
    }
}
