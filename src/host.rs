//! Host functions available to CODE (docs/host-functions.md).
//!
//! Every function reaches the outside world only through the [`BlockCtx`] of the running block: its
//! sandboxed file view (which records reads), the document text as it stands, the shared variables
//! (read-only) and the block's own variable layer and output (D30).
//!
//! Variables are plain data — none, bool, int, float, string, list, dict — held as JSON values. A value
//! is converted on `define` and rebuilt on `var`, so defined values are deeply immutable and each block
//! can run on its own Starlark heap.

// `slice` mirrors mdship INCLUDE's options one to one, hence its many parameters; the
// `#[starlark_module]` expansion does not carry item attributes, so this is module-wide.
#![allow(clippy::too_many_arguments)]

use std::cell::RefCell;
use std::collections::BTreeMap;

use anyhow::{anyhow, bail};
use starlark::any::ProvidesStaticType;
use starlark::environment::{Globals, GlobalsBuilder, LibraryExtension};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::list::AllocList;
use starlark::values::none::NoneType;
use starlark::values::structs::AllocStruct;
use starlark::values::Value;

use crate::extract::{self, Slice};
use crate::fs::{ProjectPath, Sandbox};
use crate::markdown::{self, Heading};

/// Shared or block-local variables.
pub type Vars = BTreeMap<String, serde_json::Value>;

/// Why a block stopped without failing: it must run again in a later pass (spec §7.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Abort {
    /// `var` of a name no block has defined yet.
    MissingVar(String),
}

/// Everything a running block may touch.
#[derive(ProvidesStaticType)]
pub struct BlockCtx<'a> {
    /// Variables committed by blocks that finished.
    pub shared: &'a Vars,
    /// This block's own definitions; merged into `shared` only when the block finishes (D30).
    pub local: RefCell<Vars>,
    pub output: RefCell<String>,
    pub emitted: RefCell<bool>,
    pub abort: RefCell<Option<Abort>>,
    pub sandbox: &'a Sandbox<'a>,
    /// Directory relative paths resolve against.
    pub base_dir: ProjectPath,
    /// The document as it stands now, including output spliced earlier in this update (spec §7.6).
    pub document: &'a str,
    /// Where the running block is.
    pub here: Here,
}

/// The running block's position, for `here()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Here {
    /// 1-based line of the block's opening comment.
    pub line: usize,
    /// The first document line after the block (after its closing marker, or after the opening comment
    /// if it has none), without its line ending; `None` at the end of the document.
    pub next_line: Option<String>,
}

impl<'a> BlockCtx<'a> {
    pub fn new(shared: &'a Vars, sandbox: &'a Sandbox<'a>, base_dir: ProjectPath, document: &'a str, here: Here) -> Self {
        BlockCtx {
            shared,
            local: RefCell::new(Vars::new()),
            output: RefCell::new(String::new()),
            emitted: RefCell::new(false),
            abort: RefCell::new(None),
            sandbox,
            base_dir,
            document,
            here,
        }
    }

    fn lookup(&self, name: &str) -> Option<serde_json::Value> {
        self.local.borrow().get(name).cloned().or_else(|| self.shared.get(name).cloned())
    }

    fn resolve(&self, path: &str) -> anyhow::Result<ProjectPath> {
        Ok(self.base_dir.join(path)?)
    }
}

fn ctx<'a>(eval: &'a Evaluator) -> &'a BlockCtx<'a> {
    eval.extra
        .and_then(|e| e.downcast_ref::<BlockCtx>())
        .expect("marklark: evaluator has no block context")
}

/// The globals CODE runs with: the Starlark standard library (no `print`), `struct`, and the marklark
/// host functions. Starlark's own `json` module is left out: `json(path)` is a host function.
pub fn globals() -> Globals {
    GlobalsBuilder::extended_by(&[LibraryExtension::StructType])
        .with(marklark_host)
        .build()
}

/// Text of the error a host function raises to abort the block (spec §7.4). The evaluator recognises
/// an abort by the context's `abort` field, not by this text.
const ABORT_MESSAGE: &str = "marklark: block aborted, will run again in a later pass";

