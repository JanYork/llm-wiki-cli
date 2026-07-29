# Using LWC Skill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and validate one repository-owned Agent Skill that autonomously
uses `lwc` as durable global and project memory throughout substantive sessions.

**Architecture:** A concise `using-lwc` entry Skill owns the behavioral
lifecycle. A POSIX shell bootstrap handles deterministic installation,
first-run global initialization, and project discovery; progressive references
hold the original LLM Wiki principle and detailed memory policy. The leader
implements the files, while existing subagents serve as uncontaminated
behavioral test subjects and read-only reviewers.

**Tech Stack:** Agent Skills (`SKILL.md`, `agents/openai.yaml`), POSIX shell,
the existing `lwc` CLI, Markdown references, Bash test harness.

---

## File Map

- Create `skills/using-lwc/SKILL.md`: frequently triggered behavioral entry
  point and session lifecycle.
- Create `skills/using-lwc/agents/openai.yaml`: generated Skill UI metadata.
- Create `skills/using-lwc/scripts/bootstrap.sh`: deterministic bootstrap and
  project discovery, emitting JSON.
- Create `skills/using-lwc/assets/global-purpose.md`: initial global Wiki
  purpose.
- Create `skills/using-lwc/assets/global-schema.md`: initial global Wiki
  maintenance schema.
- Create `skills/using-lwc/references/llm-wiki.md`: verbatim user-provided LLM
  Wiki text.
- Create `skills/using-lwc/references/memory-policy.md`: detailed scope,
  write-back, ingest, and safety policy.
- Create `tests/skill_bootstrap.sh`: isolated executable bootstrap regression
  tests.
- Existing `tests/install_script.sh`: release-installer regression test reused
  by final verification.
- Modify `README.md`: document the companion Skill and installation.
- Modify `README.zh-CN.md`: keep the Chinese README structurally aligned.

### Task 1: Establish RED Behavioral Baselines

**Files:**
- No repository files.

- [ ] **Step 1: Give an existing subagent the missing-tool scenario without the Skill**

Use a minimal prompt describing a strong Git project, missing `lwc`, no global
Wiki, and a substantive architecture task. Do not mention desired bootstrap,
scope, or memory behavior.

- [ ] **Step 2: Record the exact omissions and rationalizations**

Check whether the Agent independently installs `lwc`, initializes global
memory, asks about project initialization, recalls prior knowledge, and plans
write-back. RED requires at least one missing behavior.

- [ ] **Step 3: Give a second subagent the scope-and-secrecy scenario**

Place the task under existing project and global Wikis. Include a
project-specific decision, a reusable conclusion or user preference, a
transient log, an API secret, and an uncertain hypothesis. Do not provide the
intended classification.

- [ ] **Step 4: Record incorrect project/global/neither decisions**

RED requires at least one unsafe, duplicated, omitted, or mis-scoped memory
choice.

- [ ] **Step 5: Give a third subagent two retrieval scenarios**

Present a new document and a useful answer produced from it. Check whether the
Agent merely indexes/searches it or completes source analysis, source-summary
creation, affected-page updates, write-back, and maintenance.

Then place an incidental document in a non-project directory. Check that the
Agent may use the document but neither treats the directory as a project nor
suggests project Wiki initialization.

- [ ] **Step 6: Convert only observed failures into Skill requirements**

Keep the raw subagent messages as evaluation evidence in the session. Do not add
hypothetical rules that no scenario exposed.

### Task 2: Test and Implement Deterministic Bootstrap

**Files:**
- Create: `tests/skill_bootstrap.sh`
- Create:
  `skills/using-lwc/scripts/bootstrap.sh`
- Create: `skills/using-lwc/assets/global-purpose.md`
- Create: `skills/using-lwc/assets/global-schema.md`

- [ ] **Step 1: Write the failing shell test**

Cover these isolated temporary-home cases:

1. missing CLI installs through a mocked official installer;
2. missing global Wiki initializes once and applies both assets;
3. repeat execution performs no install or global overwrite;
4. existing ancestor project Wiki is detected;
5. strong `.git`/manifest project is suggested but not initialized;
6. home, temporary, Downloads, and incidental directories are excluded;
7. unsupported or failed installation preserves existing data;
8. every successful path returns parseable JSON with stable keys.

Expected JSON keys:

```text
lwc_path, lwc_version, installed, global_wiki, global_initialized,
project_wiki, project_root, project_confidence, project_evidence,
suggest_project_init
```

- [ ] **Step 2: Run the test and verify RED**

Run:

```bash
./tests/skill_bootstrap.sh
```

Expected: fail because
`skills/using-lwc/scripts/bootstrap.sh` does not exist.

- [ ] **Step 3: Initialize the Skill with the official generator**

Read `skill-creator/references/openai_yaml.md`, then run the official
`/Users/muyouzhi/.codex/skills/.system/skill-creator/scripts/init_skill.py`
with `scripts,references,assets` resources and only these interface fields:

```text
display_name=LWC Memory
short_description=Use lwc as durable Agent memory
default_prompt=Use lwc to recall and preserve durable knowledge for this task.
```

- [ ] **Step 4: Add fixed global purpose and schema assets**

The purpose defines cross-project, long-lived Agent/user knowledge. The schema
requires honest provenance, stable slugs, deduplication, uncertainty labels,
links, and no secrets. Bootstrap applies them only when it creates the global
Wiki.

- [ ] **Step 5: Implement the minimum POSIX bootstrap**

Use existing system commands and the published installer:

```text
https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh
```

