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

## Issues

- Use the bug report for reproducible defects.
- Use the feature request form to explain how a proposal serves the Agent-first persistent Wiki workflow.
