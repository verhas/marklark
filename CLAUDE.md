# marklark — orientation for agent sessions

## What this is

marklark is a planned Rust CLI + MCP server that maintains Markdown documents. Humans write
`<!--INTENT-->` blocks (prose); agents write `<!--CODE-->` blocks (hermetic Starlark); marklark runs the
code deterministically. See [README.md](README.md) for the one-page summary.

**Status (2026-11-26): design settled; library, CLI and MCP server work.** The library (`src/lib.rs`)
works on documents held in memory over a `FileSystem` abstraction (`MemoryFs`, `DiskFs`): blocks,
stamps, groups, canonical AST hash, task declarations, the evaluation loop with block-local variables,
splicing with `generated` guards, host functions, states (§9), inline values (§7.7), AI blocks (§12) and
`intent_apply`. CLI: `update`, `check`, `intent-check`, `intent-accept`, `mcp`, `skill`. The guide for agents
is `docs/agent-guide.md` (compiled into the binary: MCP `guide` tool, resource `marklark://guide`, and
`marklark skill`); keep it in step with the host functions. `examples/` has documents to try. Not yet: a step limit for
long-running CODE (deliberately deferred). The crate name is reserved (placeholder `0.0.0` on
crates.io; the first real release must be a higher version) and `marklark.dev` is registered (renewal due
2027-11-26). The repository stays
private for now; a crates.io placeholder must therefore not contain the real source.

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

1. The owner reviews the `[proposed]` items in `docs/spec.md` (output text, group binding of unstamped
   blocks, plain-data variables, inline paths, AI states and `dep` restriction, exit codes, extra MCP
   tools) and the deny list applying inside git too.
2. Use marklark on real documents (mdship's documentation is the natural test bed) and fix what that
   shows.
3. Later: a step limit for long-running CODE (deferred by the owner); a first real release (`0.1.0` or
   higher — publishing makes the source public on crates.io).

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
