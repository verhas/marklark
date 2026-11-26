//! `intent_apply`: an agent replaces the CODE group of an INTENT atomically (spec §6, §11).
//!
//! The agent passes the bodies of the new CODE blocks (and optional agent-owned `x-` stamps). marklark
//! then, in this order, refuses if any current member's generated output was edited by hand (swapping
//! code must never discard a human edit), checks every new body (it must parse and its `tasks(...)`
//! declaration must be valid), replaces all members with the new blocks stamped with `intent` and
//! `params`, and runs a full update in memory, so code that fails is refused before anything is
//! written. The result is the updated document text.

use std::fmt;

use starlark::syntax::ast::ExprP;

use crate::blocks::Kind;
use crate::canon;
use crate::document::{DocError, Document};
use crate::eval::{self, UpdateError};
use crate::fs::FileSystem;
use crate::lang;
use crate::libs;
use crate::splice;
use crate::stamps::{self, AGENT_PREFIX};
use crate::state;
use crate::tasks;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    Document(Vec<DocError>),
    /// No INTENT starts at or contains the line (a line of one of its blocks works too).
    NoIntent { line: usize },
    /// The INTENT governs an AI block; use the AI tools.
    AiGroup { line: usize },
    /// The line belongs to a different INTENT than the caller's `intent_hash` names (a stale line).
    IntentMismatch { line: usize, found: String },
    NoBlocks,
    /// A stamp key that is not agent-owned (`x-…`) or is malformed.
    BadStamp { key: String },
    /// A body contains `-->`, which would end the comment early (spec §2).
    CommentEnd { block: usize },
    /// A body does not parse, or its `tasks(...)` declaration is invalid.
    BadCode { block: usize, message: String },
    /// A current member's generated output was edited by hand.
    HandEdited { line: usize },
    /// The document does not update with the new code.
    Update(UpdateError),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyError::Document(errors) => {
                let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
                f.write_str(&lines.join("\n"))
            }
            ApplyError::NoIntent { line } => write!(f, "line {line}: no INTENT here"),
            ApplyError::AiGroup { line } => write!(f, "line {line}: this INTENT is satisfied by an AI block; use the AI tools"),
            ApplyError::IntentMismatch { line, found } => write!(
                f,
                "line {line} belongs to the INTENT \"{found}\", whose hash is not the one given; line numbers \
                 shift whenever the document changes, so list the intents again. Nothing was changed"
            ),
            ApplyError::NoBlocks => f.write_str("give at least one CODE block"),
            ApplyError::BadStamp { key } => {
                write!(f, "stamp `{key}`: agents may only add keys starting with `{AGENT_PREFIX}` (lowercase letters, digits, -)")
            }
            ApplyError::CommentEnd { block } => write!(
                f,
                "block {block}: the code contains `-->`, which ends the comment early; write \"--\" + \">\" instead"
            ),
            ApplyError::BadCode { block, message } => write!(f, "block {block}: {message}"),
            ApplyError::HandEdited { line } => write!(
                f,
                "line {line}: generated output was edited by hand; replacing the code would discard the edit"
            ),
            ApplyError::Update(e) => write!(f, "the document does not update with the new code:\n{e}"),
        }
    }
}

impl std::error::Error for ApplyError {}

