use serde_json::Value;
#[cfg(unix)]
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Condvar, Mutex, OnceLock},
};

const MAX_CLI_PROCESSES: usize = 4;

trait LimitedOutput {
    fn limited_output(&mut self) -> Output;
}

impl LimitedOutput for Command {
    fn limited_output(&mut self) -> Output {
        static ACTIVE: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
        let (active, ready) = ACTIVE.get_or_init(|| (Mutex::new(0), Condvar::new()));
        let mut count = active.lock().unwrap();
        while *count >= MAX_CLI_PROCESSES {
            count = ready.wait(count).unwrap();
        }
        *count += 1;
        drop(count);
        let output = self.output().unwrap();
        let mut count = active.lock().unwrap();
        *count -= 1;
        ready.notify_one();
        output
    }
}

struct World {
    _temp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
    backend: PathBuf,
    log: PathBuf,
}

fn assert_onboarding_guidance(text: &str) {
    for expected in [
        "`using-lwc` Skill when it is available",
        "full LWC capability guidance is missing",
        "lwc agent status",
        "LWC_READINESS",
        "lwc serve --mcp",
        "single read-only `lwc_explore` tool",
        "plain-text choice",
        "Detection is not consent",
        "1. Enable both graphs",
        "lwc --scope project config set --graph grafeo",
        "lwc --scope project graph verify",
        "lwc --scope project cg init",
        "lwc --scope project cg status",
        "continue the primary task",
    ] {
        assert!(
            text.contains(expected),
            "missing Agent guidance: {expected}"
        );
    }
    assert!(!text.contains("native LWC integration package"));
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        let backend = temp.path().join("fake-codegraph");
        let log = temp.path().join("backend.log");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        let bin = home.join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_executable(&bin.join("lwc"));
        write_backend(&backend);
        Self {
            _temp: temp,
            project,
            home,
            backend,
            log,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lwc"));
        command
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env(
                "PATH",
                std::env::join_paths([
                    self.home.join("bin"),
                    PathBuf::from("/usr/bin"),
                    PathBuf::from("/bin"),
                ])
                .unwrap(),
            )
            .env("LWC_CODEGRAPH_BINARY", &self.backend)
            .env("LWC_FAKE_LOG", &self.log);
        command
    }

    fn output(&self, args: &[&str]) -> Output {
        let mut command = self.command();
        command.args(args);
        command.limited_output()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[cfg(unix)]
fn write_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(
        path,
        "#!/bin/sh\nif [ \"${1:-}\" = \"serve\" ] && [ \"${2:-}\" = \"--help\" ]; then printf '%s\\n' 'Usage: lwc serve --mcp --path PATH'; fi\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn write_executable(path: &Path) {
    fs::write(path.with_extension("exe"), []).unwrap();
}

#[cfg(unix)]
fn write_backend(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(
        path,
        r##"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$LWC_FAKE_LOG"
action=$1
target=
location=
previous=
for arg in "$@"; do
  if [ "$previous" = "--target" ]; then target=$arg; fi
  if [ "$previous" = "--location" ]; then location=$arg; fi
  previous=$arg
done
if [ "$action" != "install" ]; then exit 0; fi
case "$target" in
  codex)
    mkdir -p "$HOME/.codex"
    printf 'model = "test"\n\n[mcp_servers.codegraph]\ncommand = "codegraph"\nargs = ["serve", "--mcp"]\n' > "$HOME/.codex/config.toml"
    printf 'user codex instructions\n\n<!-- CODEGRAPH_START -->\nCodeGraph backend block\n<!-- CODEGRAPH_END -->\n' > "$HOME/.codex/AGENTS.md"
    ;;
  claude)
    mkdir -p "$HOME/.claude"
    printf '%s\n' '{"mcpServers":{"sibling":{"command":"keep"},"codegraph":{"type":"stdio","command":"codegraph","args":["serve","--mcp"]}}}' > "$HOME/.claude.json"
    printf '%s\n' '{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"keep-hook"}]},{"hooks":[{"type":"command","command":"codegraph prompt-hook"}]}]}}' > "$HOME/.claude/settings.json"
    printf 'user claude instructions\n\n<!-- CODEGRAPH_START -->\nCodeGraph backend block\n<!-- CODEGRAPH_END -->\n' > "$HOME/.claude/CLAUDE.md"
    ;;
  cursor)
    mkdir -p "$HOME/.cursor"
    printf '%s\n' '{"mcpServers":{"codegraph":{"command":"codegraph","args":["serve","--mcp"]}}}' > "$HOME/.cursor/mcp.json"
    ;;
  opencode)
    mkdir -p "$HOME/.config/opencode"
    printf '%s\n' '{"mcp":{"codegraph":{"type":"local","command":["codegraph","serve","--mcp"],"enabled":true}}}' > "$HOME/.config/opencode/opencode.jsonc"
    printf 'CodeGraph backend instructions\n' > "$HOME/.config/opencode/AGENTS.md"
    ;;
  hermes)
    mkdir -p "$HOME/.hermes"
    printf 'mcp_servers:\n  codegraph:\n    command: codegraph\n    args:\n      - serve\n      - --mcp\n' > "$HOME/.hermes/config.yaml"
    ;;
  gemini)
    mkdir -p "$HOME/.gemini"
    printf '%s\n' '{"mcpServers":{"codegraph":{"command":"codegraph","args":["serve","--mcp"]}}}' > "$HOME/.gemini/settings.json"
    printf 'CodeGraph backend instructions\n' > "$HOME/.gemini/GEMINI.md"
    ;;
  antigravity)
    mkdir -p "$HOME/.gemini/antigravity"
    printf '%s\n' '{"mcpServers":{"codegraph":{"command":"codegraph","args":["serve","--mcp"]}}}' > "$HOME/.gemini/antigravity/mcp_config.json"
    ;;
  kiro)
    mkdir -p "$HOME/.kiro/settings"
    printf '%s\n' '{"mcpServers":{"codegraph":{"command":"codegraph","args":["serve","--mcp"]}}}' > "$HOME/.kiro/settings/mcp.json"
    ;;
  *) exit 0 ;;
esac
"##,
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn write_backend(_path: &Path) {}

#[cfg(unix)]
#[test]
fn detected_yes_install_is_idempotent_and_uninstall_restores_exact_user_bytes() {
    let world = World::new();
    fs::create_dir_all(world.home.join(".codex")).unwrap();
    fs::create_dir_all(world.home.join(".claude")).unwrap();
    let codex_config = world.home.join(".codex/config.toml");
    let codex_agents = world.home.join(".codex/AGENTS.md");
    let codex_hooks = world.home.join(".codex/hooks.json");
    let claude_mcp = world.home.join(".claude.json");
    let claude_settings = world.home.join(".claude/settings.json");
    let claude_md = world.home.join(".claude/CLAUDE.md");
    fs::write(&codex_config, "model = \"test\"\n").unwrap();
    fs::write(&codex_agents, "user codex instructions\n").unwrap();
    fs::write(
        &codex_hooks,
        "{\"description\":\"keep\",\"hooks\":{\"PreToolUse\":[]}}\n",
    )
    .unwrap();
    fs::write(
        &claude_mcp,
        "{\"mcpServers\":{\"sibling\":{\"command\":\"keep\"}}}\n",
    )
    .unwrap();
    fs::write(
        &claude_settings,
        "{\"hooks\":{\"UserPromptSubmit\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"keep-hook\"}]}]}}\n",
    )
    .unwrap();
    fs::write(&claude_md, "user claude instructions\n").unwrap();
    let originals = [
        (&codex_config, fs::read(&codex_config).unwrap()),
        (&codex_agents, fs::read(&codex_agents).unwrap()),
        (&codex_hooks, fs::read(&codex_hooks).unwrap()),
        (&claude_mcp, fs::read(&claude_mcp).unwrap()),
        (&claude_settings, fs::read(&claude_settings).unwrap()),
        (&claude_md, fs::read(&claude_md).unwrap()),
    ];

    let installed = world.ok(&["agent", "install", "--yes"]);
    assert_eq!(installed["location"], "global");
    assert_eq!(installed["targets"].as_array().unwrap().len(), 2);
    let codex = fs::read_to_string(&codex_config).unwrap();
    assert!(codex.contains("command = \"lwc\""));
    assert!(codex.contains("[mcp_servers.lwc]"));
    assert!(codex.contains("args = [\"serve\", \"--mcp\"]"));
    let claude: Value = serde_json::from_slice(&fs::read(&claude_mcp).unwrap()).unwrap();
    assert_eq!(claude["mcpServers"]["sibling"]["command"], "keep");
    assert_eq!(claude["mcpServers"]["lwc"]["command"], "lwc");
    assert_eq!(
        claude["mcpServers"]["lwc"]["args"],
        serde_json::json!(["serve", "--mcp"])
    );
    assert!(claude["mcpServers"].get("codegraph").is_none());
    assert!(
        !world.log.exists(),
        "Agent install must not invoke CodeGraph"
    );
    let settings: Value = serde_json::from_slice(&fs::read(&claude_settings).unwrap()).unwrap();
    let settings_text = settings.to_string();
    assert!(settings_text.contains("keep-hook"));
    assert!(settings_text.contains("UserPromptSubmit"));
    assert!(settings_text.contains("SessionStart"));
    assert!(!settings_text.contains("codegraph prompt-hook"));
    assert!(settings_text.contains("agent hook --agent claude"));
    let codex_hook_text = fs::read_to_string(&codex_hooks).unwrap();
    assert!(codex_hook_text.contains("\"description\": \"keep\""));
    assert!(codex_hook_text.contains("SessionStart"));
    assert!(!codex_hook_text.contains("PostCompact"));
    assert!(codex_hook_text.contains("agent hook --agent codex"));
    assert!(codex_hook_text.contains("--scope all agent hook --agent codex --event SessionStart"));
    assert!(settings_text.contains("--scope all agent hook --agent claude --event SessionStart"));
    assert!(
        !settings_text.contains("--scope all agent hook --agent claude --event UserPromptSubmit")
    );
    let codex_guidance = fs::read_to_string(&codex_agents).unwrap();
    let claude_guidance = fs::read_to_string(&claude_md).unwrap();
    assert!(codex_guidance.contains("LWC_AGENT_START"));
    assert!(claude_guidance.contains("LWC_AGENT_START"));
    assert!(codex_guidance.contains("LWC_READINESS.md_trans"));
    assert!(claude_guidance.contains("LWC_READINESS.md_trans"));
    assert!(codex_guidance.contains("LWC_READINESS.office"));
    assert!(claude_guidance.contains("LWC_READINESS.office"));
    assert!(codex_guidance.contains("config set --office officecli"));
    assert!(claude_guidance.contains("config set --office officecli"));
    assert_onboarding_guidance(&codex_guidance);
    assert_onboarding_guidance(&claude_guidance);

    let installed_bytes = [
        fs::read(&codex_config).unwrap(),
        fs::read(&codex_agents).unwrap(),
        fs::read(&codex_hooks).unwrap(),
        fs::read(&claude_mcp).unwrap(),
        fs::read(&claude_settings).unwrap(),
        fs::read(&claude_md).unwrap(),
    ];
    let reinstalled = world.ok(&["agent", "install", "--yes"]);
    assert!(
        reinstalled["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|target| target["status"] == "unchanged")
    );
    for (index, path) in [
        &codex_config,
        &codex_agents,
        &codex_hooks,
        &claude_mcp,
        &claude_settings,
        &claude_md,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(fs::read(path).unwrap(), installed_bytes[index]);
    }

    let refreshed = world.ok(&["agent", "refresh", "--target", "codex,claude"]);
    assert!(
        refreshed["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|target| target["status"] == "unchanged")
    );

    world.ok(&["agent", "uninstall", "--target", "codex,claude", "--yes"]);
    for (path, original) in originals {
        assert_eq!(fs::read(path).unwrap(), original);
    }
    assert!(!world.home.join(".agents/skills/using-lwc").exists());
}

#[cfg(unix)]
#[test]
fn codex_local_install_uses_official_project_config_hooks_and_agents_paths() {
    let world = World::new();
    let before = directory_snapshot(&world.project);

    world.ok(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--location",
        "local",
        "--yes",
    ]);

    let config = fs::read_to_string(world.project.join(".codex/config.toml")).unwrap();
    assert!(config.contains("[mcp_servers.lwc]"));
    assert!(config.contains("args = [\"serve\", \"--mcp\"]"));
    let hooks = fs::read_to_string(world.project.join(".codex/hooks.json")).unwrap();
    assert!(hooks.contains("agent hook --agent codex"));
    assert!(
        fs::read_to_string(world.project.join("AGENTS.md"))
            .unwrap()
            .contains("LWC_AGENT_START")
    );

    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--location",
        "local",
        "--yes",
    ]);
    let after = directory_snapshot(&world.project)
        .into_iter()
        .filter(|(path, _)| !path.starts_with(".lwc"))
        .collect::<Vec<_>>();
    assert_eq!(after, before);
}

