use super::*;
use std::fs;

pub(super) static COPILOT_CLI: CopilotCliTarget = CopilotCliTarget;
pub(super) struct CopilotCliTarget;

impl CopilotCliTarget {
    fn root(&self, environment: &TargetEnvironment<'_>) -> PathBuf {
        std::env::var_os("COPILOT_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| environment.home.join(".copilot"))
    }

    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        if environment.location == AgentLocation::Global {
            let root = self.root(environment);
            TargetPaths {
                mcp: Some(root.join("mcp-config.json")),
                instruction: Some(root.join("copilot-instructions.md")),
                hook: Some(root.join("hooks/lwc.json")),
                skill_dir: Some(root.join("skills/using-lwc")),
                aux: Vec::new(),
            }
        } else {
            TargetPaths {
                mcp: Some(environment.cwd.join(".github/mcp.json")),
                instruction: Some(environment.cwd.join(".github/copilot-instructions.md")),
                hook: Some(environment.cwd.join(".github/hooks/lwc.json")),
                skill_dir: Some(environment.cwd.join(".github/skills/using-lwc")),
                aux: Vec::new(),
            }
        }
    }
}

impl AgentTarget for CopilotCliTarget {
    fn id(&self) -> &'static str {
        "copilot-cli"
    }
    fn display_name(&self) -> &'static str {
        "GitHub Copilot CLI"
    }
    fn mcp_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn lifecycle_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn permissions_mode(&self, _location: AgentLocation) -> &'static str {
        "user_managed"
    }
    fn supports_location(&self, _location: AgentLocation) -> bool {
        true
    }
    fn instructions_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn skills_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }

    fn detect(&self, environment: &TargetEnvironment<'_>) -> DetectionResult {
        let paths = self.paths(environment);
        DetectionResult {
            installed: install::command_exists("copilot")
                || (environment.location == AgentLocation::Global
                    && self.root(environment).join("mcp-config.json").exists()),
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
        if location == AgentLocation::Local {
            return format!(
                "# GitHub Copilot CLI workspace MCP: <repo>/.github/mcp.json.\n{{\"mcpServers\":{{\"lwc\":{{\"type\":\"stdio\",\"command\":\"lwc\",\"args\":[\"serve\",\"--mcp\"],\"tools\":[\"*\"]}}}}}}\n\n{}\n",
                install::guidance()
            );
        }
        let path = "${COPILOT_HOME:-~/.copilot}/mcp-config.json";
        format!(
            "# GitHub Copilot CLI: add to {path}.\n{{\"mcpServers\":{{\"lwc\":{{\"type\":\"stdio\",\"command\":\"lwc\",\"args\":[\"serve\",\"--mcp\"],\"tools\":[\"*\"]}}}}}}\n\n{}\n",
            install::guidance()
        )
    }

    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
