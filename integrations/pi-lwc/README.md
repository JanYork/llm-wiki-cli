# LWC for Pi

This optional Pi package loads bounded, explicitly enabled LWC strong-tag
context at session start and after compaction. Pi has no built-in MCP client;
use `lwc cg ...` through Pi's shell tool for structural code queries.

Review the package before installation, ensure `lwc` is on `PATH`, then use
`pi install /absolute/path/to/integrations/pi-lwc` (or `-l` for project scope).
Remove it with `pi remove <the same package source>`. Package installation does
not imply trust, and it must not be combined with `lwc agent install --target
pi` for the same scope.
