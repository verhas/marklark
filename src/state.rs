//! Intent/code states (spec §9), and accepting a hand edit (`intent-accept`).
//!
//! States are computed from the document alone, except `broken`, which needs a run: see
//! [`with_run_result`].

use std::fmt;

use crate::blocks::Kind;
use crate::canon;
use crate::document::{DocError, Document, Parsed};
use crate::eval::UpdateError;
use crate::splice;
use crate::stamps;

/// The states of spec §9 that are reported (`none` and `verified` are not findings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    /// INTENT with no block after it, and no group elsewhere carries its hash.
    Unresolved,
    /// INTENT with no block after it, but a group elsewhere carries its hash.
    Detached,
    /// INTENT followed by blocks without an `intent` stamp.
    Unverified,
    /// The group's `intent` stamp differs from the current INTENT text.
    IntentChanged,
    /// A member's code differs from its `params` stamp (or the stamp is missing).
    ParamsEdited,
    /// A block with an `intent` stamp that belongs to no group.
    Orphan,
    /// A member fails at run time.
    Broken,
    /// An AI block's inputs changed (or it has none yet): its text must be regenerated.
    NeedsUpdate,
    /// An AI block's text was edited by hand; regeneration is blocked until it is accepted.
    OutputEdited,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Unresolved => "unresolved",
            State::Detached => "detached",
            State::Unverified => "unverified",
            State::IntentChanged => "intent_changed",
            State::ParamsEdited => "params_edited",
            State::Orphan => "orphan",
            State::Broken => "broken",
            State::NeedsUpdate => "needs_update",
            State::OutputEdited => "output_edited",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How serious a state is for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Ok,
    Warning,
    Error,
}

/// Which command judges the states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Update,
    IntentCheck,
}

/// The severity table of spec §9.
pub fn severity(state: State, command: Command, strict: bool) -> Severity {
    use Severity::*;
    use State::*;
    if strict {
        return Error;
    }
    match (command, state) {
        (_, Broken) => Error,
        (Command::Update, Unresolved | Detached) => Warning,
        (Command::Update, _) => Ok,
        (Command::IntentCheck, Unresolved | Detached | IntentChanged | OutputEdited) => Error,
        (Command::IntentCheck, Unverified | ParamsEdited | Orphan | NeedsUpdate) => Warning,
    }
}

/// One reported state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub state: State,
    /// 1-based line of the block concerned (the INTENT for group-level states, else the member).
    pub line: usize,
    pub detail: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}: {}", self.line, self.state, self.detail)
    }
}

/// The states of a document that can be computed without running anything.
pub fn analyze(doc: &Document) -> Result<Vec<Finding>, Vec<DocError>> {
    let parsed = doc.parse()?;
    Ok(analyze_parsed(&doc.text, &parsed))
}

fn analyze_parsed(text: &str, parsed: &Parsed) -> Vec<Finding> {
    let mut findings = Vec::new();
    let intent_stamp = |i: usize| parsed.stamps[i].get("intent");

    for group in &parsed.groups {
        let intent_block = &parsed.blocks[group.intent];
        let current = stamps::intent_digest(intent_block.body_text(text));
        if group.members.is_empty() {
            let elsewhere = parsed
                .blocks
                .iter()
                .enumerate()
                .find(|(i, b)| b.kind != Kind::Intent && intent_stamp(*i) == Some(current.as_str()));
            findings.push(match elsewhere {
                Some((_, b)) => Finding {
                    state: State::Detached,
                    line: intent_block.line,
                    detail: format!("no block follows this INTENT, but the block at line {} carries its hash", b.line),
                },
                None => Finding {
                    state: State::Unresolved,
                    line: intent_block.line,
                    detail: "no block satisfies this INTENT yet".into(),
                },
            });
            continue;
        }
        let first = group.members[0];
        match intent_stamp(first) {
            None => findings.push(Finding {
                state: State::Unverified,
                line: intent_block.line,
                detail: "the blocks after this INTENT were never checked against it (no `intent` stamp)".into(),
            }),
            Some(g) if g != current => findings.push(Finding {
                state: State::IntentChanged,
                line: intent_block.line,
                detail: "the INTENT text changed since its blocks were checked against it".into(),
            }),
            Some(_) => {}
        }
        if intent_stamp(first).is_none() {
            continue;
        }
        for &m in &group.members {
            let block = &parsed.blocks[m];
            if block.kind != Kind::Code {
                continue;
            }
            let detail = match (parsed.stamps[m].get("params"), canon::params_digest(block.body_text(text))) {
                (_, Err(_)) => None, // reported as broken by a run
                (None, Ok(_)) => Some("the code has no `params` stamp".to_owned()),
                (Some(stamp), Ok(now)) if stamp != now => Some("the code was edited since it was checked".to_owned()),
                _ => None,
            };
            if let Some(detail) = detail {
                findings.push(Finding { state: State::ParamsEdited, line: block.line, detail });
            }
        }
    }

    for (i, block) in parsed.blocks.iter().enumerate() {
        if block.kind != Kind::Intent && intent_stamp(i).is_some() && parsed.group_of(i).is_none() {
            findings.push(Finding {
                state: State::Orphan,
                line: block.line,
                detail: "this block has an `intent` stamp but no INTENT governs it".into(),
            });
        }
    }
    findings.sort_by_key(|f| f.line);
    findings
}

