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

## Local knowledge and graph-engine boundary

- SQLite documents, provenance, frozen revisions, and explicit relation facts
  remain authoritative. Grafeo and embedded SurrealDB stores are disposable
  sidecar projections and must never be the only copy of knowledge.
- Existing config, runtime, or sidecar symlinks are rejected. Projection errors
  are sanitized and must not disclose storage paths, environment values,
  document text, or credentials.
- A document mutation may commit before its rebuildable projection Work.
  Graph reads fail closed when the selected sidecar is unavailable. Do not edit
  engine sidecars or Work queue files manually.
- Search and graph reads are local and unrecorded by default. Semantic relation
  reasons are durable content, so do not place secrets, credentials, sensitive
  prompts, or raw chain-of-thought in them.
- Cargo packages explicitly exclude `.agent/`, `.local-benchmarks/`, and
  `.pi-subagents/`; plan notes, real-corpus stores, prompts, transcripts, and
  review artifacts must never enter release archives.
