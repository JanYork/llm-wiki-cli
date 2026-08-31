use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
use tempfile::TempDir;

struct TestWorld {
    _temp: TempDir,
    project: PathBuf,
    home: PathBuf,
}

impl TestWorld {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self {
            _temp: temp,
            project,
            home,
        }
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.command(args);
        assert!(
            output.status.success(),
            "command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn err(&self, args: &[&str]) -> Value {
        let output = self.command(args);
        assert!(
            !output.status.success(),
            "command {args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stderr).unwrap()
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.project.join(relative);
        fs::write(&path, content).unwrap();
        path
    }

    fn put_page(&self, scope: &[&str], slug: &str, title: &str, summary: &str, body: &str) {
        let file = self.write(&format!("{slug}.md"), body);
        let mut args = scope.to_vec();
        args.extend([
            "page",
            "put",
            slug,
            "--title",
            title,
            "--summary",
            summary,
            "--file",
            file.to_str().unwrap(),
            "--provenance",
            "agent-observed",
        ]);
        self.ok(&args);
    }
}

fn issue<'a>(lint: &'a Value, code: &str) -> &'a Value {
    lint["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == code)
        .unwrap_or_else(|| panic!("missing {code}: {lint}"))
}

#[test]
fn lint_adds_severity_counts_without_changing_total_or_code_counts() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.put_page(
        &[],
        "structured",
        "Canonical title",
        "A complete summary.",
        "# Different title\n\n# Canonical title\n\n### Skipped level\n\n[[structured]]\n",
    );

    let lint = world.ok(&["lint", "--limit", "1"]);
    assert_eq!(lint["total"], 3, "{lint}");
    assert_eq!(lint["blocking_total"], 0, "{lint}");
    assert_eq!(lint["advisory_total"], 3, "{lint}");
    assert_eq!(lint["issues"].as_array().unwrap().len(), 1);
    assert!(lint["has_more"].as_bool().unwrap());
    assert_eq!(
        lint["counts"]["body_h1_title_mismatch"], 1,
        "legacy counts remain code-based: {lint}"
    );
    assert_eq!(lint["counts"]["duplicate_body_h1"], 1, "{lint}");
    assert_eq!(lint["counts"]["heading_level_jump"], 1, "{lint}");

    let complete = world.ok(&["lint"]);
    for code in [
        "body_h1_title_mismatch",
        "duplicate_body_h1",
        "heading_level_jump",
    ] {
        assert_eq!(issue(&complete, code)["severity"], "warning");
    }
}

#[test]
fn legacy_integrity_issues_remain_blocking_errors() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.put_page(
        &[],
        "missing-summary",
        "Missing summary",
        "",
        "[[missing-summary]]\n",
    );

    let lint = world.ok(&["lint"]);
    assert_eq!(lint["total"], 1, "{lint}");
    assert_eq!(lint["blocking_total"], 1, "{lint}");
    assert_eq!(lint["advisory_total"], 0, "{lint}");
    assert_eq!(issue(&lint, "missing_summary")["severity"], "error");
}

#[test]
fn paginated_lint_returns_blocking_errors_before_advisories() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.put_page(
        &[],
        "advisory",
        "Canonical",
        "A complete summary.",
        "# Different\n\n[[advisory]]\n",
    );
    world.put_page(&[], "blocking", "Blocking", "", "[[blocking]]\n");

    let lint = world.ok(&["lint", "--limit", "1"]);
    assert_eq!(lint["total"], 2, "{lint}");
    assert_eq!(lint["blocking_total"], 1, "{lint}");
    assert_eq!(lint["advisory_total"], 1, "{lint}");
    assert_eq!(lint["issues"][0]["severity"], "error", "{lint}");
    assert_eq!(lint["issues"][0]["code"], "missing_summary", "{lint}");
}

