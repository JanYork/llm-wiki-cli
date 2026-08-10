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

pub const CONFIG_VERSION: u32 = 3;
pub const DEFAULT_TRANS_TIMEOUT_SECONDS: u16 = 120;
pub const MIN_TRANS_TIMEOUT_SECONDS: u16 = 1;
pub const MAX_TRANS_TIMEOUT_SECONDS: u16 = 900;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GraphSetting {
    Disabled,
    Grafeo,
    Surrealdb,
    Inherit,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TransSetting {
    Disabled,
    Anydoc,
    Markitdown,
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
pub struct TransSettings {
    #[serde(default = "inherit_trans")]
    pub setting: TransSetting,
    #[serde(default = "default_trans_timeout_seconds")]
    pub timeout_seconds: u16,
    #[serde(default)]
    pub anydoc_args: Vec<String>,
    #[serde(default)]
    pub markitdown_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub version: u32,
    #[serde(default = "default_graph")]
    pub graph: GraphSettings,
    #[serde(default = "default_trans")]
    pub trans: TransSettings,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EffectiveGraphConfig {
    pub setting: GraphSetting,
    pub origin: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EffectiveTransConfig {
    pub setting: TransSetting,
    pub origin: String,
    pub timeout_seconds: u16,
    pub anydoc_args: Vec<String>,
    pub markitdown_args: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ConfigPatch {
    pub graph: Option<GraphSetting>,
    pub trans: Option<TransSettings>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfigFileV2 {
    pub version: u32,
    #[serde(default = "default_graph")]
    pub graph: GraphSettings,
}

fn inherit_graph() -> GraphSetting {
    GraphSetting::Inherit
}

fn inherit_trans() -> TransSetting {
    TransSetting::Inherit
}

fn default_graph() -> GraphSettings {
    GraphSettings {
        setting: GraphSetting::Inherit,
    }
}

fn default_trans_timeout_seconds() -> u16 {
    DEFAULT_TRANS_TIMEOUT_SECONDS
}

fn default_trans() -> TransSettings {
    TransSettings {
        setting: TransSetting::Inherit,
        timeout_seconds: default_trans_timeout_seconds(),
        anydoc_args: Vec::new(),
        markitdown_args: Vec::new(),
    }
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            graph: default_graph(),
            trans: default_trans(),
        }
    }
}

pub fn parse_graph_setting(value: &str) -> Result<GraphSetting> {
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

pub fn parse_trans_setting(value: &str) -> Result<TransSetting> {
    match value {
        "disabled" => Ok(TransSetting::Disabled),
        "anydoc" => Ok(TransSetting::Anydoc),
        "markitdown" => Ok(TransSetting::Markitdown),
        _ => Err(AppError::new(
            "invalid_trans_engine",
            format!("unsupported trans engine '{value}'; use disabled, anydoc, or markitdown"),
        )),
    }
}

pub fn validate_trans_timeout(timeout_seconds: u16) -> Result<u16> {
    if (MIN_TRANS_TIMEOUT_SECONDS..=MAX_TRANS_TIMEOUT_SECONDS).contains(&timeout_seconds) {
        return Ok(timeout_seconds);
    }
    Err(AppError::new(
        "invalid_input",
        format!(
            "trans timeout must be between {MIN_TRANS_TIMEOUT_SECONDS} and {MAX_TRANS_TIMEOUT_SECONDS} seconds"
        ),
    ))
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
                AppError::new("invalid_config", format!("invalid config: {error}"))
            })?;
            let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
            if version == 1 {
                return Ok(ConfigFile::default());
            }
            if version == 2 {
                let legacy: ConfigFileV2 = serde_json::from_value(value).map_err(|error| {
                    AppError::new("invalid_config", format!("invalid config: {error}"))
                })?;
                return Ok(ConfigFile {
                    version: CONFIG_VERSION,
                    graph: legacy.graph,
                    trans: default_trans(),
                });
            }
            if version != u64::from(CONFIG_VERSION) {
                return Err(AppError::new(
                    "unsupported_config_version",
                    format!("unsupported config version {version}"),
                ));
            }
            serde_json::from_value(value).map_err(|error| {
                AppError::new("invalid_config", format!("invalid config: {error}"))
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn resolve_graph(scope: &str, database: &Path) -> Result<EffectiveGraphConfig> {
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

pub fn resolve(scope: &str, database: &Path) -> Result<EffectiveGraphConfig> {
    resolve_graph(scope, database)
}

pub fn resolve_trans(scope: &str, database: &Path) -> Result<EffectiveTransConfig> {
    let mut effective = EffectiveTransConfig {
        setting: TransSetting::Disabled,
        origin: "built-in".to_string(),
        timeout_seconds: default_trans_timeout_seconds(),
        anydoc_args: Vec::new(),
        markitdown_args: Vec::new(),
    };

    if let Some(path) = global_config_path() {
        let global = load_file(&path)?;
        if global.trans.setting != TransSetting::Inherit {
            effective = EffectiveTransConfig {
                setting: global.trans.setting,
                origin: "global".to_string(),
                timeout_seconds: global.trans.timeout_seconds,
                anydoc_args: global.trans.anydoc_args,
                markitdown_args: global.trans.markitdown_args,
            };
        }
    }
    if scope == "project" {
        let project = load_file(&config_path_for_database(database)?)?;
        if project.trans.setting != TransSetting::Inherit {
            effective = EffectiveTransConfig {
                setting: project.trans.setting,
                origin: "project".to_string(),
                timeout_seconds: project.trans.timeout_seconds,
                anydoc_args: project.trans.anydoc_args,
                markitdown_args: project.trans.markitdown_args,
            };
        }
    }
    Ok(effective)
}

pub fn build_trans_settings(
    database: &Path,
    setting: TransSetting,
    timeout_seconds: Option<u16>,
    args: Vec<String>,
) -> Result<TransSettings> {
    let path = config_path_for_database(database)?;
    let existing = load_file(&path)?;
    let mut trans = if existing.trans.setting == TransSetting::Inherit {
        default_trans()
    } else {
        existing.trans
    };
    trans.setting = setting;
    if let Some(timeout_seconds) = timeout_seconds {
        trans.timeout_seconds = validate_trans_timeout(timeout_seconds)?;
    }
    match setting {
        TransSetting::Disabled => {
            if !args.is_empty() {
                return Err(AppError::new(
                    "invalid_input",
                    "config set --trans disabled does not accept --trans-arg",
                ));
            }
        }
        TransSetting::Anydoc => trans.anydoc_args = args,
        TransSetting::Markitdown => trans.markitdown_args = args,
        TransSetting::Inherit => {}
    }
    Ok(trans)
}

pub fn inherit_trans_settings() -> TransSettings {
    default_trans()
}

pub fn update(database: &Path, patch: ConfigPatch) -> Result<(PathBuf, ConfigFile)> {
    let path = config_path_for_database(database)?;
    reject_symlink(&path)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("invalid_config_path", "config has no parent"))?;
    reject_symlink(parent)?;
    fs::create_dir_all(parent)?;
    let mut config = load_file(&path)?;
    config.version = CONFIG_VERSION;
    if let Some(setting) = patch.graph {
        config.graph.setting = setting;
    }
    if let Some(trans) = patch.trans {
        config.trans = trans;
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

pub fn response(scope: &str, database: &Path) -> Result<Value> {
    let graph = resolve_graph(scope, database)?;
    let trans = resolve_trans(scope, database)?;
    Ok(json!({
        "scope": scope,
        "path": config_path_for_database(database)?,
        "graph": graph,
        "trans": trans,
    }))
}
