//! AI blocks (spec §12, D33, D36): agent-written prose governed by an INTENT.
//!
//! ```markdown
//! <!--INTENT
//! Summarise SessionManager in src/auth.py, in the style of docs/brief.md.
//! -->
//! <!--AI
//! #@ intent: blake3:…
//! #@ dep: slice("../src/auth.py", start="class SessionManager", end="^class ") blake3:…
//! #@ dep: read("brief.md") blake3:…
//! #@ deps: blake3:…
//! #@ generated: 812:blake3:…
//! -->
//! …prose…
//! <!--/AI-->
//! ```
//!
//! Making a region generative is the human's decision, expressed in the INTENT or in the request to the
//! agent; the agent never chooses prose on its own (D39). Then:
//!
//! - **resolution** ([`resolve`]): the agent derives the dependencies from the INTENT; marklark creates
//!   the AI block after the INTENT if there is none yet, checks the dependencies and writes them as
//!   `dep` stamps, plus `intent` and `deps` (the hash of the dependency specs);
//! - **generation** ([`context`] then [`write`]): marklark extracts every dependency and gives the text
//!   to the agent; the agent returns the prose; marklark writes it with each dependency's content hash
//!   and the `generated` stamp. The agent never reads dependency files itself.
//!
//! A `dep` line is text a human may edit (D36) that marklark evaluates, so it is restricted to a single
//! `read(...)` or `slice(...)` call with literal arguments, run in the same sandbox as CODE.

use std::fmt;

use starlark::syntax::ast::{AstLiteral, ExprP, StmtP};

use crate::blocks::Kind;
use crate::document::{DocError, Document, Parsed};
use crate::eval;
use crate::fs::FileSystem;
use crate::lang;
use crate::splice;
use crate::state::{self, Finding, State};
use crate::stamps::{self, BLAKE3_PREFIX, Stamps};

/// The functions a `dep` may call.
const DEP_FUNCTIONS: &[&str] = &["read", "slice"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiError {
    Document(Vec<DocError>),
    /// No AI block at the line.
    NoAiBlock { line: usize },
    /// The AI block has no INTENT before it.
    NoIntent { line: usize },
    /// A dependency spec that is not a single `read(...)` / `slice(...)` call with literal arguments.
    BadDep { spec: String, message: String },
    /// A dependency that could not be extracted (missing file, a pattern that matches nothing, …).
    Extract { spec: String, message: String },
    /// The prose was edited by hand; regeneration would discard it (accept or restore it first).
    HandEdited { line: usize },
    /// The AI block has no dependencies yet: resolve it first.
    Unresolved { line: usize },
    /// The line belongs to a different INTENT than the caller's `intent_hash` names (a stale line).
    IntentMismatch { line: usize, found: String },
    /// The INTENT is satisfied by CODE; turning it into prose is the human's decision (D39).
    HasCode { line: usize },
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiError::Document(errors) => {
                let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
                f.write_str(&lines.join("\n"))
            }
            AiError::NoAiBlock { line } => write!(
                f,
                "line {line}: no AI block here (ai_resolve on the INTENT's line creates one, when the human asked for prose)"
            ),
            AiError::HasCode { line } => write!(
                f,
                "line {line}: this INTENT is satisfied by CODE; replacing code with prose is the human's decision, \
                 so the human removes the CODE first"
            ),
            AiError::NoIntent { line } => write!(f, "line {line}: this AI block has no INTENT before it"),
            AiError::BadDep { spec, message } => write!(f, "dependency `{spec}`: {message}"),
            AiError::Extract { spec, message } => write!(f, "dependency `{spec}` cannot be read: {message}"),
            AiError::HandEdited { line } => write!(
                f,
                "line {line}: the AI block's text was edited by hand; accept it with intent-accept, or restore it"
            ),
            AiError::Unresolved { line } => write!(f, "line {line}: the AI block has no dependencies yet; resolve it first"),
            AiError::IntentMismatch { line, found } => write!(
                f,
                "line {line} belongs to the INTENT \"{found}\", whose hash is not the one given; line numbers \
                 shift whenever the document changes, so list the intents again. Nothing was changed"
            ),
        }
    }
}

impl std::error::Error for AiError {}

/// One `#@ dep:` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    /// The `read(...)` / `slice(...)` call.
    pub spec: String,
    /// `blake3:<hex>` of the extracted content when the prose was last written; `None` for a line a
    /// human added and nothing has been generated from yet.
    pub hash: Option<String>,
}