#[cfg(unix)]
#[test]
fn codex_global_install_honors_the_official_codex_home() {
    let world = World::new();
    let codex_home = world.home.join("custom-codex");
    let output = world
        .command()
        .env("CODEX_HOME", &codex_home)
        .args([
            "agent",
            "install",
            "--target",
            "codex",
            "--location",
            "global",
            "--yes",
        ])
        .limited_output();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(codex_home.join("config.toml").is_file());
    assert!(codex_home.join("AGENTS.md").is_file());
    assert!(codex_home.join("hooks.json").is_file());
    assert!(
        world
            .home
            .join(".agents/skills/using-lwc/SKILL.md")
            .is_file()
    );
    assert!(!world.home.join(".codex/config.toml").exists());

    let output = world
        .command()
        .env("CODEX_HOME", &codex_home)
        .args([
            "agent",
            "uninstall",
            "--target",
            "codex",
            "--location",
            "global",
            "--yes",
        ])
        .limited_output();
    assert!(output.status.success());
    assert!(!codex_home.join("config.toml").exists());
    assert!(!codex_home.join("AGENTS.md").exists());
    assert!(!codex_home.join("hooks.json").exists());
    assert!(
        !world
            .home
            .join(".agents/skills/using-lwc/SKILL.md")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn agent_install_bundles_temporal_memory_reference_byte_identically() {
    let world = World::new();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--location",
        "global",
        "--yes",
    ]);
    let relative = "references/temporal-memory.md";
    assert_eq!(
        fs::read(world.home.join(".agents/skills/using-lwc").join(relative)).unwrap(),
        fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("skills/using-lwc")
                .join(relative)
        )
        .unwrap()
    );
    for skill in [
        "using-todo",
        "using-plan",
        "using-sync",
        "using-tutor",
        "using-book",
        "using-practice",
    ] {
        assert_eq!(
            fs::read(
                world
                    .home
                    .join(".agents/skills")
                    .join(skill)
                    .join("SKILL.md")
            )
            .unwrap_or_else(|error| panic!("installed {skill} is missing: {error}")),
            fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("skills")
                    .join(skill)
                    .join("SKILL.md")
            )
            .unwrap()
        );
    }
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(
        !world
            .home
            .join(".agents/skills/using-todo/SKILL.md")
            .exists()
    );
    assert!(
        !world
            .home
            .join(".agents/skills/using-plan/SKILL.md")
            .exists()
    );
    assert!(
        !world
            .home
            .join(".agents/skills/using-sync/SKILL.md")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn kiro_global_install_honors_kiro_home_and_installs_official_accessories() {
    let world = World::new();
    let kiro_home = world.home.join("custom-kiro");
    let output = world
        .command()
        .env("KIRO_HOME", &kiro_home)
        .args([
            "agent",
            "install",
            "--target",
            "kiro",
            "--location",
            "global",
            "--yes",
        ])
        .limited_output();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["targets"][0]["permissions"], "user_managed");

    let mcp: Value =
        serde_json::from_slice(&fs::read(kiro_home.join("settings/mcp.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["lwc"]["autoApprove"], Value::Null);
    assert!(kiro_home.join("steering/lwc.md").is_file());
    assert!(kiro_home.join("skills/using-lwc/SKILL.md").is_file());
    assert!(kiro_home.join("hooks/lwc.json").is_file());
    assert!(!world.home.join(".kiro/settings/mcp.json").exists());

    let output = world
        .command()
        .env("KIRO_HOME", &kiro_home)
        .args([
            "agent",
            "uninstall",
            "--target",
            "kiro",
            "--location",
            "global",
            "--yes",
        ])
        .limited_output();
    assert!(output.status.success());
    assert!(!kiro_home.join("settings/mcp.json").exists());
    assert!(!kiro_home.join("steering/lwc.md").exists());
    assert!(!kiro_home.join("hooks/lwc.json").exists());
    assert!(!kiro_home.join("skills/using-lwc/SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn installer_migrates_only_owned_legacy_and_rejects_foreign_lwc_without_writes() {
    let world = World::new();
    fs::create_dir_all(world.home.join(".codex")).unwrap();
    let config = world.home.join(".codex/config.toml");
    let original = format!(
        "model = \"test\"\n\n[mcp_servers.codegraph]\ncommand = {}\nargs = [\"cg\", \"serve\", \"--mcp\"]\n",
        serde_json::to_string("lwc").unwrap()
    );
    fs::write(&config, &original).unwrap();
    world.ok(&["agent", "install", "--target", "codex", "--yes"]);
    let installed = fs::read_to_string(&config).unwrap();
    assert!(installed.contains("[mcp_servers.lwc]"));
    assert!(!installed.contains("[mcp_servers.codegraph]"));
    world.ok(&["agent", "uninstall", "--target", "codex", "--yes"]);
    assert_eq!(fs::read_to_string(config).unwrap(), original);

    let foreign = World::new();
    let config = foreign.home.join(".claude.json");
    fs::write(
        &config,
        "{\"mcpServers\":{\"codegraph\":{\"command\":\"codegraph\",\"args\":[\"serve\",\"--mcp\"]},\"lwc\":{\"command\":\"foreign\",\"args\":[]}}}\n",
    )
    .unwrap();
    let before = fs::read(&config).unwrap();
    let output = foreign.output(&["agent", "install", "--target", "claude", "--yes"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("agent_mcp_conflict"));
    assert_eq!(fs::read(config).unwrap(), before);
    assert!(!foreign.home.join(".claude/CLAUDE.md").exists());
    assert!(!foreign.home.join(".claude/settings.json").exists());
}

#[cfg(unix)]
#[test]
fn installer_migrates_absolute_legacy_lwc_codegraph_entries() {
    let world = World::new();
    let lwc = world.home.join("bin/lwc");
    let command = serde_json::to_string(lwc.to_string_lossy().as_ref()).unwrap();
    let codex = world.home.join(".codex/config.toml");
    let claude = world.home.join(".claude.json");
    let opencode = world.home.join(".config/opencode/opencode.jsonc");
    let hermes = world.home.join(".hermes/config.yaml");
    for path in [&codex, &claude, &opencode, &hermes] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    fs::write(
        &codex,
        format!(
            "[mcp_servers.codegraph]\ncommand = {command}\nargs = [\"cg\", \"serve\", \"--mcp\"]\n"
        ),
    )
    .unwrap();
    fs::write(
        &claude,
        format!(
            "{{\"mcpServers\":{{\"codegraph\":{{\"command\":{command},\"args\":[\"cg\",\"serve\",\"--mcp\"]}}}}}}"
        ),
    )
    .unwrap();
    fs::write(
        &opencode,
        format!(
            "{{\"mcp\":{{\"codegraph\":{{\"type\":\"local\",\"command\":[{command},\"cg\",\"serve\",\"--mcp\"],\"enabled\":true}}}}}}"
        ),
    )
    .unwrap();
    fs::write(
        &hermes,
        format!(
            "mcp_servers:\n  codegraph:\n    command: {command}\n    args:\n      - cg\n      - serve\n      - --mcp\n"
        ),
    )
    .unwrap();
    let originals = [
        (&codex, fs::read(&codex).unwrap()),
        (&claude, fs::read(&claude).unwrap()),
        (&opencode, fs::read(&opencode).unwrap()),
        (&hermes, fs::read(&hermes).unwrap()),
    ];

    world.ok(&[
        "agent",
        "install",
        "--target",
        "codex,claude,opencode,hermes",
        "--location",
        "global",
        "--yes",
    ]);
    for path in [&codex, &claude, &opencode, &hermes] {
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("lwc"));
        assert!(!text.contains("codegraph"), "legacy MCP remained: {text}");
    }

    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "codex,claude,opencode,hermes",
        "--location",
        "global",
        "--yes",
    ]);
    for (path, original) in originals {
        assert_eq!(fs::read(path).unwrap(), original);
    }
}

#[cfg(unix)]
#[test]
fn installer_fuses_official_codegraph_surfaces_into_lwc_and_uninstall_restores_them() {
    let world = World::new();
    let codex_config = world.home.join(".codex/config.toml");
    let codex_instructions = world.home.join(".codex/AGENTS.md");
    let claude_mcp = world.home.join(".claude.json");
    let claude_settings = world.home.join(".claude/settings.json");
    let claude_instructions = world.home.join(".claude/CLAUDE.md");
    let cursor_mcp = world.home.join(".cursor/mcp.json");
    let opencode_config = world.home.join(".config/opencode/opencode.jsonc");
    let opencode_instructions = world.home.join(".config/opencode/AGENTS.md");
    let hermes_config = world.home.join(".hermes/config.yaml");
    for path in [
        &codex_config,
        &codex_instructions,
        &claude_mcp,
        &claude_settings,
        &claude_instructions,
        &cursor_mcp,
        &opencode_config,
        &opencode_instructions,
        &hermes_config,
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    fs::write(
        &codex_config,
        "model = \"test\"\n\n[mcp_servers.codegraph]\ncommand = \"codegraph\"\nargs = [\"serve\", \"--mcp\"]\n",
    )
    .unwrap();
    fs::write(
        &codex_instructions,
        "codex user rules\n\n<!-- CODEGRAPH_START -->\n## CodeGraph\nofficial block\n<!-- CODEGRAPH_END -->\n",
    )
    .unwrap();
    fs::write(
        &claude_mcp,
        r#"{"mcpServers":{"keep":{"command":"keep"},"codegraph":{"type":"stdio","command":"codegraph","args":["serve","--mcp"]}}}"#,
    )
    .unwrap();
    fs::write(
        &claude_settings,
        r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"keep-hook"}]},{"hooks":[{"type":"command","command":"codegraph prompt-hook"}]}]},"permissions":{"allow":["keep-permission","mcp__codegraph__codegraph_search","mcp__codegraph__codegraph_context","mcp__codegraph__codegraph_callers","mcp__codegraph__codegraph_callees","mcp__codegraph__codegraph_impact","mcp__codegraph__codegraph_node","mcp__codegraph__codegraph_status"]}}"#,
    )
    .unwrap();
    fs::write(
        &claude_instructions,
        "claude user rules\n\n<!-- CODEGRAPH_START -->\n## CodeGraph\nofficial block\n<!-- CODEGRAPH_END -->\n",
    )
    .unwrap();
    fs::write(
        &cursor_mcp,
        r#"{"mcpServers":{"codegraph":{"type":"stdio","command":"codegraph","args":["serve","--mcp","--path","${workspaceFolder}"]}}}"#,
    )
    .unwrap();
    fs::write(
        &opencode_config,
        "{\n  // keep this comment\n  \"mcp\": {\"codegraph\": {\"type\": \"local\", \"command\": [\"codegraph\", \"serve\", \"--mcp\"], \"enabled\": true}}\n}\n",
    )
    .unwrap();
    fs::write(
        &opencode_instructions,
        "opencode user rules\n\n<!-- CODEGRAPH_START -->\n## CodeGraph\nofficial block\n<!-- CODEGRAPH_END -->\n",
    )
    .unwrap();
    fs::write(
        &hermes_config,
        "mcp_servers:\n  codegraph:\n    command: codegraph\n    args:\n      - serve\n      - --mcp\n    timeout: 120\n    connect_timeout: 60\n    enabled: true\nplatform_toolsets:\n  cli:\n    - hermes-cli\n    - mcp-codegraph\n",
    )
    .unwrap();
    let originals = [
        (&codex_config, fs::read(&codex_config).unwrap()),
        (&codex_instructions, fs::read(&codex_instructions).unwrap()),
        (&claude_mcp, fs::read(&claude_mcp).unwrap()),
        (&claude_settings, fs::read(&claude_settings).unwrap()),
        (
            &claude_instructions,
            fs::read(&claude_instructions).unwrap(),
        ),
        (&cursor_mcp, fs::read(&cursor_mcp).unwrap()),
        (&opencode_config, fs::read(&opencode_config).unwrap()),
        (
            &opencode_instructions,
            fs::read(&opencode_instructions).unwrap(),
        ),
        (&hermes_config, fs::read(&hermes_config).unwrap()),
    ];

    world.ok(&[
        "agent",
        "install",
        "--target",
        "codex,claude,cursor,opencode,hermes",
        "--location",
        "global",
        "--yes",
    ]);

    for path in [
        &codex_config,
        &claude_mcp,
        &cursor_mcp,
        &opencode_config,
        &hermes_config,
    ] {
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("lwc"), "missing fused LWC MCP in {path:?}");
        assert!(
            !text.contains("codegraph"),
            "standalone CodeGraph MCP remained in {path:?}: {text}"
        );
    }
    for path in [
        &codex_instructions,
        &claude_instructions,
        &opencode_instructions,
    ] {
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("LWC_AGENT_START"));
        assert!(!text.contains("CODEGRAPH_START"));
    }
    let claude_settings_text = fs::read_to_string(&claude_settings).unwrap();
    assert!(claude_settings_text.contains("keep-hook"));
    assert!(claude_settings_text.contains("keep-permission"));
    assert!(!claude_settings_text.contains("codegraph"));
    assert!(
        !fs::read_to_string(&hermes_config)
            .unwrap()
            .contains("mcp-codegraph")
    );

    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "codex,claude,cursor,opencode,hermes",
        "--location",
        "global",
        "--yes",
    ]);
    for (path, original) in originals {
        assert_eq!(
            fs::read(path).unwrap(),
            original,
            "did not restore {path:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn installer_migrates_official_project_codegraph_files_without_orphaning_them() {
    let world = World::new();
    let cursor_mcp = world.project.join(".cursor/mcp.json");
    let cursor_rule = world.project.join(".cursor/rules/codegraph.mdc");
    let claude_legacy_mcp = world.project.join(".claude.json");
    let claude_instructions = world.project.join(".claude/CLAUDE.md");
    for path in [
        &cursor_mcp,
        &cursor_rule,
        &claude_legacy_mcp,
        &claude_instructions,
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    fs::write(
        &cursor_mcp,
        format!(
            "{{\"mcpServers\":{{\"codegraph\":{{\"type\":\"stdio\",\"command\":\"codegraph\",\"args\":[\"serve\",\"--mcp\",\"--path\",{}]}}}}}}",
            serde_json::to_string(world.project.to_string_lossy().as_ref()).unwrap()
        ),
    )
    .unwrap();
    fs::write(
        &cursor_rule,
        "---\ndescription: CodeGraph structural code intelligence\nalwaysApply: true\n---\n<!-- CODEGRAPH_START -->\n## CodeGraph\nofficial block\n<!-- CODEGRAPH_END -->\n",
    )
    .unwrap();
    fs::write(
        &claude_legacy_mcp,
        r#"{"mcpServers":{"codegraph":{"type":"stdio","command":"codegraph","args":["serve","--mcp"]}}}"#,
    )
    .unwrap();
    fs::write(
        &claude_instructions,
        "<!-- CODEGRAPH_START -->\n## CodeGraph\nofficial block\n<!-- CODEGRAPH_END -->\n",
    )
    .unwrap();
    let originals = [
        (&cursor_mcp, fs::read(&cursor_mcp).unwrap()),
        (&cursor_rule, fs::read(&cursor_rule).unwrap()),
        (&claude_legacy_mcp, fs::read(&claude_legacy_mcp).unwrap()),
        (
            &claude_instructions,
            fs::read(&claude_instructions).unwrap(),
        ),
    ];

    world.ok(&[
        "agent",
        "install",
        "--target",
        "cursor,claude",
        "--location",
        "local",
        "--yes",
    ]);
    assert!(!cursor_rule.exists());
    assert!(!claude_legacy_mcp.exists());
    assert!(world.project.join(".mcp.json").is_file());
    assert!(world.project.join(".cursor/rules/lwc.mdc").is_file());

    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "cursor,claude",
        "--location",
        "local",
        "--yes",
    ]);
    for (path, original) in originals {
        assert_eq!(
            fs::read(path).unwrap(),
            original,
            "did not restore {path:?}"
        );
    }
    assert!(!world.project.join(".mcp.json").exists());
    assert!(!world.project.join(".cursor/rules/lwc.mdc").exists());
}

