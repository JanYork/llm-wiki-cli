use super::*;
use std::fs;

pub(super) static OPENCODE: OpenCodeTarget = OpenCodeTarget;
pub(super) struct OpenCodeTarget;

impl OpenCodeTarget {
    fn global_root(&self, environment: &TargetEnvironment<'_>) -> PathBuf {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| environment.home.join(".config"))
            .join("opencode")
    }

    fn config(&self, environment: &TargetEnvironment<'_>) -> PathBuf {
        let base = if environment.location == AgentLocation::Global {
            self.global_root(environment)
        } else {
            environment.cwd.to_path_buf()
        };
        let jsonc = base.join("opencode.jsonc");
        let json = base.join("opencode.json");
        if jsonc.exists() || !json.exists() {
            jsonc
        } else {
            json
        }
    }
    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        let global = environment.location == AgentLocation::Global;
        let root = self.global_root(environment);
        TargetPaths {
            mcp: Some(self.config(environment)),
            instruction: Some(if global {
                root.join("AGENTS.md")
            } else {
                environment.cwd.join("AGENTS.md")
            }),
            hook: Some(if global {
                root.join("plugins/lwc.js")
            } else {
                environment.cwd.join(".opencode/plugins/lwc.js")
            }),
            skill_dir: Some(if global {
                root.join("skills/using-lwc")
            } else {
                environment.cwd.join(".opencode/skills/using-lwc")
            }),
            aux: Vec::new(),
        }
    }
}

impl AgentTarget for OpenCodeTarget {
    fn id(&self) -> &'static str {
        "opencode"
    }
    fn display_name(&self) -> &'static str {
        "OpenCode"
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
        DetectionResult {
            installed: install::command_exists("opencode")
                || self.global_root(environment).exists()
                || environment.cwd.join("opencode.jsonc").exists()
                || environment.cwd.join("opencode.json").exists(),
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
        install::standard_config(self.id(), location)
    }
    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf> {
        install::all_paths(&self.paths(environment))
    }
}
