# Decision log

Decisions made with the project owner, with their reasoning. All dated 2026-11-26 unless noted; they came
out of one design discussion that began as a feature idea for mdship.

## D1. A new product, separate from mdship

marklark is a new product, not mdship 2.

*Reasoning*: mdship carries a lot of ballast and is written in Python. mdship stays maintained and
pip-installable for enterprise environments where pip is allowed but arbitrary binaries are not; marklark
targets environments where a single binary is fine. The two serve different environments rather than
one replacing the other.

## D2. Rust

*Reasoning*: single static binary, and `starlark-rust` (Meta, used by Buck2) is native, so the
interpreter ships inside the binary with no runtime.

## D3. Name: marklark

"Markdown" + "Starlark". Availability checked 2026-11-26: free on crates.io, PyPI and npm; no GitHub
repository named `marklark`; the GitHub user `MarkLark` exists (a personal account, last updated
2023-05-13, unrelated old Python repos) — judged not a conflict. `marklark.com` is registered;
`marklark.dev` / `.io` / `.org` showed no name servers (probably free, unconfirmed).
Crate name reserved 2026-11-26: placeholder `marklark 0.0.0` published on crates.io (manifest and a
one-line doc comment only, no source and no repository link while the repository is private). Domain
`marklark.dev` registered 2026-11-26 (name.com, one year; renewal due 2027-11-26).

## D4. Two levels: INTENT (human) and CODE (agent)

*Reasoning*: in mdship, humans wrote the declarative placeholders and mdship ran them; that justified a
dedicated placeholder per task. If agents write the executable part, it can be lower level and more
general. But the executable part must stay **analysable, reviewable and safe** — those needs grow, not
shrink, when agents write it, because human review becomes the bottleneck. Hence D5–D6.

## D5. CODE is hermetic Starlark, not Python

*Reasoning*: plain Python would make every update arbitrary code execution, turn the trust prompt into
something users click through, and make INTENT text in a cloned document a prompt-injection path to
execution. Starlark is deterministic, has no ambient I/O, and agents write it fluently because it looks
like Python.

## D6. All I/O through host functions; reads are recorded

*Reasoning*: restores what declarative placeholders gave mdship — the host knows every input and its
hash (dependency tracking with nothing to declare), can say exactly what failed, and confines access.
The former placeholder types become library functions, so agent-written code for simple cases is a
one-liner as reviewable as a placeholder.

## D7. INTENT body is raw prose

Never parsed as YAML. *Reasoning*: lowest friction for humans; a sentence like "version: the one in
the pom" must not become a mapping.

## D8. Standalone INTENT block binding the following block(s)

Chosen over (only) an `intent:` key inside the placeholder. *Reasoning*: the user need not know which
construct satisfies the intent; the agent can change the implementation without touching the human's
text; deleting the code leaves the intent in place, so it can be re-resolved.
(In mdship both forms were discussed; for marklark, whether to also keep a key form is an open question
with a leaning to *no*.)

## D9. Groups identified by the shared `intent` stamp

One INTENT may be satisfied by several consecutive CODE blocks. Membership: consecutive, only blank lines
between members, identical `intent` stamp, no other intent source. The stamp works as a group id even
after the INTENT text changes, because all members still share the old value. Human content between
members breaks the group (by design: marklark cannot tell human text from leftover output).

## D10. Stamps are written only by `intent_apply` / `intent-accept`, never by `update`

*Reasoning*: otherwise a stamp no longer means "this code was checked against this intent".

## D11. Two stamps: intent hash and params hash

*Reasoning*: the two mismatches mean different things — intent edited (the code must be re-derived) vs
code hand-edited (someone must check it still fits). Sources read by the code are **not** hashed into
these stamps: sources are meant to change.

## D12. Severity: `params_edited` is a warning, an error with `--strict`

Same pattern for `unverified` and `orphan`. `unresolved` warns in `update` (errors with `--strict`) and
is always an error in `intent-check`. `update` never blocks on intent drift.

## D13. The resolver never creates generated-text (AI) blocks

*Reasoning*: making a region generative instead of deterministic is the user's decision, not the agent's.

## D14. Evaluation: repeat until done, with tasks

- A block may declare the tasks it takes part in with a single call at its start:
  `tasks(depends_on=[...], completes=[...])`. Both parameters are keyword-only and optional.
- A task is complete when all blocks that list it in `completes` have run.
- A block whose needed task is not complete is aborted and treated as not run yet.
- Blocks with no declarations run in the first pass.
- A pass in which no block runs is a dead end: the dependencies cannot be satisfied.

