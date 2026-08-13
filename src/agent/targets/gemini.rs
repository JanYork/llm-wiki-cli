use super::*;
use std::fs;

pub(super) static GEMINI: GeminiTarget = GeminiTarget;
pub(super) struct GeminiTarget;

impl GeminiTarget {
    fn paths(&self, environment: &TargetEnvironment<'_>) -> TargetPaths {
        let global = environment.location == AgentLocation::Global;
        let settings = if global {
            environment.home.join(".gemini/settings.json")
        } else {
            environment.cwd.join(".gemini/settings.json")
        };
        TargetPaths {
            mcp: Some(settings.clone()),
            instruction: Some(if global {
                environment.home.join(".gemini/GEMINI.md")
            } else {
                environment.cwd.join("GEMINI.md")
            }),
            hook: Some(settings),
            skill_dir: Some(if global {
                environment.home.join(".gemini/skills/using-lwc")
            } else {
                environment.cwd.join(".gemini/skills/using-lwc")
            }),
            aux: Vec::new(),
        }
    }
}

impl AgentTarget for GeminiTarget {
    fn id(&self) -> &'static str {
        "gemini"
    }
    fn display_name(&self) -> &'static str {
        "Gemini CLI"
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
            installed: install::command_exists("gemini")
                || paths.mcp.as_ref().is_some_and(|path| path.exists())
                || paths.skill_dir.as_ref().is_some_and(|path| path.exists()),
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
