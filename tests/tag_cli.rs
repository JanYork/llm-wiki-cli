use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct World {
    _temp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        let world = Self {
            _temp: temp,
            project,
            home,
        };
        world.ok(&["init"]);
        world
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

    fn output(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .args(args)
            .output()
            .unwrap()
    }

    fn page(&self, slug: &str, body: &str) {
        let path = self.project.join(format!("{slug}.md"));
        fs::write(&path, body).unwrap();
        self.ok(&[
            "page",
            "put",
            slug,
            "--title",
            slug,
            "--file",
            path.to_str().unwrap(),
            "--provenance",
            "user-provided",
        ]);
    }
}

#[test]
fn tag_policy_cascades_and_checkpoint_restore_are_exact() {
    let world = World::new();
    world.page("rule", "rule body");
    world.ok(&[
        "tag",
        "set",
        "Rules",
        "rule",
        "--reason",
        "core",
        "--priority",
        "7",
    ]);
    let policy = world.ok(&[
        "tag",
        "autoload",
        "Rules",
        "--enable",
        "--priority",
        "100",
        "--limit",
        "3",
        "--max-chars",
        "4096",
        "--reason",
        "session rules",
    ]);
    assert_eq!(policy["policy"]["autoload"], true);
    assert_eq!(world.ok(&["tag", "list"])["tags"][0]["pages"], 1);
    world.ok(&["checkpoint", "create", "tagged"]);

    world.ok(&["tag", "delete", "Rules"]);
    world.ok(&["checkpoint", "restore", "tagged"]);
    assert_eq!(world.ok(&["load", "tag", "Rules"])["returned"], 1);

    world.ok(&["page", "remove", "rule"]);
    let empty = world.ok(&["load", "tag", "Rules"]);
    assert_eq!(empty["returned"], 0);
    assert_eq!(world.ok(&["tag", "delete", "Rules"])["action"], "deleted");
}

#[test]
fn scope_all_tag_load_keeps_duplicate_slugs_and_prefers_project_on_ties() {
    let world = World::new();
    world.page("shared", "project body");
    let file = world.project.join("global.md");
    fs::write(&file, "global body").unwrap();
    world.ok(&["--scope", "global", "init"]);
    world.ok(&[
        "--scope",
        "global",
        "page",
        "put",
        "shared",
        "--title",
        "shared",
        "--file",
        file.to_str().unwrap(),
        "--provenance",
        "user-provided",
    ]);
    for scope in ["project", "global"] {
        world.ok(&[
            "--scope",
            scope,
            "tag",
            "set",
            "Rules",
            "shared",
            "--priority",
            "10",
            "--reason",
            "core",
        ]);
    }

    let loaded = world.ok(&["--scope", "all", "load", "tag", "Rules", "--limit", "2"]);
    assert_eq!(loaded["returned"], 2);
    assert_eq!(loaded["pages"][0]["scope"], "project");
    assert_eq!(loaded["pages"][1]["scope"], "global");
    assert_eq!(loaded["pages"][0]["page"]["slug"], "shared");
    assert_eq!(loaded["pages"][1]["page"]["slug"], "shared");

    let listed = world.ok(&["--scope", "all", "tag", "list"]);
    assert_eq!(listed["stores"].as_array().unwrap().len(), 2);
    assert_eq!(listed["stores"][0]["scope"], "project");
    assert_eq!(listed["stores"][1]["scope"], "global");

    let rejected = world.output(&[
        "--scope", "all", "tag", "set", "Rules", "shared", "--reason", "no",
    ]);
    assert!(!rejected.status.success());
}

#[test]
fn tag_load_is_ordered_complete_bounded_and_page_replacement_safe() {
    let world = World::new();
    world.page("alpha", "alpha body");
    world.page("beta", "beta body");
    assert_eq!(
        world.ok(&[
            "tag",
            "set",
            "Rules",
            "alpha",
            "--priority",
            "10",
            "--reason",
            "core",
        ])["action"],
        "created"
    );
    world.ok(&[
        "tag",
        "set",
        "Rules",
        "beta",
        "--priority",
        "20",
        "--reason",
        "core",
    ]);

    let first = world.ok(&["load", "tag", "Rules", "--limit", "1"]);
    assert_eq!(first["pages"][0]["page"]["slug"], "beta");
    assert_eq!(first["pages"][0]["page"]["body"], "beta body");
    assert_eq!(first["has_more"], true);
    assert_eq!(first["returned"], 1);

    world.page("beta", "replacement body");
    let replaced = world.ok(&["load", "tag", "Rules", "--limit", "1"]);
    assert_eq!(replaced["pages"][0]["page"]["body"], "replacement body");
}

#[test]
fn tag_changeset_commit_and_rollback_preserve_tag_state() {
    let world = World::new();
    world.page("rule", "rule body");
    world.ok(&["changeset", "begin", "tag-change"]);
    world.ok(&[
        "--changeset",
        "tag-change",
        "tag",
        "set",
        "Rules",
        "rule",
        "--priority",
        "5",
        "--reason",
        "required",
    ]);
    let committed = world.ok(&[
        "changeset",
        "commit",
        "tag-change",
        "--allow-lint-issues",
        "--reason",
        "test fixture has an intentionally isolated page",
    ]);
    assert_eq!(world.ok(&["load", "tag", "Rules"])["returned"], 1);

    world.ok(&[
        "changeset",
        "rollback",
        committed["changeset_id"].as_str().unwrap(),
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["load", "tag", "Rules"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "tag_not_found");
}

#[test]
fn tag_changeset_delete_restores_memberships_and_detects_live_conflicts() {
    let world = World::new();
    world.page("rule", "rule body");
    world.ok(&[
        "tag",
        "set",
        "Rules",
        "rule",
        "--priority",
        "1",
        "--reason",
        "base",
    ]);

    world.ok(&["changeset", "begin", "delete-tag"]);
    world.ok(&["--changeset", "delete-tag", "tag", "delete", "Rules"]);
    let committed = world.ok(&[
        "changeset",
        "commit",
        "delete-tag",
        "--allow-lint-issues",
        "--reason",
        "isolated test page",
    ]);
    assert!(!world.output(&["load", "tag", "Rules"]).status.success());
    world.ok(&[
        "changeset",
        "rollback",
        committed["changeset_id"].as_str().unwrap(),
    ]);
    assert_eq!(world.ok(&["load", "tag", "Rules"])["returned"], 1);

    world.ok(&["changeset", "begin", "conflict"]);
    world.ok(&[
        "--changeset",
        "conflict",
        "tag",
        "set",
        "Rules",
        "rule",
        "--priority",
        "2",
        "--reason",
        "draft",
    ]);
    world.ok(&[
        "tag",
        "set",
        "Rules",
        "rule",
        "--priority",
        "3",
        "--reason",
        "live",
    ]);
    let failed = world.output(&[
        "changeset",
        "commit",
        "conflict",
        "--allow-lint-issues",
        "--reason",
        "isolated test page",
    ]);
    assert!(!failed.status.success());
    let error: Value = serde_json::from_slice(&failed.stderr).unwrap();
    assert_eq!(error["error"]["code"], "changeset_conflict");
    assert_eq!(
        world.ok(&["load", "tag", "Rules"])["pages"][0]["priority"],
        3
    );
}