*Correction (2026-11-26)*: the first draft had two separate functions, `depends_on(...)` and
`completes(...)`. Replaced by one `tasks(...)` call with named parameters. *Reasoning*: with two calls
the order could be wrong — a `depends_on` placed before `completes` would abort the block before it
revealed which tasks it completes, leaving the evaluation clueless about the tasks the block takes part
in. One call states both sides at once, so no such ordering exists.

## D15. Declarations are collected from all blocks before the first pass

The `tasks(...)` call must be the first statement, appear at most once, and take list literals of string
literals. *Reasoning*: if declarations were discovered while running, a block aborted on an unmet
dependency would never reveal what it completes, so a task could look complete before all its
contributors ran. Collecting up front also
catches typos (a needed task nobody completes) and task cycles before anything runs.

## D16. No YAML

INTENT is prose, CODE is Starlark, stamps are simple key/value lines. *Reasoning*: less ballast, one
fewer dependency (and the Rust YAML ecosystem is unsettled).

*Scope (clarified 2026-11-26)*: D16 is about marklark's own constructs. It does not forbid reading YAML
where a document or project needs it: host functions such as `yaml(path)` and `front_matter()` parse
YAML data (via `yaml-rust2`).

## D17. Byte-preserving edits; guarded output; git-aware backups

Carried over from mdship as lessons (see [mdship-lessons.md](mdship-lessons.md)).

## D18. Gitignored files are refused by default

*Reasoning*: prevents an injected intent from copying secrets (e.g. `.env`) into a document that may be
published.

## D19. Stamp syntax: `#@ key: value` on every line

Each stamp is one line `#@ key: value`, written by marklark at the end of the CODE body. Every stamp line
carries the `#@` prefix; there is no header line and no `__stamps__` variable.

*Reasoning*: stamp lines are Starlark comments, so the code parses unchanged. The `@` sets them apart
from ordinary comments (`# intent: …` is a natural thing for an agent to write), so marklark can find,
rewrite and exclude them from the `params` hash without ambiguity. A per-line prefix makes each line
self-identifying: a `#@ stamps` header followed by plain `# key: value` lines would need a rule for
where the trailer ends, and a normal comment added after it would become a malformed stamp or be
swallowed. A `__stamps__ = """…"""` assignment was rejected: it would execute (code could read its own
metadata), it must be excluded from `params` anyway (textually, losing the benefit of being code, or via
the AST, tying stamps to the parser), it reads to a reviewer as code rather than metadata, and it
reserves a name.

## D20. `params` hash over the canonical form of the parsed code

The `params` stamp hashes a canonical printout of the parsed Starlark code, not the source text.
marklark has its own canonical printer over the AST, independent of the `starlark-rust` version. The
stamp value carries the printer version: `#@ params: ast1:blake3:<hex>`.

*Reasoning*: file transfers and editors change line endings, indentation, quotes and spacing. If such a
change counted as a code edit, the `params_edited` warning would send someone hunting for a difference
that is not there. Comparing parsed code makes reformatting and comments invisible; any change to what
the code does is caught. The parser exposes the AST (`AstModule::statement()`, `starlark::syntax::ast`),
and the crate's own `Display` printer was verified (2026-11-26, `starlark_syntax` 0.14.2) to give
identical output for a reformatted block and different output for a semantic edit. The crate's printer
is **not** used for the stamp: its docs say the AST internals change regularly, and a changed printout
after an upgrade would make every stamp mismatch at once. With marklark's own printer an AST change in the
crate breaks the build instead of silently changing hashes; the `ast1` tag allows a deliberate change of
the canonical form later. Side effects: comment-only edits are not edits, and `#@` stamp lines are
excluded automatically. Code that does not parse cannot be hashed, but it cannot run either.

## D21. Variables: `define()` / `var()`; redefinition only with `lenient=True`

Blocks exchange values through host functions, not module globals:

- `define("x", value)` publishes a frozen value; if `x` is already defined it is an error.
- `define("x", value, lenient=True)` may redefine `x` — needed for aggregation, where several blocks
  contribute to one value.
- `var("x")` reads it; if `x` is not defined yet, the block aborts as *not run yet*.

*Reasoning*: an undefined `var` can act as an implicit dependency ("not run yet"), which module globals
cannot — an undefined global is a Starlark compile error. Redefinition is an error by default because
an accidental reuse of a name otherwise silently overwrites a value another block relies on; the
explicit `lenient=True` keeps the deliberate case (aggregation) possible and visible in review.

## D22. Project root

The project root is the git repository root. Outside git it is the nearest directory at or above the
document that contains a `marklark.toml`; without one, it is the document's directory.