/// Replaces the CODE group of the INTENT at `line` with `bodies` and returns the updated document.
/// With `expected_intent` (an `intent_hash` from a listing), refuses if the line belongs to another
/// INTENT.
pub fn intent_apply(
    doc: &Document,
    fs: &dyn FileSystem,
    line: usize,
    expected_intent: Option<&str>,
    bodies: &[String],
    agent_stamps: &[(String, String)],
) -> Result<String, ApplyError> {
    if bodies.is_empty() {
        return Err(ApplyError::NoBlocks);
    }
    for (key, _) in agent_stamps {
        let valid = key.starts_with(AGENT_PREFIX)
            && key.len() > AGENT_PREFIX.len()
            && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            return Err(ApplyError::BadStamp { key: key.clone() });
        }
    }
    let parsed = doc.parse().map_err(ApplyError::Document)?;
    let text = &doc.text;
    let at = state::block_at(&parsed, text, line).ok_or(ApplyError::NoIntent { line })?;
    let group = parsed.group_of(at).ok_or(ApplyError::NoIntent { line })?;
    state::check_target(parsed.blocks[group.intent].body_text(text), expected_intent)
        .map_err(|found| ApplyError::IntentMismatch { line, found })?;
    if group.members.iter().any(|&m| parsed.blocks[m].kind == Kind::Ai) {
        return Err(ApplyError::AiGroup { line });
    }
    for &m in &group.members {
        splice::check_guard(text, &parsed.blocks[m], &parsed.stamps[m])
            .map_err(|_| ApplyError::HandEdited { line: parsed.blocks[m].line })?;
    }

    let eol = splice::line_ending(text);
    let intent = stamps::intent_digest(parsed.blocks[group.intent].body_text(text));
    let mut new_blocks = String::new();
    for (n, body) in bodies.iter().enumerate() {
        let block = n + 1;
        if body.contains("-->") {
            return Err(ApplyError::CommentEnd { block });
        }
        let code = body.trim_matches('\n').replace("\r\n", "\n");
        tasks::collect(&code).map_err(|e| ApplyError::BadCode { block, message: e.to_string() })?;
        let params = canon::params_digest(&code).map_err(|e| ApplyError::BadCode { block, message: e.message })?;
        let libs: Vec<String> = libs::closure(doc, fs, &code)
            .map_err(|message| ApplyError::BadCode { block, message })?
            .iter()
            .map(|(p, h)| libs::stamp_value(p, h))
            .collect();
        let mut pairs: Vec<(&str, &str)> = vec![("intent", &intent), ("params", &params)];
        pairs.extend(libs.iter().map(|l| ("lib", l.as_str())));
        pairs.extend(agent_stamps.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let stamped = stamps::rewrite(&format!("\n{code}\n"), &pairs);
        new_blocks.push_str(&format!("<!--CODE{stamped}-->\n"));
        if calls_emit(&code) {
            new_blocks.push_str("<!--/CODE-->\n");
        }
    }
    let new_blocks = if eol == "\n" { new_blocks } else { new_blocks.replace('\n', eol) };

    // The span to replace: all members (with the blank lines between them), or an insertion point
    // right after the INTENT.
    let (start, end) = match (group.members.first(), group.members.last()) {
        (Some(&first), Some(&last)) => (parsed.blocks[first].open.start, after_line(text, parsed.blocks[last].end())),
        _ => {
            let at = after_line(text, parsed.blocks[group.intent].end());
            (at, at)
        }
    };
    let mut out = String::with_capacity(text.len() + new_blocks.len());
    out.push_str(&text[..start]);
    if start == text.len() && !text.is_empty() && !text.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(&new_blocks);
    out.push_str(&text[end..]);

    let updated = eval::update(&Document { path: doc.path.clone(), text: out }, fs).map_err(ApplyError::Update)?;
    Ok(updated.text)
}

/// The position after the line that `pos` is on (after its `\n`), or the end of the text.
fn after_line(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map_or(text.len(), |i| pos + i + 1)
}

/// Whether the code calls `emit` anywhere (then the block needs an output region).
fn calls_emit(code: &str) -> bool {
    let Ok(module) = lang::parse("CODE", code) else { return false };
    let mut found = false;
    module.statement().visit_expr(|e| visit(e, &mut found));
    found
}

