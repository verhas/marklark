# Host functions — reference

CODE blocks are Starlark with no ambient I/O: everything outside the block's own computation goes
through the functions below. This is the reference for what is implemented; `docs/agent-guide.md` is the
shorter guide for agents (rules, workflow, patterns).

Notation: in a signature, parameters before `*` may be given by position; parameters after `*` only by
name (`slice("a.py", start="x")`, not `slice("a.py", "x")`). Parameters with `=` are optional.

## 0. Common rules

**Paths.** Every path is a string naming a file of the project:

- relative paths resolve against the **document's directory**: `read("data/app.toml")`;
- a path starting with `//` resolves against the **project root**: `read("//VERSION")` (a single `/`
  works too; `//` is the documented form, the same as in `load`);
- `..` may not leave the project root, and symlinks that point outside it are refused;
- gitignored files, anything inside `.git/`, and the built-in deny list (`.env`, `.env.*`, `*.pem`,
  `*.key`, `id_rsa*`, `id_dsa*`, `id_ecdsa*`, `id_ed25519*`) are refused, with or without git (D18, D34);
- a path argument that contains a line break, or is longer than 1,024 characters, is refused: it is
  file content passed where a path belongs (`toml(read("x.toml"))` — write `toml("x.toml")`). The
  message gives only the length, never the content.

Inside a library function (§9) paths still resolve against the document of the block that calls it.

**Errors.** Every function fails loudly: a missing file, a refused path, a pattern that matches
nothing, a malformed data file. The error stops the update and names the document line and column of
the call. Nothing fails silently or returns "nothing" instead.

**Recorded reads.** Every file a block reads is recorded with its hash (D6).

**Plain data.** "Data" below means none, bool, int, float, string, list and dict — what `define` can
store (§7). Dicts keep the order of the source file. Values are indexed as in Python: `d["key"]`,
`xs[0]`.

## 1. Output

### `emit(text)`

Appends `text` to the block's output region. `text` must be a string — use `str(x)` for other values.

- The block needs an output region (`<!--/CODE-->` after it); `intent_apply` adds it for any block that
  calls `emit`. Calling `emit` in a block without one is an error.
- When the block finishes, the output gets the document's line ending, and a final line ending if it
  has none.
- Output that contains a marklark block marker at the start of a line is an error.

## 2. Reading files

### `read(path)` → string

The whole file, which must be UTF-8 text.

### `slice(path, *, start=None, end=None, include_start=False, include_end=False, repeat=False, range=None, section=None)` → string

A piece of a text file, with mdship INCLUDE semantics. Use **one** of three selectors; combining them is
an error. Without a selector, the whole file is returned.

**`start` / `end`** — regular expressions (§4), each tested against one line at a time (without its line
ending):

- `start` alone: from the line after the first matching line to the end of the file;
- `end` alone: from the beginning of the file up to (not including) the first matching line;
- both: from after the first `start` line up to the next `end` line. While a piece is open only `end`
  is tested, so the same pattern may open and close it (`start="// MARK", end="// MARK"`);
- `include_start=True` / `include_end=True` keep the matching lines themselves;
- `repeat=True` continues after each `end`: every `start` … `end` piece is concatenated. Every `start`
  must then be followed by an `end`;
- a `start` or `end` that matches no line is an error.

**`range="x..y"`** — lines x to y, 1-based, inclusive. `1 <= x <= y <= ` number of lines, else an error.

**`section="Title"`** — a Markdown heading whose text equals `Title` (case-insensitive; a leading
numbering such as `2.3.` is ignored on both sides), plus everything up to the next heading of the same
or a higher level. No such heading is an error.

Lines keep their own line endings.

```python
slice("src/App.java", start="// START", end="// END")
slice("src/app.py", start=r"^def main\(", end=r"^\S", include_start=True)   # one top-level function
slice("CHANGELOG.md", section="2.1.0")
slice("notes.txt", range="1..10")
```

### `glob(pattern)` → list of strings

The readable files matching `pattern`, sorted. The pattern is relative to the document's directory (or
starts with `//` for the root). `*` and `?` stay within one directory; `**` crosses directories:
`glob("src/**/*.py")`. Ignored and denied files are never listed. The returned paths can be passed to the
other functions: relative to the document's directory when the file is below it, otherwise starting with
`//`.

### `json(path)` → data

The parsed JSON file.

### `yaml(path)` → data

The parsed YAML file. With several YAML documents in the file, the first is used; an empty file gives
`None`. Mapping keys become strings. `.inf` and `.nan` stay text.