*Reasoning*: file access is confined to the project root (D18, spec §8); the root needs a definition that
works with no configuration in the common case (git) and still has a sensible answer outside it.

## D23. `starlark-rust` dialect details are settled during development

Dialect flags, termination (loops, recursion, step limits), string formatting and freezing are verified
and pinned while building the core, not before. They are not blocking. Known so far: the standard dialect
rejects a top-level `for`, so CODE needs the extended dialect or that flag enabled.

*Reasoning*: these are implementation details that can be refactored later; checking them against code
is cheaper than designing them on paper.

*Found during development (2026-11-26, `starlark` 0.14.3)*: CODE uses the extended dialect with `load`
disabled (`src/lang.rs`). `while` is not part of the grammar. Recursion is not rejected, but unbounded
recursion ends in a "call stack overflow" error, so it terminates. Still open: there is no step limit,
so a CODE block with huge nested loops can run for a very long time; the evaluator's statement hook is
crate-private, so a limit needs another mechanism (e.g. a watchdog).

## D24. Generated text is a dedicated `<!--AI-->` construct

An INTENT satisfied by agent-written prose uses its own construct, `<!--AI--> … <!--/AI-->`, not a CODE
block with stamps only. Its stamps (inputs, intent hash, output hash) follow the `#@` syntax (D19).

*Reasoning*: generated prose is a different contract from deterministic code; a separate construct keeps
"CODE always runs deterministically" true. `AI` says plainly what the region is (successor of mdship's AI
placeholder).

## D25. Inline values: a short and a long form

- `<!--$var-->value` — short form for humans. The value is a single word (no whitespace), as in mdship.
- `<!--= expr-->value<!--/=-->` — long form; the value may contain spaces.

*Reasoning*: humans write inline references by hand in running text; the short form is what they will
actually type. The long form covers values the short form cannot delimit.

## D26. Only the standalone INTENT block

There is no `intent:` key inside CODE or any other construct.

*Reasoning*: one form, less to explain (see D8).

## D27. Deferred to phase 2

- **External tools and external file generation** — e.g. Mermaid rendering. This is a general mechanism
  (invoking an external tool, writing files outside the document), not a Mermaid feature. Mermaid image
  rendering is dropped for now.
- **Escape hatch to real processes** (`run_script`).
- **Several documents** — cross-document variables, headings or tasks.

*Reasoning*: none is needed for the core, and each widens the security model; they deserve their own
design pass.

## D28. No whole-document transforms in CODE

CODE does not read-modify or rewrite the whole document. Heading renumbering is not a marklark feature;
anyone who needs it can use mdship's command.

*Reasoning*: renumbering in mdship is a convenience tool outside the placeholder philosophy, not part of
it. Keeping CODE strictly local to its output region is a property worth more than the convenience.

## D29. Machine-readable reports are JSON

`check` and `intent-check` (and other reporting commands) can emit JSON.

## D30. Block-local variable state, committed only when the block finishes

A CODE block changes the shared state in two ways: the tasks it completes (or contributes to) and the
variables it defines. Both are committed only when the block finishes:

- `define` writes to the block's own local layer, not to the shared state.
- `var("x")` reads the block's local layer first, then the shared state. A value the block defined or
  redefined itself prevails.
- When the block finishes, its local layer is merged into the shared state and its tasks are credited.
- When the block aborts (e.g. `var` of an undefined name), its local layer is discarded: no definition
  made before the abort reaches the shared state.
- A block that finished is never run again in the same update, so a later redefinition of a variable it
  read does not affect it.
- `var("x", default=...)` returns the default when `x` is not defined, instead of aborting.

*Reasoning*: an aborted block counts as not run; it must leave no trace, or its partial definitions
would be seen by others and then defined again when it reruns. Committing at the end makes each block an
all-or-nothing step. Because finished blocks never rerun, aggregation with `lenient=True` (D21) needs no
extra rule; `default` lets the first contributor start an aggregate without aborting.

## D31. No `resolved-at`; agent-owned stamps are preserved

marklark does not write a `resolved-at` stamp and does no git-based discovery. Finding what changed since
the code was last verified is the agent's job; the agent can use git itself. Stamp ownership:

1. **Reserved keys**: `intent`, `params`, `generated`, and the AI block's `dep` and `deps` (D36). Only
   marklark writes them.
2. **Unknown keys are preserved**: `update` and `intent-accept` keep them unchanged. Being comments, they
   do not count in the `params` hash (D20).
3. **`intent_apply` accepts extra stamps** from the agent and writes them together with marklark's own;
   it refuses a reserved key.
