# Learning Suite implementation contracts

These contracts close the six discovery items in `TASK-001`. Product behavior remains
defined by the approved Learning Suite plan; this file fixes only implementation
choices that later tests and code must share.

## Fixed runtime and package layout

- Keep the existing Cargo package. Add three bin targets: `lwc-tutor`, `lwc-book`, and
  `lwc-practice`, with shared deterministic code in the existing library. A workspace,
  dynamic plugin ABI, discovery, and PATH fallback are out of scope.
- Core exposes exactly `lwc tutor ...`, `lwc book ...`, and `lwc practice ...`. It
  checks the independent global capability, installs the fixed runtime when missing,
  then forwards cwd, stdin, arguments, stdout, stderr, and exit status unchanged.
- Runtime path:
  `~/.lwc/runtime/<plugin>/<lwc-version>/<target>/lwc-<plugin>[.exe]`.
  Canonical data remains under `~/.lwc/plugins/<plugin>/` and is never removed by
  disable or runtime replacement.
- Release tag is `v<lwc-version>`. Asset names are
  `lwc-<plugin>-<lwc-version>-<target>.tar.gz` on Unix and `.zip` on Windows for the
  existing six targets. The same release's combined `SHA256SUMS` is authoritative.
- `SHA256SUMS` is at most 1 MiB and must contain exactly one strict lowercase 64-hex,
  two-space entry for the requested basename. Archives are at most 256 MiB. Missing,
  duplicate, malformed, renamed, mismatched, oversized, or wrong-tag inputs fail
  before publication.

## CLI and JSON boundary

- Plugin successes emit one UTF-8 JSON object:
  `{ "schema_version": 1, "plugin": "...", "command": "...", "result": ... }`.
  Failures use LWC's existing `{ "error": { "code", "message", "details" } }`
  shape and exit non-zero. Core never wraps either stream a second time.
- Mutation commands accept `--json JSON|-|@PATH`. Input is capped at 64 MiB, must be
  UTF-8, rejects unknown fields, and resolves `@PATH` relative to cwd. Entity creation
  requires `request_id`; mutation of existing state requires `if_revision`.
- IDs are opaque lowercase identifiers. Cross-plugin references always contain
  `kind`, exact `id`, and `revision_or_hash`; title/tag lookup never supplies identity.
- Initial command families are fixed to those listed in the architecture:
  Tutor `subject|session|turn|learner|soul|goal|plan|status`; Book
  `subject|import|prepare|status|search|show|peek|read|synthesis`; Practice
  `subject|bank|item|set|paper|attempt|response|grade|review|next|status`.

## Book conversion, blobs, and reading window

- TXT and Markdown normalize directly. EPUB and text PDF invoke the configured
  converter adapter through a shared Rust function; the Book binary never starts a
  nested `lwc` process. HTML, scanned/OCR PDF, MOBI, and AZW3 remain unsupported.
- Exact original bytes are committed first. Normalization records input hash,
  converter/arguments, output hash, UTF-8 validation, anomalies, and ordered blocks.
  Direct text removes one UTF-8 BOM and normalizes CRLF/CR to LF; converter output is
  otherwise preserved after UTF-8 and non-empty validation.
- Original and normalized bodies are content-addressed files at
  `~/.lwc/plugins/book/blobs/sha256/<first-two>/<hash>`. SQLite stores hashes,
  lengths, kinds, and references. Publication uses a private staged file, hash and
  length readback, atomic rename, then the referencing SQLite transaction. Orphaned
  unreferenced blobs are safe and are not purged in v1.
- `book read next` accepts `budget` with unit `tokens` or `utf8_bytes`. With a reported
  token budget it uses floor(55%) for source text; without one it uses a configurable
  64 KiB UTF-8-byte fallback. Every lease reports the requested unit/value, applied
  source limit, exact used bytes/chars, and block range. No model-name guessing occurs.

The Pro capacity probe wrote a 256 MiB deterministic corpus both ways. APFS copied the
external file in 3 ms (clone timing is not portable); incremental SQLite BLOB commit
took 752 ms and produced a 270,292,632-byte WAL. The decisive result is the extra
full-size SQLite/WAL surface, so content-addressed files carry large bytes while
SQLite retains transactional metadata.

## Practice scheduling

- Pin `rs-fsrs = "=1.2.1"` with Serde support. It is the Open Spaced Repetition
  project's scheduler-only Rust crate, uses the MIT license, and avoids the optimizer
  and numerical dependency surface of the `fsrs` crate.
- Persist the scheduler crate/version, parameters, complete ordered review events, and
  resulting card state. Acceptance includes the upstream rating sequence whose
  scheduled-day vector is `0,4,15,48,136,351,0,0,7,13,24,43,77`.

Primary sources:

- <https://github.com/open-spaced-repetition/rs-fsrs>
- <https://github.com/open-spaced-repetition/fsrs-rs>

## Soul and Sync boundaries

- Soul's default full-body budget is 64 KiB and configurable up to 256 KiB. The limit
  is on UTF-8 bytes, never silent truncation. Exceeding it requires an explicit
  evidence-preserving semantic revision. The entire current body is returned with its
  byte count, hash, and revision for every Tutor teaching turn.
- Sync keeps Wiki `StoreIdentity` unchanged. Protocol v2 adds three fixed plugin units,
  each with a separate `PluginStoreIdentity`, canonical manifest, bounded record
  stream, and content-addressed blob stream. Derived FTS/Wiki/graph files never cross.
- A missing runtime does not remove a unit from inventory: the destination validates
  and atomically preserves the canonical export as `preserved_not_ready`. Two changed
  stores require the exact plugin schema merge implementation and baseline; otherwise
  the session fails before any Wiki or plugin publication.
