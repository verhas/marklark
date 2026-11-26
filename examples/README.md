# marklark examples

A tiny fake project — a Python module (`src/greet.py`), data files (`data/app.toml`,
`data/settings.json`), a `CHANGELOG.md` and a `style.md` — and three documents to run marklark on.

| Document | What it shows |
|---|---|
| `basics.md` | Ready-made CODE: front matter, a table of contents that waits for a generated heading (`tasks`), values from TOML shared between blocks (`define`/`var`), inline values, a code excerpt (`slice`), a table from JSON, a changelog section |
| `intents.md` | Three INTENTs without code — for an agent to resolve through MCP |
| `ai.md` | An INTENT that asks for prose — the agent creates an AI block for it through MCP |

To reset the examples after trying them: `git checkout -- examples`.

## Command line

Build and install once, from the repository root:

```sh
cargo install --path .          # puts `marklark` into ~/.cargo/bin
```

Then, from the repository root:

```sh
marklark check examples/basics.md            # structure, stamps, task graph — runs nothing
marklark update examples/basics.md           # runs the CODE blocks and fills in the output
marklark update examples/basics.md           # again: nothing changes (deterministic)
marklark check --determinism examples/basics.md
marklark intent-check examples/intents.md    # three `unresolved` intents → exit code 1
marklark intent-check --json examples/intents.md
```

Things to try in `basics.md` after an update:

- Change `version` in `data/app.toml` and update: the inline values, the heading and the table of
  contents follow.
- Edit a line of generated output by hand and update: marklark stops with an integrity error instead of
  overwriting your edit. Delete that block's `#@ generated:` line to accept losing the edit.
- Put `-->` inside a string in a CODE block and run `check`: marklark reports that the comment ends early.

Options:

| Option | Commands | Effect |
|---|---|---|
| `--strict` | `update`, `intent-check` | every intent state other than verified is an error |
| `--json` | `check`, `intent-check` | machine-readable report on stdout |
| `--determinism` | `check` | runs the update twice and compares |
| `--bak` / `--no-bak` | `update`, `intent-accept` | always / never write a `.bak` copy (default: only when git does not already hold the file's content) |
| `--line N` | `intent-accept` | the INTENT (or one of its blocks) to accept |
| `--drop-stamp` | `intent-accept` | remove a block's `intent`/`params` stamps instead |

Exit codes: `0` success, `1` a document has a problem, `2` a file could not be read or written.

## With Claude Code (MCP)

Register marklark as an MCP server — once:

```sh
claude mcp add marklark -- marklark mcp                     # this project only (default scope "local")
claude mcp add --scope user marklark -- marklark mcp        # or: in every project
claude mcp list                                             # should show marklark as connected
```

If `marklark` is not on your `PATH`, give the full path instead, e.g.
`claude mcp add marklark -- /path/to/marklark/target/release/marklark mcp`.

Optionally install the guide for agents as a Claude Code skill, so Claude knows the Starlark subset,
every host function and the patterns before it writes CODE (the MCP server also offers the same guide
through its `guide` tool):

```sh
marklark skill            # .claude/skills/marklark/SKILL.md in the current project
marklark skill --user     # or ~/.claude/skills/marklark/SKILL.md for all projects
```

Start `claude` in the repository root (file paths in the tools are relative to where Claude runs) and
check with `/mcp` that the marklark server is connected. Then ask, for example:

> Use only the marklark MCP tools, not file edits. Run intent_list on examples/intents.md and resolve
> every unresolved intent with intent_apply. Then run intent_check to confirm.

Then change an intent yourself — e.g. in `examples/intents.md` replace "(only that function)" with
"(and the shout function after it)" — and ask:

> Run intent_check on examples/intents.md and bring every changed intent's code up to date with
> intent_apply.

The INTENT in `examples/ai.md` asks for prose, so the agent answers it with an AI block, which
`ai_resolve` creates:

> Using only the marklark MCP tools, satisfy the intent in examples/ai.md.

Agents never choose prose on their own: an INTENT that can be met by computing text gets CODE.

Afterwards, edit the INTENT text in a document yourself and run `marklark intent-check` on it: the
intent shows as `intent_changed` until an agent re-applies the code, or you confirm it with
`marklark intent-accept FILE --line N`.
