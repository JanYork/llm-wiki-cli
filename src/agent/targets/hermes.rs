use super::*;
use std::fs;

pub(super) static HERMES: HermesTarget = HermesTarget;
pub(super) struct HermesTarget;

impl HermesTarget {
    fn root(&self, environment: &TargetEnvironment<'_>) -> PathBuf {
        std::env::var_os("HERMES_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| environment.home.join(".hermes"))
    }
    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        if environment.location == AgentLocation::Local {
            return TargetPaths {
                mcp: None,
                instruction: Some(environment.cwd.join("AGENTS.md")),
                hook: None,
                skill_dir: None,
                aux: Vec::new(),
            };
        }
        let root = self.root(environment);
        TargetPaths {
            mcp: Some(root.join("config.yaml")),
            instruction: Some(root.join("SOUL.md")),
            hook: Some(root.join("config.yaml")),
            skill_dir: Some(root.join("skills/using-lwc")),
            aux: Vec::new(),
        }
    }
}

impl AgentTarget for HermesTarget {
    fn id(&self) -> &'static str {
        "hermes"
    }
    fn display_name(&self) -> &'static str {
        "Hermes Agent"
    }
    fn lifecycle_mode(&self, location: AgentLocation) -> &'static str {
        if location == AgentLocation::Global {
            "installed"
        } else {
            "unsupported"
        }
    }
    fn hook_capabilities(&self, location: AgentLocation) -> &'static [HookCapability] {
        if location == AgentLocation::Global {
            hook_capabilities(self.id())
        } else {
            &[]
        }
    }
    fn permissions_mode(&self, _location: AgentLocation) -> &'static str {
        "not_applicable"
    }
    fn instructions_mode(&self, _location: AgentLocation) -> &'static str {
        "installed"
    }
    fn skills_mode(&self, location: AgentLocation) -> &'static str {
        if location == AgentLocation::Global {
            "installed"
        } else {
            "unsupported"
        }
    }
    fn mcp_mode(&self, location: AgentLocation) -> &'static str {
        if location == AgentLocation::Global {
            "installed"
        } else {
            "unsupported"
        }
    }
    fn supports_location(&self, _location: AgentLocation) -> bool {
        true
    }
    fn detect(&self, environment: &TargetEnvironment<'_>) -> DetectionResult {
        let paths = self.paths(environment);
        DetectionResult {
            installed: install::command_exists("hermes")
                || (environment.location == AgentLocation::Global
                    && self.root(environment).exists()),
            already_configured: paths.mcp.as_ref().is_some_and(|path| {
                fs::read_to_string(path).is_ok_and(|text| text.contains("  lwc:"))
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
                "# Hermes project integration: AGENTS.md instructions. Project MCP, Skill and Shell Hooks are unsupported.\n{}\n",
                install::guidance()
            );
        }
        "# Hermes Agent global integration (print-only)\n\
mcp_servers:\n  lwc:\n    command: lwc\n    args:\n      - serve\n      - --mcp\n\
platform_toolsets:\n  cli:\n    - mcp-lwc\n\
Skill: ~/.hermes/skills/using-lwc\n\
Hook: config.yaml hooks.pre_llm_call and hooks.pre_tool_call\n\
Hook events: pre_llm_call, pre_tool_call (matcher: terminal, timeout: 2, fail_closed: false)\n\
Instructions: ~/.hermes/SOUL.md marker\n"
            .into()
    }
    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        if self.supports_location(environment.location) {
            install::all_paths(&self.paths(environment))
        } else {
            Vec::new()
        }
    }
}
