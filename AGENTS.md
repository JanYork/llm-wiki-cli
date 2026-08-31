<!-- AGENT-BRAIN:START -->
## Project agent brain

- Read [`.agents/project-memory.md`](.agents/project-memory.md) at the start of substantive repository work. It owns the concise project map, authoritative documentation map, LWC Wiki entry points, and maintenance triggers.
- Use the project LWC Wiki for durable detailed knowledge. Start with `lwc --scope project search "<task terms>" --limit 20`, then open only relevant pages and current source/code evidence.
- During relevant work and before completion, evaluate whether verified reusable knowledge should update the Wiki or `.agents/project-memory.md`. Make no memory change when nothing qualifies. Update this discovery map in the same task whenever brain structure changes.
- Require a resolved safety notice and a separate human confirmation only for materially destructive or hard-to-recover actions, credential/secret handling, privilege or security-boundary changes, permanent deletion, history rewriting/force-push, and production publication that the user did not already explicitly authorize. Ordinary explicitly requested Git commits, fast-forward merges, and non-force pushes need no extra confirmation after scope checks.
- Reconfirm only when the target, affected user data, destructive method, or risk class materially changes. Incidental details such as discovered file counts, generated commit hashes, or equivalent safe command syntax do not invalidate authorization. Never expose or persist literal secrets.
- Archive before permanent deletion of brain knowledge.
<!-- AGENT-BRAIN:END -->
