use super::*;
use std::fs;

pub(super) static COPILOT_VSCODE: CopilotVscodeTarget = CopilotVscodeTarget;
pub(super) struct CopilotVscodeTarget;

impl CopilotVscodeTarget {
    fn user_dir(&self, environment: &TargetEnvironment<'_>) -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            environment
                .home
                .join("Library/Application Support/Code/User")
        }
        #[cfg(windows)]
        {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| environment.home.join("AppData/Roaming"))
                .join("Code/User")
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| environment.home.join(".config"))
                .join("Code/User")
        }
    }

    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        if environment.location == AgentLocation::Global {
            TargetPaths {
                mcp: Some(self.user_dir(environment).join("mcp.json")),
                instruction: None,
                hook: Some(environment.home.join(".copilot/hooks/lwc.json")),
                skill_dir: Some(environment.home.join(".copilot/skills/using-lwc")),
                aux: Vec::new(),
            }
        } else {
            TargetPaths {
                mcp: Some(environment.cwd.join(".vscode/mcp.json")),
                instruction: Some(environment.cwd.join(".github/copilot-instructions.md")),
                hook: Some(environment.cwd.join(".github/hooks/lwc.json")),
                skill_dir: Some(environment.cwd.join(".github/skills/using-lwc")),
                aux: Vec::new(),
            }
        }
    }
}

impl AgentTarget for CopilotVscodeTarget {
    fn id(&self) -> &'static str {
        "copilot-vscode"
    }
    fn display_name(&self) -> &'static str {
        "VS Code (GitHub Copilot Chat)"
    }
    fn lifecycle_mode(&self, _location: AgentLocation) -> &'static str {
        "configured_preview"
    }
    fn permissions_mode(&self, _location: AgentLocation) -> &'static str {
        "user_managed"
    }
    fn mcp_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
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
            installed: environment
                .home
                .join(".vscode/extensions")
                .read_dir()
                .is_ok_and(|entries| {
                    entries.filter_map(std::result::Result::ok).any(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with("github.copilot-chat-")
                    })
                })
                || paths.mcp.as_ref().is_some_and(|path| {
                    fs::read_to_string(path).is_ok_and(|text| text.contains("\"lwc\""))
                }),
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
            "the default VS Code user profile mcp.json"
        } else {
            "<repo>/.vscode/mcp.json"
        };
        format!(
            "# VS Code Copilot: add to {path}.\n{{\"servers\":{{\"lwc\":{{\"type\":\"stdio\",\"command\":\"lwc\",\"args\":[\"serve\",\"--mcp\"]}}}}}}\nHook events (Preview): {}\n\n{}\n",
            install::hook_events_summary(self.id(), location),
            install::guidance()
        )
    }

    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
