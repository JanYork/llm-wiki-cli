# Security Policy

If GitHub private vulnerability reporting is enabled for this repository, use the repository Security tab to submit the report privately.

If private vulnerability reporting is not enabled, contact the maintainers privately to coordinate disclosure. Do not open a public issue with exploit details, credentials, tokens, database contents, or other sensitive material.

Include:

- affected `lwc` version or commit
- environment details
- reproduction steps
- impact summary
- any mitigations or workarounds you have confirmed

Please wait for a coordinated response before publishing full details.

## Local knowledge and native graph boundary

- SQLite content remains authoritative. GraphQLite databases are disposable
  sidecar projections and must never be used as the only copy of knowledge.
- Supported macOS/Linux builds embed a pinned GraphQLite 0.6.0 runtime. At first
  use LWC writes it only under the owned `.lwc/runtime/` directory, verifies its
  SHA-256 identity, loads the explicit `sqlite3_graphqlite_init` entry point,
  and runs the extension self-test. Windows builds exclude the runtime and use
  rslg. LWC does not load an arbitrary runtime path from configuration or the
  process environment.
- Existing config, runtime, or sidecar symlinks are rejected. Projection errors
  are sanitized and must not disclose loader search paths, environment values,
  document text, or credentials.
- A canonical mutation may commit before its rebuildable physical projection.
  In that case LWC returns `graph_projection_failed`, records stale state, and
  fails graph reads closed until a writable recovery succeeds. Do not edit the
  sidecar or `graph_projection_state` manually.
- Superseded GraphQLite sidecars are retained and listed by `lwc graph status`.
  LWC intentionally does not delete them automatically; review backup and
  retention requirements before any manual cleanup.
- Search and graph reads are local and unrecorded by default. Semantic relation
  reasons are durable content, so do not place secrets, credentials, sensitive
  prompts, or raw chain-of-thought in them.
- Cargo packages explicitly exclude `.agent/`, `.local-benchmarks/`, and
  `.pi-subagents/`; plan notes, real-corpus stores, prompts, transcripts, and
  review artifacts must never enter release archives.
