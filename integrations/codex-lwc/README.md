# LWC native Codex plugin

This is a **Codex-only installation source**. Hooks are owned by the native
`codex-lwc` plugin, displayed as **LWC** under plugin-provided hooks, rather than
entries written to user `hooks.json`. Claude and Pi installation modes are unchanged.

The plugin uses the default `hooks/hooks.json` discovery path. Do not add a
`hooks` field to the manifest: the current ingestion validator rejects it.
Skills include `using-discussion`; MCP exposes three read-only context tools and
the dedicated opt-in `lwc_discussion` write/recovery tool.

Ensure a compatible `lwc` binary is available on PATH before using the plugin.
To select this native installation route from this repository:

```bash
codex plugin marketplace add /absolute/path/to/integrations/codex-lwc
codex plugin add codex-lwc@lwc-local
codex plugin list --marketplace lwc-local --json
```

This existing repository marketplace is explicitly registered; it is not the
implicitly discovered personal marketplace. Start a new Codex task after install.
Codex manages trust review and the UI's source grouping; LWC does not bypass it.

## Switching from direct integration

Use **one owner**. Before activating the plugin, inspect
`lwc agent status --target codex --location global`. Remove only LWC-owned direct
integration using `lwc agent uninstall --target codex --location global --yes`.
If a project-local direct integration is active, inspect/remove it at that exact
scope too. Preserve unrelated hooks and user-owned instructions. A foreign/manual
LWC entry requires an ownership check; do not erase an entire hooks file.
Then install the native plugin with the commands above. Do not run the direct
`lwc agent install --target codex` again while this plugin is enabled.

Uninstall the native package with `codex plugin remove codex-lwc@lwc-local`.
The direct installer remains available for users who deliberately choose it.
No Claude/Pi migration is performed by this route.

Source: [official hooks discovery](https://learn.chatgpt.com/docs/hooks) and
[plugin packaging](https://developers.openai.com/plugins/build/plugins).
