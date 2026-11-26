# marklark specification (draft)

Status labels: **[decided]** — agreed with the project owner; **[proposed]** — the current leaning,
not yet agreed; the matching entry in [open-questions.md](open-questions.md) holds the alternatives.

---

## 1. Model [decided]

A marklark document is ordinary Markdown with instructions in HTML comments. Maintenance has two levels:

| Level | Construct | Written by | Meaning |
|---|---|---|---|
| Declarative | `<!--INTENT-->` | human | What this part of the document should be and where it comes from. Source of truth. |
| Executable | `<!--CODE-->` | agent (a human may) | A hermetic Starlark program that satisfies the intent. A cached, checkable compilation of it. |

`marklark update` runs CODE deterministically; no AI is ever involved in an update. Agents are involved
only when an intent has no code yet, or when deterministic checks report that intent and code have
drifted apart.

An intent can be satisfied in two ways:

- **deterministically** — by CODE, re-run on every update;
- **generatively** — by text an agent writes directly (prose), stamped with the inputs it was written
  from, regenerated only when those inputs change. This is mdship's AI placeholder. It is a dedicated
  `<!--AI--> … <!--/AI-->` construct that follows mdship's AI placeholder by concept (D24, D33; §12).

## 2. Recognising blocks [decided, details proposed]

A block is an HTML comment that:

- starts at the beginning of a line (leading whitespace allowed),
- is not inside a fenced code block or an inline code span,
- has the block name immediately after `<!--`: `<!--INTENT`, `<!--CODE`, `<!--/CODE-->`.

Anything else, including ordinary HTML comments, is left alone byte for byte.

**`-->` inside a block** [decided]: a comment ends at the first `-->`, so CODE must not contain the
sequence `-->` anywhere (including inside string literals; write `"--" + ">"`). Detection is by shape and
therefore limited: marklark reports a block whose comment ends at a `-->` followed by more text on the
same line (CODE and AI blocks), or that leaves a stray `-->` line behind it, with a dedicated message —
the rest of the block would otherwise show in the rendered document. Output emitted
by CODE must not contain the block's own closing marker; `emit` refuses it.

## 3. INTENT [decided]

```markdown
<!--INTENT
Define the variable 'version' from the project version in pom.xml.
-->
```

- The body is **raw prose**. It is never parsed as YAML or anything else.
- It is human-owned: no marklark command and no agent tool ever writes into an INTENT block.
- An INTENT applies to the run of CODE blocks directly after it (§6).
- Only the standalone form exists in marklark (D26): mdship's alternative `intent:` key inside a
  placeholder is not carried over.

## 4. CODE [decided]

```markdown
<!--CODE
tasks(depends_on=["headings collected"], completes=["toc"])

lines = []
for h in headings(min_level=2, max_level=3):
    lines.append("  " * (h.level - 2) + "- [" + h.text + "](#" + h.anchor + ")")
emit("\n".join(lines))
#@ intent: blake3:…
#@ params: ast1:blake3:…
#@ generated: 212:blake3:…
-->
- [Introduction](#introduction)
…
<!--/CODE-->
```

- The body is Starlark (§8 for the dialect and the security model).
- **Output text** [proposed]: emitted text gets the document's line ending, and non-empty output always
  ends with a line ending so the closing marker keeps its own line. Output that contains a marklark block
  marker at the start of a line is an error.
- **Output region**: a CODE block that calls `emit` must have a closing `<!--/CODE-->`; the text between
  the opening comment and the closing marker is its output region, replaced on every update. A CODE block
  that only defines variables has no closing marker and no output region. Agents always write the
  closing marker for emitting blocks.
- **Stamps** (D19): machine-written metadata lines of the form `#@ key: value`, every line carrying the
  `#@` prefix, written at the end of the body. They are Starlark comments, so the program parses
  unchanged. Stamp lines are excluded from the `params` hash. Defined keys are listed in §5.
