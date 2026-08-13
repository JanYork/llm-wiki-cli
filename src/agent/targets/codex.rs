use super::*;
use std::fs;

pub(super) static CODEX: CodexTarget = CodexTarget;

pub(super) struct CodexTarget;

impl CodexTarget {
    fn root(&self, environment: &TargetEnvironment<'_>) -> PathBuf {
        std::env::var_os("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| environment.home.join(".codex"))
    }

    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        let global = environment.location == AgentLocation::Global;
        let root = self.root(environment);
        TargetPaths {
            mcp: Some(if global {
                root.join("config.toml")
            } else {
                environment.cwd.join(".codex/config.toml")
            }),
            instruction: Some(if global {
                root.join("AGENTS.md")
            } else {
                environment.cwd.join("AGENTS.md")
            }),
            hook: Some(if global {
                root.join("hooks.json")
            } else {
                environment.cwd.join(".codex/hooks.json")
            }),
            skill_dir: Some(if global {
                environment.home.join(".agents/skills/using-lwc")
            } else {
                environment.cwd.join(".agents/skills/using-lwc")
            }),
            aux: Vec::new(),
        }
    }
}

impl AgentTarget for CodexTarget {
    fn adaptation(&self) -> &'static str {
        "strong"
    }
    fn id(&self) -> &'static str {
        "codex"
    }
    fn display_name(&self) -> &'static str {
        "Codex"
    }
    fn mcp_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn lifecycle_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
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
        let already_configured = paths.mcp.as_ref().is_some_and(|path| {
            fs::read_to_string(path).is_ok_and(|text| text.contains("[mcp_servers.lwc]"))
        });
        DetectionResult {
            installed: install::command_exists("codex")
                || self.root(environment).exists()
                || (environment.location == AgentLocation::Local
                    && environment.cwd.join(".codex").exists()),
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

    fn print_config(&self, location: AgentLocation) -> String {
        let (config, hooks) = if location == AgentLocation::Global {
            (
                "${CODEX_HOME:-~/.codex}/config.toml",
                "${CODEX_HOME:-~/.codex}/hooks.json",
            )
        } else {
            ("<repo>/.codex/config.toml", "<repo>/.codex/hooks.json")
        };
        format!(
            "# Codex: add MCP to {config} and hooks to {hooks}.\n\
[mcp_servers.lwc]\ncommand = \"lwc\"\nargs = [\"serve\", \"--mcp\"]\n\n\
{}\n\n\
{{\"hooks\":{{\"SessionStart\":[{{\"hooks\":[{{\"type\":\"command\",\"command\":\"lwc --scope all agent hook --agent codex --event SessionStart\"}}]}}]}}}}\n",
            install::guidance()
        )
    }

    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
