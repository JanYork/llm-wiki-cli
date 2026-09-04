# LWC CodeGraph Index

## Use when

Use CodeGraph for structural questions about checked-out code: symbol definition,
signature, callers/callees, dependency flow, file topology, reachability, or
change impact across symbols/files.

## Skip when

Skip CodeGraph for a single-file literal edit, formatting-only work, docs/config
only work, comments/log strings, or when native text search already proves the
answer. Use `rg` for literal text.

## Minimum workflow

### Code intelligence recommendation

For every nontrivial code task, run:

```bash
lwc --scope project cg status
```

The pinned CodeGraph runtime is installed once globally per version/platform,
while every project owns its separate `.lwc/codegraph` index. If
`initialized=true`, use the native read-only `lwc_codegraph` tool. Its `command`
accepts only `search`, `callers`, `callees`, `impact`, `node`, `explore`,
`status`, and `files`, either as short names or with the `codegraph_` prefix.
Pass `arguments` as an object. LWC validates the project root and always
overwrites any nested `projectPath` with that canonical path.

Choose `node` for one known symbol, `search` for candidates,
`callers`/`callees` for direction, `impact` before changing shared code, and
`files` for topology. Reserve `explore` for broad flows. A successful call
returns CodeGraph's complete `CallToolResult` unchanged; LWC transport,
protocol, and timeout failures remain structured LWC errors. The outer 60-second
deadline covers CodeGraph's 45-second busy queue while still bounding a hung
child.

Keep `lwc_explore` for bounded Wiki memory and compatible mixed exploration. In
code mode, one exact identifier or qualified identifier routes to `node` with
code included; natural-language and multi-token queries continue to use
`explore`.

If `initialized=false`, explain tree-sitter-derived structure, project-local
indexing, telemetry disabled, and single-file document-granular commits. Always
ask for consent before `cg init`; continue the primary task while awaiting the
answer.
After consent:

```bash
lwc --scope project cg init
lwc --scope project cg status
```

If the task depends on current dirty or uncommitted code, run `lwc --scope
project cg sync` before the first structural query, after relevant code edits,
and before a final structural claim. Use CodeGraph to locate the smallest source
surface, then read the exact files that prove behavior.

## Consent boundaries

Querying an existing user-authorized index needs no additional consent.
Downloading the pinned global runtime and creating a project index does. Never
pass another project path, use a `codegraph` from `PATH`, ingest the index, or
edit its database directly.

## Completion evidence

- Status separates global runtime health from project index initialization.
- Structural claims were made against the current synced index.
- Exact checked-out source confirms the relevant behavior; checked-out code is
  the current implementation evidence when memory differs.
- The project index stayed project-local and telemetry stayed disabled.