- **Task declaration** — `tasks(depends_on=[...], completes=[...])` — must be the first statement (§7.1).

## 5. Stamps and checksums

### 5.1 Keys [decided]

| Key | Value | Written by | Purpose |
|---|---|---|---|
| `intent` | hash of the governing INTENT's text, whitespace runs collapsed to one space and trimmed | `intent_apply`, `intent-accept` | Group id (§6) and intent-drift detection |
| `params` | `ast1:blake3:<hex>` — hash of the canonical printout of the parsed CODE body (D20); comments, including stamp lines, and formatting do not count | `intent_apply`, `intent-accept` | Detects hand edits of the code since it was last verified against the intent |
| `generated` | `<length>:<hash>` of the output region, line endings normalised to `\n`; length in Unicode characters (D38) | `update` | Detects hand edits of generated output |

Hash algorithm (D37): BLAKE3, written as `blake3:<hex>`. There is no compatibility to keep with
mdship's MD5.

### 5.2 Who writes what [decided]

- `intent`, `params`: only `intent_apply` (MCP, the agent after verifying its work) and
  `marklark intent-accept` (CLI, a human confirming that a hand edit still fits the intent).
  **`update` never writes them**; otherwise a stamp would no longer mean "someone checked this code
  against this intent".
- `generated`: written by `update` whenever it replaces an output region.

### 5.2a Agent-owned stamps [decided]

The keys above and the AI block's `dep` and `deps` (§12) are **reserved**: only marklark writes them —
except that a human may edit `dep` lines (§12).
Any other key is preserved unchanged by `update` and `intent-accept`. `intent_apply` accepts extra stamps
from the agent, writes them with marklark's own, and refuses a reserved key. Agent keys start with `x-`
(e.g. `#@ x-resolved-at: 09fe57`) so future marklark keys cannot clash with them; marklark treats their
values as opaque text (D31). marklark does no git-based discovery — finding what changed since the code
was verified is the agent's job.

### 5.3 What is deliberately not hashed [decided]

The *sources* the code reads. Sources are supposed to change, and the document follows them; hashing
them into the intent stamps would make every commit look like drift. Whether the code still reads the
*right* thing after a source change (e.g. a reused method name) is a judgement call left to an agent's
deep check.

## 6. Intent binding and groups [decided]

Starting from an INTENT, take the first CODE block after it with only blank lines in between. Let **G**
be its `intent` stamp. Keep adding blocks while all hold:

1. only blank lines separate the end of the previous member (its closing marker, or its opening comment
   if it has no output region) from the next opening comment;
2. the next block's `intent` stamp equals **G**;
3. it is a CODE block (an INTENT starts a new group; generated-text blocks are never group members).

[proposed] An INTENT may also bind one AI block (§12) — always a group of its own. Blocks without an
`intent` stamp count as sharing it, so an INTENT followed by several unstamped CODE blocks binds them all.

**G** need not equal the current INTENT hash — only be shared. Equal means verified; a different shared
value means the INTENT text was edited, and marklark still knows exactly which blocks belong to it.
Human text between members splits the group; a user who wants prose between two code blocks writes two
INTENTs.

`intent_apply` replaces a whole group atomically: it removes every member, writes the new blocks, and
stamps them all. Before removing anything it checks every member's `generated` stamp; if a human edited
generated output it refuses (integrity error) — swapping code must never discard a human edit.

## 7. Evaluation [decided]

### 7.1 Declarations

A CODE block may start with one call declaring the tasks it takes part in:

```python
tasks(depends_on=["headings collected"], completes=["toc"])
```

- `depends_on` — the block may run only after these tasks are complete;
- `completes` — the block contributes to completing these tasks.

Both parameters are keyword-only and optional (default: empty). Their values must be list literals of
string literals. The call must be the block's first statement and may appear at most once; a `tasks`
call anywhere else, a second call, a positional argument, or a non-literal value is an error. At run
time the call does nothing: its content was already collected.