#[cfg(unix)]
#[test]
fn uninstall_removes_directories_created_by_lwc_and_does_not_redetect_codex() {
    let world = World::new();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(world.home.join(".codex").is_dir());
    assert!(world.home.join(".agents").is_dir());

    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(!world.home.join(".codex").exists());
    assert!(!world.home.join(".agents").exists());
    assert!(!world.home.join(".lwc").exists());
    let result = world.ok(&["agent", "install", "--location", "global", "--yes"]);
    assert_eq!(result["detected"], serde_json::json!([]));
    assert_eq!(result["targets"], serde_json::json!([]));

    fs::create_dir_all(world.home.join(".codex")).unwrap();
    world.ok(&["agent", "install", "--target", "codex", "--yes"]);
    world.ok(&["agent", "uninstall", "--target", "codex", "--yes"]);
    assert!(
        world.home.join(".codex").is_dir(),
        "a pre-existing Agent root must remain"
    );
}

#[cfg(unix)]
#[test]
fn shared_copilot_skill_directories_are_removed_after_the_last_owner() {
    let world = World::new();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode,copilot-jetbrains",
        "--location",
        "global",
        "--yes",
    ]);
    let shared_skill = world.home.join(".copilot/skills/using-lwc");
    assert!(shared_skill.is_dir());

    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "copilot-vscode",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(shared_skill.is_dir());
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "copilot-jetbrains",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(!world.home.join(".copilot").exists());
}

#[cfg(unix)]
#[test]
fn one_target_conflict_does_not_block_later_targets() {
    let world = World::new();
    fs::write(
        world.home.join(".claude.json"),
        r#"{"mcpServers":{"lwc":{"command":"foreign","args":[]}}}"#,
    )
    .unwrap();

    let output = world.output(&[
        "agent",
        "install",
        "--target",
        "claude,codex",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "agent_install_partial");
    assert_eq!(error["error"]["details"]["targets"][0]["target"], "claude");
    assert_eq!(error["error"]["details"]["targets"][0]["status"], "failed");
    assert_eq!(error["error"]["details"]["targets"][1]["target"], "codex");
    assert_eq!(
        error["error"]["details"]["targets"][1]["status"],
        "installed"
    );
    assert!(world.home.join(".codex/config.toml").is_file());
    assert!(!world.home.join(".claude/CLAUDE.md").exists());
}

#[cfg(unix)]
#[test]
fn refresh_upgrades_a_matching_v1_manifest_and_keeps_original_uninstall_snapshot() {
    let world = World::new();
    fs::create_dir_all(world.home.join(".codex")).unwrap();
    let config = world.home.join(".codex/config.toml");
    let original = "model = \"test\"\n";
    fs::write(&config, original).unwrap();
    world.ok(&["agent", "install", "--target", "codex", "--yes"]);

    let legacy = format!(
        "{original}\n[mcp_servers.codegraph]\ncommand = {}\nargs = [\"cg\", \"serve\", \"--mcp\"]\n",
        serde_json::to_string("lwc").unwrap()
    );
    fs::write(&config, &legacy).unwrap();
    let manifest_path = world
        .home
        .join(".lwc/agent-installs/codex-global-global.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["version"] = 1.into();
    manifest.as_object_mut().unwrap().remove("mcp");
    let digest = Sha256::digest(legacy.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let config_text = config.to_string_lossy();
    for file in manifest["files"].as_array_mut().unwrap() {
        if file["path"] == config_text.as_ref() {
            file["post_hash"] = digest.clone().into();
        }
    }
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    world.ok(&["agent", "refresh", "--target", "codex"]);
    let refreshed = fs::read_to_string(&config).unwrap();
    assert!(refreshed.contains("[mcp_servers.lwc]"));
    assert!(!refreshed.contains("[mcp_servers.codegraph]"));
    let upgraded: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(upgraded["version"], 7);
    assert_eq!(upgraded["mcp"]["name"], "lwc");

    world.ok(&["agent", "uninstall", "--target", "codex", "--yes"]);
    assert_eq!(fs::read_to_string(config).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn refresh_upgrades_a_v1_manifest_that_did_not_track_new_accessories() {
    let world = World::new();
    world.ok(&["agent", "install", "--target", "pi", "--yes"]);
    let manifest_path = world.home.join(".lwc/agent-installs/pi-global-global.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["version"] = 1.into();
    manifest.as_object_mut().unwrap().remove("lwc_version");
    manifest["files"] = serde_json::json!([]);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    world.ok(&["agent", "refresh", "--target", "pi"]);
    let status = world.ok(&["agent", "status", "--target", "pi"]);
    assert_eq!(status["targets"][0]["status"], "installed");
    let upgraded: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    assert!(upgraded["files"].as_array().unwrap().iter().any(|file| {
        file["path"]
            .as_str()
            .unwrap()
            .ends_with("skills/using-lwc/SKILL.md")
    }));
}

#[cfg(unix)]
#[test]
fn refresh_reinstalls_accessories_created_by_an_older_lwc_version() {
    let world = World::new();
    world.ok(&["agent", "install", "--target", "codex", "--yes"]);
    let manifest_path = world
        .home
        .join(".lwc/agent-installs/codex-global-global.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["lwc_version"] = "0.0.0".into();
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let status = world.ok(&["agent", "status", "--target", "codex"]);
    assert_eq!(status["targets"][0]["status"], "modified");

    let refreshed = world.ok(&["agent", "refresh", "--target", "codex"]);
    assert_eq!(refreshed["targets"][0]["status"], "installed");
    let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["lwc_version"], env!("CARGO_PKG_VERSION"));
}

#[cfg(unix)]
#[test]
fn refresh_removes_matching_accessory_from_an_old_manifest_path() {
    let world = World::new();
    world.ok(&["agent", "install", "--target", "cursor", "--yes"]);
    let manifest_path = world
        .home
        .join(".lwc/agent-installs/cursor-global-global.json");
    let old_hook = world.home.join(".cursor/legacy-hooks.json");
    let owned = "{\"hooks\":{\"sessionStart\":[{\"command\":\"lwc agent hook --agent cursor --event session_start\"}]}}\n";
    fs::write(&old_hook, owned).unwrap();

    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["lwc_version"] = "0.0.0".into();
    let digest = Sha256::digest(owned.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    manifest["files"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": old_hook,
            "original": null,
            "original_mode": null,
            "post_hash": digest,
        }));
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    world.ok(&["agent", "refresh", "--target", "cursor"]);
    assert!(!old_hook.exists(), "matching old accessory was orphaned");
}

#[cfg(unix)]
#[test]
fn refresh_detects_a_stale_path_even_at_the_current_lwc_version() {
    let world = World::new();
    world.ok(&["agent", "install", "--target", "cursor", "--yes"]);
    let manifest_path = world
        .home
        .join(".lwc/agent-installs/cursor-global-global.json");
    let current_hook = world.home.join(".cursor/hooks.json");
    let old_hook = world.home.join(".cursor/legacy-hooks.json");
    fs::rename(&current_hook, &old_hook).unwrap();

    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    for file in manifest["files"].as_array_mut().unwrap() {
        if file["path"] == current_hook.to_string_lossy().as_ref() {
            file["path"] = old_hook.to_string_lossy().into_owned().into();
        }
    }
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let refreshed = world.ok(&["agent", "refresh", "--target", "cursor"]);
    assert_eq!(refreshed["targets"][0]["status"], "installed");
    assert!(current_hook.is_file());
    assert!(!old_hook.exists());
    let repeated = world.ok(&["agent", "refresh", "--target", "cursor"]);
    assert_eq!(repeated["targets"][0]["status"], "unchanged");
}

#[cfg(unix)]
#[test]
fn refresh_preserves_and_tracks_a_user_modified_old_accessory() {
    let world = World::new();
    world.ok(&["agent", "install", "--target", "cursor", "--yes"]);
    let manifest_path = world
        .home
        .join(".lwc/agent-installs/cursor-global-global.json");
    let old_hook = world.home.join(".cursor/legacy-hooks.json");
    let owned = "{\"hooks\":{\"sessionStart\":[{\"command\":\"lwc agent hook --agent cursor --event session_start\"}]}}\n";
    let modified = format!("{owned}user content\n");
    fs::write(&old_hook, &modified).unwrap();

    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["lwc_version"] = "0.0.0".into();
    let owned_digest = Sha256::digest(owned.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    manifest["files"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": old_hook,
            "original": null,
            "original_mode": null,
            "post_hash": owned_digest,
        }));
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    world.ok(&["agent", "refresh", "--target", "cursor"]);
    assert_eq!(fs::read_to_string(&old_hook).unwrap(), modified);
    let refreshed: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let tracked = refreshed["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == old_hook.to_string_lossy().as_ref())
        .expect("modified old accessory remains tracked");
    assert_eq!(tracked["original"], modified);

    world.ok(&["agent", "uninstall", "--target", "cursor", "--yes"]);
    assert_eq!(fs::read_to_string(old_hook).unwrap(), modified);
}

#[cfg(unix)]
#[test]
fn yes_detects_agent_executables_before_their_first_config() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new();
    let bin = world.home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for agent in ["claude", "code", "codex", "copilot", "pi"] {
        let executable = bin.join(agent);
        fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::create_dir_all(
        world
            .home
            .join(".vscode/extensions/github.copilot-chat-1.0.0"),
    )
    .unwrap();
    let path =
        std::env::join_paths([bin, PathBuf::from("/usr/bin"), PathBuf::from("/bin")]).unwrap();
    let output = world
        .command()
        .env("PATH", path)
        .args(["agent", "install", "--yes"])
        .limited_output();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let installed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        installed["detected"],
        serde_json::json!(["claude", "codex", "copilot-vscode", "copilot-cli", "pi"])
    );
    assert_eq!(installed["targets"].as_array().unwrap().len(), 5);
}

#[cfg(unix)]
#[test]
fn yes_detects_the_official_antigravity_cli_command() {
    let world = World::new();
    write_executable(&world.home.join("bin/agy"));

    let result = world.ok(&["agent", "install", "--yes"]);
    assert!(
        result["detected"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("antigravity"))
    );
    assert_eq!(result["targets"][0]["target"], "antigravity");
}

#[cfg(unix)]
#[test]
fn local_detection_uses_the_host_not_a_generic_project_directory() {
    let world = World::new();
    write_executable(&world.home.join("bin/code"));
    fs::create_dir_all(world.project.join(".github")).unwrap();

    let result = world.ok(&["agent", "install", "--location", "local", "--yes"]);
    assert!(
        !result["detected"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("copilot-vscode"))
    );
    assert!(
        !result["detected"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("copilot-jetbrains"))
    );
}

#[cfg(unix)]
#[test]
fn detection_does_not_treat_shared_directories_as_installed_agents() {
    let world = World::new();
    fs::write(world.project.join("AGENTS.md"), "project instructions\n").unwrap();
    fs::create_dir_all(world.home.join(".gemini/config/plugins")).unwrap();
    fs::create_dir_all(world.home.join("Library/Application Support/JetBrains")).unwrap();

    for target in ["hermes", "gemini", "copilot-jetbrains", "antigravity"] {
        let status = world.ok(&["agent", "status", "--target", target, "--location", "local"]);
        assert_eq!(status["targets"][0]["detected"], false, "{target}");
    }

    let antigravity = world.ok(&[
        "agent",
        "status",
        "--target",
        "antigravity",
        "--location",
        "global",
    ]);
    assert_eq!(antigravity["targets"][0]["detected"], false);
}

#[cfg(unix)]
#[test]
fn copilot_vscode_files_do_not_make_jetbrains_detected() {
    let world = World::new();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode",
        "--location",
        "local",
        "--yes",
    ]);

    let status = world.ok(&[
        "agent",
        "status",
        "--target",
        "copilot-jetbrains",
        "--location",
        "local",
    ]);
    assert_eq!(status["targets"][0]["detected"], false);
}

#[cfg(unix)]
#[test]
fn install_rejects_a_non_lwc_command_on_path() {
    let world = World::new();
    fs::write(
        world.home.join("bin/lwc"),
        "#!/bin/sh\nprintf 'lwc 99.0.0\\n'\nexit 0\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        world.home.join("bin/lwc"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let output = world.output(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "lwc_command_incompatible");
    assert!(!world.home.join(".codex/config.toml").exists());
}

#[cfg(unix)]
#[test]
fn cursor_install_rejects_an_lwc_without_workspace_path_support() {
    let world = World::new();
    fs::write(
        world.home.join("bin/lwc"),
        "#!/bin/sh\nif [ \"${1:-}\" = \"serve\" ] && [ \"${2:-}\" = \"--help\" ]; then printf '%s\\n' 'Usage: lwc serve --mcp'; fi\nexit 0\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        world.home.join("bin/lwc"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let output = world.output(&[
        "agent",
        "install",
        "--target",
        "cursor",
        "--location",
        "local",
        "--yes",
    ]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "lwc_command_incompatible");
    assert!(!world.project.join(".cursor/mcp.json").exists());
}

#[cfg(unix)]
#[test]
fn install_compatibility_probe_is_bounded() {
    use std::{
        os::unix::process::CommandExt,
        thread,
        time::{Duration, Instant},
    };

    let world = World::new();
    fs::write(world.home.join("bin/lwc"), "#!/bin/sh\nsleep 30\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        world.home.join("bin/lwc"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let mut command = world.command();
    command.process_group(0).args([
        "agent",
        "install",
        "--target",
        "codex",
        "--location",
        "global",
        "--yes",
    ]);
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            unsafe extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            unsafe { kill(-(child.id() as i32), 9) };
            child.wait().unwrap();
            break None;
        }
        thread::sleep(Duration::from_millis(25));
    };
    assert!(
        status.is_some_and(|status| !status.success()),
        "Agent install must reject a hung global lwc probe within twelve seconds"
    );
}

#[cfg(unix)]
#[test]
fn yes_does_not_guess_an_agent_when_detection_is_empty() {
    let world = World::new();
    let before = directory_snapshot(&world.home);
    let installed = world.ok(&["agent", "install", "--yes"]);
    assert_eq!(installed["detected"], serde_json::json!([]));
    assert_eq!(installed["targets"], serde_json::json!([]));
    assert_eq!(directory_snapshot(&world.home), before);
}

#[cfg(unix)]
#[test]
fn refresh_rebases_uninstall_snapshot_without_losing_user_edits() {
    let world = World::new();
    let guidance = world.home.join(".codex/AGENTS.md");

    world.ok(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--location",
        "global",
        "--yes",
    ]);
    let installed = fs::read_to_string(&guidance).unwrap();
    fs::write(&guidance, format!("user instructions\n{installed}")).unwrap();

    world.ok(&[
        "agent",
        "refresh",
        "--target",
        "codex",
        "--location",
        "global",
    ]);
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--location",
        "global",
        "--yes",
    ]);

    assert_eq!(
        fs::read_to_string(&guidance).unwrap(),
        "user instructions\n"
    );
    assert!(!world.home.join(".pi/agent/extensions/lwc.js").exists());
}

