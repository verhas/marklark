# marklark

**Status: design phase. No code yet.**

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

marklark is the successor in spirit to [mdship](https://github.com/verhas/mdship), which has one
dedicated placeholder type per task (INCLUDE, IMPORT, SIP, SLURP, …) and is written in Python. marklark
replaces that family with one programmable construct, is written in Rust, and ships as a single binary.
mdship remains maintained for environments where only `pip` installs are allowed.

## Documentation

| File | Contents |
|---|---|
| [docs/spec.md](docs/spec.md) | The specification draft: constructs, stamps, states, evaluation model, security |
| [docs/host-functions.md](docs/host-functions.md) | The host-function library, sized against every mdship feature |
| [docs/decisions.md](docs/decisions.md) | Decision log with the reasoning behind each decision |
| [docs/open-questions.md](docs/open-questions.md) | What is still undecided, with current leanings |
| [docs/mdship-lessons.md](docs/mdship-lessons.md) | What to carry over from mdship (lessons, not code) |
| [CLAUDE.md](CLAUDE.md) | Orientation for agent sessions working on this repository |
