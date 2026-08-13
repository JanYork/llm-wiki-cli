use super::*;
use std::fs;

pub(super) static KIRO: KiroTarget = KiroTarget;
pub(super) struct KiroTarget;

impl KiroTarget {
    fn root(&self, environment: &TargetEnvironment<'_>) -> PathBuf {
        if environment.location == AgentLocation::Global {
            std::env::var_os("KIRO_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| environment.home.join(".kiro"))
        } else {
            environment.cwd.join(".kiro")
        }
    }

    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        let root = self.root(environment);
        TargetPaths {
            mcp: Some(root.join("settings/mcp.json")),
            instruction: Some(root.join("steering/lwc.md")),
            hook: Some(root.join("hooks/lwc.json")),
            skill_dir: Some(root.join("skills/using-lwc")),
            aux: Vec::new(),
        }
    }
}

impl AgentTarget for KiroTarget {
    fn id(&self) -> &'static str {
        "kiro"
    }
    fn display_name(&self) -> &'static str {
        "Kiro"
    }
    fn mcp_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn lifecycle_mode(&self, _location: AgentLocation) -> &'static str {
        "configured_preview"
    }
    fn instructions_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn skills_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn permissions_mode(&self, _location: AgentLocation) -> &'static str {
        "user_managed"
    }
    fn supports_location(&self, _location: AgentLocation) -> bool {
        true
    }
    fn detect(&self, environment: &TargetEnvironment<'_>) -> DetectionResult {
        let paths = self.paths(environment);
        DetectionResult {
            installed: install::command_exists("kiro")
                || install::command_exists("kiro-cli")
                || self.root(environment).exists(),
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
        install::configure_standard(self.id(), paths, options)
    }
    fn unconfigure(&self, paths: &TargetPaths, executable: &str) -> Result<()> {
        install::unconfigure_standard(self.id(), paths, executable)
    }
    fn print_config(&self, location: AgentLocation) -> String {
        let path = if location == AgentLocation::Global {
            "${KIRO_HOME:-~/.kiro}/settings/mcp.json"
        } else {
            "<repo>/.kiro/settings/mcp.json"
        };
        format!(
            "# Kiro: add to {path}.\n\
{{\"mcpServers\":{{\"lwc\":{{\"type\":\"stdio\",\"command\":\"lwc\",\"args\":[\"serve\",\"--mcp\"]}}}}}}\n\
Skill: <kiro-root>/skills/using-lwc\n\
Hook: <kiro-root>/hooks/lwc.json SessionStart\n\
Instructions: <kiro-root>/steering/lwc.md\n"
        )
    }
    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