#[cfg(unix)]
#[test]
fn copilot_cli_installs_its_official_hook_without_touching_siblings() {
    let world = World::new();
    let sibling = world.home.join(".copilot/hooks/user.json");
    let hook = world.home.join(".copilot/hooks/lwc.json");
    fs::create_dir_all(sibling.parent().unwrap()).unwrap();
    fs::write(&sibling, "user hook\n").unwrap();

    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-cli",
        "--location",
        "global",
        "--yes",
    ]);
    let installed: Value = serde_json::from_slice(&fs::read(&hook).unwrap()).unwrap();
    assert_eq!(installed["version"], 1);
    assert_eq!(installed["hooks"]["sessionStart"][0]["type"], "command");
    assert!(
        installed["hooks"]["sessionStart"][0]["bash"]
            .as_str()
            .unwrap()
            .contains("agent hook --agent copilot-cli")
    );
    assert_eq!(fs::read_to_string(&sibling).unwrap(), "user hook\n");

    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "copilot-cli",
        "--location",
        "global",
        "--yes",
    ]);

    assert!(!hook.exists());
    assert_eq!(fs::read_to_string(&sibling).unwrap(), "user hook\n");
}

#[cfg(unix)]
#[test]
fn copilot_cli_modified_hook_preserves_user_entries_on_uninstall() {
    let world = World::new();
    let hook = world.home.join(".copilot/hooks/lwc.json");
    world.ok(&["agent", "install", "--target", "copilot-cli", "--yes"]);
    let mut value: Value = serde_json::from_slice(&fs::read(&hook).unwrap()).unwrap();
    value["hooks"]["sessionEnd"] = serde_json::json!([{
        "type": "command",
        "bash": "user-command"
    }]);
    fs::write(&hook, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    world.ok(&["agent", "uninstall", "--target", "copilot-cli", "--yes"]);
    let value: Value = serde_json::from_slice(&fs::read(&hook).unwrap()).unwrap();
    assert_eq!(value["hooks"]["sessionEnd"][0]["bash"], "user-command");
    assert_eq!(value["hooks"]["sessionStart"], Value::Null);
}

#[cfg(unix)]
#[test]
fn copilot_cli_install_preserves_entries_already_in_its_hook_file() {
    let world = World::new();
    let hook = world.home.join(".copilot/hooks/lwc.json");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(
        &hook,
        r#"{"version":1,"description":"keep","hooks":{"sessionEnd":[{"type":"command","bash":"user-command"}]}}"#,
    )
    .unwrap();

    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-cli",
        "--location",
        "global",
        "--yes",
    ]);

    let installed: Value = serde_json::from_slice(&fs::read(&hook).unwrap()).unwrap();
    assert_eq!(installed["description"], "keep");
    assert_eq!(installed["hooks"]["sessionEnd"][0]["bash"], "user-command");
    assert!(
        installed["hooks"]["sessionStart"][0]["bash"]
            .as_str()
            .unwrap()
            .contains("agent hook --agent copilot-cli")
    );
}