impl Dep {
    pub fn parse(value: &str) -> Dep {
        match value.rsplit_once(' ') {
            Some((spec, hash)) if is_digest(hash) => Dep { spec: spec.trim_end().to_owned(), hash: Some(hash.to_owned()) },
            _ => Dep { spec: value.trim().to_owned(), hash: None },
        }
    }

    fn stamp_value(&self) -> String {
        match &self.hash {
            Some(h) => format!("{} {h}", self.spec),
            None => self.spec.clone(),
        }
    }
}

fn is_digest(s: &str) -> bool {
    s.strip_prefix(BLAKE3_PREFIX).is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The `deps` stamp: hash of the dependency specs (not their content), in order.
pub fn deps_digest<S: AsRef<str>>(specs: &[S]) -> String {
    let joined: Vec<&str> = specs.iter().map(AsRef::as_ref).collect();
    stamps::blake3_digest(joined.join("\n").as_bytes())
}

/// Hash of a dependency's extracted content (line endings normalised, D38).
pub fn content_digest(text: &str) -> String {
    stamps::blake3_digest(stamps::normalise_line_endings(text).as_bytes())
}

/// Checks that a dependency spec is a single `read(...)` or `slice(...)` call with literal arguments.
pub fn validate_spec(spec: &str) -> Result<(), AiError> {
    let bad = |message: &str| AiError::BadDep { spec: spec.to_owned(), message: message.to_owned() };
    if spec.contains('\n') {
        return Err(bad("must be on one line"));
    }
    let module = lang::parse("dep", spec).map_err(|e| bad(&e.message))?;
    let StmtP::Expression(expr) = &module.statement().node else {
        return Err(bad("must be a single read(...) or slice(...) call"));
    };
    let ExprP::Call(callee, args) = &expr.node else {
        return Err(bad("must be a single read(...) or slice(...) call"));
    };
    match &callee.node {
        ExprP::Identifier(id) if DEP_FUNCTIONS.contains(&id.node.ident.as_str()) => {}
        _ => return Err(bad("must call read(...) or slice(...)")),
    }
    for arg in &args.args {
        let literal = match &arg.node.expr().node {
            ExprP::Literal(AstLiteral::String(_) | AstLiteral::Int(_)) => true,
            ExprP::Identifier(id) => matches!(id.node.ident.as_str(), "True" | "False"),
            _ => false,
        };
        if !literal {
            return Err(bad("arguments must be literals"));
        }
    }
    Ok(())
}

/// Extracts a dependency's content.
pub fn extract(doc: &Document, fs: &dyn FileSystem, spec: &str) -> Result<String, AiError> {
    validate_spec(spec)?;
    eval::eval_string(spec, doc, fs)
        .map(|(text, _)| text)
        .map_err(|message| AiError::Extract { spec: spec.to_owned(), message: first_error_line(&message) })
}

fn first_error_line(message: &str) -> String {
    message
        .lines()
        .find(|l| l.starts_with("error:"))
        .map(|l| l.trim_start_matches("error:").trim().to_owned())
        .unwrap_or_else(|| message.trim().to_owned())
}

/// An AI block located for an operation.
struct Located {
    parsed: Parsed,
    index: usize,
    intent_text: String,
}

/// Locates the AI block and, with `expected_intent`, checks it is governed by that INTENT.
fn locate_checked(doc: &Document, line: usize, expected_intent: Option<&str>) -> Result<Located, AiError> {
    let located = locate(doc, line)?;
    state::check_target(&located.intent_text, expected_intent).map_err(|found| AiError::IntentMismatch { line, found })?;
    Ok(located)
}

fn locate(doc: &Document, line: usize) -> Result<Located, AiError> {
    let parsed = doc.parse().map_err(AiError::Document)?;
    let index = state::block_at(&parsed, &doc.text, line)
        .and_then(|i| {
            // A line of the governing INTENT selects its AI block too.
            match parsed.blocks[i].kind {
                Kind::Ai => Some(i),
                Kind::Intent => parsed.group_of(i).and_then(|g| g.members.first().copied()),
                Kind::Code => None,
            }
        })
        .filter(|&i| parsed.blocks[i].kind == Kind::Ai)
        .ok_or(AiError::NoAiBlock { line })?;
    let group = parsed.group_of(index).ok_or(AiError::NoIntent { line })?;
    let intent_text = parsed.blocks[group.intent].body_text(&doc.text).to_owned();
    Ok(Located { parsed, index, intent_text })
}

fn deps_of(stamps: &Stamps) -> Vec<Dep> {
    stamps.get_all("dep").map(Dep::parse).collect()
}

/// Rewrites an AI block's stamps: `intent`, the `dep` lines, `deps`, `generated` (each `None` leaves the
/// current value), then every other stamp in its original order. Optionally replaces the prose.
fn rewrite_block(
    doc: &Document,
    located: &Located,
    deps: &[Dep],
    generated: Option<&str>,
    prose: Option<&str>,
) -> String {
    let text = &doc.text;
    let block = &located.parsed.blocks[located.index];
    let stamps = &located.parsed.stamps[located.index];
    let intent = stamps::intent_digest(&located.intent_text);
    let specs: Vec<&str> = deps.iter().map(|d| d.spec.as_str()).collect();
    let deps_hash = deps_digest(&specs);
    let dep_values: Vec<String> = deps.iter().map(Dep::stamp_value).collect();
    let generated = generated.map(str::to_owned).or_else(|| stamps.get("generated").map(str::to_owned));

    let mut pairs: Vec<(&str, &str)> = vec![("intent", intent.as_str())];
    pairs.extend(dep_values.iter().map(|v| ("dep", v.as_str())));
    pairs.push(("deps", deps_hash.as_str()));
    if let Some(g) = &generated {
        pairs.push(("generated", g.as_str()));
    }
    for s in &stamps.stamps {
        if !["intent", "dep", "deps", "generated"].contains(&s.key.as_str()) {
            pairs.push((s.key.as_str(), s.value.as_str()));
        }
    }
    let body = stamps::rewrite(block.body_text(text), &pairs);
    let region = block.output.clone().expect("AI blocks always have a region");
    let mut out = String::with_capacity(text.len() + 256);
    out.push_str(&text[..block.body.start]);
    out.push_str(&body);
    out.push_str(&text[block.body.end..region.start]);
    out.push_str(prose.unwrap_or(&text[region.clone()]));
    out.push_str(&text[region.end..]);
    out
}

/// Resolution: stores the dependencies the agent derived from the INTENT. Each spec is checked and
/// test-extracted. Content hashes are left empty until the next generation; the `intent` and `deps`
/// stamps are written now. Returns the new document text.
///
/// With `expected_intent` (an `intent_hash` from a listing), refuses if the line belongs to another INTENT.
pub fn resolve(
    doc: &Document,
    fs: &dyn FileSystem,
    line: usize,
    expected_intent: Option<&str>,
    specs: &[String],
) -> Result<String, AiError> {
    let created = with_ai_block(doc, line, expected_intent)?;
    let doc = created.as_ref().unwrap_or(doc);
    let located = locate_checked(doc, line, expected_intent)?;
    let mut deps = Vec::new();
    for spec in specs {
        extract(doc, fs, spec)?;
        deps.push(Dep { spec: spec.trim().to_owned(), hash: None });
    }
    Ok(rewrite_block(doc, &located, &deps, None, None))
}

/// If `line` is on an INTENT that has no block yet, returns the document with an empty AI block
/// inserted right after the INTENT (D39). An INTENT that has CODE is refused. Otherwise `None`.
fn with_ai_block(doc: &Document, line: usize, expected_intent: Option<&str>) -> Result<Option<Document>, AiError> {
    let parsed = doc.parse().map_err(AiError::Document)?;
    let Some(at) = state::block_at(&parsed, &doc.text, line) else { return Ok(None) };
    let Some(group) = parsed.group_of(at) else { return Ok(None) };
    if group.members.iter().any(|&m| parsed.blocks[m].kind == Kind::Code) {
        return Err(AiError::HasCode { line });
    }
    if !group.members.is_empty() {
        return Ok(None);
    }
    let intent = &parsed.blocks[group.intent];
    state::check_target(intent.body_text(&doc.text), expected_intent).map_err(|found| AiError::IntentMismatch { line, found })?;
    let text = &doc.text;
    let eol = splice::line_ending(text);
    let at = text[intent.end()..].find('\n').map_or(text.len(), |i| intent.end() + i + 1);
    let mut out = String::with_capacity(text.len() + 32);
    out.push_str(&text[..at]);
    if at == text.len() && !text.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(&format!("<!--AI{eol}-->{eol}<!--/AI-->{eol}"));
    out.push_str(&text[at..]);
    Ok(Some(Document { path: doc.path.clone(), text: out }))
}

/// One dependency with its current content, for the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepContent {
    pub spec: String,
    pub content: String,
    /// The content differs from what the prose was last written from (or was never recorded).
    pub changed: bool,
}

