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

pub const CONFIG_VERSION: u32 = 7;
pub const DEFAULT_TRANS_TIMEOUT_SECONDS: u16 = 120;
pub const MIN_TRANS_TIMEOUT_SECONDS: u16 = 1;
pub const MAX_TRANS_TIMEOUT_SECONDS: u16 = 900;
pub const DEFAULT_MEMORY_MAX_AGE_DAYS: u32 = 365;
pub const DEFAULT_MEMORY_MAX_BYTES: u64 = 256 * 1024 * 1024;

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

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OfficeSetting {
    Disabled,
    Officecli,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MemorySetting {
    Disabled,
    Enabled,
    Inherit,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilitySetting {
    Disabled,
    Enabled,
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
pub struct MemorySettings {
    #[serde(default = "inherit_memory")]
    pub setting: MemorySetting,
    #[serde(default = "default_memory_max_age_days")]
    pub max_age_days: u32,
    #[serde(default = "default_memory_max_bytes")]
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub version: u32,
    #[serde(default = "default_graph")]
    pub graph: GraphSettings,
    #[serde(default = "default_trans")]
    pub trans: TransSettings,
    #[serde(default = "default_office")]
    pub office: OfficeSetting,
    #[serde(default = "default_memory")]
    pub memory: MemorySettings,
    #[serde(default = "inherit_capability")]
    pub todo: CapabilitySetting,
    #[serde(default = "inherit_capability")]
    pub plan: CapabilitySetting,
    #[serde(default = "disabled_capability")]
    pub tutor: CapabilitySetting,
    #[serde(default = "disabled_capability")]
    pub book: CapabilitySetting,
    #[serde(default = "disabled_capability")]
    pub practice: CapabilitySetting,
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EffectiveOfficeConfig {
    pub setting: OfficeSetting,
    pub origin: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EffectiveMemoryConfig {
    pub setting: MemorySetting,
    pub origin: String,
    pub max_age_days: u32,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EffectiveCapabilityConfig {
    pub setting: CapabilitySetting,
    pub origin: String,
}

#[derive(Debug, Clone, Default)]
pub struct ConfigPatch {
    pub graph: Option<GraphSetting>,
    pub trans: Option<TransSettings>,
    pub office: Option<OfficeSetting>,
    pub memory: Option<MemorySettings>,
    pub todo: Option<CapabilitySetting>,
    pub plan: Option<CapabilitySetting>,
    pub tutor: Option<CapabilitySetting>,
    pub book: Option<CapabilitySetting>,
    pub practice: Option<CapabilitySetting>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfigFileV2 {
    pub version: u32,
    #[serde(default = "default_graph")]
    pub graph: GraphSettings,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfigFileV3 {
    pub version: u32,
    #[serde(default = "default_graph")]
    pub graph: GraphSettings,
    #[serde(default = "default_trans")]
    pub trans: TransSettings,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfigFileV4 {
    pub version: u32,
    #[serde(default = "default_graph")]
    pub graph: GraphSettings,
    #[serde(default = "default_trans")]
    pub trans: TransSettings,
    #[serde(default = "default_office")]
    pub office: OfficeSetting,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfigFileV5 {
    pub version: u32,
    #[serde(default = "default_graph")]
    pub graph: GraphSettings,
    #[serde(default = "default_trans")]
    pub trans: TransSettings,
    #[serde(default = "default_office")]
    pub office: OfficeSetting,
    #[serde(default = "default_memory")]
    pub memory: MemorySettings,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfigFileV6 {
    pub version: u32,
    #[serde(default = "default_graph")]
    pub graph: GraphSettings,
    #[serde(default = "default_trans")]
    pub trans: TransSettings,
    #[serde(default = "default_office")]
    pub office: OfficeSetting,
    #[serde(default = "default_memory")]
    pub memory: MemorySettings,
    #[serde(default = "inherit_capability")]
    pub todo: CapabilitySetting,
    #[serde(default = "inherit_capability")]
    pub plan: CapabilitySetting,
}

fn inherit_graph() -> GraphSetting {
    GraphSetting::Inherit
}

fn inherit_trans() -> TransSetting {
    TransSetting::Inherit
}

fn inherit_memory() -> MemorySetting {
    MemorySetting::Inherit
}

fn inherit_capability() -> CapabilitySetting {
    CapabilitySetting::Inherit
}

fn disabled_capability() -> CapabilitySetting {
    CapabilitySetting::Disabled
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

fn default_office() -> OfficeSetting {
    OfficeSetting::Disabled
}

fn default_memory_max_age_days() -> u32 {
    DEFAULT_MEMORY_MAX_AGE_DAYS
}

fn default_memory_max_bytes() -> u64 {
    DEFAULT_MEMORY_MAX_BYTES
}

fn default_memory() -> MemorySettings {
    MemorySettings {
        setting: MemorySetting::Inherit,
        max_age_days: default_memory_max_age_days(),
        max_bytes: default_memory_max_bytes(),
    }
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            graph: default_graph(),
            trans: default_trans(),
            office: default_office(),
            memory: default_memory(),
            todo: inherit_capability(),
            plan: inherit_capability(),
            tutor: disabled_capability(),
            book: disabled_capability(),
            practice: disabled_capability(),
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

pub fn parse_office_setting(value: &str) -> Result<OfficeSetting> {
    match value {
        "disabled" => Ok(OfficeSetting::Disabled),
        "officecli" => Ok(OfficeSetting::Officecli),
        _ => Err(AppError::new(
            "invalid_office_engine",
            format!("unsupported office engine '{value}'; use disabled or officecli"),
        )),
    }
}

pub fn parse_memory_setting(value: &str) -> Result<MemorySetting> {
    match value {
        "disabled" => Ok(MemorySetting::Disabled),
        "enabled" => Ok(MemorySetting::Enabled),
        "inherit" => Ok(MemorySetting::Inherit),
        _ => Err(AppError::new(
            "invalid_memory_setting",
            format!("unsupported memory setting '{value}'; use disabled, enabled, or inherit"),
        )),
    }
}

pub fn parse_capability_setting(name: &str, value: &str) -> Result<CapabilitySetting> {
    match value {
        "disabled" => Ok(CapabilitySetting::Disabled),
        "enabled" => Ok(CapabilitySetting::Enabled),
        "inherit" => Ok(CapabilitySetting::Inherit),
        _ => Err(AppError::new(
            "invalid_capability_setting",
            format!("unsupported {name} setting '{value}'; use disabled, enabled, or inherit"),
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

fn validate_trans_settings(trans: &TransSettings) -> Result<()> {
    validate_trans_timeout(trans.timeout_seconds)
        .map(|_| ())
        .map_err(|error| {
            AppError::new(
                "invalid_config",
                format!("invalid trans timeout_seconds: {}", error.message),
            )
        })?;
    validate_trans_args_for_config(&trans.anydoc_args).map_err(|error| {
        AppError::new(
            "invalid_config",
            format!("invalid anydoc_args: {}", error.message),
        )
    })?;
    validate_trans_args_for_config(&trans.markitdown_args).map_err(|error| {
        AppError::new(
            "invalid_config",
            format!("invalid markitdown_args: {}", error.message),
        )
    })
}

fn validate_config_file(config: ConfigFile) -> Result<ConfigFile> {
    validate_trans_settings(&config.trans)?;
    if config.memory.setting == MemorySetting::Enabled
        && (config.memory.max_age_days == 0 || config.memory.max_bytes == 0)
    {
        return Err(AppError::new(
            "invalid_config",
            "enabled memory requires positive max_age_days and max_bytes",
        ));
    }
    Ok(config)
}

pub fn config_path_for_database(database: &Path) -> Result<PathBuf> {
    let mut parent = database.parent().ok_or_else(|| {
        AppError::new(
            "invalid_config_path",
            "wiki database has no configuration directory",
        )
    })?;
    if parent.file_name().and_then(|name| name.to_str()) == Some("changesets") {
        parent = parent.parent().ok_or_else(|| {
            AppError::new(
                "invalid_config_path",
                "changeset database has no configuration directory",
            )
        })?;
    }
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
                return validate_config_file(ConfigFile {
                    version: CONFIG_VERSION,
                    graph: legacy.graph,
                    trans: default_trans(),
                    office: default_office(),
                    memory: default_memory(),
                    todo: inherit_capability(),
                    plan: inherit_capability(),
                    tutor: disabled_capability(),
                    book: disabled_capability(),
                    practice: disabled_capability(),
                });
            }
            if version == 3 {
                let legacy: ConfigFileV3 = serde_json::from_value(value).map_err(|error| {
                    AppError::new("invalid_config", format!("invalid config: {error}"))
                })?;
                return validate_config_file(ConfigFile {
                    version: CONFIG_VERSION,
                    graph: legacy.graph,
                    trans: legacy.trans,
                    office: default_office(),
                    memory: default_memory(),
                    todo: inherit_capability(),
                    plan: inherit_capability(),
                    tutor: disabled_capability(),
                    book: disabled_capability(),
                    practice: disabled_capability(),
                });
            }
            if version == 4 {
                let legacy: ConfigFileV4 = serde_json::from_value(value).map_err(|error| {
                    AppError::new("invalid_config", format!("invalid config: {error}"))
                })?;
                return validate_config_file(ConfigFile {
                    version: CONFIG_VERSION,
                    graph: legacy.graph,
                    trans: legacy.trans,
                    office: legacy.office,
                    memory: default_memory(),
                    todo: inherit_capability(),
                    plan: inherit_capability(),
                    tutor: disabled_capability(),
                    book: disabled_capability(),
                    practice: disabled_capability(),
                });
            }
            if version == 5 {
                let legacy: ConfigFileV5 = serde_json::from_value(value).map_err(|error| {
                    AppError::new("invalid_config", format!("invalid config: {error}"))
                })?;
                return validate_config_file(ConfigFile {
                    version: CONFIG_VERSION,
                    graph: legacy.graph,
                    trans: legacy.trans,
                    office: legacy.office,
                    memory: legacy.memory,
                    todo: inherit_capability(),
                    plan: inherit_capability(),
                    tutor: disabled_capability(),
                    book: disabled_capability(),
                    practice: disabled_capability(),
                });
            }
            if version == 6 {
                let legacy: ConfigFileV6 = serde_json::from_value(value).map_err(|error| {
                    AppError::new("invalid_config", format!("invalid config: {error}"))
                })?;
                return validate_config_file(ConfigFile {
                    version: CONFIG_VERSION,
                    graph: legacy.graph,
                    trans: legacy.trans,
                    office: legacy.office,
                    memory: legacy.memory,
                    todo: legacy.todo,
                    plan: legacy.plan,
                    tutor: disabled_capability(),
                    book: disabled_capability(),
                    practice: disabled_capability(),
                });
            }
            if version != u64::from(CONFIG_VERSION) {
                return Err(AppError::new(
                    "unsupported_config_version",
                    format!("unsupported config version {version}"),
                ));
            }
            let config = serde_json::from_value(value).map_err(|error| {
                AppError::new("invalid_config", format!("invalid config: {error}"))
            })?;
            validate_config_file(config)
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

pub fn resolve_office() -> Result<EffectiveOfficeConfig> {
    let setting = match global_config_path() {
        Some(path) => load_file(&path)?.office,
        None => OfficeSetting::Disabled,
    };
    Ok(EffectiveOfficeConfig {
        setting,
        origin: if setting == OfficeSetting::Disabled {
            "built-in".to_owned()
        } else {
            "global".to_owned()
        },
    })
}

pub fn resolve_memory(scope: &str, database: &Path) -> Result<EffectiveMemoryConfig> {
    let mut effective = EffectiveMemoryConfig {
        setting: MemorySetting::Enabled,
        origin: "built-in".to_owned(),
        max_age_days: default_memory_max_age_days(),
        max_bytes: default_memory_max_bytes(),
    };

    if let Some(path) = global_config_path() {
        let global = load_file(&path)?;
        if global.memory.setting != MemorySetting::Inherit {
            effective = EffectiveMemoryConfig {
                setting: global.memory.setting,
                origin: "global".to_owned(),
                max_age_days: global.memory.max_age_days,
                max_bytes: global.memory.max_bytes,
            };
        }
    }
    if scope == "project" {
        let project = load_file(&config_path_for_database(database)?)?;
        if project.memory.setting != MemorySetting::Inherit {
            effective = EffectiveMemoryConfig {
                setting: project.memory.setting,
                origin: "project".to_owned(),
                max_age_days: project.memory.max_age_days,
                max_bytes: project.memory.max_bytes,
            };
        }
    }
    Ok(effective)
}

fn resolve_capability(
    scope: &str,
    database: &Path,
    select: fn(&ConfigFile) -> CapabilitySetting,
) -> Result<EffectiveCapabilityConfig> {
    let mut effective = EffectiveCapabilityConfig {
        setting: CapabilitySetting::Disabled,
        origin: "built-in".to_owned(),
    };
    if let Some(path) = global_config_path() {
        let setting = select(&load_file(&path)?);
        if setting != CapabilitySetting::Inherit {
            effective = EffectiveCapabilityConfig {
                setting,
                origin: "global".to_owned(),
            };
        }
    }
    if scope == "project" {
        let setting = select(&load_file(&config_path_for_database(database)?)?);
        if setting != CapabilitySetting::Inherit {
            effective = EffectiveCapabilityConfig {
                setting,
                origin: "project".to_owned(),
            };
        }
    }
    Ok(effective)
}
pub fn resolve_todo(scope: &str, database: &Path) -> Result<EffectiveCapabilityConfig> {
    resolve_capability(scope, database, |config| config.todo)
}
pub fn resolve_plan(scope: &str, database: &Path) -> Result<EffectiveCapabilityConfig> {
    resolve_capability(scope, database, |config| config.plan)
}

pub fn resolve_learning(name: &str) -> Result<EffectiveCapabilityConfig> {
    let setting = match global_config_path() {
        Some(path) => {
            let config = load_file(&path)?;
            match name {
                "tutor" => config.tutor,
                "book" => config.book,
                "practice" => config.practice,
                _ => CapabilitySetting::Disabled,
            }
        }
        None => CapabilitySetting::Disabled,
    };
    Ok(EffectiveCapabilityConfig {
        setting,
        origin: if setting == CapabilitySetting::Enabled {
            "global".to_owned()
        } else {
            "built-in".to_owned()
        },
    })
}
pub fn inherit_capability_setting() -> CapabilitySetting {
    CapabilitySetting::Inherit
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
        TransSetting::Anydoc => {
            validate_trans_args_for_write(&args)?;
            trans.anydoc_args = args;
        }
        TransSetting::Markitdown => {
            validate_trans_args_for_write(&args)?;
            trans.markitdown_args = args;
        }
        TransSetting::Inherit => {}
    }
    Ok(trans)
}

pub fn inherit_trans_settings() -> TransSettings {
    default_trans()
}

pub fn build_memory_settings(
    database: &Path,
    setting: MemorySetting,
    max_age_days: Option<u32>,
    max_bytes: Option<u64>,
) -> Result<MemorySettings> {
    let existing = load_file(&config_path_for_database(database)?)?;
    let mut memory = if existing.memory.setting == MemorySetting::Inherit {
        default_memory()
    } else {
        existing.memory
    };
    memory.setting = setting;
    if let Some(max_age_days) = max_age_days {
        if max_age_days == 0 {
            return Err(AppError::new(
                "invalid_input",
                "memory max age days must be positive",
            ));
        }
        memory.max_age_days = max_age_days;
    }
    if let Some(max_bytes) = max_bytes {
        if max_bytes == 0 {
            return Err(AppError::new(
                "invalid_input",
                "memory max bytes must be positive",
            ));
        }
        memory.max_bytes = max_bytes;
    }
    Ok(memory)
}

pub fn inherit_memory_settings() -> MemorySettings {
    default_memory()
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
    if let Some(office) = patch.office {
        config.office = office;
    }
    if let Some(memory) = patch.memory {
        config.memory = memory;
    }
    if let Some(todo) = patch.todo {
        config.todo = todo;
    }
    if let Some(plan) = patch.plan {
        config.plan = plan;
    }
    if let Some(tutor) = patch.tutor {
        config.tutor = tutor;
    }
    if let Some(book) = patch.book {
        config.book = book;
    }
    if let Some(practice) = patch.practice {
        config.practice = practice;
    }
    validate_config_file(config.clone())?;
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

fn validate_trans_args_for_write(args: &[String]) -> Result<()> {
    if let Some(message) = detect_secret_arg_problem(args) {
        return Err(AppError::new("possible_secret_detected", message));
    }
    Ok(())
}

fn validate_trans_args_for_config(args: &[String]) -> Result<()> {
    if let Some(message) = detect_secret_arg_problem(args) {
        return Err(AppError::new("possible_secret_detected", message));
    }
    Ok(())
}

fn detect_secret_arg_problem(args: &[String]) -> Option<String> {
    for (index, arg) in args.iter().enumerate() {
        if let Some((flag, _value)) = arg.split_once('=')
            && is_explicit_credential_flag(flag)
        {
            return Some(format!(
                "trans args contain explicit credential flag {flag} with an inline value"
            ));
        }
        if is_explicit_credential_flag(arg) {
            return Some(format!("trans args contain explicit credential flag {arg}"));
        }
        let reasons =
            crate::secret_scan::detect_possible_secret_reasons(Path::new("trans-arg.txt"), arg);
        if !reasons.is_empty() {
            return Some(format!(
                "trans arg at position {} looks sensitive: {}",
                index + 1,
                reasons.join(", ")
            ));
        }
    }
    None
}

fn is_explicit_credential_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--api-key"
            | "--apikey"
            | "--token"
            | "--access-token"
            | "--secret"
            | "--client-secret"
            | "--password"
            | "--subscription-key"
    )
}

pub fn response(scope: &str, database: &Path) -> Result<Value> {
    let graph = resolve_graph(scope, database)?;
    let trans = resolve_trans(scope, database)?;
    let office = resolve_office()?;
    let memory = resolve_memory(scope, database)?;
    let todo = resolve_todo(scope, database)?;
    let plan = resolve_plan(scope, database)?;
    let tutor = resolve_learning("tutor")?;
    let book = resolve_learning("book")?;
    let practice = resolve_learning("practice")?;
    Ok(json!({
        "scope": scope,
        "path": config_path_for_database(database)?,
        "graph": graph,
        "trans": trans,
        "office": office,
        "memory": memory,
        "todo":todo,
        "plan":plan,
        "tutor":tutor,
        "book":book,
        "practice":practice,
    }))
}