> Why one call: with two separate functions a block could state `depends_on` before `completes` and be
> aborted before revealing what it completes, leaving the evaluation clueless about the tasks it takes
> part in. One call states both sides at once, so the problem cannot arise.

Before the first pass, marklark collects the `tasks` declarations of **every** CODE block in the
document.

> Why up front: a task is complete when *all* blocks that list it in `completes` have run. If
> declarations were only discovered while running, a block aborted on an unmet dependency would never
> reveal what it completes, and a task could look complete while one of its contributors had not run.

Static checks on the collected declarations, before anything runs:

- a task that some block lists in `depends_on` but no block lists in `completes` → error naming the task
  (catches typos);
- a cycle in the task graph → error naming the cycle.

### 7.2 Task completion

A task is complete when every CODE block that lists it in `completes` has run successfully.

### 7.3 The loop

1. **Pass 1** runs, in document order, every block. A block with unmet `depends_on` is skipped (not
   run). Blocks without a `tasks` declaration run in this pass.
2. A block that **aborts** (§7.4) is treated as *not run yet*; its output, recorded reads and variable
   definitions are discarded.
3. Each later pass runs, in document order, the blocks not run yet whose dependencies are now complete.
4. Stop when every block has run (**success**), or when a pass runs no block (**dead end**): the
   remaining dependencies cannot be satisfied. The dead-end report lists every block not run, with its
   unmet tasks and missing variables.

Every block runs at most once per update.

### 7.4 Aborts

A running block aborts (and counts as not run yet) when:

- it reads a variable that no block has defined yet (§7.5);
- a declared dependency is not complete (only possible if declarations change mid-run; normally decided
  before the block starts).

Any other Starlark error fails the update with the error and the block's location.

### 7.5 Variables [decided]

Blocks exchange values through host functions, not bare globals (D21):

- `define("name", value)` — publishes a frozen value; defining a name that is already defined is an
  error;
- `define("name", value, lenient=True)` — may redefine the name; meant for aggregation, where several
  blocks contribute to one value;
- `var("name")` — reads it; aborts the block as *not run yet* if it is not defined yet;
- `var("name", default=value)` — returns `value` instead of aborting when the name is not defined.

**Block-local state (D30).** `define` writes to the running block's own layer; `var` reads that layer
first, then the shared state, so a block sees its own definitions. When the block finishes, its layer is
merged into the shared state and its tasks are credited. When it aborts, the layer is discarded — nothing
it defined before the abort becomes visible. A finished block never runs again in the same update, so a
later `lenient=True` redefinition does not affect blocks that already read the variable.

[proposed] Values are plain data — none, bool, int, float, string, list, dict — copied on `define` and
rebuilt on `var`. A defined value is therefore deeply immutable, and each block runs on its own
Starlark heap. Functions and other non-data values cannot be defined.

This makes variable passing an implicit dependency, so a `depends_on` task is only needed for dependencies no
variable expresses (typically "the document's headings are final"). A misspelled variable surfaces as a
dead end; the dead-end report names the variable so the typo is obvious.

### 7.6 Reading the document

Host functions that read the document (`headings()`, `document()`, `here()`, …) see the document as it
stands at that moment, including output already spliced in during this update. Ordering such reads
after the blocks that generate headings is exactly what tasks are for.

### 7.7 Inline values [decided forms, details proposed]

Values can be shown inside running text (D25):

- `<!--$var-->value` — the short form, for humans to write by hand. The value is a single word: it ends
  at the first whitespace. If the current value contains whitespace, `update` reports an error naming the
  long form.
- `<!--= expr-->value<!--/=-->` — the long form; the value may contain anything except the closing
  marker.

[proposed] Both forms take a variable path: a name followed by `.key` and `[index]` steps
(`app.version`, `ports[0]`). The value must be a string, number or bool (bools render as `true` /
`false`). Inline values are filled after all CODE has run. References inside code blocks, code spans,
marklark block comments and output regions are left alone. An undefined variable, a non-scalar value, or
a long form without its closing marker fails the update with the line.

