# LWC for Claude Code

This optional plugin uses the same fused `lwc agent hook` and project-scoped
CodeGraph MCP boundary as the direct installer. Review it and ensure `lwc` is on
`PATH`.

```bash
claude plugin marketplace add /absolute/path/to/integrations/claude-lwc
claude plugin install claude-lwc@lwc-local
claude plugin enable claude-lwc@lwc-local
claude plugin uninstall claude-lwc@lwc-local
```

Installation and enablement are separate trust states. Do not combine this
plugin with `lwc agent install --target claude` in the same scope.
