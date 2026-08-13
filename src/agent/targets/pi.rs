use super::*;

pub(super) static PI: PiTarget = PiTarget;

pub(super) struct PiTarget;

impl PiTarget {
    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        let base = if environment.location == AgentLocation::Global {
            environment.home.join(".pi/agent/extensions")
        } else {
            environment.cwd.join(".pi/extensions")
        };
        TargetPaths {
            mcp: None,
            instruction: None,
            hook: Some(base.join("lwc.js")),
            skill_dir: Some(if environment.location == AgentLocation::Global {
                environment.home.join(".pi/agent/skills/using-lwc")
            } else {
                environment.cwd.join(".pi/skills/using-lwc")
            }),
            aux: Vec::new(),
        }
    }
}

impl AgentTarget for PiTarget {
    fn adaptation(&self) -> &'static str {
        "strong"
    }
    fn id(&self) -> &'static str {
        "pi"
    }
    fn display_name(&self) -> &'static str {
        "Pi Agent"
    }
    fn mcp_mode(&self, _location: AgentLocation) -> &'static str {
        "extension_bridge"
    }
    fn lifecycle_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn instructions_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn skills_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn permissions_mode(&self, _location: AgentLocation) -> &'static str {
        "not_applicable"
    }
    fn supports_location(&self, _location: AgentLocation) -> bool {
        true
    }

    fn detect(&self, environment: &TargetEnvironment<'_>) -> DetectionResult {
        let paths = self.paths(environment);
        DetectionResult {
            installed: install::command_exists("pi")
                || environment.home.join(".pi").exists()
                || (environment.location == AgentLocation::Local
                    && environment.cwd.join(".pi").exists()),
            already_configured: paths.hook.as_ref().is_some_and(|path| path.is_file()),
            config_path: paths.hook,
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
            "{}\nlwc --scope all agent hook --agent pi --event session_start\n# Pi uses its native extension bridge.\n",
            install::guidance()
        )
    }

    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
