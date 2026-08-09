use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const CONFIG_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GraphSetting {
    Disabled,
    Grafeo,
    Surrealdb,
    Inherit,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GraphSettings {
    #[serde(default = "inherit_graph")]
    pub setting: GraphSetting,
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
    pub setting: GraphSetting,
    pub origin: String,
}

fn inherit_graph() -> GraphSetting {
    GraphSetting::Inherit
}

fn default_graph() -> GraphSettings {
    GraphSettings {
        setting: GraphSetting::Inherit,
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

pub fn parse_setting(value: &str) -> Result<GraphSetting> {
    match value {
        "disabled" => Ok(GraphSetting::Disabled),
        "grafeo" => Ok(GraphSetting::Grafeo),
        "surrealdb" => Ok(GraphSetting::Surrealdb),
        "inherit" => Ok(GraphSetting::Inherit),
        _ => Err(AppError::new(
            "invalid_graph_engine",
            format!(
                "unsupported graph engine '{value}'; use disabled, grafeo, surrealdb, or inherit"
            ),
        )),
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
            let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
                AppError::new("invalid_config", format!("invalid graph config: {error}"))
            })?;
            let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
            if version == 1 {
                return Ok(ConfigFile::default());
            }
            if version != u64::from(CONFIG_VERSION) {
                return Err(AppError::new(
                    "unsupported_config_version",
                    format!("unsupported config version {version}"),
                ));
            }
            serde_json::from_value(value).map_err(|error| {
                AppError::new("invalid_config", format!("invalid graph config: {error}"))
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn resolve(scope: &str, database: &Path) -> Result<EffectiveGraphConfig> {
    let mut setting = GraphSetting::Disabled;
    let mut origin = "built-in".to_string();

    if let Some(path) = global_config_path() {
        let global = load_file(&path)?;
        if global.graph.setting != GraphSetting::Inherit {
            setting = global.graph.setting;
            origin = "global".to_string();
        }
    }
    if scope == "project" {
        let project = load_file(&config_path_for_database(database)?)?;
        if project.graph.setting != GraphSetting::Inherit {
            setting = project.graph.setting;
            origin = "project".to_string();
        }
    }
    Ok(EffectiveGraphConfig { setting, origin })
}

pub fn update(database: &Path, setting: GraphSetting) -> Result<(PathBuf, ConfigFile)> {
    let path = config_path_for_database(database)?;
    reject_symlink(&path)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("invalid_config_path", "config has no parent"))?;
    reject_symlink(parent)?;
    fs::create_dir_all(parent)?;
    let mut config = load_file(&path)?;
    config.version = CONFIG_VERSION;
    config.graph.setting = setting;
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

pub fn response(scope: &str, database: &Path) -> Result<Value> {
    let effective = resolve(scope, database)?;
    Ok(json!({
        "scope": scope,
        "path": config_path_for_database(database)?,
        "graph": effective,
    }))
}
