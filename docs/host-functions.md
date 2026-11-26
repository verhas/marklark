# Host functions (draft)

All I/O in CODE goes through these functions; the host records every read with its hash. Everything here
is **[proposed]** — names and signatures are a first cut.

**Implemented** (2026-11-26): `read`, `slice`, `glob`, `json`, `toml`, `re_search`, `re_findall`, `re_sub`,
`reindent`, `fence`, `headings`, `anchor`, `here`, `document`, `define`, `var`, `emit`, `tasks`.
Flags for the regex functions: `i`, `m`, `s`, `x`. Not yet: `xml`.

The sizing question from the design discussion was: *can a small library cover everything mdship's
placeholder family does?* If ~10–20 functions suffice, one programmable construct is clearly better than
a placeholder zoo; if it took ~40, the placeholders were capturing more than it seemed. The mapping below
lands at **about 20 functions**, most of them small.

## 1. The library

### Reading files

| Function | Returns | Notes |
|---|---|---|
| `read(path)` | str | Whole text file |
| `slice(path, start=None, end=None, range=None, section=None, include_start=False, include_end=False, repeat=False)` | str | mdship INCLUDE/AI-dep slicing. A `start`/`end` that never matches is an error, never "include nothing". `repeat=True` concatenates every start/end segment |
| `glob(pattern)` | list[str] | Sorted, inside the project root, gitignored files excluded |
| `json(path)` / `toml(path)` | value | Parsed into plain data (TOML dates become strings). No `yaml()`: no YAML (D16) |
| `xml(path, xmlns=None)` | value | Wrapped in the root element name; namespaces stripped unless mapped in `xmlns` (mdship IMPORT semantics) |

### Text helpers

Starlark has no regular expressions, so they must come from the host.

| Function | Returns |
|---|---|
| `re_search(pattern, text, flags="")` | None, or a struct: `text`, `groups` (list; None for groups that did not take part), `named` (dict), `start`, `end` (character offsets) |
| `re_findall(pattern, text, flags="")` | list — like Python: strings without groups, the group with one, lists with several |
| `re_sub(pattern, repl, text, count=0, flags="")` | str — in `repl`, `$1` / `${name}` refer to groups, `$$` is `$` |
| `reindent(text, margin)` | str — mdship INCLUDE `margin` |
| `fence(text, lang="")` | str — wraps in a code fence longer than any backtick run inside |

### Document

| Function | Returns |
|---|---|
| `document()` | str — the current document text (§7.6 of the spec) |
| `headings(min_level=1, max_level=6)` | list of `{level, text, line, anchor, path}` |
| `anchor(text)` | str — the GitHub-style slug, same algorithm as `headings()` |
| ~~`front_matter()`~~ | dropped: front matter is YAML (D16) |
| `here()` | `{line, next_line}` — this block's position (`next_line`: the first line after the block, or None); covers mdship SUP |

### Variables and output

| Function | Effect |
|---|---|
| `define(name, value, lenient=False)` | Publish a frozen value; dotted names create nesting. Redefining a name is an error unless `lenient=True` (spec §7.5) |
| `var(name, default=…)` | Read a value — the block's own definitions first, then the shared state. If undefined: returns `default` when given, otherwise aborts the block as *not run yet* (spec §7.5) |
| `emit(text)` | Append to the block's output region |

### Declarations (not host I/O, listed for completeness)

`tasks(*, depends_on=[], completes=[])` — spec §7.1.

## 2. mdship feature → marklark

| mdship | marklark |
|---|---|
| SET | `define("app", {"name": "MyApp", "version": "1.0"})` |
| IMPORT (JSON/YAML/TOML/XML) | `define("pom", xml("pom.xml"))` |
| SLURP (regex with name+value groups, strategies) | `re_findall` over `read`/`glob`, strategy is a few lines of code |
| SIP (named vars, one group each) | `re_search` per variable; `strategy: fail` = check the match count |
| SUP (value from the next line) | `re_search(p, here().next_line)` |
| INCLUDE (`from`, `range`, `start`/`end`, `section`, `prefix`/`postfix`, `margin`) | `emit(fence(reindent(slice(...), n), "python"))` |
| JINJA2 / TEMPLATE | Starlark string building with `var(...)` values; no template engine needed |
| TOC | `headings()` + `anchor()` in a block whose `tasks(depends_on=[...])` names the tasks that generate headings |
| PYTHON `define:` / `run:` | Just CODE |
| `transform:` / `audit:` hooks | Just code inside the block |
| Variable references `<!--$var-->` | `<!--$var-->value` (single word) and `<!--= expr-->value<!--/=-->` (spec §7.7) |
| AI placeholder | `<!--AI-->` block, same concept as mdship (D33, spec §12) |
| AI `deps:` | Declared in the AI block, as in mdship (spec §12) |
| MERMAID (renders an image file with an external tool) | **Not covered**: external tools and file generation are phase 2 (D27) |
| Front-matter heading numbering | **Not covered**: CODE never edits the whole document (D28); use mdship |
| `fix-headings`, `reflow`, `find-replace`, tables, … | Editing commands, not placeholders; out of scope for CODE. Possibly separate CLI commands later |

## 3. Notes

- Every reading function fails loudly with the call site in the message; that is what produces the
  `broken` state and its diagnostics.
- Functions that read files take paths relative to the document's directory, resolved and confined to
  the project root.
- No function exposes time, environment variables, randomness, the network, or process execution.
