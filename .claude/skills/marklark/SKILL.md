---
name: marklark
description: "Use when maintaining Markdown documents with marklark: resolving <!--INTENT--> blocks, writing <!--CODE--> blocks (Starlark) or <!--AI--> blocks, using the marklark MCP tools, or running marklark update / check / intent-check. Covers the rules, the workflow, the Starlark subset, every host function, and patterns."
---

# marklark — guide for agents

marklark maintains Markdown documents. A human writes an `<!--INTENT-->` block in plain prose; you
satisfy it — usually with a `<!--CODE-->` block of hermetic Starlark that marklark runs deterministically
on every update, sometimes with an `<!--AI-->` block of prose. Work through the marklark MCP tools, never
by editing the document's marklark blocks directly.

## The rules

1. **Never edit an INTENT.** It is the human's. If it is unclear or impossible, say so; do not reword it.
2. **Never edit `#@` stamp lines or generated output by hand.** marklark writes them. A hand edit of
   generated output blocks the next update until a human accepts it.
3. **CODE by default; prose only when the human asks.** If the INTENT can be met by reading files and
   computing text, write CODE. Use an AI block only when the human asks for prose — in the INTENT
   ("write a summary…") or in their request to you. Never switch an intent that has CODE to prose.
4. **Always pass the intent hash.** Every writing tool takes `line` and `intent` (the `intent_hash`).
   Line numbers shift after every write; list the intents again before the next write.
5. **No `-->` anywhere in CODE**, not even in a string or a comment: it ends the HTML comment. Write
   `"--" + ">"`.

## Workflow (MCP)

| Step | Tool |
|---|---|
| Find work | `intent_list` (every intent, what satisfies it, its states, its `intent_hash`), `intent_check` |
| Understand one intent | `intent_context` — its text, current code, stamps, output, states, failures |
| Look at the document cheaply | `list_headings`, `get_section`, `get_lines` |
| Satisfy / change an intent with CODE | `intent_apply` with all the intent's blocks; it checks, stamps, runs the whole document in memory, and writes nothing if anything fails |
| Re-run everything | `update` |
| Prose (only when asked) | `ai_resolve` (creates the AI block if needed, stores the dependencies) → `ai_context` (their content) → `ai_update` (your prose) |

States you will see: `unresolved` (no code yet), `unverified` (code never checked against the intent),
`intent_changed` (the human edited the intent — re-derive the code), `params_edited` (someone edited the
code — check it still fits), `broken` (it fails when run), `orphan`, `detached`, and for AI blocks
`needs_update` (an input changed) and `output_edited` (a human edited the prose — leave it; a human
accepts it with `marklark intent-accept`).

## The language: Starlark

Starlark looks like Python. In CODE you have: `def` (no recursion — it overflows), `lambda`, `if`/`for` at
the top level, list and dict comprehensions, strings with `%` and `.format()`, `len`, `range`, `sorted`
(with `key=`, `reverse=`), `enumerate`, `zip`, `min`, `max`, `any`, `all`, `int`, `float`, `str`, `repr`,
`type`, `struct(a=1)`, `fail("message")`, and the usual string, list and dict methods.

Formatting: `%s`, `%d`, `%r` and `"{}".format()` work, but there are **no widths or alignment** —
`%3d`, `{:>3}`, `.rjust()` and `.ljust()` fail. Pad by hand: `" " * (3 - len(s)) + s`.

Not available: `while`, f-strings, `import`, `print`, `set`, `map`/`filter`, classes, exceptions
(`try`), time, randomness, environment variables, network, processes.

## Host functions

Paths are relative to the document's directory (`//path` from the project root) and must stay inside
the project. Gitignored files and
secrets (`.env`, `*.pem`, keys) are refused. **File functions take a path and read the file themselves:
`toml("data/app.toml")`, not `toml(read(...))`.** Every function fails loudly — a pattern that matches
nothing is an error, never an empty result.

### Output

| Function | |
|---|---|
| `emit(text)` | Appends to the block's output. `intent_apply` adds the `<!--/CODE-->` region for blocks that emit. A trailing newline is added if missing. |

### Reading files

| Function | Returns |
|---|---|
| `read(path)` | the whole text file |
| `slice(path, start=, end=, include_start=False, include_end=False, repeat=False)` | lines after the first line matching the regex `start`, up to the next line matching `end`; `repeat=True` joins every such segment |
| `slice(path, range="10..20")` | lines 10–20, 1-based, inclusive |
| `slice(path, section="Title")` | a Markdown heading (case-insensitive, numbering ignored) and everything up to the next heading of the same or a higher level |
| `glob(pattern)` | matching paths, sorted (`*` stays within a directory, `**` crosses directories) |
| `json(path)`, `yaml(path)`, `toml(path)` | the parsed file as plain data |
| `xml(path, xmlns={"m": "http://…"})` | `{root: …}`; attributes as `@name`, text as `_text` (or the element is just its text), repeated elements as lists; namespaces stripped unless mapped, then `m_name` |

