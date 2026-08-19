# LWC Office Reading

## Use when

Use the optional Office capability when the user needs to inspect a local
`.docx`, `.xlsx`, or `.pptx` with OfficeCLI-compatible read commands.

## Skip when

Skip it for plain text/Markdown, document editing or creation, and conversion to
reviewable Markdown. Use `document-conversion.md` for conversion and never route
OfficeCLI write commands through LWC.

## Minimum workflow

Inspect `LWC_READINESS.office`. If it is disabled and an Office read is actually
needed, ask once whether to enable it globally or continue without it. Detection
is not consent. After consent, run:

```bash
lwc --scope global config set --office officecli
lwc office COMMAND ...
```

`lwc office` forwards only `view`, `get`, `query`, `validate`, `dump`, `raw`,
and `help`; all following arguments and child output are passed through. The
first read downloads the pinned, SHA-256-verified binary to the versioned global
LWC runtime cache. LWC disables OfficeCLI auto-update and resident mode.

Read commands may create explicitly requested derived output with options such
as `--out` or `--save`, or open a browser, but they never permit commands that
modify the source Office document. Results are not added to the Wiki
automatically.

## Consent boundaries

Never enable or download OfficeCLI merely because a Hook reports it missing.
The global configuration command is durable consent; its next Office read may
download and execute the pinned runtime. A user who ran that command manually
has already enabled the capability. Never fall back to an `officecli` from
`PATH`.

Disable without deleting the cached runtime:

```bash
lwc --scope global config set --office disabled
```

## Completion evidence

- `LWC_READINESS.office.ready=true` after the first successful read.
- The requested OfficeCLI stdout, stderr, and exit status were preserved.
- The source Office document was not modified and no output was ingested unless
  separately requested.