### `toml(path)` → data

The parsed TOML file. Dates and times become strings.

### `xml(path, *, xmlns=None)` → data

The parsed XML file, with mdship IMPORT's conventions:

- the result is wrapped in the root element's name: `xml("pom.xml")["project"]["version"]`;
- attributes become keys `@name`;
- an element with only text is that text; an element with text and attributes or children keeps the text
  under `_text`; an empty element is an empty dict;
- repeated child elements become a list (a single one is not a list);
- namespaces are stripped. `xmlns` maps names to namespace URLs that must not be stripped; their elements
  and attributes become `name_local`: `xml("x.xml", xmlns={"m": "http://maven.apache.org/POM/4.0.0"})`;
- two different names that would become the same key (e.g. `<a:id>` and `<b:id>`) are an error — map one
  of the namespaces with `xmlns`;
- XML with a DTD is refused.

## 3. The document

### `headings(*, min_level=1, max_level=6)` → list of structs

The document's headings in order, as structs with the fields:

| Field | |
|---|---|
| `level` | 1–6 |
| `text` | the heading's text, inline markup removed |
| `line` | 1-based line |
| `anchor` | the GitHub-style link anchor, unique in the document (`-1`, `-2`, … for repeats) |
| `path` | list of the parent headings' texts, outermost first |

Headings in code blocks and in the YAML front matter are not headings. Headings generated by other
blocks earlier in the same update are included — order the blocks with `tasks` (§8) so the generating
ones run first. Fields are accessed with a dot: `h.text`.

### `anchor(text)` → string

The GitHub-style anchor of a heading text: lowercase; letters, digits, `_` and `-` kept; spaces become `-`;
everything else dropped.

### `front_matter()` → data or `None`

The document's YAML front matter (a `---` line at the very start, up to the next `---` or `...` line),
parsed. `None` when the document has none.

### `here()` → struct

| Field | |
|---|---|
| `line` | 1-based line of this block's opening `<!--CODE` |
| `next_line` | the first line after the block, without its line ending; `None` at the end of the document |

### `document()` → string

The whole document as it stands now, including output spliced in earlier in this update.

## 4. Text

Regular expressions use the syntax of Rust's `regex` crate: like Python's `re`, without look-around and
backreferences. `flags` is a string of: `i` ignore case, `m` multi-line (`^`/`$` at line breaks), `s` `.`
matches a line break, `x` verbose. Other letters are an error. Write patterns as raw strings: `r"\d+"`.

### `re_search(pattern, text, flags="")` → struct or `None`

The first match, or `None`:

| Field | |
|---|---|
| `text` | the matched text |
| `groups` | list of the groups' texts, `None` for a group that did not take part (`m.groups[0]` is group 1) |
| `named` | dict of the named groups (`(?P<name>…)`) |
| `start`, `end` | character offsets of the match in `text` |

### `re_findall(pattern, text, flags="")` → list

All non-overlapping matches, like Python: without groups a list of the matched texts; with one group a
list of that group's texts; with several groups a list of lists.

### `re_sub(pattern, repl, text, count=0, flags="")` → string