/// Adds the `broken` finding for a run that failed in a block.
pub fn with_run_result(mut findings: Vec<Finding>, run: &Result<(), UpdateError>) -> Vec<Finding> {
    if let Err(e) = run {
        let (line, detail) = match e {
            UpdateError::Runtime { line, message } => (*line, first_error_line(message)),
            UpdateError::Declaration { line, error } => (*line, error.to_string()),
            UpdateError::EmitWithoutRegion { line } | UpdateError::OutputBreaksBlocks { line } => (*line, e.to_string()),
            UpdateError::DeadEnd(stuck) => {
                for s in stuck {
                    findings.push(Finding { state: State::Broken, line: s.line, detail: "cannot run (dead end)".into() });
                }
                findings.sort_by_key(|f| f.line);
                return findings;
            }
            other => (0, other.to_string()),
        };
        findings.push(Finding { state: State::Broken, line, detail });
        findings.sort_by_key(|f| f.line);
    }
    findings
}

fn first_error_line(message: &str) -> String {
    message
        .lines()
        .find(|l| l.starts_with("error:"))
        .map(|l| l.trim_start_matches("error:").trim().to_owned())
        .unwrap_or_else(|| message.lines().next().unwrap_or("").to_owned())
}

// ---------------------------------------------------------------------------------------------------
// intent-accept
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptError {
    Document(Vec<DocError>),
    /// No INTENT, CODE or AI block starts at or contains the line.
    NoBlock { line: usize },
    /// The block at the line is not governed by an INTENT, so there is nothing to accept against.
    NoIntent { line: usize },
    /// A member's code does not parse, so it cannot be stamped.
    Unparsable { line: usize, message: String },
    /// The group is an AI block: accept it with `ai::accept`, which also rehashes its inputs.
    AiBlock { line: usize },
}

impl fmt::Display for AcceptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcceptError::Document(errors) => {
                let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
                f.write_str(&lines.join("\n"))
            }
            AcceptError::NoBlock { line } => write!(f, "line {line}: no INTENT, CODE or AI block here"),
            AcceptError::NoIntent { line } => write!(f, "line {line}: this block is not governed by an INTENT"),
            AcceptError::Unparsable { line, message } => write!(f, "line {line}: the code does not parse: {message}"),
            AcceptError::AiBlock { line } => write!(f, "line {line}: this is an AI block; accept it with ai::accept"),
        }
    }
}

impl std::error::Error for AcceptError {}

/// The index of the block that starts at or contains `line`.
pub(crate) fn block_at(parsed: &Parsed, text: &str, line: usize) -> Option<usize> {
    let line_of_end = |end: usize| text[..end].matches('\n').count() + 1;
    parsed.blocks.iter().position(|b| b.line <= line && line <= line_of_end(b.end().max(b.open.end)))
}

/// `intent-accept`: a human confirms that the blocks governed by the INTENT at (or containing) `line`
/// still fit it. Every CODE member gets fresh `intent` and `params` stamps; other stamps stay. Returns
/// the new document text.
pub fn accept(doc: &Document, line: usize) -> Result<String, AcceptError> {
    let parsed = doc.parse().map_err(AcceptError::Document)?;
    let text = &doc.text;
    let at = block_at(&parsed, text, line).ok_or(AcceptError::NoBlock { line })?;
    let group = parsed.group_of(at).ok_or(AcceptError::NoIntent { line })?;
    if group.members.is_empty() {
        return Err(AcceptError::NoBlock { line });
    }
    if let Some(&ai) = group.members.iter().find(|&&m| parsed.blocks[m].kind == Kind::Ai) {
        return Err(AcceptError::AiBlock { line: parsed.blocks[ai].line });
    }
    let intent = stamps::intent_digest(parsed.blocks[group.intent].body_text(text));

    let mut updates = Vec::new();
    for &m in &group.members {
        let block = &parsed.blocks[m];
        let params = canon::params_digest(block.body_text(text))
            .map_err(|e| AcceptError::Unparsable { line: block.line, message: e.message })?;
        updates.push((m, vec![("intent", intent.clone()), ("params", params)]));
    }
    let mut out = text.clone();
    // From the last block to the first, so earlier byte ranges stay valid.
    for (m, set) in updates.into_iter().rev() {
        let pairs: Vec<(&str, Option<&str>)> = set.iter().map(|(k, v)| (*k, Some(v.as_str()))).collect();
        out = splice::set_stamps(&out, &parsed.blocks[m], &parsed.stamps[m], &pairs);
    }
    Ok(out)
}

