use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

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
            .env("PATH", "/usr/bin:/bin")
            .env("LWC_CODEGRAPH_BINARY", &self.backend)
            .env("LWC_FAKE_LOG", &self.log);
        command
    }

    fn output(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
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
    let claude_mcp = world.home.join(".claude.json");
    let claude_settings = world.home.join(".claude/settings.json");
    let claude_md = world.home.join(".claude/CLAUDE.md");
    fs::write(&codex_config, "model = \"test\"\n").unwrap();
    fs::write(&codex_agents, "user codex instructions\n").unwrap();
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
        (&claude_mcp, fs::read(&claude_mcp).unwrap()),
        (&claude_settings, fs::read(&claude_settings).unwrap()),
        (&claude_md, fs::read(&claude_md).unwrap()),
    ];

    let installed = world.ok(&["agent", "install", "--yes"]);
    assert_eq!(installed["location"], "global");
    assert_eq!(installed["targets"].as_array().unwrap().len(), 2);
    let executable = env!("CARGO_BIN_EXE_lwc");
    let codex = fs::read_to_string(&codex_config).unwrap();
    assert!(codex.contains(&format!("command = \"{executable}\"")));
    assert!(codex.contains("args = [\"cg\", \"serve\", \"--mcp\"]"));
    let claude: Value = serde_json::from_slice(&fs::read(&claude_mcp).unwrap()).unwrap();
    assert_eq!(claude["mcpServers"]["sibling"]["command"], "keep");
    assert_eq!(claude["mcpServers"]["codegraph"]["command"], executable);
    assert_eq!(
        claude["mcpServers"]["codegraph"]["args"],
        serde_json::json!(["cg", "serve", "--mcp"])
    );
    let settings: Value = serde_json::from_slice(&fs::read(&claude_settings).unwrap()).unwrap();
    let settings_text = settings.to_string();
    assert!(settings_text.contains("keep-hook"));
    assert!(settings_text.contains("UserPromptSubmit"));
    assert!(settings_text.contains("SessionStart"));
    assert!(!settings_text.contains("codegraph prompt-hook"));
    assert!(settings_text.contains("agent hook --agent claude"));
    assert!(world.home.join(".codex/hooks/lwc.json").is_file());
    let codex_guidance = fs::read_to_string(&codex_agents).unwrap();
    let claude_guidance = fs::read_to_string(&claude_md).unwrap();
    assert!(codex_guidance.contains("LWC_AGENT_START"));
    assert!(claude_guidance.contains("LWC_AGENT_START"));
    assert_onboarding_guidance(&codex_guidance);
    assert_onboarding_guidance(&claude_guidance);

    let installed_bytes = [
        fs::read(&codex_config).unwrap(),
        fs::read(&codex_agents).unwrap(),
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
    assert!(!world.home.join(".codex/hooks/lwc.json").exists());
}

#[cfg(unix)]
#[test]
fn yes_detects_agent_executables_before_their_first_config() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new();
    let bin = world.home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for agent in ["claude", "codex", "pi"] {
        let executable = bin.join(agent);
        fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path =
        std::env::join_paths([bin, PathBuf::from("/usr/bin"), PathBuf::from("/bin")]).unwrap();
    let output = world
        .command()
        .env("PATH", path)
        .args(["agent", "install", "--yes"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let installed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        installed["detected"],
        serde_json::json!(["claude", "codex", "pi"])
    );
    assert_eq!(installed["targets"].as_array().unwrap().len(), 3);
}

#[cfg(unix)]
#[test]
fn refresh_rebases_uninstall_snapshot_without_losing_user_edits() {
    let world = World::new();
    let guidance = world.home.join(".pi/agent/extensions/lwc-guidance.md");

    world.ok(&[
        "agent",
        "install",
        "--target",
        "pi",
        "--location",
        "global",
        "--yes",
    ]);
    let installed = fs::read_to_string(&guidance).unwrap();
    fs::write(&guidance, format!("user instructions\n{installed}")).unwrap();

    world.ok(&["agent", "refresh", "--target", "pi", "--location", "global"]);
    world.ok(&[
        "agent",
        "uninstall",
        "--target",
        "pi",
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
fn print_config_is_pure_and_pi_is_an_explicit_no_mcp_lifecycle_target() {
    let world = World::new();
    let before = directory_snapshot(&world.home);
    let printed = world.output(&["agent", "install", "--print-config", "codex"]);
    assert!(printed.status.success());
    let text = String::from_utf8(printed.stdout).unwrap();
    assert!(text.contains("mcp_servers.codegraph"));
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
    assert_eq!(pi["targets"][0]["mcp"], "unsupported");
    assert!(world.project.join(".pi/extensions/lwc.js").is_file());
    let extension = fs::read_to_string(world.project.join(".pi/extensions/lwc.js")).unwrap();
    assert!(extension.contains("session_start"));
    assert!(extension.contains("session_before_compact"));
    assert!(extension.contains("before_agent_start"));
    assert!(extension.contains("agent hook --agent pi"));
}

#[cfg(unix)]
#[test]
fn every_advertised_global_target_round_trips_and_capability_gaps_are_explicit() {
    for target in [
        "claude",
        "cursor",
        "codex",
        "opencode",
        "hermes",
        "gemini",
        "antigravity",
        "kiro",
        "pi",
    ] {
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
        assert_eq!(
            result["targets"][0]["mcp"],
            if target == "pi" {
                "unsupported"
            } else {
                "installed"
            }
        );
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
fn prompt_hook_opt_out_and_unsafe_marker_paths_fail_closed() {
    use std::os::unix::fs::symlink;

    let world = World::new();
    fs::create_dir_all(world.home.join(".claude")).unwrap();
    fs::write(world.home.join(".claude/settings.json"), "{\"hooks\":{}}\n").unwrap();
    world.ok(&[
        "agent",
        "install",
        "--target",
        "claude",
        "--yes",
        "--no-codegraph-prompt-hook",
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