4. **Agent keys start with `x-`** (e.g. `#@ x-resolved-at: 09fe57`), so a key marklark adds later can never
   clash with one an agent already uses. To marklark their values are opaque text.

*Reasoning*: a commit stamp cannot name the commit that contains it — it can only name the parent of a
later commit — so it over-reports, and rebase or squash can remove the commit altogether. Making it
precise would cost machinery for something the agent can do on its own. Dropping it also removes the only
place where marklark's semantics depended on git (see D32).

## D32. marklark works without git

Git is optional. Where marklark uses git, it is a convenience with a defined fallback:

- **Project root** (D22): git root; otherwise the nearest `marklark.toml`, otherwise the document's
  directory.
- **Backups** (D17): skipped when git already holds the file's content; without git a `.bak` is always
  written (unless `--no-bak`).
- **Gitignored files** (D18): refused when git is present; outside git, see open question Q17.

No stamp, state or CODE semantics depend on git.

## D33. The AI block follows mdship's AI placeholder

The `<!--AI-->` block (D24) is, by concept, mdship's AI placeholder: a prompt, an optional shared brief,
declared dependencies (whole files or slices), and generated content between the opening comment and
`<!--/AI-->`. marklark records checksums of the prompt, the brief, each dependency slice and the
generated content; a changed input means the content needs regenerating, a hand-edited content blocks
regeneration until accepted. There is no architectural or structural change. The surface follows the
marklark decisions that apply: no YAML (D16), `#@` stamps (D19), BLAKE3, and stamp ownership (D31).

*Reasoning*: the mdship design works and is proven in use; generated prose is not where marklark's new
ideas are.

## D34. Secrets outside git: `.gitignore` files plus a built-in deny list

Without git (D32), marklark still honours `.gitignore` files found under the project root (parsed by
marklark itself, e.g. with the `ignore` crate), and always refuses a small built-in default deny list
(`.env`, `*.pem`, `id_*`, …).

*Reasoning*: keeps D18's protection against an injected intent copying secrets into a document, whether
or not the directory is a git repository.

## D35. Exit codes are stable

Exit codes are part of the CLI contract and stay stable once released; the values are fixed when the CLI
is built.

## D36. AI block: prompt in the INTENT, block holds stamps only

Refines D33.

- **The prompt is the governing INTENT.** The human writes there what to produce and which sources it may
  come from. The `<!--AI-->` opening comment holds only `#@` stamps — no human-edited part, like a CODE
  block with no code.
- **Two steps.** *Resolution* (once, possibly long ago): the agent derives the dependencies from the INTENT
  and they are stored as `dep` stamps. *Generation* (whenever an input changed): marklark extracts every
  dependency and hands the content to the agent; the agent returns the prose; marklark writes the
  dependency hashes and the `generated` stamp. The agent passes no dependency specifications at
  generation, as in mdship.
- **`dep` stamp format**: a `slice()` call followed by the hash of the extracted content, e.g.
  `#@ dep: slice("src/auth.py", start="class SessionManager", end="^class ") blake3:…`. marklark evaluates
  it with the same `slice()` host function CODE uses.
- **No separate brief.** A brief named in the INTENT is just another dependency.
- **Human-edited dependencies** are a supported special case. A `#@ deps:` stamp hashes the dependency
  specifications (not their content). A human may edit, add or remove `dep` lines — a new line may omit
  the hash; the mismatch is reported like `params_edited`, and `intent-accept` re-stamps `deps`. The next
  generation fills in the content hashes.
- **Content always ends at `<!--/AI-->`**, written by marklark's apply tool together with the block. No
  `_terminate_`, no heading-based boundary.

*Reasoning*: keeps INTENT the single human-owned source of truth for both CODE and AI blocks, while the
mdship workflow (marklark extracts, the agent writes, checksums decide when to regenerate) stays intact.

## D37. Hash algorithm: BLAKE3

All stamps hash with BLAKE3, written as `blake3:<hex>`.

*Reasoning*: fast, cryptographic, a maintained Rust crate (`blake3`); there is no compatibility to keep
with mdship's MD5.

## D38. `generated` stamp counts characters over normalised line endings

The `generated` stamp is `<length>:blake3:<hex>` where line endings are first normalised to `\n`, the
length counts Unicode characters (not bytes), and the hash is over the UTF-8 of the normalised text.

*Reasoning*: the stamp must survive a CRLF/LF conversion and a re-encoding of the file (mdship also
counted characters). A byte count or a hash of raw bytes would turn either into a false "hand edit".