/// What the agent needs to (re)write an AI block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiContext {
    /// 1-based line of the AI block.
    pub line: usize,
    /// The governing INTENT's text — the prompt.
    pub intent: String,
    pub deps: Vec<DepContent>,
    /// The current prose.
    pub content: String,
    pub findings: Vec<Finding>,
}

/// Generation, step 1: everything the agent needs, with every dependency extracted.
pub fn context(doc: &Document, fs: &dyn FileSystem, line: usize) -> Result<AiContext, AiError> {
    let located = locate(doc, line)?;
    let block = &located.parsed.blocks[located.index];
    let stamps = &located.parsed.stamps[located.index];
    let mut deps = Vec::new();
    for dep in deps_of(stamps) {
        let content = extract(doc, fs, &dep.spec)?;
        let changed = dep.hash.as_deref() != Some(content_digest(&content).as_str());
        deps.push(DepContent { spec: dep.spec, content, changed });
    }
    let findings = analyze(doc, fs).unwrap_or_default().into_iter().filter(|f| f.line == block.line).collect();
    Ok(AiContext {
        line: block.line,
        intent: located.intent_text.trim().to_owned(),
        deps,
        content: block.output_text(&doc.text).unwrap_or_default().to_owned(),
        findings,
    })
}

/// Generation, step 2: writes the agent's prose, recording every dependency's content hash and the
/// `generated` stamp. Refuses to overwrite prose a human edited. Returns the new document text.
///
/// With `expected_intent` (an `intent_hash` from a listing), refuses if the line belongs to another INTENT.
pub fn write(
    doc: &Document,
    fs: &dyn FileSystem,
    line: usize,
    expected_intent: Option<&str>,
    prose: &str,
) -> Result<String, AiError> {
    let located = locate_checked(doc, line, expected_intent)?;
    let block = &located.parsed.blocks[located.index];
    let stamps = &located.parsed.stamps[located.index];
    splice::check_guard(&doc.text, block, stamps).map_err(|_| AiError::HandEdited { line: block.line })?;
    let deps = rehash(doc, fs, &deps_of(stamps))?;
    if deps.is_empty() {
        return Err(AiError::Unresolved { line: block.line });
    }
    let output = splice::region_text(prose, splice::line_ending(&doc.text));
    let generated = stamps::generated_digest(&output);
    Ok(rewrite_block(doc, &located, &deps, Some(&generated), Some(&output)))
}