/// `intent-accept --drop-stamp`: removes the `intent` and `params` stamps from the block at (or
/// containing) `line` — e.g. to turn an orphan into a plain CODE block.
pub fn drop_stamps(doc: &Document, line: usize) -> Result<String, AcceptError> {
    let parsed = doc.parse().map_err(AcceptError::Document)?;
    let at = block_at(&parsed, &doc.text, line).ok_or(AcceptError::NoBlock { line })?;
    if parsed.blocks[at].kind == Kind::Intent {
        return Err(AcceptError::NoBlock { line });
    }
    Ok(splice::set_stamps(&doc.text, &parsed.blocks[at], &parsed.stamps[at], &[("intent", None), ("params", None)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(text: &str) -> Vec<(State, usize)> {
        analyze(&Document::new(text)).unwrap().into_iter().map(|f| (f.state, f.line)).collect()
    }

    const INTENT: &str = "<!--INTENT\nShow the version.\n-->\n";

    fn verified_doc() -> String {
        let unstamped = format!("{INTENT}<!--CODE\nemit('1')\n-->\n<!--/CODE-->\n");
        accept(&Document::new(unstamped), 1).unwrap()
    }

    #[test]
    fn unresolved_and_detached() {
        assert_eq!(states(INTENT), [(State::Unresolved, 1)]);
        let hash = stamps::intent_digest("Show the version.");
        let doc = format!("{INTENT}text\n<!--CODE\n#@ intent: {hash}\n-->\n");
        assert_eq!(states(&doc), [(State::Detached, 1), (State::Orphan, 5)]);
    }

    #[test]
    fn unverified_then_accepted_is_verified() {
        let unstamped = format!("{INTENT}<!--CODE\nemit('1')\n-->\n<!--/CODE-->\n");
        assert_eq!(states(&unstamped), [(State::Unverified, 1)]);
        let accepted = verified_doc();
        assert!(states(&accepted).is_empty(), "{accepted}");
        assert!(accepted.contains("#@ intent: blake3:") && accepted.contains("#@ params: ast1:blake3:"));
        // Accepting twice changes nothing.
        assert_eq!(accept(&Document::new(accepted.clone()), 4).unwrap(), accepted);
    }

    #[test]
    fn intent_changed_and_params_edited() {
        let doc = verified_doc();
        let intent_edited = doc.replace("Show the version.", "Show the full version.");
        assert_eq!(states(&intent_edited), [(State::IntentChanged, 1)]);
        let code_edited = doc.replace("emit('1')", "emit('2')");
        assert_eq!(states(&code_edited), [(State::ParamsEdited, 4)]);
        // Reformatting is not an edit (D20).
        let reformatted = doc.replace("emit('1')", "emit( \"1\" )  # same");
        assert!(states(&reformatted).is_empty());
        // Group membership survives the INTENT edit: accepting re-stamps the whole group.
        assert!(states(&accept(&Document::new(intent_edited), 2).unwrap()).is_empty());
    }

    #[test]
    fn drop_stamps_turns_orphan_into_plain_block() {
        let doc = "text\n<!--CODE\nx = 1\n#@ intent: blake3:aa\n#@ x-note: keep\n-->\n";
        assert_eq!(states(doc), [(State::Orphan, 2)]);
        let dropped = drop_stamps(&Document::new(doc), 3).unwrap();
        assert_eq!(dropped, "text\n<!--CODE\nx = 1\n#@ x-note: keep\n-->\n");
        assert!(states(&dropped).is_empty());
    }

    #[test]
    fn accept_errors() {
        assert_eq!(accept(&Document::new("text\n"), 1), Err(AcceptError::NoBlock { line: 1 }));
        assert_eq!(accept(&Document::new("<!--CODE-->\n"), 1), Err(AcceptError::NoIntent { line: 1 }));
    }

    #[test]
    fn severities() {
        assert_eq!(severity(State::Unresolved, Command::Update, false), Severity::Warning);
        assert_eq!(severity(State::IntentChanged, Command::Update, false), Severity::Ok);
        assert_eq!(severity(State::IntentChanged, Command::IntentCheck, false), Severity::Error);
        assert_eq!(severity(State::ParamsEdited, Command::IntentCheck, false), Severity::Warning);
        assert_eq!(severity(State::ParamsEdited, Command::IntentCheck, true), Severity::Error);
    }
}