An existing command is usable only when `lwc --version` matches
`^lwc [0-9]+\.[0-9]+\.[0-9]+` and scoped init help is available. Do not check
the network for updates.

Detect project roots by walking ancestors. Strong markers are `.git`,
`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `pom.xml`,
`build.gradle`, `build.gradle.kts`, `Gemfile`, `composer.json`, `.sln`, and
`.xcodeproj`. A README plus `src`, `docs`, or `tests` is weak evidence. Emit
evidence and confidence; never create project state.

- [ ] **Step 6: Run syntax and bootstrap tests**

Run:

```bash
sh -n skills/using-lwc/scripts/bootstrap.sh
bash -n tests/skill_bootstrap.sh
./tests/skill_bootstrap.sh
```

Expected: all bootstrap cases pass with no real home or network mutation.

### Task 3: Write the Minimal Behavioral Skill

**Files:**
- Replace generated: `skills/using-lwc/SKILL.md`
- Create: `skills/using-lwc/references/llm-wiki.md`
- Create: `skills/using-lwc/references/memory-policy.md`
- Regenerate: `skills/using-lwc/agents/openai.yaml`

- [ ] **Step 1: Preserve the original principle**

Copy the complete user-provided `# LLM Wiki` text verbatim into
`references/llm-wiki.md`. Do not summarize, translate, or add frontmatter to
that file.

- [ ] **Step 2: Write the memory policy around observed RED failures**

Keep the detailed decision table and workflows in `memory-policy.md`:

- project/global/both/neither classification;
- session bootstrap, recall, and write-back;
- immutable source ingest versus mere indexing;
- query answers written back as pages;
- lint and semantic maintenance;
- provenance, uncertainty, contradictions, deduplication, and secret exclusion;
- when to read `llm-wiki.md`.

- [ ] **Step 3: Write a concise, broadly discoverable `SKILL.md`**

Frontmatter contains only:

```yaml
---
name: using-lwc
description: Use when beginning or continuing substantive project, research, planning, debugging, decision-making, or knowledge work that may benefit a later session.
---
```

The body must require:

1. run bootstrap once;
2. inspect returned project state and ask once when project initialization is
   appropriate;
3. recall bounded global/project context before re-deriving knowledge;
4. apply the memory policy throughout the task;
5. preserve durable results and maintain the Wiki without blocking the primary
   task;
6. read the original principle only for schema evolution, substantial ingest,
   or workflow audits.

- [ ] **Step 4: Regenerate UI metadata**

Run
`/Users/muyouzhi/.codex/skills/.system/skill-creator/scripts/generate_openai_yaml.py`
with the same three interface values used during initialization.

- [ ] **Step 5: Validate the Skill structure**

Run:

```bash
python3 /Users/muyouzhi/.codex/skills/.system/skill-creator/scripts/quick_validate.py \
  skills/using-lwc
```

Expected: valid Skill.

### Task 4: GREEN Behavioral Evaluation and Refinement

**Files:**
- Modify only observed gaps in:
  `skills/using-lwc/SKILL.md`
- Modify only observed gaps in:
  `skills/using-lwc/references/memory-policy.md`
- Modify bootstrap only for mechanical failures:
  `skills/using-lwc/scripts/bootstrap.sh`

- [ ] **Step 1: Re-run each RED scenario with the Skill**

Use the same three existing subagents. Give each only the Skill path and its
original task scenario. Do not reveal expected answers or previous failures.

- [ ] **Step 2: Score observable behavior**

For each response, record pass/fail for:

- install autonomy;
- global initialization;
- project detection without false initialization;
- recall before re-derivation;
- correct scope classification;
- source integration and page updates;
- durable answer write-back;
- lint/maintenance judgment;
- secret and transient-data exclusion;
- keeping the user's main task moving.

- [ ] **Step 3: Add variation and pressure**

At least one scenario must combine urgency, a request to “just save
everything,” an ambiguous project root, and an existing stale memory. Verify
the Agent still preserves safety and resolves contradictions.

- [ ] **Step 4: Close only demonstrated loopholes**

Patch the smallest relevant instruction, then re-run the failed scenario.
Repeat until all required behaviors pass or three refinement rounds expose a
genuine product/CLI limitation.

- [ ] **Step 5: Request independent read-only review**

Have one existing subagent review the final Skill for triggering quality,
standards compliance, command accuracy, security, and unnecessary complexity.
Fix blockers and re-run affected evaluations.

### Task 5: Document and Ship

**Files:**
- Modify: `README.md`
- Modify: `README.zh-CN.md`

- [ ] **Step 1: Add companion Skill documentation**

Document:

- repository path and purpose;
- installation through the user's Agent Skill mechanism or a local copy;
- automatic CLI install/global initialization;
- project initialization consent boundary;
- global versus project memory behavior;
- supported and tested Skill format without claiming universal compatibility.

- [ ] **Step 2: Run all relevant checks**

```bash
git diff --check
sh -n skills/using-lwc/scripts/bootstrap.sh
bash -n tests/skill_bootstrap.sh
./tests/skill_bootstrap.sh
./tests/install_script.sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

Run `shellcheck` when installed. Use a temporary Cargo target directory so
`target/` is absent afterward.

- [ ] **Step 3: Verify repository hygiene**

Confirm `.lwc/`, `.omx/`, `target/`, temporary homes, generated evaluation
artifacts, and personal absolute paths are absent from the deliverable.

- [ ] **Step 4: Commit with Lore trailers**

The final commit must state the autonomous authority boundary, rejected
always-on global `AGENTS.md` injection, behavioral scenarios tested, mechanical
checks, and any remaining platform gaps.