/// `intent-accept` for an AI block: a human confirms the prose (possibly hand-edited) and the
/// dependencies (possibly hand-edited) fit the current INTENT and inputs. Re-stamps everything.
pub fn accept(doc: &Document, fs: &dyn FileSystem, line: usize) -> Result<String, AiError> {
    let located = locate(doc, line)?;
    let block = &located.parsed.blocks[located.index];
    let stamps = &located.parsed.stamps[located.index];
    let deps = rehash(doc, fs, &deps_of(stamps))?;
    let generated = stamps::generated_digest(block.output_text(&doc.text).unwrap_or_default());
    Ok(rewrite_block(doc, &located, &deps, Some(&generated), None))
}

fn rehash(doc: &Document, fs: &dyn FileSystem, deps: &[Dep]) -> Result<Vec<Dep>, AiError> {
    deps.iter()
        .map(|d| Ok(Dep { spec: d.spec.clone(), hash: Some(content_digest(&extract(doc, fs, &d.spec)?)) }))
        .collect()
}

/// AI-specific states of every AI block: dependencies edited, inputs changed, prose edited by hand.
/// (`unverified` / `intent_changed` come from [`state::analyze`], which covers AI blocks too.)
pub fn analyze(doc: &Document, fs: &dyn FileSystem) -> Result<Vec<Finding>, Vec<DocError>> {
    let parsed = doc.parse()?;
    let mut findings = Vec::new();
    for (i, block) in parsed.blocks.iter().enumerate() {
        if block.kind != Kind::Ai || parsed.group_of(i).is_none() {
            continue;
        }
        let stamps = &parsed.stamps[i];
        let deps = deps_of(stamps);
        let push = |findings: &mut Vec<Finding>, state, detail: String| findings.push(Finding { state, line: block.line, detail });
        if stamps.get("intent").is_none() {
            continue; // unverified: reported by state::analyze
        }
        if deps.is_empty() {
            push(&mut findings, State::NeedsUpdate, "the AI block has no dependencies yet".into());
            continue;
        }
        let specs: Vec<&str> = deps.iter().map(|d| d.spec.as_str()).collect();
        if stamps.get("deps") != Some(deps_digest(&specs).as_str()) {
            push(&mut findings, State::ParamsEdited, "the dependencies were edited since they were checked".into());
        }
        if splice::check_guard(&doc.text, block, stamps).is_err() {
            push(&mut findings, State::OutputEdited, "the text was edited by hand".into());
        }
        let changed: Vec<String> = deps
            .iter()
            .filter(|d| match extract(doc, fs, &d.spec) {
                Ok(content) => d.hash.as_deref() != Some(content_digest(&content).as_str()),
                Err(_) => true,
            })
            .map(|d| d.spec.clone())
            .collect();
        if !changed.is_empty() {
            push(&mut findings, State::NeedsUpdate, format!("inputs changed: {}", changed.join(", ")));
        }
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{MemoryFs, ProjectPath};

    const DOC: &str = "# Doc\n<!--INTENT\nSummarise the notes.\n-->\n<!--AI\n-->\n<!--/AI-->\ntail\n";
    /// The same document before the agent created the AI block.
    const BARE: &str = "# Doc\n<!--INTENT\nSummarise the notes.\n-->\ntail\n";

    fn fs() -> MemoryFs {
        MemoryFs::new().with("docs/notes.txt", "alpha\nbeta\n").with("docs/brief.md", "Be brief.\n")
    }

    fn doc(text: &str) -> Document {
        Document::at(ProjectPath::new("docs/a.md").unwrap(), text)
    }

    fn findings(text: &str, fs: &MemoryFs) -> Vec<State> {
        let d = doc(text);
        let mut all: Vec<State> = state::analyze(&d).unwrap().into_iter().map(|f| f.state).collect();
        all.extend(analyze(&d, fs).unwrap().into_iter().map(|f| f.state));
        all
    }

    fn resolved_and_written(fs: &MemoryFs) -> String {
        let specs = vec![r#"read("notes.txt")"#.to_owned(), r#"read("brief.md")"#.to_owned()];
        let resolved = resolve(&doc(DOC), fs, 5, None, &specs).unwrap();
        write(&doc(&resolved), fs, 5, None, "Two notes: alpha and beta.").unwrap()
    }

    #[test]
    fn dep_specs_are_restricted() {
        assert!(validate_spec(r#"read("a.txt")"#).is_ok());
        assert!(validate_spec(r#"slice("a.py", start="x", include_end=True)"#).is_ok());
        for bad in [r#"emit("x")"#, r#"read(var("p"))"#, "read('a') + read('b')", "x = read('a')", "read('a')\nread('b')"] {
            assert!(validate_spec(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn dep_stamp_values() {
        let h = stamps::blake3_digest(b"x");
        assert_eq!(Dep::parse(&format!(r#"read("a b") {h}"#)), Dep { spec: r#"read("a b")"#.into(), hash: Some(h.clone()) });
        assert_eq!(Dep::parse(r#"read("a b")"#), Dep { spec: r#"read("a b")"#.into(), hash: None });
    }

    #[test]
    fn full_cycle() {
        let fs = fs();
        assert_eq!(findings(DOC, &fs), [State::Unverified]);
        let text = resolved_and_written(&fs);
        assert!(text.contains("#@ dep: read(\"notes.txt\") blake3:"), "{text}");
        assert!(text.contains("-->\nTwo notes: alpha and beta.\n<!--/AI-->\ntail\n"), "{text}");
        assert!(findings(&text, &fs).is_empty(), "{:?}\n{text}", findings(&text, &fs));

        // The context gives the agent the prompt and every dependency's content.
        let ctx = context(&doc(&text), &fs, 5).unwrap();
        assert_eq!(ctx.intent, "Summarise the notes.");
        assert_eq!(ctx.deps[0].content, "alpha\nbeta\n");
        assert!(ctx.deps.iter().all(|d| !d.changed));
    }

    #[test]
    fn input_change_intent_change_and_hand_edits() {
        let fs = fs();
        let text = resolved_and_written(&fs);
        let changed_fs = fs.clone().with("docs/notes.txt", "alpha\ngamma\n");
        assert_eq!(findings(&text, &changed_fs), [State::NeedsUpdate]);
        assert!(context(&doc(&text), &changed_fs, 5).unwrap().deps[0].changed);

        assert_eq!(findings(&text.replace("Summarise the notes.", "List the notes."), &fs), [State::IntentChanged]);

        let edited = text.replace("Two notes", "2 notes");
        assert_eq!(findings(&edited, &fs), [State::OutputEdited]);
        assert_eq!(write(&doc(&edited), &fs, 5, None, "new").unwrap_err(), AiError::HandEdited { line: 5 });
        let accepted = accept(&doc(&edited), &fs, 5).unwrap();
        assert!(findings(&accepted, &fs).is_empty());
    }

    #[test]
    fn human_edited_dependencies() {
        let fs = fs();
        let text = resolved_and_written(&fs);
        // A human adds a dependency line without a hash.
        let edited = text.replace("#@ deps:", "#@ dep: read(\"brief.md\", )\n#@ deps:");
        let states = findings(&edited, &fs);
        assert!(states.contains(&State::ParamsEdited) && states.contains(&State::NeedsUpdate), "{states:?}");
        let accepted = accept(&doc(&edited), &fs, 5).unwrap();
        assert!(findings(&accepted, &fs).is_empty(), "{accepted}");
    }

    #[test]
    fn resolve_creates_the_ai_block() {
        let fs = fs();
        let hash = stamps::intent_digest("Summarise the notes.");
        let resolved = resolve(&doc(BARE), &fs, 2, Some(&hash), &[r#"read("notes.txt")"#.into()]).unwrap();
        assert!(resolved.starts_with("# Doc\n<!--INTENT\nSummarise the notes.\n-->\n<!--AI\n#@ intent: "), "{resolved}");
        assert!(resolved.ends_with("-->\n<!--/AI-->\ntail\n"), "{resolved}");
        let written = write(&doc(&resolved), &fs, 2, Some(&hash), "Alpha and beta.").unwrap();
        assert!(findings(&written, &fs).is_empty(), "{written}");
        // A wrong hash creates nothing.
        let wrong = stamps::intent_digest("other");
        assert!(matches!(resolve(&doc(BARE), &fs, 2, Some(&wrong), &[]), Err(AiError::IntentMismatch { .. })));
    }

    #[test]
    fn resolve_refuses_an_intent_with_code() {
        let with_code = "<!--INTENT\nSummarise.\n-->\n<!--CODE\nx = 1\n-->\n";
        assert_eq!(resolve(&doc(with_code), &fs(), 1, None, &[]).unwrap_err(), AiError::HasCode { line: 1 });
    }

    #[test]
    fn errors() {
        let fs = fs();
        assert_eq!(write(&doc(DOC), &fs, 5, None, "x").unwrap_err(), AiError::Unresolved { line: 5 });
        assert!(matches!(resolve(&doc(DOC), &fs, 5, None, &[r#"read("missing.txt")"#.into()]), Err(AiError::Extract { .. })));
        assert!(matches!(resolve(&doc(DOC), &fs, 5, None, &[r#"read("../../etc/passwd")"#.into()]), Err(AiError::Extract { .. })));
        assert_eq!(resolve(&doc(DOC), &fs, 1, None, &[]).unwrap_err(), AiError::NoAiBlock { line: 1 });
        let wrong = stamps::intent_digest("Some other intent.");
        assert_eq!(
            resolve(&doc(DOC), &fs, 5, Some(&wrong), &[]).unwrap_err(),
            AiError::IntentMismatch { line: 5, found: "Summarise the notes.".into() }
        );
        let right = stamps::intent_digest("Summarise the notes.");
        assert!(resolve(&doc(DOC), &fs, 5, Some(&right), &[r#"read("notes.txt")"#.into()]).is_ok());
        let no_intent = "<!--AI\n-->\n<!--/AI-->\n";
        assert_eq!(context(&doc(no_intent), &fs, 1).unwrap_err(), AiError::NoIntent { line: 1 });
    }
}
