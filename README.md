# marklark

**Status: first release, 0.1.0.** The library, the CLI and the MCP server implement the specification.

marklark keeps Markdown documents up to date from their environment — source files, data files, other
parts of the same document — using instructions hidden in HTML comments, so the rendered document is
unaffected.

It splits document maintenance into two levels:

1. **Intent** — a human writes, in plain prose, what a part of the document should contain and where it
   comes from.
2. **Code** — an agent turns the intent into a small, hermetic [Starlark](https://github.com/bazelbuild/starlark)
   program. marklark runs those programs deterministically, with no AI involved, every time the document
   is updated.

```markdown
<!--INTENT
Show the test proving that shift-headings refuses to demote a heading below h6.
-->
<!--CODE
emit(fence(slice("tests/test_markdown.py",
                 start=r"def test_shift_rejects_below_h6", end=r"^    def "), "python"))
#@ intent: blake3:3f2a…
#@ params: ast1:blake3:9c41…
#@ generated: 412:blake3:77e0…
-->
…generated content…
<!--/CODE-->
```

The `#@` stamps record that the code was checked against the intent (`intent`, `params`) and what it
generated (`generated`). When a human edits the intent, the code, or the generated text, marklark notices
and says so — it never silently overwrites a human edit.

A region can also be **generative**: when the INTENT asks for prose, the agent answers it with an
`<!--AI-->` block, written from dependencies marklark extracts and hashes, and flagged for regeneration
when those inputs change.

## Install

```sh
uv tool install marklark     # or: pipx install marklark / pip install marklark
npm install -g marklark
cargo install marklark
```

Or download a binary from [GitHub Releases](https://github.com/verhas/marklark/releases). See
[INSTALL.md](INSTALL.md) for platforms, updating and uninstalling.

## Use

```sh
marklark update FILE...                 # run all CODE blocks, splice their output into the files
marklark check FILE...                  # structure, stamps, declarations, task graph, guards; runs nothing
marklark check --determinism FILE...    # run every update twice and compare
marklark intent-check FILE...           # report intent states (unresolved, intent_changed, params_edited, …)
marklark intent-accept FILE --line N    # a human confirms the code (or AI text) still fits its INTENT
marklark mcp                            # MCP server on stdin/stdout, for agents
marklark skill [--user]                 # install the guide for agents as a Claude Code skill
```

`--strict` turns every intent state other than verified into an error; `--json` prints machine-readable
reports. Exit codes: `0` success, `1` a document has a problem, `2` a file could not be read or written.
A `.bak` copy is written before a file changes, unless git already holds the file's content.

To give an agent the MCP tools in Claude Code:

```sh
claude mcp add marklark -- marklark mcp
```

The tools: `intent_list`, `intent_context`, `intent_apply`, `intent_check`, `update`, `ai_resolve`,
`ai_context`, `ai_update`, `guide`, `list_headings`, `get_section`, `get_lines`. The guide for agents
([docs/agent-guide.md](docs/agent-guide.md)) is served by the `guide` tool and as the resource
`marklark://guide`, and `marklark skill` installs it as a Claude Code skill. See
[examples/](examples/README.md) for a walkthrough.

## What CODE can do

CODE is Starlark with no ambient I/O. Everything goes through host functions: reading files (`read`,
`slice`, `glob`, `json`, `yaml`, `toml`, `xml`), text (`re_search`, `re_findall`, `re_sub`, `reindent`,
`fence`), the document (`headings`, `anchor`, `front_matter`, `here`, `document`), variables shared
between blocks (`define`, `var`), and output (`emit`). Blocks can order themselves with
`tasks(depends_on=[...], completes=[...])`. Files are read only inside the project root; gitignored files
and secrets such as `.env` are refused. See [docs/host-functions.md](docs/host-functions.md).

Values can also appear inline in running text: `<!--$version-->1.2.3` or
`<!--= app.name-->My App<!--/=-->`.

## As a library

The `marklark` crate works on documents held in memory: `Document` is text plus an optional path, and
file access goes through the `FileSystem` trait (`MemoryFs` for in-memory files, `DiskFs` for a
directory tree). `eval::update`, `state::analyze`, `apply::intent_apply` and the `ai` module are what the
CLI and the MCP server use.

## Relationship to mdship

marklark is the successor in spirit to [mdship](https://github.com/verhas/mdship), which has one
dedicated placeholder type per task (INCLUDE, IMPORT, SIP, SLURP, …) and is written in Python. marklark
replaces that family with one programmable construct, is written in Rust, and ships as a single binary.
mdship remains maintained for environments where only `pip` installs are allowed.

## Documentation

| File | Contents |
|---|---|
| [INSTALL.md](INSTALL.md) | Installing, updating and uninstalling, per channel and platform |
| [docs/spec.md](docs/spec.md) | The specification: constructs, stamps, states, evaluation model, security |
| [docs/host-functions.md](docs/host-functions.md) | The host-function library, sized against every mdship feature |
| [docs/agent-guide.md](docs/agent-guide.md) | The guide for agents: rules, workflow, Starlark subset, host functions, patterns |
| [docs/decisions.md](docs/decisions.md) | Decision log with the reasoning behind each decision |
| [docs/open-questions.md](docs/open-questions.md) | What is still undecided |
| [docs/mdship-lessons.md](docs/mdship-lessons.md) | What to carry over from mdship (lessons, not code) |
| [CLAUDE.md](CLAUDE.md) | Orientation for agent sessions working on this repository |