### 7.8 Determinism check

Given the same inputs, an update must produce identical output. `marklark check --determinism` runs the
whole evaluation twice and fails on any difference; this catches a host function that is accidentally
not hermetic.

## 8. Starlark and the security model [decided principles, details proposed]

- **Language**: Starlark via `starlark-rust`. Deterministic; no I/O, time, environment or network except
  through host functions. Dialect flags (termination, recursion, string formatting) are pinned during
  development (D23); CODE needs top-level `for`, which the standard dialect rejects.
- **All I/O goes through host functions**, and the host records every read with its hash. Benefits:
  dependency tracking without declaring deps, precise diagnostics ("`slice()` at line 2: `start` matched
  nothing"), and the `broken` state below.
- **File access**: read-only, confined to the project root — the git repository root; outside git the
  nearest directory at or above the document containing `marklark.toml`; otherwise the document's
  directory (D22); paths are resolved and
  symlinks that escape the root are refused. **Gitignored files are refused by default**, so an injected
  intent cannot copy `.env` into a document that is later published. Outside git, marklark still honours
  `.gitignore` files under the project root and always refuses a built-in deny list (`.env`, `*.pem`,
  `id_*`, …) (D34).
- **Writes**: CODE can only produce text for its own output region (plus `define`d values). It never
  modifies the rest of the document (D28). Invoking external tools and writing other files (e.g.
  rendered diagrams) is phase 2 (D27).
- **Escape hatch** to real processes (`run_script`) — phase 2 (D27); if added, it sits behind an explicit
  per-project trust gate, which then stays the rare exception.
- **Prompt injection**: INTENT text from a document of unknown origin is untrusted input to the agent.
  The sandbox bounds the damage: even malicious CODE can only put wrong text into the document.

## 9. States [decided]

Computed deterministically, per group:

| State | Condition | `update` | `intent-check` | with `--strict` |
|---|---|---|---|---|
| `none` | CODE with no INTENT and no `intent` stamp | runs | — | — |
| `unresolved` | INTENT with no CODE after it and no matching group elsewhere | warns | error | error |
| `detached` | INTENT with no CODE after it, but a group elsewhere carries its hash | warns | error | error |
| `unverified` | INTENT followed by CODE without stamps | runs | warning | error |
| `intent_changed` | group stamp ≠ current INTENT hash | runs | error | error |
| `params_edited` | a member's `params` stamp ≠ its code | runs | warning | error |
| `orphan` | a CODE block with an `intent` stamp that belongs to no group | runs | warning | error |
| `broken` | a member fails at run time (missing file, no match, …) | fails | error | error |
| `needs_update` [proposed] | an AI block's inputs changed, or it has no dependencies yet (§12) | — | warning | error |
| `output_edited` [proposed] | an AI block's text was edited by hand (§12) | — | error | error |
| `verified` | everything matches and runs | runs | — | — |

`update` never blocks on intent drift: the document builds as it did before. The hard failures in
`update` are `broken`, a dead end, a Starlark error, and an edited output region.

## 10. Writing the document [decided]

- **Splice by byte range.** marklark never re-renders a document from a parse tree. A Markdown parser
  (`comrak` or `pulldown-cmark`) is used only to locate fenced code blocks and headings by source
  position.
- **Generated output is guarded.** If an output region no longer matches its `generated` stamp, `update`
  stops with an integrity error naming the block; deleting the stamp line accepts the loss.
- **Backups**: a `.bak` copy is written unless git already holds the file's content (tracked, no
  unstaged or staged changes); without git it is always written. `--bak` / `--no-bak` override.
- **Git is optional** (D32): marklark works without it; no stamp, state or CODE semantics depend on it.

## 11. Interfaces [proposed]

Reporting commands can emit JSON (D29). Exit codes are stable once released (D35) [values proposed]:
`0` success; `1` a document has a problem (block errors, a failed CODE block, a dead end, an edited
output region); `2` marklark could not do its job (a file could not be read or written).