#[cfg(unix)]
#[test]
fn uninstall_preserves_preexisting_lwc_permission_after_unrelated_user_edit() {
    let world = World::new();
    let settings = world.home.join(".claude/settings.json");
    let mcp = world.home.join(".claude.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"permissions":{"allow":["mcp__lwc__lwc_explore"]}}"#,
    )
    .unwrap();
    fs::write(
        &mcp,
        r#"{"mcpServers":{"lwc":{"type":"stdio","command":"lwc","args":["serve","--mcp"]}}}"#,
    )
    .unwrap();

    world.ok(&["agent", "install", "--target", "claude", "--yes"]);
    let mut value: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    value["userSetting"] = serde_json::json!("keep");
    fs::write(&settings, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let mut value: Value = serde_json::from_slice(&fs::read(&mcp).unwrap()).unwrap();
    value["userSetting"] = serde_json::json!("keep");
    fs::write(&mcp, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    world.ok(&["agent", "refresh", "--target", "claude"]);
    let value: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert!(
        value["permissions"]["allow"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("mcp__lwc__lwc_explore"))
    );
    let value: Value = serde_json::from_slice(&fs::read(&mcp).unwrap()).unwrap();
    assert_eq!(value["mcpServers"]["lwc"]["command"], "lwc");

    world.ok(&["agent", "uninstall", "--target", "claude", "--yes"]);
    let value: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(value["userSetting"], "keep");
    assert_eq!(
        value["permissions"]["allow"],
        serde_json::json!(["mcp__lwc__lwc_explore"])
    );
    assert_eq!(value["hooks"]["SessionStart"], Value::Null);
    let value: Value = serde_json::from_slice(&fs::read(&mcp).unwrap()).unwrap();
    assert_eq!(value["userSetting"], "keep");
    assert_eq!(value["mcpServers"]["lwc"]["command"], "lwc");
}

#[cfg(unix)]
#[test]
fn codex_uninstall_preserves_preexisting_lwc_content_after_unrelated_user_edit() {
    let world = World::new();
    let root = world.home.join(".codex");
    fs::create_dir_all(&root).unwrap();
    let config = root.join("config.toml");
    let instructions = root.join("AGENTS.md");
    fs::write(
        &config,
        "model = \"test\"\n\n[mcp_servers.lwc]\ncommand = \"lwc\"\nargs = [\"serve\", \"--mcp\"]\n",
    )
    .unwrap();
    let output = world.output(&["agent", "install", "--print-config", "codex"]);
    assert!(output.status.success());
    let guidance = String::from_utf8(output.stdout).unwrap();
    let marker = guidance
        .split_once("<!-- LWC_AGENT_START -->")
        .map(|(_, tail)| format!("<!-- LWC_AGENT_START -->{tail}"))
        .unwrap();
    let marker = marker.split("\n\n{\"hooks\"").next().unwrap();
    fs::write(&instructions, format!("user rules\n{marker}\n")).unwrap();

    world.ok(&["agent", "install", "--target", "codex", "--yes"]);
    fs::write(
        &config,
        format!(
            "{}\n[features]\nhooks = true\n",
            fs::read_to_string(&config).unwrap()
        ),
    )
    .unwrap();
    fs::write(
        &instructions,
        format!(
            "{}\nuser edit\n",
            fs::read_to_string(&instructions).unwrap()
        ),
    )
    .unwrap();

    world.ok(&["agent", "refresh", "--target", "codex"]);
    assert!(
        fs::read_to_string(&config)
            .unwrap()
            .contains("[mcp_servers.lwc]")
    );
    assert!(
        fs::read_to_string(&instructions)
            .unwrap()
            .contains("<!-- LWC_AGENT_START -->")
    );

    world.ok(&["agent", "uninstall", "--target", "codex", "--yes"]);
    let config = fs::read_to_string(&config).unwrap();
    assert!(config.contains("[mcp_servers.lwc]"));
    assert!(config.contains("[features]\nhooks = true"));
    let instructions = fs::read_to_string(&instructions).unwrap();
    assert!(instructions.contains("<!-- LWC_AGENT_START -->"));
    assert!(instructions.contains("user edit"));
}

#[cfg(unix)]
#[test]
fn antigravity_foreign_lwc_hook_conflicts_before_any_write() {
    let world = World::new();
    let hook = world.home.join(".gemini/config/plugins/lwc/hooks.json");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, r#"{"keep":true,"lwc":{"owner":"user"}}"#).unwrap();
    let before = directory_snapshot(&world.home);

    let output = world.output(&[
        "agent",
        "install",
        "--target",
        "antigravity",
        "--location",
        "global",
        "--yes",
    ]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("agent_config_conflict"));
    assert_eq!(directory_snapshot(&world.home), before);
}

#[cfg(unix)]
#[test]
fn foreign_canonical_skill_conflicts_before_any_write() {
    let world = World::new();
    let skill = world.home.join(".agents/skills/using-lwc/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, "user-owned skill\n").unwrap();
    let before = directory_snapshot(&world.home);

    let output = world.output(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--location",
        "global",
        "--yes",
    ]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("agent_config_conflict"));
    assert_eq!(directory_snapshot(&world.home), before);
}

#[cfg(unix)]
#[test]
fn shared_skill_owned_by_another_manifest_can_upgrade() {
    let world = World::new();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-jetbrains",
        "--location",
        "global",
        "--yes",
    ]);
    let skill = world.home.join(".copilot/skills/using-lwc/SKILL.md");
    let previous = b"previous LWC canonical skill\n";
    fs::write(&skill, previous).unwrap();
    let manifest_path = world
        .home
        .join(".lwc/agent-installs/copilot-jetbrains-global-global.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["lwc_version"] = "0.0.0".into();
    let digest = Sha256::digest(previous)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let file = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|file| file["path"] == skill.to_string_lossy().as_ref())
        .unwrap();
    file["post_hash"] = digest.into();
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode",
        "--location",
        "global",
        "--yes",
    ]);

    assert_eq!(
        fs::read(&skill).unwrap(),
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/using-lwc/SKILL.md")).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn foreign_dedicated_plugin_conflicts_before_any_write() {
    let world = World::new();
    let plugin = world.home.join(".config/opencode/plugins/lwc.js");
    fs::create_dir_all(plugin.parent().unwrap()).unwrap();
    fs::write(&plugin, "export const Lwc = 'user-owned'\n").unwrap();
    let before = directory_snapshot(&world.home);

    let output = world.output(&[
        "agent",
        "install",
        "--target",
        "opencode",
        "--location",
        "global",
        "--yes",
    ]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("agent_config_conflict"));
    assert_eq!(directory_snapshot(&world.home), before);
}

#[cfg(unix)]
#[test]
fn copilot_jetbrains_installs_the_official_global_mcp_path() {
    let world = World::new();
    let guessed = world.home.join(".config/github-copilot/intellij/mcp.json");

    let installed = world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-jetbrains",
        "--location",
        "global",
        "--yes",
    ]);

    assert_eq!(installed["targets"][0]["adaptation"], "strong");
    assert_eq!(installed["targets"][0]["mcp"], "installed");
    assert!(fs::read_to_string(&guessed).unwrap().contains("\"lwc\""));
    assert!(
        world
            .home
            .join(".copilot/skills/using-lwc/SKILL.md")
            .is_file()
    );
    let printed = world.output(&[
        "agent",
        "install",
        "--print-config",
        "copilot-jetbrains",
        "--location",
        "global",
    ]);
    let printed = String::from_utf8(printed.stdout).unwrap();
    assert!(printed.contains("github-copilot/intellij/mcp.json"));
}

#[cfg(unix)]
#[test]
fn copilot_vscode_global_mcp_installs_the_default_user_profile() {
    let world = World::new();
    let default_mcp = if cfg!(target_os = "macos") {
        world
            .home
            .join("Library/Application Support/Code/User/mcp.json")
    } else {
        world.home.join(".config/Code/User/mcp.json")
    };

    let installed = world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode",
        "--location",
        "global",
        "--yes",
    ]);

    assert_eq!(installed["targets"][0]["adaptation"], "strong");
    assert_eq!(installed["targets"][0]["mcp"], "installed");
    assert!(
        fs::read_to_string(&default_mcp)
            .unwrap()
            .contains("\"lwc\"")
    );
    assert!(
        world
            .home
            .join(".copilot/skills/using-lwc/SKILL.md")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn copilot_shared_skill_does_not_false_detect_the_cli() {
    let world = World::new();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode",
        "--location",
        "global",
        "--yes",
    ]);

    let detected = world.ok(&["agent", "install", "--yes"]);

    assert!(
        !detected["detected"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("copilot-cli"))
    );
    assert_eq!(
        detected["targets"][0]["target"],
        serde_json::json!("copilot-vscode")
    );
}

