# Contributing

Thanks for helping improve `lwc`.

## Before You Open A PR

- Keep changes small and focused.
- Prefer bug fixes, tests, and documentation updates over speculative scope.
- If behavior changes, update tests and any affected user-facing docs.
- If a PR changes user-visible facts, keep `README.md` and `README.zh-CN.md` aligned.

## Local Checks

Run these before opening a pull request:

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
```

## Pull Requests

- Explain the problem and the chosen fix.
- Include reproduction and verification steps for bug fixes.
- Keep JSON output contracts and documented CLI behavior explicit when changed.

## Releases

- Update `Cargo.toml` and `Cargo.lock`, then pass every local check before tagging.
- Create an annotated `vX.Y.Z` tag. Its body is the GitHub Release description;
  lightweight tags and blank annotation bodies are rejected by the release job.
- Write the annotation for users, with a short summary followed by `Highlights`,
  `Safety and compatibility`, `Verification`, and `Upgrade` sections as relevant.
  Include behavior and limits, not only commit titles or a changelog link.
- Never move a published tag. Ship a corrective version instead.

Example:

```bash
git tag -a v0.6.0 -m "lwc v0.6.0" -m "Highlights
- Explain the user-visible change.

Verification
- List the release gates that passed.

Upgrade
- Note compatibility or required action."
```

## Issues

- Use the bug report for reproducible defects.
- Use the feature request form to explain how a proposal serves the Agent-first persistent Wiki workflow.
