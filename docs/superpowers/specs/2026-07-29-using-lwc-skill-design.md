# Using LWC Skill Design

## Goal

Add a repository-owned Agent Skill that makes `lwc` the default durable
knowledge layer for substantive project, research, planning, debugging, and
decision-making sessions.

The Skill must turn useful work into a persistent, compounding Wiki rather than
an archive of raw chat. It should recall relevant knowledge before work,
integrate sources during work, and preserve durable conclusions after work.

## Source Principle

Store the user-provided **LLM Wiki** text verbatim at
`skills/using-lwc/references/llm-wiki.md`. It is the conceptual authority for
the Skill.

Load that reference when evolving a Wiki schema, planning a substantial ingest,
resolving a maintenance ambiguity, or auditing whether the workflow still
produces accumulated knowledge. Do not load it on every turn.

## Deliverable

```text
skills/
└── using-lwc/
    ├── SKILL.md
    ├── agents/
    │   └── openai.yaml
    ├── scripts/
    │   ├── bootstrap.sh
    │   └── install-lwc.sh
    ├── assets/
    │   ├── global-purpose.md
    │   └── global-schema.md
    └── references/
        ├── llm-wiki.md
        └── memory-policy.md
```

The implementation also adds a short companion-Skill section to the repository
root `README.md` and `README.zh-CN.md`. It must not add a README inside the
Skill directory.

`using-lwc` is the only entry Skill. Splitting installation, recall, ingest,
and maintenance into independently triggered Skills is rejected because an
Agent could load only part of the required lifecycle.

## Triggering

The description should cause the Skill to load at the start of substantive
sessions involving:

- a software project or other durable work product;
- research, analysis, debugging, planning, architecture, or decisions;
- recurring user preferences, constraints, goals, or operating practices;
- sources or conclusions likely to matter in a later session.

It should not trigger for greetings, simple translation, current time, trivial
rewrites, or other work with no plausible future value.

## Session Lifecycle

### 1. Bootstrap once

Run `scripts/bootstrap.sh` once near the start of work in each active root.

The script must:

1. Find a usable `lwc` executable.
2. If absent, install the latest GitHub Release using the reviewed installer
   bundled with the Skill, without asking the user.
3. Initialize `~/.lwc/wiki.db` when global memory is absent.
4. On first global initialization, set a global purpose and schema for stable
   cross-project knowledge.
5. Find an existing project Wiki in the current directory or its ancestors.
6. Detect a likely project root without creating project files.
7. Return machine-readable state for the Agent.

The script owns deterministic environment work. It does not decide whether a
conversation is worth remembering or initialize a project Wiki.

A usable executable must return a version matching `lwc <version>` and expose
the scoped `init` command. Do not perform a network version check when such an
executable exists. If the command is absent or belongs to another program,
install the official CLI into `~/.local/bin` and use that absolute path:

```sh
LWC_INSTALL_DIR="$HOME/.local/bin" sh scripts/install-lwc.sh
```

`scripts/install-lwc.sh` must remain byte-for-byte identical to the reviewed
root `install.sh`. It downloads only the selected release archive and
`SHA256SUMS`, verifies integrity before replacement, and never executes a
downloaded shell script. This trusts the Skill package and GitHub Release
publishing boundary; SHA-256 is integrity protection, not publisher signing.

On first global initialization, apply the fixed repository-owned
`assets/global-purpose.md` and `assets/global-schema.md` through `lwc purpose
set` and `lwc schema set`. These files define the initial cross-project memory
contract. `references/memory-policy.md` explains Agent judgment and is not
passed to those commands. Existing user-edited global purpose and schema must
never be overwritten.

### 2. Decide whether project memory applies

Treat a directory as a strong project candidate when an ancestor contains
`.git` or a recognized build/package manifest. A weaker combination such as a
README plus source/docs directories may be considered only when the active task
clearly concerns that directory.

Never suggest initialization for the filesystem root, the user's home
directory, temporary/cache directories, Downloads, Desktop, or a directory
being used only as incidental input.

If a strong project candidate lacks an ancestor `.lwc/wiki.db`, ask one concise,
non-blocking question identifying the proposed root and benefit. Continue any
independent work while awaiting the answer. Initialize only after consent.

### 3. Recall before reasoning

Read bounded context once:

- existing project Wiki plus global Wiki: `lwc --scope all context`;
- global Wiki only when no project Wiki exists.