#[cfg(unix)]
#[test]
fn copilot_cli_local_scope_installs_the_host_discovered_mcp_file() {
    let world = World::new();
    let guessed = world.project.join(".github/mcp.json");

    let installed = world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-cli",
        "--location",
        "local",
        "--yes",
    ]);

    assert_eq!(installed["targets"][0]["adaptation"], "strong");
    assert_eq!(installed["targets"][0]["mcp"], "installed");
    assert!(guessed.exists());
    assert!(world.project.join(".github/hooks/lwc.json").is_file());
    assert!(
        world
            .project
            .join(".github/skills/using-lwc/SKILL.md")
            .is_file()
    );
    assert!(
        world
            .project
            .join(".github/copilot-instructions.md")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn kiro_install_preserves_entries_already_in_its_hook_file() {
    let world = World::new();
    let hook = world.home.join(".kiro/hooks/lwc.json");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(
        &hook,
        r#"{"version":"v1","description":"keep","hooks":[{"name":"User hook","trigger":"SessionEnd","action":{"type":"command","command":"user-command"}}]}"#,
    )
    .unwrap();

    world.ok(&[
        "agent",
        "install",
        "--target",
        "kiro",
        "--location",
        "global",
        "--yes",
    ]);

    let installed: Value = serde_json::from_slice(&fs::read(&hook).unwrap()).unwrap();
    assert_eq!(installed["description"], "keep");
    assert!(
        installed["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["action"]["command"] == "user-command" })
    );
    assert!(installed["hooks"].as_array().unwrap().iter().any(|entry| {
        entry["action"]["command"]
            .as_str()
            .is_some_and(|command| command.contains("--scope all agent hook"))
    }));
}

#[cfg(unix)]
#[test]
fn print_config_is_pure_and_pi_installs_an_mcp_tool_bridge() {
    let world = World::new();
    let before = directory_snapshot(&world.home);
    let printed = world.output(&["agent", "install", "--print-config", "codex"]);
    assert!(printed.status.success());
    let text = String::from_utf8(printed.stdout).unwrap();
    assert!(text.contains("mcp_servers.lwc"));
    assert!(!text.contains("mcp_servers.codegraph"));
    assert!(text.contains("LWC_AGENT_START"));
    assert!(text.contains("agent hook --agent codex"));
    assert_onboarding_guidance(&text);
    assert_eq!(directory_snapshot(&world.home), before);
    assert!(!world.log.exists(), "print-config must not call CodeGraph");

    let pi = world.ok(&[
        "agent",
        "install",
        "--target",
        "pi",
        "--location",
        "local",
        "--yes",
    ]);
    assert_eq!(pi["targets"][0]["mcp"], "extension_bridge");
    assert!(world.project.join(".pi/extensions/lwc.js").is_file());
    let extension = fs::read_to_string(world.project.join(".pi/extensions/lwc.js")).unwrap();
    assert!(extension.contains("session_start"));
    assert!(extension.contains("session_before_compact"));
    assert!(extension.contains("before_agent_start"));
    assert!(extension.contains("[\"--scope\", \"all\", \"agent\", \"hook\", \"--agent\", \"pi\""));
    assert!(extension.contains("registerTool"));
    assert!(extension.contains("name: \"lwc_explore\""));
    assert!(extension.contains("spawn(\"lwc\", [\"serve\", \"--mcp\"]"));
}

#[cfg(unix)]
#[test]
fn every_advertised_global_target_round_trips_and_capability_gaps_are_explicit() {
    #[cfg(target_os = "macos")]
    let copilot_vscode_mcp = "Library/Application Support/Code/User/mcp.json";
    #[cfg(all(unix, not(target_os = "macos")))]
    let copilot_vscode_mcp = ".config/Code/User/mcp.json";
    let cases = [
        (
            "claude",
            "strong",
            ".claude.json",
            Some(".claude/CLAUDE.md"),
            Some(".claude/settings.json"),
            Some(".claude/skills/using-lwc"),
        ),
        (
            "cursor",
            "strong",
            ".cursor/mcp.json",
            None,
            Some(".cursor/hooks.json"),
            Some(".cursor/skills/using-lwc"),
        ),
        (
            "codex",
            "strong",
            ".codex/config.toml",
            Some(".codex/AGENTS.md"),
            Some(".codex/hooks.json"),
            Some(".agents/skills/using-lwc"),
        ),
        (
            "opencode",
            "strong",
            ".config/opencode/opencode.jsonc",
            Some(".config/opencode/AGENTS.md"),
            Some(".config/opencode/plugins/lwc.js"),
            Some(".config/opencode/skills/using-lwc"),
        ),
        (
            "hermes",
            "strong",
            ".hermes/config.yaml",
            Some(".hermes/SOUL.md"),
            Some(".hermes/config.yaml"),
            Some(".hermes/skills/using-lwc"),
        ),
        (
            "gemini",
            "strong",
            ".gemini/settings.json",
            Some(".gemini/GEMINI.md"),
            Some(".gemini/settings.json"),
            Some(".gemini/skills/using-lwc"),
        ),
        (
            "antigravity",
            "strong",
            ".gemini/config/plugins/lwc/mcp_config.json",
            Some(".gemini/config/plugins/lwc/rules/lwc.md"),
            Some(".gemini/config/plugins/lwc/hooks.json"),
            Some(".gemini/config/plugins/lwc/skills/using-lwc"),
        ),
        (
            "kiro",
            "strong",
            ".kiro/settings/mcp.json",
            Some(".kiro/steering/lwc.md"),
            Some(".kiro/hooks/lwc.json"),
            Some(".kiro/skills/using-lwc"),
        ),
        (
            "copilot-vscode",
            "strong",
            copilot_vscode_mcp,
            None,
            None,
            Some(".copilot/skills/using-lwc"),
        ),
        (
            "copilot-cli",
            "strong",
            ".copilot/mcp-config.json",
            Some(".copilot/copilot-instructions.md"),
            Some(".copilot/hooks/lwc.json"),
            Some(".copilot/skills/using-lwc"),
        ),
        (
            "copilot-jetbrains",
            "strong",
            ".config/github-copilot/intellij/mcp.json",
            None,
            None,
            Some(".copilot/skills/using-lwc"),
        ),
        (
            "pi",
            "strong",
            ".pi/agent/extensions/lwc.js",
            None,
            Some(".pi/agent/extensions/lwc.js"),
            Some(".pi/agent/skills/using-lwc"),
        ),
    ];
    for (target, adaptation, mcp_path, instruction_path, hook_path, skill_dir) in cases {
        let world = World::new();
        let before = directory_snapshot(&world.home);
        let result = world.ok(&[
            "agent",
            "install",
            "--target",
            target,
            "--location",
            "global",
            "--yes",
        ]);
        assert_eq!(result["targets"][0]["target"], target);
        assert_eq!(result["targets"][0]["adaptation"], adaptation);
        assert_eq!(
            result["targets"][0]["mcp"],
            if target == "pi" {
                "extension_bridge"
            } else {
                "installed"
            }
        );
        assert_eq!(
            result["targets"][0]["skills"],
            if target == "copilot-jetbrains" {
                "configured_preview"
            } else {
                "installed"
            }
        );
        assert_eq!(
            result["targets"][0]["instructions"],
            if instruction_path.is_some() || target == "pi" {
                "installed"
            } else {
                "user_managed"
            }
        );
        assert_eq!(
            result["targets"][0]["lifecycle_hook_mode"],
            if target == "kiro" {
                "configured_preview"
            } else if hook_path.is_some() {
                "installed"
            } else {
                "unsupported"
            }
        );
        assert_eq!(
            result["targets"][0]["permissions"],
            if target == "claude" {
                "installed"
            } else if target.starts_with("copilot-") || target == "kiro" {
                "user_managed"
            } else {
                "not_applicable"
            }
        );
        let mcp = if mcp_path.is_empty() {
            String::new()
        } else {
            fs::read_to_string(world.home.join(mcp_path)).unwrap()
        };
        if !mcp_path.is_empty() {
            assert!(mcp.contains("lwc"), "{target} MCP/bridge missing");
            assert!(
                mcp.contains("serve") || target == "pi",
                "{target} MCP args missing"
            );
        }
        if target == "hermes" {
            assert!(mcp.lines().any(|line| line.trim() == "- mcp-lwc"));
        }
        if target == "antigravity" {
            let config: Value = serde_json::from_str(&mcp).unwrap();
            assert_eq!(config["mcpServers"]["lwc"]["type"], Value::Null);
        }
        if target == "gemini" {
            let config: Value = serde_json::from_str(&mcp).unwrap();
            assert_eq!(config["mcpServers"]["lwc"]["type"], Value::Null);
        }
        if let Some(instruction_path) = instruction_path {
            assert!(
                fs::read_to_string(world.home.join(instruction_path))
                    .unwrap()
                    .contains("LWC_AGENT_START")
            );
        }
        if let Some(hook_path) = hook_path {
            let hook = fs::read_to_string(world.home.join(hook_path))
                .unwrap_or_else(|error| panic!("{target} hook file missing: {error}"));
            assert!(
                hook.contains("agent") && hook.contains("hook"),
                "{target} hook missing"
            );
            assert!(
                hook.contains("--scope all") || hook.contains("\"--scope\", \"all\""),
                "{target} boundary hook must load global and project strong Tags"
            );
        }
        if let Some(skill_dir) = skill_dir {
            assert_eq!(
                fs::read(world.home.join(skill_dir).join("SKILL.md")).unwrap(),
                fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/using-lwc/SKILL.md"))
                    .unwrap(),
                "{target} Skill differs from canonical"
            );
        }
        world.ok(&[
            "agent",
            "uninstall",
            "--target",
            target,
            "--location",
            "global",
            "--yes",
        ]);
        let after = directory_snapshot(&world.home)
            .into_iter()
            .filter(|(path, _)| !path.starts_with(".lwc"))
            .collect::<Vec<_>>();
        assert_eq!(after, before, "{target} did not round trip exactly");
    }
}

#[cfg(unix)]
#[test]
fn every_registered_target_is_strong_and_has_no_self_managed_gap() {
    let targets = [
        "claude",
        "cursor",
        "codex",
        "opencode",
        "hermes",
        "gemini",
        "antigravity",
        "kiro",
        "copilot-vscode",
        "copilot-cli",
        "copilot-jetbrains",
        "pi",
    ];
    for target in targets {
        let world = World::new();
        let result = world.ok(&[
            "agent",
            "install",
            "--target",
            target,
            "--location",
            "global",
            "--yes",
        ]);
        let receipt = &result["targets"][0];
        assert_eq!(receipt["adaptation"], "strong", "{target}");
        for surface in [
            "mcp",
            "skills",
            "instructions",
            "lifecycle_hook_mode",
            "permissions",
        ] {
            assert_ne!(receipt[surface], "self_managed", "{target} {surface}");
        }
        assert!(receipt["lifecycle_hook"].is_boolean(), "{target}");

        let printed = world.output(&[
            "agent",
            "install",
            "--print-config",
            target,
            "--location",
            "global",
        ]);
        assert!(printed.status.success(), "{target}");
        let printed = String::from_utf8(printed.stdout).unwrap();
        assert!(!printed.contains("self_managed"), "{target}: {printed}");
        assert!(!printed.contains("weak"), "{target}: {printed}");
    }
}

#[cfg(unix)]
#[test]
fn target_hooks_use_each_hosts_official_schema() {
    let cases = [
        ("cursor", ".cursor/hooks.json"),
        ("gemini", ".gemini/settings.json"),
        ("antigravity", ".gemini/config/plugins/lwc/hooks.json"),
        ("kiro", ".kiro/hooks/lwc.json"),
        ("copilot-cli", ".copilot/hooks/lwc.json"),
    ];
    for (target, path) in cases {
        let world = World::new();
        world.ok(&["agent", "install", "--target", target, "--yes"]);
        let value: Value =
            serde_json::from_slice(&fs::read(world.home.join(path)).unwrap()).unwrap();
        match target {
            "cursor" => {
                assert_eq!(value["version"], 1);
                assert!(
                    value["hooks"]["sessionStart"][0]["command"]
                        .as_str()
                        .unwrap()
                        .contains("--agent cursor")
                );
                assert!(
                    value["hooks"]["preCompact"][0]["command"]
                        .as_str()
                        .unwrap()
                        .contains("preCompact")
                );
            }
            "gemini" => assert!(
                value["hooks"]["SessionStart"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains("--agent gemini")
            ),
            "antigravity" => assert!(
                value["lwc"]["PreInvocation"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains("--agent antigravity")
            ),
            "kiro" => {
                assert_eq!(value["version"], "v1");
                assert_eq!(value["hooks"][0]["trigger"], "SessionStart");
                assert!(
                    value["hooks"][0]["action"]["command"]
                        .as_str()
                        .unwrap()
                        .contains("--event SessionStart")
                );
            }
            "copilot-cli" => {
                assert_eq!(value["version"], 1);
                assert_eq!(value["hooks"]["sessionStart"][0]["type"], "command");
            }
            _ => unreachable!(),
        }
    }

    let hermes = World::new();
    hermes.ok(&["agent", "install", "--target", "hermes", "--yes"]);
    let yaml = fs::read_to_string(hermes.home.join(".hermes/config.yaml")).unwrap();
    assert!(yaml.contains("hooks:\n  pre_llm_call:\n    - command:"));

    let opencode = World::new();
    opencode.ok(&["agent", "install", "--target", "opencode", "--yes"]);
    let plugin = fs::read_to_string(opencode.home.join(".config/opencode/plugins/lwc.js")).unwrap();
    assert!(plugin.contains("experimental.chat.system.transform"));
    assert!(plugin.contains("experimental.session.compacting"));
    assert!(plugin.contains("output.system.push"));
    assert!(plugin.contains("output.context.push"));
}

#[cfg(unix)]
#[test]
fn antigravity_installs_an_official_plugin_and_preserves_legacy_config() {
    let world = World::new();
    let plugin = world.home.join(".gemini/config/plugins/lwc");
    let unified = plugin.join("mcp_config.json");
    let legacy = world.home.join(".gemini/antigravity/mcp_config.json");
    fs::create_dir_all(unified.parent().unwrap()).unwrap();
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, r#"{"mcpServers":{"lwc":{"command":"lwc","args":["serve","--mcp"]},"keep":{"command":"keep"}}}"#).unwrap();

    let result = world.ok(&[
        "agent",
        "install",
        "--target",
        "antigravity",
        "--location",
        "global",
        "--yes",
    ]);
    let writes = result["targets"][0]["writes"].as_array().unwrap();
    assert!(writes.contains(&serde_json::json!(unified)));
    assert!(writes.contains(&serde_json::json!(plugin.join("plugin.json"))));
    assert!(
        writes
            .iter()
            .any(|path| path.as_str().unwrap().ends_with("/plugins/lwc/hooks.json"))
    );
    assert!(writes.iter().any(|path| {
        path.as_str()
            .unwrap()
            .ends_with("/plugins/lwc/skills/using-lwc/SKILL.md")
    }));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(plugin.join("plugin.json")).unwrap()).unwrap()["name"],
        "lwc"
    );
    let unified_config: Value = serde_json::from_slice(&fs::read(&unified).unwrap()).unwrap();
    #[cfg(target_os = "macos")]
    assert_eq!(
        unified_config["mcpServers"]["lwc"]["command"],
        world.home.join("bin/lwc").to_string_lossy().as_ref()
    );
    #[cfg(not(target_os = "macos"))]
    assert_eq!(unified_config["mcpServers"]["lwc"]["command"], "lwc");
    #[cfg(target_os = "macos")]
    let hooks: Value =
        serde_json::from_slice(&fs::read(plugin.join("hooks.json")).unwrap()).unwrap();
    #[cfg(target_os = "macos")]
    assert!(
        hooks["lwc"]["PreInvocation"][0]["command"]
            .as_str()
            .unwrap()
            .contains(world.home.join("bin/lwc").to_str().unwrap())
    );
    let legacy_config: Value = serde_json::from_slice(&fs::read(&legacy).unwrap()).unwrap();
    assert_eq!(legacy_config["mcpServers"]["lwc"]["command"], "lwc");
    assert_eq!(legacy_config["mcpServers"]["keep"]["command"], "keep");

    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "antigravity",
        "--location",
        "global",
        "--yes",
    ]);
    for path in [
        plugin.join("plugin.json"),
        plugin.join("mcp_config.json"),
        plugin.join("hooks.json"),
        plugin.join("rules/lwc.md"),
        plugin.join("skills/using-lwc/SKILL.md"),
    ] {
        assert!(!path.exists(), "{} survived uninstall", path.display());
    }
    assert_eq!(
        fs::read_to_string(legacy).unwrap(),
        r#"{"mcpServers":{"lwc":{"command":"lwc","args":["serve","--mcp"]},"keep":{"command":"keep"}}}"#
    );
}

#[cfg(target_os = "macos")]
#[test]
fn antigravity_hook_shell_quotes_the_resolved_lwc_path() {
    let world = World::new();
    let bin = world.home.join("bin$gui's tools");
    fs::create_dir(&bin).unwrap();
    let executable = bin.join("lwc");
    write_executable(&executable);
    let path =
        std::env::join_paths([bin, PathBuf::from("/usr/bin"), PathBuf::from("/bin")]).unwrap();
    let output = world
        .command()
        .env("PATH", path)
        .args([
            "agent",
            "install",
            "--target",
            "antigravity",
            "--location",
            "global",
            "--yes",
        ])
        .limited_output();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let hook: Value = serde_json::from_slice(
        &fs::read(world.home.join(".gemini/config/plugins/lwc/hooks.json")).unwrap(),
    )
    .unwrap();
    let command = hook["lwc"]["PreInvocation"][0]["command"].as_str().unwrap();
    assert!(
        Command::new("/bin/sh")
            .args(["-c", command])
            .env("PATH", "/usr/bin:/bin")
            .status()
            .unwrap()
            .success(),
        "generated hook was not shell-safe: {command}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn antigravity_refresh_upgrades_a_v6_bare_lwc_launcher() {
    let world = World::new();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "antigravity",
        "--location",
        "global",
        "--yes",
    ]);
    let config = world
        .home
        .join(".gemini/config/plugins/lwc/mcp_config.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    value["mcpServers"]["lwc"]["command"] = "lwc".into();
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    fs::write(&config, &bytes).unwrap();

    let manifest_path = world
        .home
        .join(".lwc/agent-installs/antigravity-global-global.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["version"] = 6.into();
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    for file in manifest["files"].as_array_mut().unwrap() {
        if file["path"] == config.to_string_lossy().as_ref() {
            file["post_hash"] = digest.clone().into();
        }
    }
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let refreshed = world.ok(&[
        "agent",
        "refresh",
        "--target",
        "antigravity",
        "--location",
        "global",
    ]);
    assert_eq!(refreshed["targets"][0]["status"], "installed");
    let value: Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
    assert_eq!(
        value["mcpServers"]["lwc"]["command"],
        world.home.join("bin/lwc").to_string_lossy().as_ref()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn antigravity_refresh_tracks_a_relocated_global_lwc() {
    let world = World::new();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "antigravity",
        "--location",
        "global",
        "--yes",
    ]);
    let replacement_bin = world.home.join("replacement-bin");
    fs::create_dir(&replacement_bin).unwrap();
    let replacement = replacement_bin.join("lwc");
    write_executable(&replacement);
    let path = std::env::join_paths([
        replacement_bin,
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ])
    .unwrap();

    let status = world
        .command()
        .env("PATH", &path)
        .args([
            "agent",
            "status",
            "--target",
            "antigravity",
            "--location",
            "global",
        ])
        .limited_output();
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["targets"][0]["status"], "modified");

    let output = world
        .command()
        .env("PATH", path)
        .args([
            "agent",
            "refresh",
            "--target",
            "antigravity",
            "--location",
            "global",
        ])
        .limited_output();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let refreshed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(refreshed["targets"][0]["status"], "installed");
    let value: Value = serde_json::from_slice(
        &fs::read(
            world
                .home
                .join(".gemini/config/plugins/lwc/mcp_config.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        value["mcpServers"]["lwc"]["command"],
        replacement.to_string_lossy().as_ref()
    );
}

#[cfg(unix)]
#[test]
fn antigravity_migrates_standalone_codegraph_into_the_lwc_plugin() {
    let world = World::new();
    let legacy = world.home.join(".gemini/antigravity/mcp_config.json");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(
        &legacy,
        r#"{"mcpServers":{"codegraph":{"command":"codegraph","args":["serve","--mcp"]},"keep":{"command":"keep"}}}"#,
    )
    .unwrap();

    world.ok(&[
        "agent",
        "install",
        "--target",
        "antigravity",
        "--location",
        "global",
        "--yes",
    ]);

    let migrated: Value = serde_json::from_slice(&fs::read(legacy).unwrap()).unwrap();
    assert!(migrated["mcpServers"].get("codegraph").is_none());
    assert_eq!(migrated["mcpServers"]["keep"]["command"], "keep");
}

#[cfg(unix)]
#[test]
fn uninstall_does_not_overwrite_a_recreated_legacy_path() {
    let world = World::new();
    let legacy = world.project.join(".cursor/rules/codegraph.mdc");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(
        &legacy,
        "---\ndescription: CodeGraph structural code intelligence\nalwaysApply: true\n---\n",
    )
    .unwrap();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "cursor",
        "--location",
        "local",
        "--yes",
    ]);
    assert!(!legacy.exists());

    fs::write(&legacy, "user replacement after install\n").unwrap();
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "cursor",
        "--location",
        "local",
        "--yes",
    ]);

    assert_eq!(
        fs::read_to_string(legacy).unwrap(),
        "user replacement after install\n"
    );
}

