//! Agent installer adapters, following CodeGraph's MIT-licensed target/registry design.

use super::install::{self, AgentLocation, TargetPaths};
use crate::error::Result;
use std::path::{Path, PathBuf};

mod antigravity;
mod claude;
mod codex;
mod copilot_cli;
mod copilot_jetbrains;
mod copilot_vscode;
mod cursor;
mod gemini;
mod hermes;
mod kiro;
mod opencode;
mod pi;

#[derive(Debug)]
pub(super) struct DetectionResult {
    pub installed: bool,
    pub already_configured: bool,
    pub config_path: Option<PathBuf>,
}

pub(super) struct TargetEnvironment<'a> {
    pub location: AgentLocation,
    pub home: &'a Path,
    pub cwd: &'a Path,
}

#[derive(Clone, Copy)]
pub(super) struct InstallOptions {
    pub prompt_hook: bool,
}

pub(super) struct WriteResult {
    pub status: &'static str,
    pub files: Vec<PathBuf>,
    pub notes: Vec<String>,
}

impl WriteResult {
    pub fn not_installed() -> Self {
        Self {
            status: "not_installed",
            files: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn unsupported(note: impl Into<String>) -> Self {
        Self {
            status: "unsupported",
            files: Vec::new(),
            notes: vec![note.into()],
        }
    }
}

pub(super) trait AgentTarget: Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn adaptation(&self) -> &'static str {
        "strong"
    }
    fn mcp_mode(&self, location: AgentLocation) -> &'static str;
    fn permissions_mode(&self, location: AgentLocation) -> &'static str;
    fn instructions_mode(&self, location: AgentLocation) -> &'static str;
    fn skills_mode(&self, location: AgentLocation) -> &'static str;
    fn lifecycle_mode(&self, location: AgentLocation) -> &'static str;
    fn lifecycle_hook(&self, location: AgentLocation) -> bool {
        matches!(
            self.lifecycle_mode(location),
            "installed" | "configured_preview"
        )
    }
    fn supports_location(&self, location: AgentLocation) -> bool;
    fn detect(&self, environment: &TargetEnvironment<'_>) -> DetectionResult;
    fn install(
        &self,
        environment: &TargetEnvironment<'_>,
        options: InstallOptions,
    ) -> Result<WriteResult>;
    fn uninstall(&self, environment: &TargetEnvironment<'_>) -> Result<WriteResult>;
    fn configure(&self, paths: &TargetPaths, options: InstallOptions) -> Result<()>;
    fn unconfigure(&self, paths: &TargetPaths, executable: &str) -> Result<()>;
    fn print_config(&self, location: AgentLocation) -> String;
    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf>;
}

pub(super) static ALL_TARGETS: [&dyn AgentTarget; 12] = [
    &claude::CLAUDE,
    &cursor::CURSOR,
    &codex::CODEX,
    &opencode::OPENCODE,
    &hermes::HERMES,
    &gemini::GEMINI,
    &antigravity::ANTIGRAVITY,
    &kiro::KIRO,
    &copilot_vscode::COPILOT_VSCODE,
    &copilot_cli::COPILOT_CLI,
    &copilot_jetbrains::COPILOT_JETBRAINS,
    &pi::PI,
];

pub(super) fn get_target(id: &str) -> Option<&'static dyn AgentTarget> {
    ALL_TARGETS.iter().copied().find(|target| target.id() == id)
}

pub(super) fn target_ids() -> Vec<&'static str> {
    ALL_TARGETS.iter().map(|target| target.id()).collect()
}

pub(super) fn native_install(
    target: &dyn AgentTarget,
    environment: &TargetEnvironment<'_>,
    options: InstallOptions,
    paths: TargetPaths,
) -> Result<WriteResult> {
    install::install_native(target, environment, options, paths)
}

pub(super) fn native_uninstall(
    target: &dyn AgentTarget,
    environment: &TargetEnvironment<'_>,
    paths: TargetPaths,
) -> Result<WriteResult> {
    install::uninstall_native(target, environment, paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_are_unique_and_stable() {
        let ids = target_ids();
        assert_eq!(
            ids,
            [
                "claude",
                "cursor",
                "codex",
                "opencode",
                "hermes",
                "gemini",
                "antigravity",
                "kiro",
                "copilot-vscode",
                "copilot-cli",
                "copilot-jetbrains",
                "pi"
            ]
        );
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
    }
}
