# Agent workflow

Use `lwc` as durable external memory. The database stores evidence and compiled knowledge; it does not replace your reasoning.

## Trust boundary

1. SQLite `source` records are immutable snapshots and the source of truth.
2. SQLite `page` records are compiled knowledge maintained by agents.
3. Every page declares structured provenance. Source-grounded pages cite source
   IDs; durable user statements, Agent observations, and hypotheses use the
   matching explicit provenance class.
4. Never invent a source ID for non-source knowledge.
5. Never edit `.lwc/wiki.db` directly. Use the CLI so citations, links, FTS, and logs stay consistent.
6. Split inputs larger than 64 MiB before ingestion.
7. Treat `.lwc/raw`, `.lwc/wiki`, `.lwc/schema.md`, and `.lwc/purpose.md` as
   generated projections. Rebuild them with `lwc maintenance materialize`.
8. Read commands keep current stores read-only. A writable legacy store may be
   migrated transactionally once before the requested read.
9. Project initialization locally excludes `.lwc/` from Git unless
   `--no-git-exclude` is explicit.
10. File-path revision history is observational. Content remains globally
    deduplicated, so one source ID may appear at multiple paths or revisions.
11. A multi-command knowledge update belongs in one changeset. The draft is a
    private SQLite snapshot; live canonical state and live Markdown stay
    unchanged until commit.

## Start a session

```bash
lwc context --limit 50
```

This returns the purpose, schema, page index, and recent operations. Use
`lwc --scope all context` only when shared global knowledge is relevant.

## Atomic changesets

Wrap one logical update that needs multiple mutations in a named changeset:

```bash
lwc changeset begin architecture-refresh
lwc --changeset architecture-refresh source add-manifest sources.json
lwc --changeset architecture-refresh ingest claim 1
# analyze, write cited pages, and complete ingest with the same selector
lwc --changeset architecture-refresh lint
lwc --changeset architecture-refresh search "expected answer" --limit 5
lwc changeset show architecture-refresh
lwc changeset commit architecture-refresh
```

Every supported command with `--changeset <NAME>` reads or writes only the
draft, so later commands see earlier staged work without exposing a partial
Wiki. Draft writes do not materialize Markdown. `changeset show` reports the
base and draft revisions, staged operations by action, lint total, empty and
conflict state, and whether commit is currently allowed.

Commit rejects an empty draft, a live/draft revision conflict, and lint issues.
Repair new lint issues in the draft. Use
`--allow-lint-issues --reason "specific reviewed pre-existing debt"` only when
the remaining issues existed before this changeset and the reason is auditable.
On `changeset_conflict` or `changeset_changed`, do not force or merge: preserve
live work, inspect or discard the stale draft, begin a fresh changeset, and
reapply the reviewed update.

Commit freezes the reviewed draft before checkpoint/publication. After that
point, every routed mutation fails transactionally with `changeset_frozen`,
including when a committed draft remains only because WAL checkpoint or cleanup
needs recovery. Retry the same commit, or discard after a reported conflict;
never stage new work into a frozen draft.

```bash
lwc changeset discard architecture-refresh
lwc changeset rollback <CHANGESET_ID>
```

Discard deletes only an uncommitted draft. Successful commit creates and
records a pre-commit checkpoint, publishes canonical SQLite in one transaction,
truncates WAL when no reader prevents it, removes owned draft files, and
materializes live Markdown once. Rollback uses the exact returned ID and only
succeeds while no later live mutation exists; it creates a pre-rollback
checkpoint and has no force option. A committed cleanup or materialization
error is not a database rollback—follow its structured recovery fields.

Use one explicit `project` or `global` scope consistently for begin, routed
commands, show, commit, discard, and rollback. `--scope all` is invalid, and
`init`, `maintenance`, `checkpoint`, and nested changeset commands reject the
selector.

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

Before relying on file-backed evidence, check the selected source IDs:

```bash
lwc source status 12 18
```

Treat `lineage_state=superseded` as a newer observed snapshot for that path.
Treat any `filesystem_state` other than `current` as requiring review. For
`modified`, inspect the exact change and its direct citation candidates first:

```bash
lwc source diff 12
lwc source refs 12 --limit 1000 --offset 0
```

If more than one tracked path is reported, repeat diff with the exact `--path`.
If `diff.truncated=true`, retry up to `--max-chars 100000` and keep the review
explicitly incomplete if it is still truncated. A single refs query with
`has_more=false` is a complete point-in-time list of direct citers. A paginated
scan must be de-duplicated and labelled non-atomic and potentially incomplete.
These are review candidates, not automatically affected pages. The Agent must
decide whether the edit changes meaning; only then run `source add` on the same
path, ingest the new source, and revise the claims that changed.