#[cfg(unix)]
#[test]
fn cursor_mcp_is_anchored_to_the_selected_workspace() {
    let local = World::new();
    let printed = local.output(&[
        "agent",
        "install",
        "--print-config",
        "cursor",
        "--location",
        "local",
    ]);
    assert!(printed.status.success());
    assert!(
        String::from_utf8(printed.stdout)
            .unwrap()
            .contains(local.project.canonicalize().unwrap().to_str().unwrap())
    );
    local.ok(&[
        "agent",
        "install",
        "--target",
        "cursor",
        "--location",
        "local",
        "--yes",
    ]);
    let config: Value =
        serde_json::from_slice(&fs::read(local.project.join(".cursor/mcp.json")).unwrap()).unwrap();
    assert_eq!(
        config["mcpServers"]["lwc"]["args"],
        serde_json::json!([
            "serve",
            "--mcp",
            "--path",
            local.project.canonicalize().unwrap()
        ])
    );
    let manifest_path = fs::read_dir(local.home.join(".lwc/agent-installs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("cursor-local-")
        })
        .unwrap();
    let manifest: Value = serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["mcp"]["args"], config["mcpServers"]["lwc"]["args"]);

    let global = World::new();
    let printed = global.output(&[
        "agent",
        "install",
        "--print-config",
        "cursor",
        "--location",
        "global",
    ]);
    assert!(printed.status.success());
    assert!(
        String::from_utf8(printed.stdout)
            .unwrap()
            .contains("${workspaceFolder}")
    );
    global.ok(&[
        "agent",
        "install",
        "--target",
        "cursor",
        "--location",
        "global",
        "--yes",
    ]);
    let config: Value =
        serde_json::from_slice(&fs::read(global.home.join(".cursor/mcp.json")).unwrap()).unwrap();
    assert_eq!(
        config["mcpServers"]["lwc"]["args"],
        serde_json::json!(["serve", "--mcp", "--path", "${workspaceFolder}"])
    );
}

#[cfg(unix)]
#[test]
fn cursor_print_config_escapes_the_workspace_path_as_json() {
    let world = World::new();
    let project = world.project.join("quoted\"workspace");
    fs::create_dir(&project).unwrap();
    let output = world
        .command()
        .current_dir(&project)
        .args([
            "agent",
            "install",
            "--print-config",
            "cursor",
            "--location",
            "local",
        ])
        .limited_output();
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    let mcp: Value = serde_json::from_str(
        output
            .lines()
            .find_map(|line| line.strip_prefix("MCP: "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        mcp["mcpServers"]["lwc"]["args"][3],
        project.canonicalize().unwrap().to_string_lossy().as_ref()
    );
}

#[cfg(unix)]
#[test]
fn copilot_vscode_hook_uses_the_vscode_event_and_command_schema() {
    let world = World::new();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode",
        "--location",
        "local",
        "--yes",
    ]);
    let hook: Value =
        serde_json::from_slice(&fs::read(world.project.join(".github/hooks/lwc.json")).unwrap())
            .unwrap();
    let entry = &hook["hooks"]["SessionStart"][0];
    assert!(
        entry["command"]
            .as_str()
            .unwrap()
            .contains("--agent copilot-vscode")
    );
    assert!(
        entry["windows"]
            .as_str()
            .unwrap()
            .contains("--agent copilot-vscode")
    );
    assert!(hook["hooks"].get("sessionStart").is_none());
}

#[cfg(unix)]
#[test]
fn copilot_vscode_replaces_its_legacy_camel_case_hook() {
    let world = World::new();
    let hook = world.project.join(".github/hooks/lwc.json");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(
        &hook,
        r#"{"version":1,"hooks":{"sessionStart":[{"type":"command","bash":"lwc --scope all agent hook --agent copilot-vscode --event sessionStart"}],"SessionEnd":[{"type":"command","command":"keep"}]}}"#,
    )
    .unwrap();

    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode",
        "--location",
        "local",
        "--yes",
    ]);

    let value: Value = serde_json::from_slice(&fs::read(hook).unwrap()).unwrap();
    assert!(value["hooks"].get("sessionStart").is_none());
    assert_eq!(value["hooks"]["SessionEnd"][0]["command"], "keep");
    assert!(
        value["hooks"]["SessionStart"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--agent copilot-vscode")
    );
}

#[cfg(unix)]
#[test]
fn all_targets_install_together_idempotently_and_uninstall_shared_files_exactly() {
    let world = World::new();
    let shared_skill = world.home.join(".copilot/skills/using-lwc/SKILL.md");
    fs::create_dir_all(shared_skill.parent().unwrap()).unwrap();
    fs::write(
        &shared_skill,
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/using-lwc/SKILL.md")).unwrap(),
    )
    .unwrap();
    let global_before = directory_snapshot(&world.home);
    let installed = world.ok(&[
        "agent",
        "install",
        "--target",
        "all",
        "--location",
        "global",
        "--yes",
    ]);
    assert_eq!(installed["targets"].as_array().unwrap().len(), 12);
    assert!(
        installed["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|target| target["status"] == "installed")
    );
    let reinstalled = world.ok(&[
        "agent",
        "install",
        "--target",
        "all",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(
        reinstalled["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|target| target["status"] == "unchanged")
    );
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "all",
        "--location",
        "global",
        "--yes",
    ]);
    let global_after = directory_snapshot(&world.home)
        .into_iter()
        .filter(|(path, _)| !path.starts_with(".lwc"))
        .collect::<Vec<_>>();
    assert_eq!(global_after, global_before);

    fs::write(
        world.project.join("AGENTS.md"),
        "user project instructions\n",
    )
    .unwrap();
    let local_before = directory_snapshot(&world.project);
    let installed = world.ok(&[
        "agent",
        "install",
        "--target",
        "all",
        "--location",
        "local",
        "--yes",
    ]);
    assert_eq!(installed["targets"].as_array().unwrap().len(), 12);
    let unsupported = installed["targets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|target| target["status"] == "unsupported")
        .map(|target| target["target"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(unsupported.is_empty());
    let cursor_rule = fs::read_to_string(world.project.join(".cursor/rules/lwc.mdc")).unwrap();
    assert!(cursor_rule.contains("alwaysApply: true"));
    assert!(cursor_rule.contains("LWC_AGENT_START"));
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "all",
        "--location",
        "local",
        "--yes",
    ]);
    let local_after = directory_snapshot(&world.project)
        .into_iter()
        .filter(|(path, _)| !path.starts_with(".lwc"))
        .collect::<Vec<_>>();
    assert_eq!(local_after, local_before);
}

#[cfg(unix)]
#[test]
fn copilot_vscode_preserves_jsonc_and_removes_only_owned_content() {
    let world = World::new();
    let mcp = world.project.join(".vscode/mcp.json");
    fs::create_dir_all(mcp.parent().unwrap()).unwrap();
    fs::write(
        &mcp,
        "{\n  // user server\n  \"servers\": {\n    \"keep\": { \"command\": \"keep\", },\n  },\n}\n",
    )
    .unwrap();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode",
        "--location",
        "local",
        "--yes",
    ]);
    let installed = fs::read_to_string(&mcp).unwrap();
    assert!(installed.contains("// user server"));
    assert!(installed.contains("\"keep\""));
    assert!(installed.contains("\"lwc\""));
    let first_install = directory_snapshot(&world.home);
    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode",
        "--location",
        "local",
        "--yes",
    ]);
    assert_eq!(directory_snapshot(&world.home), first_install);

    fs::write(&mcp, format!("{installed}// user edit after install\n")).unwrap();
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "copilot-vscode",
        "--location",
        "local",
        "--yes",
    ]);

    let uninstalled = fs::read_to_string(mcp).unwrap();
    assert!(uninstalled.contains("// user server"));
    assert!(uninstalled.contains("// user edit after install"));
    assert!(uninstalled.contains("\"keep\""));
    assert!(!uninstalled.contains("\"lwc\""));
    assert!(
        !world
            .home
            .join(".copilot/instructions/lwc-vscode.instructions.md")
            .exists()
    );
    assert!(
        !world
            .home
            .join(".config/github-copilot/intellij/global-copilot-instructions.md")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn copilot_vscode_uninstall_preserves_a_preexisting_lwc_entry_after_user_edits() {
    let world = World::new();
    let config = world.project.join(".vscode/mcp.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "{\n  // user-owned LWC\n  \"servers\": {\n    \"lwc\": { \"type\": \"stdio\", \"command\": \"lwc\", \"args\": [\"serve\", \"--mcp\"] },\n  },\n}\n",
    )
    .unwrap();

    world.ok(&[
        "agent",
        "install",
        "--target",
        "copilot-vscode",
        "--location",
        "local",
        "--yes",
    ]);
    let installed = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{installed}// user edit after install\n")).unwrap();
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "copilot-vscode",
        "--location",
        "local",
        "--yes",
    ]);

    let uninstalled = fs::read_to_string(config).unwrap();
    assert!(uninstalled.contains("// user-owned LWC"));
    assert!(uninstalled.contains("// user edit after install"));
    assert!(uninstalled.contains("\"lwc\""));
    assert!(uninstalled.contains("\"command\": \"lwc\""));
}

#[cfg(unix)]
#[test]
fn hermes_enables_the_lwc_toolset_and_preserves_user_yaml() {
    let world = World::new();
    let config = world.home.join(".hermes/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "theme: dark\nmcp_servers:\n  keep:\n    command: keep\nplatform_toolsets:\n  cli:\n  - hermes-cli\n  - browser\n",
    )
    .unwrap();

    world.ok(&[
        "agent",
        "install",
        "--target",
        "hermes",
        "--location",
        "global",
        "--yes",
    ]);
    let installed = fs::read_to_string(&config).unwrap();
    assert!(installed.contains("  lwc:"));
    assert_eq!(installed.matches("- mcp-lwc").count(), 1);
    assert!(installed.contains("  - browser"));
    let first_install = directory_snapshot(&world.home);
    world.ok(&[
        "agent",
        "install",
        "--target",
        "hermes",
        "--location",
        "global",
        "--yes",
    ]);
    assert_eq!(directory_snapshot(&world.home), first_install);

    fs::write(&config, format!("{installed}user_after: true\n")).unwrap();
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "hermes",
        "--location",
        "global",
        "--yes",
    ]);
    let uninstalled = fs::read_to_string(config).unwrap();
    assert!(uninstalled.contains("theme: dark"));
    assert!(uninstalled.contains("  keep:"));
    assert!(uninstalled.contains("  - browser"));
    assert!(uninstalled.contains("user_after: true"));
    assert!(!uninstalled.contains("  lwc:"));
    assert!(!uninstalled.contains("mcp-lwc"));
    assert!(!uninstalled.contains("agent hook --agent hermes"));
}