Replaces matches: all of them, or the first `count`. In `repl`, `$1` or `${name}` insert a group and `$$` is
a literal `$` (not Python's `\1`).

### `reindent(text, margin)` → string

Re-indents `text` so that its least indented non-blank line starts with exactly `margin` spaces, keeping
the relative indentation of the others. Blank lines become empty. Both arguments by position.

### `fence(text, lang="")` → string

Wraps `text` in a Markdown code fence that is longer than any run of backticks inside it (at least three):
the opening fence with `lang`, the text with a final line ending, the closing fence and a line ending.

## 5. Formatting in Starlark itself

`%s`, `%d`, `%r` and `"{}".format(...)` work. There are no widths, precision or alignment: `%3d`, `{:>3}`,
`.rjust()` and `.ljust()` fail. Pad by hand: `" " * (3 - len(s)) + s`. There are no f-strings.

## 6. Starlark built-ins available

`len`, `range`, `str`, `int`, `float`, `bool`, `repr`, `type`, `list`, `dict`, `tuple`, `sorted` (with `key=`,
`reverse=`), `reversed`, `enumerate`, `zip`, `min`, `max`, `any`, `all`, `hasattr`, `getattr`, `dir`, `hash`,
`fail(message)`, `struct(**fields)`, and the usual string, list and dict methods. `def`, `lambda`,
comprehensions, and top-level `if` / `for`.

Not available: `while`, f-strings, `print`, `set`, `map`, `filter`, exceptions (`try`), classes, recursion
(it ends in a stack overflow), and anything about time, randomness, the environment, the network or
processes.

## 7. Sharing values between blocks

### `define(name, value, *, lenient=False)`

Publishes `value` under `name` for later blocks (and for inline values).

- `value` must be plain data; a struct becomes a dict; functions and other values are an error.
- A dotted name creates or extends nested dicts: `define("app.version", "1.0")` gives
  `{"app": {"version": "1.0"}}`. A part that already exists and is not a dict is an error.
- Defining a name that is already defined is an error, unless `lenient=True` (D21).
- The definition becomes visible to other blocks only when this block finishes; if the block stops
  early (see `var`), its definitions are discarded (D30).

### `var(name, *, default=…)` → data

Reads a variable: this block's own definitions first, then those of blocks that finished. Dotted names
read nested values: `var("app.version")`.

- Not defined, without `default`: the block stops and runs again in a later pass, after other blocks have
  run. If no block ever defines it, the update ends in a *dead end* that names the variable.
- Not defined, with `default`: returns `default`.
- The value is **frozen**: `v["k"] = …` or `xs.append(…)` on it is an error. Build a new value —
  `dict(v.items())`, `list(xs)`, `xs + [item]` — and publish it with `define(…, lenient=True)`.
- The value is a snapshot taken at the call: after redefining a name, call `var` again to see the new
  value.

## 8. Ordering blocks

### `tasks(*, depends_on=[], completes=[])`

Declares the named tasks the block waits for and the tasks it helps complete. A task is complete when
every block that lists it in `completes` has run; a block runs only after all tasks in its `depends_on`
are complete.

- It must be the block's first statement (`load` statements may come before it), at most once.
- Both arguments must be list literals of string literals: `tasks(completes=["toc"])`.
- Declarations are collected from all blocks before anything runs. A task that some block depends on but
  no block completes, and a cycle between tasks, are errors.
- At run time the call does nothing.

## 9. Libraries

### `load("path.star", "name", ..., alias = "name")`

Starlark's `load` statement, for shared functions in `.star` files of the project (D40).

- `//` paths are relative to the project root; other paths to the file that contains the `load` (the
  document, or another library). Only `.star` files can be loaded.
- A library's top level may define functions and constants only: `emit`, `define`, `var` and `tasks` fail
  there. Its functions, called by a block, run as part of that block and may use every host function;
  their paths resolve against the document.
- A library is loaded once per update and reused, but every block that uses it must `load` it itself.
- A library may load other libraries; a load cycle is an error.
- Blocks verified against their INTENT record the libraries they use in `#@ lib:` stamps, so a library
  change shows as `lib_changed` (spec §9).

## 10. mdship feature → marklark

| mdship | marklark |
|---|---|
| SET | `define("app", {"name": "MyApp", "version": "1.0"})` |
| IMPORT (JSON/YAML/TOML/XML) | `define("pom", xml("pom.xml"))` |
| SLURP (regex with name+value groups, strategies) | `re_findall` over `read`/`glob`; a strategy is a few lines of code |
| SIP (named vars, one group each) | `re_search` per variable; `strategy: fail` = check for `None` and `fail(...)` |
| SUP (value from the next line) | `re_search(p, here().next_line)` |
| INCLUDE (`from`, `range`, `start`/`end`, `section`, `prefix`/`postfix`, `margin`) | `emit(fence(reindent(slice(...), n), "python"))` |
| JINJA2 / TEMPLATE | Starlark string building with `var(...)` values; no template engine needed |
| TOC | `headings()` + `anchor()` in a block whose `tasks(depends_on=[...])` names the tasks that generate headings |
| PYTHON `define:` / `run:` | Just CODE |
| `transform:` / `audit:` hooks | Just code inside the block |
| Variable references `<!--$var-->` | `<!--$var-->value` (single word) and `<!--= path-->value<!--/=-->` (spec §7.7) |
| AI placeholder | `<!--AI-->` block, created by the agent when the INTENT asks for prose (D33, D36, D39, spec §12) |
| AI `deps:` | `#@ dep:` stamps derived by the agent from the INTENT (spec §12) |
| MERMAID (renders an image file with an external tool) | **Not covered**: external tools and file generation are phase 2 (D27) |
| Front-matter heading numbering | **Not covered**: CODE never edits the whole document (D28); use mdship |
| `fix-headings`, `reflow`, `find-replace`, tables, … | Editing commands, not placeholders; out of scope for CODE |