fn visit(e: &starlark::syntax::ast::AstExpr, found: &mut bool) {
    if let ExprP::Identifier(id) = &e.node
        && id.node.ident == "emit"
    {
        *found = true;
    }
    e.node.visit_expr(|child| visit(child, found));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemoryFs;
    use crate::state::analyze;

    const DOC: &str = "# Doc\n\n<!--INTENT\nShow the version from VERSION.\n-->\n\nAfter.\n";

    fn fs() -> MemoryFs {
        MemoryFs::new().with("VERSION", "2.5.0\n")
    }

    fn apply(text: &str, line: usize, bodies: &[&str]) -> Result<String, ApplyError> {
        let bodies: Vec<String> = bodies.iter().map(|b| b.to_string()).collect();
        intent_apply(&Document::new(text), &fs(), line, None, &bodies, &[])
    }

    #[test]
    fn resolves_an_unresolved_intent_and_runs_it() {
        let out = apply(DOC, 3, &["emit('Version ' + read('VERSION').strip())"]).unwrap();
        assert!(out.contains("-->\n<!--CODE\nemit('Version ' + read('VERSION').strip())\n#@ intent: blake3:"), "{out}");
        assert!(out.contains("-->\nVersion 2.5.0\n<!--/CODE-->\n\nAfter.\n"), "{out}");
        assert!(analyze(&Document::new(out)).unwrap().is_empty());
    }

    #[test]
    fn replaces_the_whole_group_atomically() {
        let first = apply(DOC, 3, &["define('v', read('VERSION').strip())", "emit(var('v'))"]).unwrap();
        assert_eq!(first.matches("<!--CODE").count(), 2);
        // Replace both with one block, addressing the group by a line of its second member.
        let line = first.lines().position(|l| l == "emit(var('v'))").unwrap() + 1;
        let second = apply(&first, line, &["emit('v' + read('VERSION').strip())"]).unwrap();
        assert_eq!(second.matches("<!--CODE").count(), 1, "{second}");
        assert!(second.contains("v2.5.0"));
        assert!(analyze(&Document::new(second)).unwrap().is_empty());
    }

    #[test]
    fn refuses_bad_input() {
        assert!(matches!(apply(DOC, 3, &["emit("]), Err(ApplyError::BadCode { block: 1, .. })));
        assert!(matches!(apply(DOC, 3, &["x = 1\ntasks()"]), Err(ApplyError::BadCode { .. })));
        assert_eq!(apply(DOC, 3, &["emit('-->')"]), Err(ApplyError::CommentEnd { block: 1 }));
        assert!(matches!(apply(DOC, 3, &["emit(read('missing'))"]), Err(ApplyError::Update(_))));
        assert_eq!(apply(DOC, 1, &["x = 1"]), Err(ApplyError::NoIntent { line: 1 }));
        let bad_stamp = intent_apply(&Document::new(DOC), &fs(), 3, None, &["x = 1".into()], &[("intent".into(), "x".into())]);
        assert!(matches!(bad_stamp, Err(ApplyError::BadStamp { .. })));
    }

    #[test]
    fn refuses_to_discard_hand_edits_and_keeps_agent_stamps() {
        let bodies = vec!["emit('a')".to_owned()];
        let stamped = intent_apply(&Document::new(DOC), &fs(), 3, None, &bodies, &[("x-resolved-at".into(), "09fe57".into())]).unwrap();
        assert!(stamped.contains("#@ x-resolved-at: 09fe57"));
        let edited = stamped.replace("-->\na\n<!--/CODE-->", "-->\nA by hand\n<!--/CODE-->");
        let err = apply(&edited, 3, &["emit('b')"]).unwrap_err();
        assert!(matches!(err, ApplyError::HandEdited { .. }), "{err}");
    }

    #[test]
    fn stale_line_with_intent_hash_is_refused() {
        // The scenario from real use: two intents; after the first is applied, the second's old line
        // number points into the first intent's new block.
        let doc = "<!--INTENT\nFirst.\n-->\n\n<!--INTENT\nSecond.\n-->\n";
        let second = stamps::intent_digest("Second.");
        let first_applied = apply(doc, 1, &["emit('one')\nx = 1\ny = 2"]).unwrap();
        let stale_line = 5; // was the second INTENT's line; now inside the first intent's CODE block
        let err = intent_apply(&Document::new(first_applied.clone()), &fs(), stale_line, Some(&second), &["emit('two')".into()], &[])
            .unwrap_err();
        assert_eq!(err, ApplyError::IntentMismatch { line: 5, found: "First.".into() });
        // With the right line and the hash, it works.
        let line = first_applied.lines().position(|l| l == "Second.").unwrap();
        let ok = intent_apply(&Document::new(first_applied), &fs(), line, Some(&second), &["emit('two')".into()], &[]).unwrap();
        assert!(ok.contains("one") && ok.contains("two"));
    }

    #[test]
    fn library_stamps_and_lib_changed() {
        let fs = MemoryFs::new().with("lib/fmt.star", "def shout(s):\n    return s.upper()\n").with("lib/other.star", "x = 1\n");
        let doc = "<!--INTENT\nShout.\n-->\n\n<!--INTENT\nWhisper.\n-->\n";
        let bodies = vec!["load('//lib/fmt.star', 'shout')\nemit(shout('hi'))".to_owned()];
        let applied = intent_apply(&Document::new(doc), &fs, 1, None, &bodies, &[]).unwrap();
        let line = applied.lines().position(|l| l == "Whisper.").unwrap();
        let applied = intent_apply(&Document::new(applied), &fs, line, None, &["emit('hi')".to_owned()], &[]).unwrap();
        assert!(applied.contains("#@ lib: //lib/fmt.star blake3:"), "{applied}");
        assert_eq!(applied.matches("#@ lib:").count(), 1);
        let states = |text: &str, fs: &MemoryFs| -> Vec<(state::State, usize)> {
            state::analyze_libs(&Document::new(text), fs).unwrap().into_iter().map(|f| (f.state, f.line)).collect()
        };
        assert!(states(&applied, &fs).is_empty());
        // Changing the library flags exactly the block that loads it.
        let changed = fs.clone().with("lib/fmt.star", "def shout(s):\n    return s.upper() + '!'\n");
        assert_eq!(states(&applied, &changed), [(state::State::LibChanged, 4)]);
        // Accepting re-stamps it.
        let accepted = state::accept(&Document::new(applied.clone()), &changed, 1).unwrap();
        assert!(states(&accepted, &changed).is_empty());
        // Reformatting a library (line endings) is not a change.
        let crlf = fs.clone().with("lib/fmt.star", "def shout(s):\r\n    return s.upper()\r\n");
        assert!(states(&applied, &crlf).is_empty());
    }

    #[test]
    fn ai_groups_are_refused() {
        let doc = "<!--INTENT\nx\n-->\n<!--AI\n-->\n<!--/AI-->\n";
        assert_eq!(apply(doc, 1, &["x = 1"]), Err(ApplyError::AiGroup { line: 1 }));
    }
}