CLI:

```text
marklark update FILE...           run all CODE blocks, splice output
marklark check FILE...            parse, declarations, static task graph, integrity — runs nothing
marklark check --determinism ...  run twice, compare
marklark intent-check FILE...     report the §9 states; exit 1 if any error
marklark intent-accept FILE --line N [--drop-stamp]
marklark mcp                      start the MCP server on stdio
```

MCP (via `rmcp`, `marklark mcp` on stdio): `intent_list`, `intent_context` (intent text, group members,
states, failure detail, current output, agent-owned stamps — the agent's evidence for finding renames,
e.g. in git history), `intent_apply` (accepts agent-owned `x-` stamps; atomic group replacement +
stamping), plus read-only discovery primitives (`list_headings`, `get_section`, `get_lines`) so agents
need not read whole files.

[proposed] Also: `intent_check` and `update` (as the CLI commands), and for AI blocks `ai_resolve`
(store dependencies), `ai_context` (prompt + extracted dependencies) and `ai_update` (store prose).
`intent_apply` runs the whole update in memory first and writes nothing if it fails.

## 12. AI blocks [decided]

The `<!--AI-->` block is mdship's AI placeholder carried over by concept (D33, D36). `marklark update`
never touches it.

```markdown
<!--INTENT
Write a two-paragraph introduction to the session handling, from SessionManager in src/auth.py.
Follow the style in docs/brief.md.
-->
<!--AI
#@ intent: blake3:…
#@ dep: slice("src/auth.py", start="class SessionManager", end="^class ") blake3:…
#@ dep: read("docs/brief.md") blake3:…
#@ deps: blake3:…
#@ generated: 812:blake3:…
-->
…agent-written prose…
<!--/AI-->
```

- **Prompt**: the governing INTENT — what to write and which sources it may come from. The AI block's
  opening comment holds only `#@` stamps; there is no human-edited part.
- **Resolution** (once): the agent derives the dependencies from the INTENT; they are stored as `dep`
  stamps, and `deps` hashes their specifications.
- **Generation** (when an input changed): marklark extracts every dependency and gives the content to the
  agent (mdship's `ai_context`); the agent returns the prose (mdship's `ai_update`); marklark writes each
  dependency's content hash and the `generated` stamp. The agent never reads dependency files itself.
- **`dep` format**: a `slice(...)` or `read(...)` call, evaluated with the same host functions CODE uses,
  followed by the hash of the extracted content.
- **Brief**: no separate concept; a brief named in the INTENT is an ordinary dependency.
- **Needs regeneration** when the `intent` stamp, a dependency hash, or the `deps` hash no longer matches.
- **Hand-edited prose** (output ≠ `generated`) blocks regeneration until accepted, as in mdship.
- **Hand-edited dependencies** are allowed: a human may edit, add or remove `dep` lines (a new line may
  omit the hash). The `deps` mismatch is reported like `params_edited`; `marklark intent-accept`
  re-stamps `deps`, and the next generation fills in the content hashes.
- **Boundary**: content always ends at `<!--/AI-->`, written by marklark's apply tool together with the
  block. No `_terminate_`, no heading-based boundary.
- **Not carried over** from mdship: YAML (D16), `brief:`, `_terminate_`.
- **Creation** [proposed]: a human writes the empty `<!--AI--> <!--/AI-->` after the INTENT — making a
  region generative is the user's decision (D13). Agents fill it in; they never insert one.
- **`dep` restriction** [proposed]: because a `dep` line is human-editable text that marklark evaluates,
  it must be a single `read(...)` or `slice(...)` call with literal arguments, on one line. It runs in
  the same sandbox as CODE, with paths relative to the document.
- **States** (§9): `unverified` and `intent_changed` as for CODE; edited `dep` lines are reported as
  `params_edited`; changed inputs as `needs_update`; hand-edited text as `output_edited`.
