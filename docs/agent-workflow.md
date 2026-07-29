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
lwc ingest next --context-limit 50
```

`ingest next` atomically claims one task and returns the immutable source,
purpose, schema, and bounded page index. Analyze it before generating pages:

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

Finish only after writing at least one cited source-summary page:

```bash
lwc ingest complete 12
```

Use `lwc ingest fail 12 --message "reason"` for a recoverable processing error,
then `lwc ingest retry 12`. Queue state and analysis survive process restarts.

## Query

```bash
lwc search "question keywords" --limit 20
lwc page show relevant-slug
lwc source show 12
lwc graph related relevant-slug
```

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
- Multi-character CJK queries use dictionary-free adjacent bigrams; the index
  also retains non-stopword CJK unigrams. Latin text uses lowercased
  alphanumeric tokens.
- A lower numeric `rank` is more relevant.
- `--scope all` globally merges project and global hits using the same fixed
  title/summary/body scoring scale; project wins exact ties.
- Search is lexical. If no suitable hit exists, inspect the index and sources;
  do not treat an empty result as proof that the knowledge is absent.

## Projection contract

- `lwc init`, source/page writes, schema/purpose writes, and successful ingest
  completion refresh the Markdown projection.
- `lwc maintenance materialize` performs a full consistent rebuild from SQLite.
- A private manifest removes only stale files previously written by `lwc`;
  user-created files and `raw/assets` are preserved.
- Raw source contents are projected without newline normalization.
