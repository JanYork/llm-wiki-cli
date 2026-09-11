# Discussion candidate acceptance — 2026-09-11

Implementation was authorized after the initial plan-only request. No global installation, commit or release was performed. Existing unrelated changes were preserved; builds used an isolated snapshot retaining the original embedded web assets.

- TDD: initial command tests failed before implementation and passed afterward.
- Discussion: nine distinct BDD scenarios passed across scoped runs, covering originals, CAS, atomic rollback, local corrections, migration, sensitive-input rejection, dependency cycles, close requirements, Hook deduplication, compaction/context isolation and unrelated changeset publication.
- A Hook read-view/write-connection conflict was reproduced and fixed by releasing the read view before capture; its exact regression passed afterward.
- Archive export/import/merge preserved originals and history, normalized digest and explicit unbound recovery. Local bindings were excluded.
- MCP Discussion workspace/context checks and updated tool discovery passed. Skill installation byte equality and Pi integration checks passed. Codex package boundary regression passed.
- Focused warnings-denied Clippy, formatting and diff whitespace checks passed.
- Codex plugin manifest validation and real marketplace/add/list commands passed in a disposable CODEX_HOME. The installed plugin was enabled without creating user hooks.json. Native Hooks use default hooks/hooks.json discovery. Other hosts retain their installation route.

These checks were selected by changed behavior; no repeated full test suite or new three-platform runtime matrix was run. Native plugin installation does not establish that the user's active UI has switched. Host capture tests use synthetic exact-message events; complete live capture across all hosts is not claimed. The generic Skill protocol supplies the portable recording behavior.