Search with task-specific terms before re-deriving a decision, convention,
prior fix, known constraint, or researched concept. Read the relevant pages and
their cited sources when correctness depends on them.

### 4. Compile knowledge during work

Follow the LLM Wiki lifecycle:

- immutable source material enters through `source add` or `source add-dir`;
- claimed sources are analyzed before page generation;
- update all affected source, entity, concept, comparison, and synthesis pages;
- preserve citations, uncertainty, contradictions, and `[[wikilinks]]`;
- complete ingest only after a cited source-summary page exists.

Do not confuse collection or search indexing with integration.

### 5. Write back durable results

Persist a result when it is likely to save future investigation, prevent a
repeated mistake, constrain later work, or improve an existing synthesis.
Useful answers, comparisons, decisions, discoveries, and revised hypotheses
should become pages rather than disappear into chat history.

Do not persist raw chain-of-thought, secrets, credentials, transient command
output, routine progress, or unsupported guesses. Mark user-provided facts,
Agent observations, and hypotheses honestly. Cite immutable sources whenever
available.

### 6. Maintain

Run deterministic lint after a meaningful ingest batch or material Wiki
change, not after every small write. Periodically inspect semantic
contradictions, stale claims, duplicate concepts, missing links, and unanswered
questions that the CLI cannot determine mechanically.

## Scope Policy

| Destination | Store when |
| --- | --- |
| Project | Knowledge depends on this repository, product, team, domain corpus, architecture, commands, incidents, or local decisions. |
| Global | Knowledge remains useful across projects: stable user preferences, long-term goals, reusable practices, tool behavior, and general lessons. |
| Both | Store separate abstractions only: the concrete instance in project memory and the reusable lesson globally. Do not copy the same page twice. |
| Neither | The information is secret, transient, trivial, already represented, or too uncertain to improve future work. |

When uncertain between project and global, prefer project scope. Promotion to
global should require demonstrated reuse.

## Bootstrap Output

`bootstrap.sh` should emit one JSON object containing at least:

- executable path and version;
- whether installation occurred;
- global Wiki path and whether initialization occurred;
- existing project Wiki path, if any;
- detected project root, confidence, and evidence, if any;
- whether the Agent should consider asking about project initialization.

Failures must be explicit and non-destructive. A failed installation or global
initialization must not delete or replace an existing Wiki.

## Security and Autonomy

- Automatic CLI installation and global initialization are explicitly allowed.
- Project initialization requires user consent because it writes `.lwc/` into
  a worktree.
- Never persist secrets or authentication material.
- Never edit SQLite or generated Markdown directly.
- Never run network update checks merely because a new turn starts; an existing
  compatible `lwc` is sufficient.
- Keep the user's primary task moving when memory maintenance is non-blocking.

## Open-Source Quality

- Use only portable shell facilities required by the supported installer
  environments.
- Keep `SKILL.md` concise and move detailed policy into references.
- Generate and validate `agents/openai.yaml` with the official Skill tooling.
- Do not add private machine paths, local memory databases, generated `.lwc/`,
  test artifacts, or hidden runtime state.
- Add a short README section describing installation and the autonomous
  behavior without claiming universal support beyond the tested Skill format.

## Verification

Follow Skill-TDD:

1. Run pressure scenarios against Agents without the Skill and record concrete
   failures.
2. Implement the smallest Skill that closes those failures.
3. Re-run the same scenarios with the Skill.
4. Add only the rules needed to close observed loopholes.

Required scenarios:

- `lwc` missing, global Wiki missing, strong project with no project Wiki;
- existing global and project Wikis during a task with reusable conclusions;
- a non-project directory containing only incidental documents;
- conflict between project-specific knowledge and a reusable global lesson;
- pressure to save secrets, transient logs, or an unverified guess;
- new source material that must be integrated rather than merely indexed.

Mechanical checks:

- validate Skill metadata with `quick_validate.py`;
- test bootstrap installation, global initialization, idempotency, project-root
  detection, exclusions, JSON output, and failure preservation in isolated
  temporary homes;
- run `shellcheck` when available and execute syntax checks everywhere;
- forward-test the finished Skill with fresh Agent contexts;
- leave `.lwc/`, `.omx/`, `target/`, and temporary artifacts absent from the
  repository.

## Success Criteria

The design is successful when an Agent can begin a substantive session with no
prior setup, acquire `lwc`, initialize global memory, recall relevant context,
make a safe project-initialization recommendation, and preserve worthwhile
knowledge in the correct scope without requiring routine human prompting.
