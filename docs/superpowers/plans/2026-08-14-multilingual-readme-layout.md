# Multilingual README Layout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep only the English and Simplified Chinese READMEs at the repository root while preserving all seven language mirrors and every local link.

**Architecture:** Move the five secondary translations into `docs/readme/` with Git history preserved. Rebase their repository-relative links, update both root language switchers, and verify all local Markdown and HTML references with a one-off Node standard-library check.

**Tech Stack:** Git, Markdown/HTML links, Node.js standard library, existing repository shell and Node tests

---

### Task 1: Move secondary translations and rebase links

**Files:**
- Modify: `README.md`
- Modify: `README.zh-CN.md`
- Move: `README.ja.md` → `docs/readme/README.ja.md`
- Move: `README.es.md` → `docs/readme/README.es.md`
- Move: `README.pt-BR.md` → `docs/readme/README.pt-BR.md`
- Move: `README.fr.md` → `docs/readme/README.fr.md`
- Move: `README.ru.md` → `docs/readme/README.ru.md`

- [ ] **Step 1: Create the destination and preserve Git history**

```bash
mkdir -p docs/readme
git mv README.ja.md README.es.md README.pt-BR.md README.fr.md README.ru.md docs/readme/
```

Expected: the root contains only `README.md` and `README.zh-CN.md`; `git status --short` reports five renames.

- [ ] **Step 2: Update the two root language switchers**

In `README.md` and `README.zh-CN.md`, keep the English and Chinese targets unchanged and replace the five secondary targets with:

```text
docs/readme/README.ja.md
docs/readme/README.es.md
docs/readme/README.pt-BR.md
docs/readme/README.fr.md
docs/readme/README.ru.md
```

- [ ] **Step 3: Rebase links in the five moved files**

For each file under `docs/readme/`:

- English: `../../README.md`
- Simplified Chinese: `../../README.zh-CN.md`
- Secondary translations: sibling filenames such as `README.ja.md`
- Root-relative repository documents and directories: prefix `../../`, including `LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`, `THIRD_PARTY_NOTICES.md`, `skills/`, `docs/`, and `benchmarks/`
- External `https://` links and absolute raw GitHub image links: unchanged

Expected: no moved README contains a local link that still assumes it is at repository root.

- [ ] **Step 4: Inspect the rename-aware diff**

```bash
git diff --find-renames --stat
git diff --find-renames -- README.md README.zh-CN.md docs/readme
```

Expected: five renames plus link-only edits; no prose rewrite.

### Task 2: Verify affected contracts and commit

**Files:**
- Verify: `README.md`
- Verify: `README.zh-CN.md`
- Verify: `docs/readme/*.md`
- Verify: `tests/using_lwc_policy.sh`
- Verify: `tests/npm_package.mjs`

- [ ] **Step 1: Prove no active stale root paths remain**

```bash
rg -n 'README\.(ja|es|pt-BR|fr|ru)\.md' \
  --glob '!docs/superpowers/specs/**' \
  --glob '!docs/superpowers/plans/**'
```

Expected: every hit is either inside `docs/readme/` or points through `docs/readme/` from a root README.

- [ ] **Step 2: Validate local Markdown and HTML links**

Run a one-off Node script over `README.md`, `README.zh-CN.md`, and `docs/readme/*.md`. Extract Markdown link targets plus HTML `href` and `src` values; ignore external URLs, anchors, and data URLs; strip fragments and queries; resolve each remaining target relative to its README; fail if the resolved path does not exist.

Expected: exit 0 and `validated 7 README files`.

- [ ] **Step 3: Run affected existing checks**

```bash
bash tests/using_lwc_policy.sh
node --test tests/npm_package.mjs
git diff --check
```

Expected: policy check exits 0, npm package tests pass, and `git diff --check` emits no output.

- [ ] **Step 4: Commit the migration**

```bash
git add README.md README.zh-CN.md docs/readme
git commit -m "docs: organize translated READMEs"
```

Expected: one focused commit containing only the five moves and required link updates.