### Text

| Function | Returns |
|---|---|
| `re_search(pattern, text, flags="")` | `None`, or a struct: `text`, `groups` (list), `named` (dict), `start`, `end` |
| `re_findall(pattern, text, flags="")` | list of strings, or of lists when the pattern has several groups |
| `re_sub(pattern, repl, text, count=0, flags="")` | the replaced text; in `repl`, `$1` / `${name}` refer to groups |
| `reindent(text, margin)` | the least indented line gets exactly `margin` spaces |
| `fence(text, lang="")` | a Markdown code fence long enough for the text |

Regex flags: `i` ignore case, `m` multi-line, `s` dot matches newline, `x` verbose. The syntax is Rust's
`regex` crate (like Python's, without look-around or backreferences).

### The document

| Function | Returns |
|---|---|
| `headings(min_level=1, max_level=6)` | structs `level`, `text`, `line`, `anchor`, `path` (parent heading texts) — including headings other blocks generate |
| `anchor(text)` | the GitHub-style link anchor of a heading text |
| `front_matter()` | the document's YAML front matter as data, or `None` |
| `here()` | struct `line` (this block), `next_line` (the line after the block) |
| `document()` | the whole document text as it stands |

### Sharing values between blocks

| Function | |
|---|---|
| `define(name, value, lenient=False)` | publishes plain data (none, bool, int, float, string, list, dict); dotted names nest: `define("app.version", v)`. Defining a name twice is an error unless `lenient=True`. |
| `var(name, default=…)` | reads it; without a default, the block waits until another block defines it (blocks run in passes, in document order) |
| `tasks(depends_on=["…"], completes=["…"])` | first statement only, literal lists: this block runs after every block that completes the tasks it depends on |

Values from `var()` are frozen: `v["k"] = …` or `xs.append(…)` on them is an error. Build a new value —
`d = dict(var("x").items())`, `xs = list(var("xs"))`, `var("xs") + [item]` — and publish it with
`define(name, value, lenient=True)`. Values are dicts and lists, indexed as in Python: `v["version"]`,
not `v.version`. `var()` returns the value at the moment of the call: after redefining a name, call
`var()` again to see the new value — a value read earlier keeps the old content.

A block's definitions become visible to others only when it finishes. A block that waits for something no
block ever defines is a *dead end*; the error names the variable.

Inline values in running text, filled after all CODE has run: `<!--$app.version-->1.2.3` (one word) and
`<!--= app.name-->My App<!--/=-->` (any text). The value must be a string, number or bool.

### Libraries

Shared functions live in `.star` files: `load("//lib/docs.star", "include")` (`//` from the project
root; otherwise relative to the loading file). When several blocks need the same formatting or
extraction, put it in a library — one place for the human to review and change. A library's top level
may only define functions and constants (no `emit`, `define`, `var`, `tasks`); its functions may use
every host function when a block calls them, with paths relative to the document. Every block that uses
a library loads it itself. If the library you need does not exist, write it as a file (tell the human),
then load it.

## Patterns

A code excerpt between markers in a source file:

```python
emit(fence(slice("../src/greet.py", start="# START greet", end="# END greet"), "python"))
```

A whole function, from its `def` to the next top-level line:

```python
emit(fence(slice("../src/greet.py", start=r"^def greet\(", end=r"^\S", include_start=True), "python"))
```

A value from a data file, used in several places:

```python
define("version", toml("../Cargo.toml")["package"]["version"])
```

A table from JSON:

```python
s = json("data/settings.json")
emit("| Key | Value |\n|---|---|\n")
for k in sorted(s.keys()):
    emit("| %s | %s |\n" % (k, s[k]))
```

A table of contents that includes headings other blocks generate:

```python
tasks(depends_on=["sections"])          # the generating blocks declare tasks(completes=["sections"])
for h in headings(min_level=2, max_level=3):
    emit("  " * (h.level - 2) + "- [" + h.text + "](#" + h.anchor + ")\n")
```

A value extracted with a regex:

```python
m = re_search(r'version\s*=\s*"([^"]+)"', read("../build.gradle"))
if m == None:
    fail("no version in build.gradle")
emit(m.groups[0])
```

## Don'ts

- Don't parse by hand what `slice`, `re_search`, `json`, `toml`, `yaml` or `xml` already do.
- Don't pass file content where a path belongs.
- Don't hard-code values the INTENT says come from a file — read the file, so the document follows it.
- Don't make one block do everything; several small blocks sharing values with `define`/`var` are easier
  to review. Give `intent_apply` all of an intent's blocks at once.
- Don't "fix" a `broken` or `intent_changed` state by editing stamps; re-derive the code and apply it.
