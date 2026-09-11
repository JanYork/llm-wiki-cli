# CodeGraph: native results, explicit index ownership

## Use when

Use CG for definitions, callers/callees, dependency flow and change impact. Use
bounded source reads when the index cannot answer; a miss does not prove code is
absent. Reuse current routing/readiness; do not run status before every query.

## Skip when

Skip CG for literal edits, formatting, or a question already answered by bounded source reads.

## Minimum workflow

`lwc cg status` reports LWC routing: checkout, owner, executable, index and runtime
availability. `initialized` means the index path is usable, not that every current
file or call edge is indexed. `lwc cg inspect` returns native statistics.
`lwc doctor --verbose` reports the resolved checkout, Git common directory, HEAD,
capability state and explicit context binding. `lwc_inspect` provides the same
read-only diagnostics (`kind: doctor`) and shared contracts (`kind: contract`).

The default owner uses the bundled runtime and `.lwc/codegraph`. To explicitly
select an already installed independent runtime for this checkout:

```bash
lwc cg configure --executable /absolute/path/to/codegraph
lwc cg configure --bundled
```

Configuration does not install, initialize, migrate or merge indexes. Independent
mode uses `.codegraph`; a missing selected runtime/index never falls back silently.
Each worktree keeps its own index. A shared Git common directory identifies a
logical repository, not permission to query another checkout. MCP access remains
inside its authorized workspace; choose the host workspace explicitly when needed.

## Query

```bash
lwc cg help query
lwc cg query Symbol --json
lwc cg node Symbol
lwc cg callers Symbol
lwc cg affected src/example.rs --json
lwc cg tools
```

CLI forwarding preserves native stdout, stderr and exit status. Do not parse an
LWC `stdout` wrapper. `affected --stdin` accepts a bounded newline-separated file
list, validates every path, and returns upstream test candidates, not executed tests.

MCP `lwc_codegraph` supports search, callers, callees, impact, node, explore,
status and files (also `codegraph_` names). Use the published native argument schema;
`command: schema` or `lwc cg tools` exposes the selected runtime's read-only schemas.
LWC validates and supplies canonical `projectPath`. Results retain the complete
native CallToolResult, including nulls, non-text content and error flags. LWC's own
transport/path/timeout failures remain identifiable errors. Oversized protocol
frames fail explicitly instead of silently truncating results.

`lwc_explore` defaults to bounded Wiki memory. Explicit code mode returns native
results too: an exact identifier routes to node, broad text to explore. Mixed all
mode retains a memory/code envelope, but preserves the native code result inside.
Use the pure code entry for code navigation.

## Freshness and trust

```bash
lwc cg check src/example.rs --require-fresh
lwc cg --require-fresh --file src/example.rs node Example
```

MCP uses `requireFresh: true` with `files: ["src/example.rs"]`. The gate compares
SHA-256 content hashes for explicitly named files. It fails on stale, unindexed,
missing or unreadable evidence; it never infers freshness from an empty queue.
Checks name their scope and observation time. They do not promise a repository-wide
snapshot, query-wide completeness, dynamic-call coverage or files changing after
the check. Indexed commit is unknown when the runtime does not provide it.

Use `lwc cg sync` to sync an existing authorized index when current work needs fresh evidence, then
recheck. Initialization/downloads require established authorization; missing
optional CG does not block the primary task. Never ingest index databases into
Wiki or edit their state by hand.

## Consent boundaries

Read existing authorized indexes freely. Configure, install or initialize only
within established user authorization; these procedures add no approval gate.

## Completion evidence

Confirm native results, selected checkout/index, named-file evidence where needed,
and the checked-out code supporting the conclusion. A zero-match query never proves
repository-wide absence or safe deletion.
