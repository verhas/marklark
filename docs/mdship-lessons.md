# Lessons from mdship

mdship (`~/github/mdship`, Python, <https://github.com/verhas/mdship>) is the predecessor. marklark takes
no code from it, only these lessons.

## Editing

- **Never re-render a document from a parse tree.** mdship's `fix_headings` once rendered the whole
  document through an AST round-trip and silently reflowed unrelated content (it stripped two-space
  list-continuation indentation). The fix was line-based editing. marklark splices by byte range and uses
  the Markdown parser only to locate things.
- **Markers must start at a line start and must not be inside fenced code or inline code spans.**
  Documentation that shows the syntax as an example must not be processed.
- **A comment ends at the first `-->`.** mdship needed "arrow syntax escaping" for Mermaid diagrams
  (`-->` is a Mermaid arrow). marklark CODE has the same constraint.

## Integrity

- **Generated content is guarded by a length + hash stamp.** A hand edit of generated output stops the
  update with a clear message and a documented way to accept the loss (delete the stamp). Users rely on
  this; never overwrite a human edit silently.
- **Backups**: write a `.bak` unless git already holds the file's content; `--bak` / `--no-bak` override.
- **A pattern that matches nothing is an error**, never "include nothing". Silent empty output hides
  stale references.

## Architecture

- **One application layer, thin adapters.** mdship ended up with CLI and MCP adapters over an
  `operations` layer, after a refactor to stop the two drifting apart. Start marklark that way: core
  library → CLI and MCP as adapters, no logic in adapters.
- **Typed errors**, not message-prefix matching, for expected conditions (not found, integrity, file
  errors).
- **One place defines evaluation order.** In mdship it was `update_document()`; in marklark it is the
  evaluation loop.

## Agent ergonomics

- **Discovery primitives** (`list_headings`, `get_section`, `get_lines`, `list_ai_placeholders`) let
  agents work without reading whole files. They mattered more in practice than expected.
- **Report status without returning bodies.** mdship's `ai-list` gives name/line/status so an agent can
  skip up-to-date items without fetching them.
- **Agents struggled to write placeholders by hand.** That pain is what started the intent idea and,
  eventually, marklark.

## The intent idea as designed for mdship

The design discussion first worked out intents for mdship's placeholders (an `intent:` key, then an
`<!--INTENT-->` block, groups by shared `_intent_checksum_`, per-dependency intents for AI placeholders
written by agents, discovery via `_intent_resolved_at_` and git diff). marklark adopts the parts that
survive the switch to CODE; see [decisions.md](decisions.md) D8–D13.
