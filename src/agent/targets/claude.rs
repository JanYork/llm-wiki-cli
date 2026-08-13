use super::*;
use std::fs;

pub(super) static CLAUDE: ClaudeTarget = ClaudeTarget;

pub(super) struct ClaudeTarget;

impl ClaudeTarget {
    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        let global = environment.location == AgentLocation::Global;
        TargetPaths {
            mcp: Some(if global {
                environment.home.join(".claude.json")
            } else {
                environment.cwd.join(".mcp.json")
            }),
            instruction: Some(if global {
                environment.home.join(".claude/CLAUDE.md")
            } else {
                environment.cwd.join(".claude/CLAUDE.md")
            }),
            hook: Some(if global {
                environment.home.join(".claude/settings.json")
            } else {
                environment.cwd.join(".claude/settings.json")
            }),
            skill_dir: Some(if global {
                environment.home.join(".claude/skills/using-lwc")
            } else {
                environment.cwd.join(".claude/skills/using-lwc")
            }),
            aux: Vec::new(),
        }
    }
}

impl AgentTarget for ClaudeTarget {
    fn adaptation(&self) -> &'static str {
        "strong"
    }
    fn id(&self) -> &'static str {
        "claude"
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    fn mcp_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn lifecycle_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn instructions_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn permissions_mode(&self, _location: AgentLocation) -> &'static str {
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
        let already_configured = paths.mcp.as_ref().is_some_and(|path| {
            fs::read_to_string(path).is_ok_and(|text| text.contains("\"lwc\""))
        });
        DetectionResult {
            installed: install::command_exists("claude")
                || environment.home.join(".claude").exists()
                || environment.home.join(".claude.json").exists()
                || (environment.location == AgentLocation::Local
                    && environment.cwd.join(".claude").exists()),
            already_configured,
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

    fn print_config(&self, _location: AgentLocation) -> String {
        format!(
            "# Claude Code: merge these entries into the selected official scope.\n\
{{\"mcpServers\":{{\"lwc\":{{\"type\":\"stdio\",\"command\":\"lwc\",\"args\":[\"serve\",\"--mcp\"]}}}},\
\"hooks\":{{\"SessionStart\":[{{\"hooks\":[{{\"type\":\"command\",\"command\":\"lwc --scope all agent hook --agent claude --event SessionStart\"}}]}}]}}}}\n\n{}\n",
            install::guidance()
        )
    }

    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
