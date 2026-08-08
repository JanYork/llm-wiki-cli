use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PhysicalSetting {
    Enabled,
    Disabled,
    Inherit,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EngineSetting {
    Auto,
    Graphqlite,
    Rslg,
    Inherit,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GraphSettings {
    #[serde(default = "inherit_physical")]
    pub physical: PhysicalSetting,
    #[serde(default = "inherit_engine")]
    pub engine: EngineSetting,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub version: u32,
    #[serde(default = "default_graph")]
    pub graph: GraphSettings,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EffectiveGraphConfig {
    pub physical: PhysicalSetting,
    pub engine: EngineSetting,
    pub resolved_engine: EngineSetting,
    pub physical_origin: String,
    pub engine_origin: String,
    pub graphqlite_available: bool,
}

fn inherit_physical() -> PhysicalSetting {
    PhysicalSetting::Inherit
}
fn inherit_engine() -> EngineSetting {
    EngineSetting::Inherit
}
fn default_graph() -> GraphSettings {
    GraphSettings {
        physical: PhysicalSetting::Inherit,
        engine: EngineSetting::Inherit,
    }
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            graph: default_graph(),
        }
    }
}

pub fn config_path_for_database(database: &Path) -> Result<PathBuf> {
    let parent = database.parent().ok_or_else(|| {
        AppError::new(
            "invalid_config_path",
            "wiki database has no configuration directory",
        )
    })?;
    Ok(parent.join("config.json"))
}

fn global_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".lwc/config.json"))
}

pub fn load_file(path: &Path) -> Result<ConfigFile> {
    reject_symlink(path)?;
    match fs::read(path) {
        Ok(bytes) => {
            let config: ConfigFile = serde_json::from_slice(&bytes).map_err(|error| {
                AppError::new("invalid_config", format!("invalid graph config: {error}"))
            })?;
            if config.version != CONFIG_VERSION {
                return Err(AppError::new(
                    "unsupported_config_version",
                    format!("unsupported config version {}", config.version),
                ));
            }
            Ok(config)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn resolve(scope: &str, database: &Path) -> Result<EffectiveGraphConfig> {
    let mut physical = PhysicalSetting::Disabled;
    let mut engine = EngineSetting::Auto;
    let mut physical_origin = "built-in".to_string();
    let mut engine_origin = "built-in".to_string();

    if let Some(path) = global_config_path() {
        let global = load_file(&path)?;
        if global.graph.physical != PhysicalSetting::Inherit {
            physical = global.graph.physical;
            physical_origin = "global".to_string();
        }
        if global.graph.engine != EngineSetting::Inherit {
            engine = global.graph.engine;
            engine_origin = "global".to_string();
        }
    }
    if scope == "project" {
        let project = load_file(&config_path_for_database(database)?)?;
        if project.graph.physical != PhysicalSetting::Inherit {
            physical = project.graph.physical;
            physical_origin = "project".to_string();
        }
        if project.graph.engine != EngineSetting::Inherit {
            engine = project.graph.engine;
            engine_origin = "project".to_string();
        }
    }
    let available = crate::graph_backend::embedded_graphqlite_available();
    let resolved_engine = match engine {
        EngineSetting::Auto if available => EngineSetting::Graphqlite,
        EngineSetting::Auto => EngineSetting::Rslg,
        EngineSetting::Inherit => unreachable!("inherit is resolved before this point"),
        value => value,
    };
    if resolved_engine == EngineSetting::Graphqlite && !available {
        return Err(AppError::new(
            "graphqlite_unavailable",
            "GraphQLite is unavailable on this platform; use engine=rslg or auto",
        ));
    }
    Ok(EffectiveGraphConfig {
        physical,
        engine,
        resolved_engine,
        physical_origin,
        engine_origin,
        graphqlite_available: available,
    })
}

pub fn update(
    database: &Path,
    physical: Option<PhysicalSetting>,
    engine: Option<EngineSetting>,
) -> Result<(PathBuf, ConfigFile)> {
    let path = config_path_for_database(database)?;
    reject_symlink(&path)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("invalid_config_path", "config has no parent"))?;
    reject_symlink(parent)?;
    fs::create_dir_all(parent)?;
    let mut config = load_file(&path)?;
    if let Some(value) = physical {
        config.graph.physical = value;
    }
    if let Some(value) = engine {
        config.graph.engine = value;
    }
    let bytes = serde_json::to_vec_pretty(&config)
        .map_err(|error| AppError::new("json_error", error.to_string()))?;
    let temporary = parent.join(format!(
        ".config-{}-{}.tmp",
        std::process::id(),
        unique_suffix()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, &path)?;
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok((path, config))
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AppError::new(
            "unsafe_config_path",
            format!("configuration path is a symlink: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn unique_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

pub fn response(scope: &str, database: &Path) -> Result<serde_json::Value> {
    let effective = resolve(scope, database)?;
    Ok(json!({
        "scope": scope,
        "path": config_path_for_database(database)?,
        "graph": effective,
    }))
}
