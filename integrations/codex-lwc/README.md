# LWC for Codex

This optional plugin uses the same `lwc agent hook` and `lwc serve --mcp`
boundaries as the direct installer. Review it and ensure the global `lwc`
command is on `PATH`. The plugin bundles the complete `using-lwc` Skill and
does not depend on a separate Skill manager.

```bash
codex plugin marketplace add /absolute/path/to/integrations/codex-lwc
codex plugin add codex-lwc@lwc-local
codex plugin remove codex-lwc@lwc-local
```

Installation does not bypass Codex approval or trust. Do not combine this plugin
with `lwc agent install --target codex` in the same scope.