#[starlark_module]
fn marklark_host(builder: &mut GlobalsBuilder) {
    /// Declares the tasks this block depends on and completes. Collected before anything runs
    /// (spec §7.1); at run time it does nothing.
    fn tasks(
        #[starlark(require = named)] depends_on: Option<Value>,
        #[starlark(require = named)] completes: Option<Value>,
    ) -> anyhow::Result<NoneType> {
        let _ = (depends_on, completes);
        Ok(NoneType)
    }

    /// Appends text to the block's output region.
    fn emit(#[starlark(require = pos)] text: &str, eval: &mut Evaluator) -> anyhow::Result<NoneType> {
        let c = ctx(eval);
        c.output.borrow_mut().push_str(text);
        *c.emitted.borrow_mut() = true;
        Ok(NoneType)
    }

    /// Publishes a value under `name`. Redefining a name is an error unless `lenient=True` (D21).
    fn define<'v>(
        #[starlark(require = pos)] name: &str,
        #[starlark(require = pos)] value: Value<'v>,
        #[starlark(require = named, default = false)] lenient: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<NoneType> {
        let c = ctx(eval);
        if !lenient && c.lookup(name).is_some() {
            bail!("variable `{name}` is already defined; use define(..., lenient=True) to redefine it");
        }
        let data = value
            .to_json_value()
            .map_err(|e| anyhow!("define(\"{name}\", ...): the value must be plain data (none, bool, int, float, string, list, dict): {e}"))?;
        c.local.borrow_mut().insert(name.to_owned(), data);
        Ok(NoneType)
    }

    /// Reads a variable: this block's own definitions first, then the shared state. Without a default,
    /// an undefined name aborts the block as not run yet (spec §7.4).
    fn var<'v>(
        #[starlark(require = pos)] name: &str,
        #[starlark(require = named)] default: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let found = ctx(eval).lookup(name);
        match (found, default) {
            (Some(data), _) => Ok(eval.heap().alloc(data)),
            (None, Some(d)) => Ok(d),
            (None, None) => {
                *ctx(eval).abort.borrow_mut() = Some(Abort::MissingVar(name.to_owned()));
                Err(anyhow!(ABORT_MESSAGE))
            }
        }
    }

    /// The whole text file at `path` (relative to the document's directory).
    fn read(#[starlark(require = pos)] path: &str, eval: &mut Evaluator) -> anyhow::Result<String> {
        let c = ctx(eval);
        Ok(c.sandbox.read_text(&c.resolve(path)?)?)
    }

    /// Readable files matching `pattern` (relative to the document's directory), sorted. Ignored and
    /// denied files are never listed.
    fn glob<'v>(#[starlark(require = pos)] pattern: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let c = ctx(eval);
        let full = c.resolve(pattern)?;
        let matcher = globset::GlobBuilder::new(full.as_str()).literal_separator(true).build()?.compile_matcher();
        let mut found = Vec::new();
        for path in c.sandbox.list_files()? {
            if matcher.is_match(path.as_str()) {
                found.push(match path.strip_dir(&c.base_dir) {
                    Some(rel) => rel.to_owned(),
                    None => format!("/{path}"),
                });
            }
        }
        Ok(eval.heap().alloc(AllocList(found)))
    }

    /// The document's headings as structs `{level, text, line, anchor}`, filtered by level.
    fn headings<'v>(
        #[starlark(require = named, default = 1)] min_level: i32,
        #[starlark(require = named, default = 6)] max_level: i32,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        let list: Vec<Value<'v>> = markdown::headings(ctx(eval).document)
            .into_iter()
            .filter(|h| (min_level..=max_level).contains(&(h.level as i32)))
            .map(|Heading { level, text, line, anchor }| {
                heap.alloc(AllocStruct([
                    ("level", heap.alloc(level as i32)),
                    ("text", heap.alloc(text)),
                    ("line", heap.alloc(line as i32)),
                    ("anchor", heap.alloc(anchor)),
                ]))
            })
            .collect();
        Ok(heap.alloc(AllocList(list)))
    }

    /// The GitHub-style anchor slug of a heading text.
    fn anchor(#[starlark(require = pos)] text: &str) -> anyhow::Result<String> {
        Ok(markdown::slug(text))
    }

    /// Wraps `text` in a code fence longer than any backtick run inside it.
    fn fence(#[starlark(require = pos)] text: &str, #[starlark(default = "")] lang: &str) -> anyhow::Result<String> {
        Ok(markdown::fence(text, lang))
    }

    /// A piece of a text file, with mdship INCLUDE semantics: `range="x..y"`, `start`/`end` regexes
    /// (with `include_start`, `include_end`, `repeat`), or a Markdown `section` title. A selector that
    /// matches nothing is an error.
    fn slice(
        #[starlark(require = pos)] path: &str,
        #[starlark(require = named)] start: Option<&str>,
        #[starlark(require = named)] end: Option<&str>,
        #[starlark(require = named)] range: Option<&str>,
        #[starlark(require = named)] section: Option<&str>,
        #[starlark(require = named, default = false)] include_start: bool,
        #[starlark(require = named, default = false)] include_end: bool,
        #[starlark(require = named, default = false)] repeat: bool,
        eval: &mut Evaluator,
    ) -> anyhow::Result<String> {
        let c = ctx(eval);
        let text = c.sandbox.read_text(&c.resolve(path)?)?;
        let spec = Slice { start, end, range, section, include_start, include_end, repeat };
        extract::slice(&text, &spec).map_err(|e| anyhow!("slice(\"{path}\"): {e}"))
    }

    /// Re-indents `text` so its least indented line has exactly `margin` spaces (mdship `margin`).
    fn reindent(#[starlark(require = pos)] text: &str, #[starlark(require = pos)] margin: i32) -> anyhow::Result<String> {
        if margin < 0 {
            bail!("reindent: margin must not be negative");
        }
        Ok(extract::reindent(text, margin as usize))
    }

    /// The first match of `pattern` in `text`, or None. The match is a struct: `text`, `groups` (list,
    /// None for groups that did not take part), `named` (dict), `start`, `end` (character offsets).
    /// Flags: `i`, `m`, `s`, `x`.
    fn re_search<'v>(
        #[starlark(require = pos)] pattern: &str,
        #[starlark(require = pos)] text: &str,
        #[starlark(default = "")] flags: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let re = extract::compile_with_flags(pattern, flags).map_err(|e| anyhow!(e))?;
        let heap = eval.heap();
        let Some(caps) = re.captures(text) else { return Ok(Value::new_none()) };
        let whole = caps.get(0).expect("group 0 always exists");
        let groups: Vec<Value<'v>> = (1..caps.len())
            .map(|i| caps.get(i).map_or(Value::new_none(), |m| heap.alloc(m.as_str())))
            .collect();
        let named: serde_json::Map<String, serde_json::Value> = re
            .capture_names()
            .flatten()
            .map(|n| (n.to_owned(), caps.name(n).map_or(serde_json::Value::Null, |m| m.as_str().into())))
            .collect();
        Ok(heap.alloc(AllocStruct([
            ("text", heap.alloc(whole.as_str())),
            ("groups", heap.alloc(AllocList(groups))),
            ("named", heap.alloc(serde_json::Value::Object(named))),
            ("start", heap.alloc(extract::char_offset(text, whole.start()) as i32)),
            ("end", heap.alloc(extract::char_offset(text, whole.end()) as i32)),
        ])))
    }

    /// All non-overlapping matches, like Python: without groups a list of strings, with one group a list
    /// of that group, with several a list of lists.
    fn re_findall<'v>(
        #[starlark(require = pos)] pattern: &str,
        #[starlark(require = pos)] text: &str,
        #[starlark(default = "")] flags: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let re = extract::compile_with_flags(pattern, flags).map_err(|e| anyhow!(e))?;
        let heap = eval.heap();
        let n = re.captures_len() - 1;
        let group = |c: &regex::Captures, i: usize| c.get(i).map_or("", |m| m.as_str()).to_owned();
        let items: Vec<Value<'v>> = re
            .captures_iter(text)
            .map(|c| match n {
                0 => heap.alloc(group(&c, 0)),
                1 => heap.alloc(group(&c, 1)),
                _ => heap.alloc(AllocList((1..=n).map(|i| group(&c, i)))),
            })
            .collect();
        Ok(heap.alloc(AllocList(items)))
    }

    /// Replaces matches of `pattern` (all, or the first `count`). In `repl`, `$1` / `${name}` refer to
    /// groups and `$$` is a literal `$`.
    fn re_sub(
        #[starlark(require = pos)] pattern: &str,
        #[starlark(require = pos)] repl: &str,
        #[starlark(require = pos)] text: &str,
        #[starlark(default = 0)] count: i32,
        #[starlark(default = "")] flags: &str,
    ) -> anyhow::Result<String> {
        let re = extract::compile_with_flags(pattern, flags).map_err(|e| anyhow!(e))?;
        Ok(re.replacen(text, count.max(0) as usize, repl).into_owned())
    }

    /// A JSON file parsed into plain data.
    fn json<'v>(#[starlark(require = pos)] path: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let c = ctx(eval);
        let text = c.sandbox.read_text(&c.resolve(path)?)?;
        let data: serde_json::Value = serde_json::from_str(&text).map_err(|e| anyhow!("json(\"{path}\"): {e}"))?;
        Ok(eval.heap().alloc(data))
    }

    /// A TOML file parsed into plain data (dates become strings).
    fn toml<'v>(#[starlark(require = pos)] path: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let c = ctx(eval);
        let text = c.sandbox.read_text(&c.resolve(path)?)?;
        let data: toml::Value = toml::from_str(&text).map_err(|e| anyhow!("toml(\"{path}\"): {e}"))?;
        Ok(eval.heap().alloc(toml_to_json(data)))
    }

    /// The running block's position: `line` (of its opening comment) and `next_line` (the first line
    /// after the block, or None) — mdship SUP.
    fn here<'v>(eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let h = ctx(eval).here.clone();
        let heap = eval.heap();
        Ok(heap.alloc(AllocStruct([
            ("line", heap.alloc(h.line as i32)),
            ("next_line", h.next_line.map_or(Value::new_none(), |l| heap.alloc(l))),
        ])))
    }

    /// The whole document as it stands now (spec §7.6).
    fn document(eval: &mut Evaluator) -> anyhow::Result<String> {
        Ok(ctx(eval).document.to_owned())
    }
}

fn toml_to_json(v: toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => s.into(),
        toml::Value::Integer(i) => i.into(),
        toml::Value::Float(f) => serde_json::Number::from_f64(f).map_or(serde_json::Value::Null, Into::into),
        toml::Value::Boolean(b) => b.into(),
        toml::Value::Datetime(d) => d.to_string().into(),
        toml::Value::Array(a) => a.into_iter().map(toml_to_json).collect(),
        toml::Value::Table(t) => t.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect::<serde_json::Map<_, _>>().into(),
    }
}

/// Unpacks an abort recorded by a host function, if the error came from one.
pub fn take_abort(ctx: &BlockCtx) -> Option<Abort> {
    ctx.abort.borrow_mut().take()
}