`source status`, `source diff`, and `source refs` are read-only. Status hashes
live files exactly; `--all` is an explicit maintenance scan, not a session-start default.
External tracked paths require `--allow-external-source` again for each check.
Diff additionally requires `--acknowledge-sensitive-source` before returning
flagged live text. Snapshot-to-snapshot review uses
`source diff <OLD_ID> --to-source <NEW_ID>` without a live file.
Migrated legacy sources may be returned in `untracked_source_ids`; re-add the
intended file once because migration deliberately does not infer old paths.
Retry `source_status_unstable`; it means the live file or database head changed
during the bounded check, so no mixed-time result was accepted.

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
replaces its previous source IDs, explicit provenance, and extracted links, so
read the page first and pass the complete current sets. Source IDs derive
`source-grounded`; do not pass that value through `--provenance`.

For durable non-source knowledge, repeat the explicit flag for mixed pages:

```bash
lwc page put accepted-direction \
  --title "Accepted direction" \
  --kind query \
  --summary "User constraint and Agent verification state" \
  --file decision.md \
  --provenance user-provided \
  --provenance agent-observed
```

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
lwc search "question keywords" --type auto --limit 20 --explain
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

Use `--explain` before changing retrieval state. It reports the exact
lower-is-better score, bounded title/path/generic/graph signals, effective
manual adjustment, and effective query feedback. It is read-only and does not
imply that a high-ranked page is factually correct.

Use a document weight only for durable, query-independent importance and use
query feedback only after checking one concrete result:

```bash
lwc weight set page relevant-slug \
  --value 1 \
  --reason "Current canonical implementation guide" \
  --provenance agent-observed
lwc weight feedback page relevant-slug \
  --query "question keywords" \
  --signal relevant \
  --reason "Expected page verified" \
  --provenance agent-observed
```

Agents may create `agent-observed` rows when current evidence supports the
judgment. Use `user-provided` only for the user's explicit judgment; it wins
when both exist. Never infer weights from clicks, rank position, page length,
directory depth, or a single unverified answer. Clear obsolete state instead
of stacking compensating values. Document weights are limited to
`-2,-1,1,2`; feedback is `relevant` or `irrelevant`, applies only to the same
ordered-token fingerprint, and does not generalize to paraphrases. Both affect
only lexical candidates. Feedback omits the raw query from SQLite and the
operation log, but `--reason` is durable and must not repeat sensitive text.
Run mutations in one explicit `project` or `global` scope; `--scope all` is
read-only for this purpose.

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
`retrieval_weight_orphan` and `retrieval_feedback_orphan` identify adjustments
whose page or source was removed outside the guarded CLI workflow.

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
- Changesets exist only in one explicit `project` or `global` store. Identical
  names in different stores are unrelated, and `--scope all` cannot begin,
  route, commit, discard, or roll back a changeset.

## Search contract

- Search terms are plain text, never raw FTS syntax.
- `--type auto` is the page-first default. `page`, `source`, and `all` expose
  explicit retrieval layers; `--kind` applies only to page results.
- Multi-character CJK queries use dictionary-free adjacent bigrams; the index
  also retains non-stopword CJK unigrams. Latin text uses lowercased
  alphanumeric tokens.
- A lower numeric `rank` is more relevant.
- `--scope all` globally merges project and global hits using the same fixed
  field, specificity, graph, manual, and feedback scale; project wins exact
  ties.
- `--explain` is the authority for score arithmetic. A document weight is
  query-independent; feedback is keyed by the ordered tokenizer output.
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

Use a changeset for a multi-source ingest or broad replacement of existing
pages. Its successful commit creates the pre-change checkpoint automatically.
For a large one-command mutation or maintenance operation that cannot use a
changeset, create a named checkpoint:

```bash
lwc checkpoint create before-architecture-refresh
```

`checkpoint restore` validates the selected database, creates a
`pre-restore-*` copy of the current state, restores through SQLite's online
backup API, and rebuilds raw and Markdown projections.

`changeset rollback <CHANGESET_ID>` is narrower than checkpoint restore: it is
bound to one recorded commit and refuses once any later live operation changes
the revision. Use this guarded path for an immediately mistaken batch; do not
use checkpoint restore to bypass the rollback conflict.

Use `source remove <ID>` and `page remove <SLUG>` instead of editing SQLite.
Removal refuses a cited source or a page with inbound links. If a removed source
is the current revision of a path, LWC removes that path's revision series so an
older snapshot cannot become current by accident.

## Development-only benchmark

Do not run the repository benchmark during ordinary memory work. When
developing or auditing LWC itself, follow `benchmarks/README.md`, use a
sanitized corpus plus reviewed JSONL ground truth, and compare release binaries
under the same conditions.

## Projection contract

- Draft changeset mutations never write a second projection tree.
- Successful changeset commit and rollback rebuild the live projection once;
  structured post-commit errors distinguish committed SQLite from repairable
  projection/cleanup work.
- `lwc init`, source/page writes and removals, schema/purpose writes,
  checkpoint restores, and successful ingest completion refresh the Markdown
  projection.
- `lwc maintenance materialize` performs a full consistent rebuild from SQLite.
- A private manifest removes only stale files previously written by `lwc`;
  user-created files and `raw/assets` are preserved.
- Raw source contents are projected without newline normalization.
