# LWC for Claude Code

This optional plugin uses the same `lwc agent hook` and unified read-only
`lwc serve --mcp` boundary as the direct installer. Review it and ensure the
global `lwc` command is on `PATH`.

```bash
claude plugin marketplace add /absolute/path/to/integrations/claude-lwc
claude plugin install claude-lwc@lwc-local
claude plugin enable claude-lwc@lwc-local
claude plugin uninstall claude-lwc@lwc-local
```

Installation and enablement are separate trust states. Do not combine this
plugin with `lwc agent install --target claude` in the same scope.
