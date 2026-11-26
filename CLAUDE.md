# marklark — orientation for agent sessions

## What this is

marklark is a planned Rust CLI + MCP server that maintains Markdown documents. Humans write
`<!--INTENT-->` blocks (prose); agents write `<!--CODE-->` blocks (hermetic Starlark); marklark runs the
code deterministically. See [README.md](README.md) for the one-page summary.

**Status (2026-11-26): design settled; core library and a first CLI work.** The library works on
documents held in memory over a `FileSystem` abstraction (`MemoryFs`, `DiskFs`): blocks, stamps, groups,
canonical AST hash, task declarations, the evaluation loop with block-local variables, splicing with
`generated` guards, and a first set of host functions (`emit`, `define`, `var`, `read`, `glob`,
`headings`, `anchor`, `fence`, `tasks`). CLI: `update`, `check`. Not yet: states (§9), `intent-check`,
`intent-accept`, inline values, AI blocks, MCP server, remaining host functions, step limit.

## Where things stand

The design came out of a long discussion that started as a feature idea for mdship (an `intent:` key on
placeholders) and ended with the decision to build a new product. Everything decided is in
[docs/decisions.md](docs/decisions.md); everything undecided is in
[docs/open-questions.md](docs/open-questions.md). Read in this order:

1. [docs/decisions.md](docs/decisions.md) — what is settled and why. Do not re-litigate these without the
   user raising them.
2. [docs/spec.md](docs/spec.md) — the specification. Sections are labelled **[decided]** or
   **[proposed]**; proposed parts are the author's leaning, not agreed.
3. [docs/open-questions.md](docs/open-questions.md) — the work queue for the design.
4. [docs/host-functions.md](docs/host-functions.md) — the standard library draft.
5. [docs/mdship-lessons.md](docs/mdship-lessons.md) — hard-won lessons from mdship.

## Next steps

1. No open question is blocking any more. `starlark-rust` dialect details (flags, termination,
   resource limits, string formatting) are settled during development (D23).
2. Reserve the name: publish a `0.0.0` placeholder crate on crates.io, register `marklark.dev`.
3. Build the core: locate blocks → collect declarations → evaluation loop → splice.
4. CLI (`update`, `check`, `intent-check`, `intent-accept`), then the MCP server (`rmcp`).

## Conventions for this repository

- When a design decision is made, add it to `docs/decisions.md` (next `D` number, date, decision,
  reasoning), update `docs/spec.md`, and remove or update the entry in `docs/open-questions.md`.
- Keep `[decided]` / `[proposed]` labels in the spec accurate.
- Commit or push only when the user asks.

## Relationship to mdship

- mdship lives at `~/github/mdship` (Python). It stays maintained for pip-only enterprise environments.
  **Do not modify mdship from this project**, and do not port mdship code — carry over lessons only.
- mdship's placeholder documentation (`~/github/mdship/documentation/*.md`) is the best catalogue of what
  document maintenance actually needs; `docs/host-functions.md` maps every mdship feature onto marklark.
