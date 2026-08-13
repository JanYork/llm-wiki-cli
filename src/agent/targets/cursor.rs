use super::*;
use std::fs;

pub(super) static CURSOR: CursorTarget = CursorTarget;
pub(super) struct CursorTarget;

impl CursorTarget {
    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        let global = environment.location == AgentLocation::Global;
        TargetPaths {
            mcp: Some(if global {
                environment.home.join(".cursor/mcp.json")
            } else {
                environment.cwd.join(".cursor/mcp.json")
            }),
            instruction: (!global).then(|| environment.cwd.join(".cursor/rules/lwc.mdc")),
            hook: Some(if global {
                environment.home.join(".cursor/hooks.json")
            } else {
                environment.cwd.join(".cursor/hooks.json")
            }),
            skill_dir: Some(if global {
                environment.home.join(".cursor/skills/using-lwc")
            } else {
                environment.cwd.join(".cursor/skills/using-lwc")
            }),
            aux: Vec::new(),
        }
    }
}

impl AgentTarget for CursorTarget {
    fn id(&self) -> &'static str {
        "cursor"
    }
    fn display_name(&self) -> &'static str {
        "Cursor"
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
    fn instructions_mode(&self, location: AgentLocation) -> &'static str {
        if location == AgentLocation::Global {
            "user_managed"
        } else {
            "installed"
        }
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
            installed: install::command_exists("cursor")
                || environment.home.join(".cursor").exists()
                || environment.cwd.join(".cursor").exists(),
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
        let mut result = native_install(self, environment, options, self.paths(environment))?;
        if environment.location == AgentLocation::Global {
            result.notes.push("Cursor User Rules remain managed in Settings; LWC MCP, Skill and Hooks are installed as official files.".into());
        }
        Ok(result)
    }
    fn uninstall(&self, environment: &TargetEnvironment<'_>) -> Result<WriteResult> {
        native_uninstall(self, environment, self.paths(environment))
    }
    fn configure(&self, paths: &TargetPaths, options: InstallOptions) -> Result<()> {
        install::ensure_cursor_frontmatter(paths)?;
        install::configure_standard(self.id(), paths, options)
    }
    fn unconfigure(&self, paths: &TargetPaths, executable: &str) -> Result<()> {
        install::unconfigure_standard(self.id(), paths, executable)
    }
    fn print_config(&self, location: AgentLocation) -> String {
        let workspace = if location == AgentLocation::Global {
            "${workspaceFolder}".into()
        } else {
            std::env::current_dir()
                .and_then(|path| path.canonicalize())
                .unwrap_or_else(|_| PathBuf::from("."))
                .to_string_lossy()
                .into_owned()
        };
        let mcp = serde_json::json!({
            "mcpServers": {
                "lwc": {
                    "type": "stdio",
                    "command": "lwc",
                    "args": ["serve", "--mcp", "--path", workspace]
                }
            }
        });
        if location == AgentLocation::Local {
            return format!(
                "# cursor local AgentTarget integration (print-only)\n\
MCP: {mcp}\n\
Hook: lwc --scope all agent hook --agent cursor --event session_start\n\
Skill: using-lwc\n\
Instructions:\n{}\n",
                install::guidance()
            );
        }
        format!(
            "# Cursor user integration. Cursor has no official global rules file; guidance is injected by MCP initialize instructions.\n\
MCP: {mcp}\n\
Skill: ~/.cursor/skills/using-lwc\n\
Hook: ~/.cursor/hooks.json\n\
Instructions: user_managed (Cursor Settings > Rules)\n{}\n",
            install::guidance()
        )
    }
    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
