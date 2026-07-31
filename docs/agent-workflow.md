# Agent workflow

Use `lwc` as durable external memory. The database stores evidence and compiled knowledge; it does not replace your reasoning.

## Trust boundary

1. SQLite `source` records are immutable snapshots and the source of truth.
2. SQLite `page` records are compiled knowledge maintained by agents.
3. Every factual page should cite source IDs.
4. Treat uncited or contradicted pages as hypotheses until verified against sources.
5. Never edit `.lwc/wiki.db` directly. Use the CLI so citations, links, FTS, and logs stay consistent.
6. Split inputs larger than 64 MiB before ingestion.
7. Treat `.lwc/raw`, `.lwc/wiki`, `.lwc/schema.md`, and `.lwc/purpose.md` as
   generated projections. Rebuild them with `lwc maintenance materialize`.
8. Read commands keep current stores read-only. A writable legacy store may be
   migrated transactionally once before the requested read.
9. Project initialization locally excludes `.lwc/` from Git unless
   `--no-git-exclude` is explicit.

## Start a session

```bash
lwc context --limit 50
```

This returns the purpose, schema, page index, and recent operations. Use
`lwc --scope all context` only when shared global knowledge is relevant.

## Ingest

```bash
lwc source add-dir path/to/corpus/
lwc ingest list --status pending
lwc ingest next --context-limit 50 --source-max-chars 100000
```

Prefer `source add-manifest` for a reviewed multi-file set. Its JSON `sources`
entries contain `path` plus an optional `title`, and relative paths resolve from
the manifest directory. The command preflights every entry before one
transaction writes the batch.

Project sources outside the active Wiki root require
`--allow-external-source`. Do not use
`--acknowledge-sensitive-source` merely to bypass a warning: inspect or redact
the source first, and acknowledge only a safe immutable snapshot.

`ingest next` atomically claims one task and returns the immutable source,
purpose, schema, and bounded page index. If `source_window.has_more` is true,
continue from `source_window.next_offset_chars` until the complete source has
been read:

```bash
lwc source show 12 --offset-chars 100000 --max-chars 100000
```

When a manifest or scheduler already selected a specific pending source, use
`lwc ingest claim 12` instead of relying on queue order.

Offsets count Unicode characters, not bytes. Analyze the complete source before
generating pages:

```bash
lwc search "terms from the new source"
lwc page show relevant-page
lwc ingest analyze 12 --file analysis.md
```

Then create or revise the source summary, entities, concepts, and synthesis pages:

```bash
lwc page put source-12 \
  --title "Source 12 summary" \
  --kind source \
  --summary "What this source contributes" \
  --file source-summary.md \
  --source 12

lwc page put stable-concept \
  --title "Concept title" \
  --kind concept \
  --summary "One sentence for the index" \
  --file concept-page.md \
  --source 12 \
  --source 18
```

Use `[[stable-slug]]` links inside Markdown bodies. A page update atomically
replaces its previous source IDs and extracted links, so always pass the
complete current citation set.

Finish only after writing a cited source-summary page and at least one cited
non-source page:

```bash
lwc ingest complete 12
```

When a source genuinely changes no non-source page, record the exception rather
than fabricating a page:

```bash
lwc ingest complete 12 \
  --no-derived-pages-reason "Duplicate evidence; existing synthesis already covers every supported claim"
```

Use `lwc ingest fail 12 --message "reason"` for a recoverable processing error,
then `lwc ingest retry 12`. Queue state and analysis survive process restarts.

## Query

```bash
lwc search "question keywords" --type auto --limit 20
lwc page show relevant-slug
lwc source show 12 --max-chars 100000
lwc graph related relevant-slug
```

The default `--type auto` returns compiled pages first, hides the raw source
paired with a matching `kind=source` page, and falls back to sources when
needed. Use `--type source` to inspect immutable evidence, `--type page` for
compiled knowledge, `--type all` to audit both layers, and repeat `--kind` to
restrict page kinds.

Low-level searches are private and read-only by default. Add `--record` only
for a top-level query whose wording should appear in the durable operation log.

Synthesize the answer from the selected material. If it is likely to be useful again, save it:

```bash
lwc page put answer-slug \
  --title "Durable answer" \
  --kind query \
  --summary "What this answer resolves" \
  --file answer.md \
  --source 12
```

## Lint

```bash
lwc lint
```

Use `--limit` and `--offset` to walk the issue list. `counts` and `total`
always describe the complete wiki, even when the returned `issues` page is
small. Fix deterministic issues first. Then use the returned context for the
semantic pass the CLI cannot perform:

- claims contradicted by newer sources;
- stale conclusions;
- duplicated concepts under different names;
- important concepts without pages;
- missing research needed to resolve uncertainty.

`untitled_source` identifies legacy rows that still need a readable title.
`shallow_ingest` identifies completed legacy jobs with only a source summary
and no explicit no-derived-pages reason.

Lint is read-only by default. Use `lwc lint --record` only when the validation
event itself belongs in durable operation history.

If lint reports search index rows missing, orphaned, or duplicated, run:

```bash
lwc maintenance reindex
```

## Scope rules

- Default: nearest project `.lwc/wiki.db`.
- `--scope global`: `~/.lwc/wiki.db`.
- `--scope all`: combined `search` and `context`; `search --record` appends the query operation to each selected store.
- Citations and wikilinks belong to one store; cross-store relations are not created implicitly.

## Search contract

- Search terms are plain text, never raw FTS syntax.
- `--type auto` is the page-first default. `page`, `source`, and `all` expose
  explicit retrieval layers; `--kind` applies only to page results.
- Multi-character CJK queries use dictionary-free adjacent bigrams; the index
  also retains non-stopword CJK unigrams. Latin text uses lowercased
  alphanumeric tokens.
- A lower numeric `rank` is more relevant.
- `--scope all` globally merges project and global hits using the same fixed
  title/summary/body scoring scale; project wins exact ties.
- Search is lexical. If no suitable hit exists, inspect the index and sources;
  do not treat an empty result as proof that the knowledge is absent.

## Storage maintenance

The FTS5 table is contentless: canonical source and page text is stored once in
the normal tables, while FTS retains only its index. During an idle maintenance
window:

```bash
lwc maintenance compact
```

The command optimizes FTS and attempts a WAL truncate checkpoint. If `busy` is
true, an active reader prevented full reclamation; retry later. Never infer
success from the command exit alone—inspect `busy` and `after_bytes`.

## Mutation recovery

Before a multi-source ingest or broad replacement of existing pages, create a
named checkpoint:

```bash
lwc checkpoint create before-architecture-refresh
```

`checkpoint restore` validates the selected database, creates a
`pre-restore-*` copy of the current state, restores through SQLite's online
backup API, and rebuilds raw and Markdown projections.

Use `source remove <ID>` and `page remove <SLUG>` instead of editing SQLite.
Removal refuses a cited source or a page with inbound links.

## Development-only benchmark

Do not run the repository benchmark during ordinary memory work. When
developing or auditing LWC itself, follow `benchmarks/README.md`, use a
sanitized corpus plus reviewed JSONL ground truth, and compare release binaries
under the same conditions.

## Projection contract

- `lwc init`, source/page writes and removals, schema/purpose writes,
  checkpoint restores, and successful ingest completion refresh the Markdown
  projection.
- `lwc maintenance materialize` performs a full consistent rebuild from SQLite.
- A private manifest removes only stale files previously written by `lwc`;
  user-created files and `raw/assets` are preserved.
- Raw source contents are projected without newline normalization.
