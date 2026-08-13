use super::*;

pub(super) static COPILOT_JETBRAINS: CopilotJetbrainsTarget = CopilotJetbrainsTarget;
pub(super) struct CopilotJetbrainsTarget;

impl CopilotJetbrainsTarget {
    fn copilot_root(&self, environment: &TargetEnvironment<'_>) -> PathBuf {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
            return PathBuf::from(xdg).join("github-copilot");
        }
        #[cfg(windows)]
        {
            return std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| environment.home.join("AppData/Local"))
                .join("github-copilot");
        }
        #[cfg(not(windows))]
        environment.home.join(".config/github-copilot")
    }

    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        if environment.location == AgentLocation::Local {
            return TargetPaths {
                mcp: None,
                instruction: Some(environment.cwd.join(".github/copilot-instructions.md")),
                hook: None,
                skill_dir: Some(environment.cwd.join(".github/skills/using-lwc")),
                aux: Vec::new(),
            };
        }
        TargetPaths {
            mcp: Some(self.copilot_root(environment).join("intellij/mcp.json")),
            instruction: None,
            hook: None,
            skill_dir: Some(environment.home.join(".copilot/skills/using-lwc")),
            aux: Vec::new(),
        }
    }
}

impl AgentTarget for CopilotJetbrainsTarget {
    fn id(&self) -> &'static str {
        "copilot-jetbrains"
    }
    fn display_name(&self) -> &'static str {
        "JetBrains IDEs (GitHub Copilot plugin)"
    }
    fn lifecycle_mode(&self, _location: AgentLocation) -> &'static str {
        "unsupported"
    }
    fn permissions_mode(&self, _location: AgentLocation) -> &'static str {
        "user_managed"
    }
    fn instructions_mode(&self, _location: AgentLocation) -> &'static str {
        if _location == AgentLocation::Global {
            "user_managed"
        } else {
            "configured_preview"
        }
    }
    fn skills_mode(&self, _location: AgentLocation) -> &'static str {
        "configured_preview"
    }
    fn supports_location(&self, _location: AgentLocation) -> bool {
        true
    }
    fn mcp_mode(&self, location: AgentLocation) -> &'static str {
        if location == AgentLocation::Global {
            "installed"
        } else {
            "user_managed"
        }
    }

    fn detect(&self, environment: &TargetEnvironment<'_>) -> DetectionResult {
        let paths = self.paths(environment);
        DetectionResult {
            installed: self.copilot_root(environment).join("intellij").exists(),
            already_configured: paths.mcp.as_ref().is_some_and(|path| {
                std::fs::read_to_string(path).is_ok_and(|text| text.contains("\"lwc\""))
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
        result.notes.push("Hooks are unsupported; Skills and project Instructions are preview features whose host activation must be verified.".into());
        if environment.location == AgentLocation::Global {
            result
                .notes
                .push("Restart the JetBrains IDE after an MCP update.".into());
        }
        Ok(result)
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
        if location == AgentLocation::Global {
            format!(
                "# GitHub Copilot for JetBrains: add to $XDG_CONFIG_HOME|~/.config/github-copilot/intellij/mcp.json.\n{{\"servers\":{{\"lwc\":{{\"type\":\"stdio\",\"command\":\"lwc\",\"args\":[\"serve\",\"--mcp\"]}}}}}}\n\n{}\n",
                install::guidance()
            )
        } else {
            format!(
                "# JetBrains project preview customizations: .github/copilot-instructions.md and .github/skills/using-lwc. MCP remains user-configured; Hooks are unsupported.\n{}\n",
                install::guidance()
            )
        }
    }

    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