#[cfg(unix)]
#[test]
fn opencode_global_install_uses_the_official_xdg_config_home() {
    let world = World::new();
    let xdg = world.home.join("custom-xdg");
    let output = world
        .command()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "agent",
            "install",
            "--target",
            "opencode",
            "--location",
            "global",
            "--yes",
        ])
        .limited_output();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for relative in ["opencode.jsonc", "AGENTS.md"] {
        assert!(xdg.join("opencode").join(relative).is_file(), "{relative}");
    }
    assert!(xdg.join("opencode/plugins/lwc.js").is_file());
    assert!(xdg.join("opencode/skills/using-lwc/SKILL.md").is_file());
    assert!(!world.home.join(".config/opencode/opencode.jsonc").exists());
}

#[cfg(unix)]
#[test]
fn opencode_preserves_jsonc_and_removes_only_owned_content() {
    let world = World::new();
    let config = world.home.join(".config/opencode/opencode.jsonc");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "{\n  // user config\n  \"mcp\": { \"keep\": { \"command\": [\"keep\"], }, },\n  \"permission\": { \"keep_*\": \"ask\", },\n}\n",
    )
    .unwrap();

    world.ok(&[
        "agent",
        "install",
        "--target",
        "opencode",
        "--location",
        "global",
        "--yes",
    ]);
    let installed = fs::read_to_string(&config).unwrap();
    assert!(installed.contains("// user config"));
    assert!(installed.contains("\"keep\""));
    assert!(installed.contains("\"lwc\""));
    assert!(!installed.contains("\"lwc_*\""));
    let first_install = directory_snapshot(&world.home);
    world.ok(&[
        "agent",
        "install",
        "--target",
        "opencode",
        "--location",
        "global",
        "--yes",
    ]);
    assert_eq!(directory_snapshot(&world.home), first_install);

    fs::write(&config, format!("{installed}// user edit after install\n")).unwrap();
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "opencode",
        "--location",
        "global",
        "--yes",
    ]);
    let uninstalled = fs::read_to_string(config).unwrap();
    assert!(uninstalled.contains("// user config"));
    assert!(uninstalled.contains("// user edit after install"));
    assert!(uninstalled.contains("\"keep\""));
    assert!(uninstalled.contains("\"keep_*\""));
    assert!(!uninstalled.contains("\"lwc\""));
    assert!(!uninstalled.contains("\"lwc_*\""));
}

#[cfg(unix)]
#[test]
fn opencode_uninstall_preserves_a_preexisting_lwc_entry_after_user_edits() {
    let world = World::new();
    let config = world.home.join(".config/opencode/opencode.jsonc");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "{\n  // user-owned LWC\n  \"mcp\": {\n    \"lwc\": { \"type\": \"local\", \"command\": [\"lwc\", \"serve\", \"--mcp\"], \"enabled\": true },\n  },\n}\n",
    )
    .unwrap();

    world.ok(&["agent", "install", "--target", "opencode", "--yes"]);
    let installed = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{installed}// user edit after install\n")).unwrap();
    world.ok(&["agent", "uninstall", "--target", "opencode", "--yes"]);

    let uninstalled = fs::read_to_string(config).unwrap();
    assert!(uninstalled.contains("// user-owned LWC"));
    assert!(uninstalled.contains("// user edit after install"));
    assert!(uninstalled.contains("\"lwc\""));
    assert!(uninstalled.contains("\"command\": [\"lwc\", \"serve\", \"--mcp\"]"));
}

#[cfg(unix)]
#[test]
fn every_print_config_is_pure_and_global_lwc_is_required_before_writes() {
    let world = World::new();
    let before = directory_snapshot(&world.home);
    for target in [
        "claude",
        "cursor",
        "codex",
        "opencode",
        "hermes",
        "gemini",
        "antigravity",
        "kiro",
        "copilot-vscode",
        "copilot-cli",
        "copilot-jetbrains",
        "pi",
    ] {
        let output = world.output(&["agent", "install", "--print-config", target]);
        assert!(
            output.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8(output.stdout).unwrap().contains("lwc"));
    }

    let hermes = world.output(&["agent", "install", "--print-config", "hermes"]);
    let hermes = String::from_utf8(hermes.stdout).unwrap();
    assert!(hermes.contains("mcp_servers:"));
    assert!(hermes.contains("platform_toolsets:"));
    assert!(hermes.contains("- mcp-lwc"));

    let antigravity = world.output(&["agent", "install", "--print-config", "antigravity"]);
    let antigravity = String::from_utf8(antigravity.stdout).unwrap();
    assert!(antigravity.contains("mcp_config.json"));
    assert!(!antigravity.contains("\"type\":\"stdio\""));
    #[cfg(target_os = "macos")]
    assert!(
        antigravity
            .lines()
            .find(|line| line.starts_with("Hook: "))
            .unwrap()
            .contains(world.home.join("bin/lwc").to_str().unwrap())
    );
    let gemini = world.output(&["agent", "install", "--print-config", "gemini"]);
    let gemini = String::from_utf8(gemini.stdout).unwrap();
    assert!(!gemini.contains("\"type\":\"stdio\""));
    assert!(gemini.contains("MCP: {\"mcpServers\":{\"lwc\":"));
    assert!(antigravity.contains("MCP: {\"mcpServers\":{\"lwc\":"));
    let opencode = world.output(&["agent", "install", "--print-config", "opencode"]);
    let opencode = String::from_utf8(opencode.stdout).unwrap();
    assert!(opencode.contains("MCP: {\"mcp\":{\"lwc\":"));
    assert!(opencode.contains("\"command\":[\"lwc\",\"serve\",\"--mcp\"]"));
    assert_eq!(directory_snapshot(&world.home), before);

    let output = world
        .command()
        .env("PATH", "/usr/bin:/bin")
        .args(["agent", "install", "--target", "codex", "--yes"])
        .limited_output();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("lwc_command_not_global"));
    assert_eq!(directory_snapshot(&world.home), before);
}

#[cfg(unix)]
#[test]
fn unsupported_scopes_and_printed_capabilities_are_truthful() {
    let world = World::new();

    for target in ["hermes", "copilot-jetbrains"] {
        let status = world.ok(&["agent", "status", "--target", target, "--location", "local"]);
        let target_status = &status["targets"][0];
        assert_eq!(target_status["status"], "not_installed");
        assert_eq!(
            target_status["mcp"],
            if target == "copilot-jetbrains" {
                "user_managed"
            } else {
                "unsupported"
            }
        );
        assert_eq!(
            target_status["instructions"],
            if target == "copilot-jetbrains" {
                "configured_preview"
            } else {
                "installed"
            }
        );
        assert_eq!(target_status["lifecycle_hook_mode"], "unsupported");
        assert!(!target_status["paths"].as_array().unwrap().is_empty());

        let printed = world.output(&[
            "agent",
            "install",
            "--print-config",
            target,
            "--location",
            "local",
        ]);
        assert!(printed.status.success());
        assert!(
            String::from_utf8(printed.stdout)
                .unwrap()
                .contains("LWC_AGENT_START")
        );
    }

    let copilot = world.ok(&[
        "agent",
        "status",
        "--target",
        "copilot-cli",
        "--location",
        "local",
    ]);
    let copilot = &copilot["targets"][0];
    assert_eq!(copilot["status"], "not_installed");
    assert_eq!(copilot["mcp"], "installed");
    assert_eq!(copilot["skills"], "installed");
    assert_eq!(copilot["instructions"], "installed");
    assert_eq!(copilot["lifecycle_hook_mode"], "installed");

    for target in ["kiro", "copilot-vscode"] {
        let status = world.ok(&["agent", "status", "--target", target, "--location", "local"]);
        assert_eq!(
            status["targets"][0]["lifecycle_hook_mode"], "configured_preview",
            "{target} must expose its host-version-gated Hook truthfully"
        );
    }

    let cursor = String::from_utf8(
        world
            .output(&[
                "agent",
                "install",
                "--print-config",
                "cursor",
                "--location",
                "global",
            ])
            .stdout,
    )
    .unwrap();
    assert!(cursor.contains("no official global rules file"));
    assert!(cursor.contains("Hook: ~/.cursor/hooks.json"));

    let kiro = String::from_utf8(
        world
            .output(&[
                "agent",
                "install",
                "--print-config",
                "kiro",
                "--location",
                "global",
            ])
            .stdout,
    )
    .unwrap();
    assert!(kiro.contains("hooks/lwc.json SessionStart"));
    assert!(!kiro.contains("autoApprove"));
}

#[cfg(unix)]
#[test]
fn prompt_hook_opt_out_and_unsafe_marker_paths_fail_closed() {
    use std::os::unix::fs::symlink;

    let world = World::new();
    fs::create_dir_all(world.home.join(".claude")).unwrap();
    fs::write(
        world.home.join(".claude/settings.json"),
        "{\"hooks\":{\"UserPromptSubmit\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"keep-hook\"}]}]}}\n",
    )
    .unwrap();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "claude",
        "--yes",
        "--no-prompt-hook",
    ]);
    let settings = fs::read_to_string(world.home.join(".claude/settings.json")).unwrap();
    assert!(!settings.contains("codegraph prompt-hook"));
    assert!(!settings.contains("agent hook --agent claude --event UserPromptSubmit"));
    assert!(settings.contains("keep-hook"));

    let malformed = World::new();
    fs::create_dir_all(malformed.home.join(".codex")).unwrap();
    fs::write(
        malformed.home.join(".codex/AGENTS.md"),
        "<!-- LWC_AGENT_START -->\nbroken\n",
    )
    .unwrap();
    let output = malformed.output(&["agent", "install", "--target", "codex", "--yes"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("agent_marker_invalid"));

    let linked = World::new();
    fs::create_dir_all(linked.home.join(".codex")).unwrap();
    let outside = linked.home.join("outside.toml");
    fs::write(&outside, "keep\n").unwrap();
    symlink(&outside, linked.home.join(".codex/config.toml")).unwrap();
    let output = linked.output(&["agent", "install", "--target", "codex", "--yes"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe_agent_config"));
    assert_eq!(fs::read_to_string(outside).unwrap(), "keep\n");

    let linked_parent = World::new();
    let outside = linked_parent.home.join("outside");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, linked_parent.home.join(".agents")).unwrap();
    let output = linked_parent.output(&["agent", "install", "--target", "codex", "--yes"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe_agent_config"));
    assert!(directory_snapshot(&outside).is_empty());

    let linked_existing_child = World::new();
    let outside = linked_existing_child.home.join("outside");
    fs::create_dir_all(outside.join("skills/using-lwc")).unwrap();
    symlink(&outside, linked_existing_child.home.join(".agents")).unwrap();
    let output = linked_existing_child.output(&["agent", "install", "--target", "codex", "--yes"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe_agent_config"));
    assert!(directory_snapshot(&outside.join("skills/using-lwc")).is_empty());
}

#[cfg(unix)]
#[test]
fn claude_prompt_hook_option_updates_an_existing_install() {
    let world = World::new();
    let settings = world.home.join(".claude/settings.json");

    world.ok(&[
        "agent",
        "install",
        "--target",
        "claude",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(
        fs::read_to_string(&settings)
            .unwrap()
            .contains("UserPromptSubmit")
    );

    world.ok(&[
        "agent",
        "install",
        "--target",
        "claude",
        "--location",
        "global",
        "--yes",
        "--no-prompt-hook",
    ]);
    assert!(
        !fs::read_to_string(&settings)
            .unwrap()
            .contains("UserPromptSubmit")
    );

    world.ok(&[
        "agent",
        "refresh",
        "--target",
        "claude",
        "--location",
        "global",
    ]);
    assert!(
        !fs::read_to_string(&settings)
            .unwrap()
            .contains("UserPromptSubmit")
    );

    world.ok(&[
        "agent",
        "install",
        "--target",
        "claude",
        "--location",
        "global",
        "--yes",
    ]);
    assert!(
        fs::read_to_string(settings)
            .unwrap()
            .contains("UserPromptSubmit")
    );
}

fn directory_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(root: &Path, current: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}
