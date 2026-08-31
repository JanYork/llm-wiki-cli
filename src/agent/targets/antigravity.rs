use super::*;
use std::fs;

pub(super) static ANTIGRAVITY: AntigravityTarget = AntigravityTarget;
pub(super) struct AntigravityTarget;

impl AntigravityTarget {
    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        let global = environment.location == AgentLocation::Global;
        let root = if global {
            environment.home.join(".gemini/config/plugins/lwc")
        } else {
            environment.cwd.join(".agents/plugins/lwc")
        };
        TargetPaths {
            mcp: Some(root.join("mcp_config.json")),
            instruction: Some(root.join("rules/lwc.md")),
            hook: Some(root.join("hooks.json")),
            skill_dir: Some(root.join("skills/using-lwc")),
            aux: vec![root.join("plugin.json")],
        }
    }
}

impl AgentTarget for AntigravityTarget {
    fn id(&self) -> &'static str {
        "antigravity"
    }
    fn display_name(&self) -> &'static str {
        "Antigravity"
    }
    fn mcp_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn lifecycle_mode(&self, _location: AgentLocation) -> &'static str {
        "unsupported"
    }
    fn permissions_mode(&self, _location: AgentLocation) -> &'static str {
        "not_applicable"
    }
    fn instructions_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn skills_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn supports_location(&self, _location: AgentLocation) -> bool {
        true
    }
    fn detect(&self, environment: &TargetEnvironment<'_>) -> DetectionResult {
        let paths = self.paths(environment);
        DetectionResult {
            installed: install::command_exists("agy")
                || (environment.location == AgentLocation::Global
                    && environment.home.join(".gemini/antigravity").exists()),
            already_configured: paths.mcp.as_ref().is_some_and(|path| {
                fs::read_to_string(path).is_ok_and(|text| text.contains("\"lwc\""))
            }),
            config_path: paths.mcp,
        }
    }
    fn install(
        &self,
        environment: &TargetEnvironment<'_>,
        options: InstallOptions,
    ) -> Result<WriteResult> {
        native_install(self, environment, options, self.paths(environment))
    }
    fn uninstall(&self, environment: &TargetEnvironment<'_>) -> Result<WriteResult> {
        native_uninstall(self, environment, self.paths(environment))
    }
    fn configure(&self, paths: &TargetPaths, options: InstallOptions) -> Result<()> {
        install::configure_standard(self.id(), paths, options)?;
        install::install_antigravity_plugin(paths)
    }
    fn unconfigure(&self, paths: &TargetPaths, executable: &str) -> Result<()> {
        install::unconfigure_standard(self.id(), paths, executable)
    }
    fn print_config(&self, location: AgentLocation) -> String {
        let path = if location == AgentLocation::Global {
            "~/.gemini/config/plugins/lwc/mcp_config.json"
        } else {
            "<repo>/.agents/plugins/lwc/mcp_config.json"
        };
        format!(
            "# Antigravity MCP: {path}\n{}",
            install::standard_config(self.id(), location)
        )
    }
    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