#[test]
fn short_freeform_pages_and_code_fence_headings_remain_clean() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.put_page(
        &[],
        "short",
        "Short",
        "A complete summary.",
        "A short page needs no sections. [[short]]\n",
    );
    let fenced = format!(
        "# Canonical *title*\n\n## Details\n\n```markdown\n# Fake title\n\n#### Fake jump\n{}\n```\n\n[[fenced]]\n",
        "code ".repeat(1_000)
    );
    world.put_page(
        &[],
        "fenced",
        "Canonical title",
        "A complete summary.",
        &fenced,
    );

    let lint = world.ok(&["lint"]);
    assert_eq!(lint["total"], 0, "{lint}");
    assert_eq!(lint["blocking_total"], 0, "{lint}");
    assert_eq!(lint["advisory_total"], 0, "{lint}");
}

#[test]
fn body_h1_matching_is_unicode_case_insensitive_like_the_viewer() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.put_page(
        &[],
        "case-insensitive",
        "ÜBER Wiki",
        "A complete summary.",
        "# über WIKI\n\n[[case-insensitive]]\n",
    );

    let lint = world.ok(&["lint"]);
    assert_eq!(lint["total"], 0, "{lint}");
}

#[test]
fn advisory_structure_issues_do_not_block_changeset_commit() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.ok(&["changeset", "begin", "advisory-only"]);
    world.put_page(
        &["--changeset", "advisory-only"],
        "advisory-only",
        "Canonical title",
        "A complete summary.",
        "# Different title\n\n[[advisory-only]]\n",
    );

    let overlay = world.ok(&["--changeset", "advisory-only", "lint"]);
    assert_eq!(overlay["blocking_total"], 0, "{overlay}");
    assert_eq!(overlay["advisory_total"], 1, "{overlay}");

    let committed = world.ok(&["changeset", "commit", "advisory-only"]);
    assert_eq!(committed["status"], "committed");
    assert_eq!(committed["lint_issues"], 0);
}

#[test]
fn long_unsectioned_prose_is_an_informational_advisory() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let body = format!("{}\n\n[[long-prose]]\n", "prose ".repeat(700));
    world.put_page(
        &[],
        "long-prose",
        "Long prose",
        "A complete summary.",
        &body,
    );

    let lint = world.ok(&["lint"]);
    assert_eq!(lint["blocking_total"], 0, "{lint}");
    assert_eq!(lint["advisory_total"], 1, "{lint}");
    assert_eq!(issue(&lint, "long_unsectioned_region")["severity"], "info");
}

#[test]
fn blocking_errors_still_reject_changeset_commit() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.ok(&["changeset", "begin", "blocking"]);
    world.put_page(
        &["--changeset", "blocking"],
        "blocking",
        "Blocking",
        "",
        "[[blocking]]\n",
    );

    let error = world.err(&["changeset", "commit", "blocking"]);
    assert_eq!(error["error"]["code"], "changeset_lint_failed");
}

#[test]
fn default_schema_is_a_short_elastic_contract_not_a_section_template() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let schema = world.ok(&["schema", "show"])["schema"]
        .as_str()
        .unwrap()
        .to_string();
    let schema_lower = schema.to_ascii_lowercase();

    for phrase in [
        "`entity`: named people, organizations, products, systems, and datasets",
        "`concept`: ideas, techniques, patterns, and phenomena",
        "`source`: one traceable summary for each ingested source",
        "`query`: durable answers and open questions",
        "`comparison`: side-by-side analysis",
        "`synthesis`: cross-cutting conclusions",
        "title`, `summary`, `kind`, provenance, and links",
        "one logical main title",
        "one primary reader question",
        "current conclusion, definition, or status first",
        "separate current knowledge from history",
        "any section names, order, and markdown constructs",
        "short pages may omit h2 headings",
        "never add empty sections",
        "optional questions",
        "summaries as navigation",
    ] {
        assert!(
            schema_lower.contains(phrase),
            "missing {phrase:?}: {schema}"
        );
    }
}
