//! `#@ key: value` stamp lines: parsing, writing, reserved and agent-owned keys (spec §5, D19, D31).
//!
//! A stamp line is any line of a block body whose first non-blank characters are `#@`. Every stamp line
//! carries the prefix (D19), so a line identifies itself regardless of position. marklark writes stamps
//! at the end of the body; when rewriting, it removes every stamp line and appends the new set, leaving
//! all other lines byte for byte.

use std::fmt;
use std::ops::Range;

/// Prefix of the BLAKE3 digests written in stamps (D37).
pub const BLAKE3_PREFIX: &str = "blake3:";

/// Keys only marklark writes (spec §5.2a). A human may edit `dep` lines (§12), but no agent may submit
/// any of these through `intent_apply`.
pub const RESERVED_KEYS: &[&str] = &["intent", "params", "generated", "dep", "deps"];

/// Keys that may occur more than once in one block.
const REPEATABLE_KEYS: &[&str] = &["dep"];

/// Prefix of agent-owned keys (D31).
pub const AGENT_PREFIX: &str = "x-";

/// Who owns a stamp key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyClass {
    /// Written only by marklark.
    Reserved,
    /// Agent-owned (`x-…`); opaque to marklark.
    Agent,
    /// Neither: preserved unchanged, never interpreted.
    Unknown,
}

pub fn classify(key: &str) -> KeyClass {
    if RESERVED_KEYS.contains(&key) {
        KeyClass::Reserved
    } else if key.starts_with(AGENT_PREFIX) {
        KeyClass::Agent
    } else {
        KeyClass::Unknown
    }
}

/// One parsed stamp line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub key: String,
    pub value: String,
    /// Byte range of the whole line within the body, including its line terminator.
    pub line_range: Range<usize>,
    /// 0-based line index within the body.
    pub line_index: usize,
}

/// A malformed stamp section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StampError {
    /// A `#@` line that is not `#@ key: value`.
    Malformed { line_index: usize, line: String },
    /// A non-repeatable key that occurs more than once.
    Duplicate { key: String, line_index: usize },
}

impl fmt::Display for StampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StampError::Malformed { line_index, line } => write!(
                f,
                "body line {}: malformed stamp `{}` (expected `#@ key: value`)",
                line_index + 1,
                line.trim_end()
            ),
            StampError::Duplicate { key, line_index } => {
                write!(f, "body line {}: stamp `{key}` occurs more than once", line_index + 1)
            }
        }
    }
}

impl std::error::Error for StampError {}

/// The stamps of one block body, in the order they appear.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stamps {
    pub stamps: Vec<Stamp>,
}

impl Stamps {
    /// Parses every stamp line of `body`.
    pub fn parse(body: &str) -> Result<Stamps, StampError> {
        let mut stamps: Vec<Stamp> = Vec::new();
        for (line_index, (range, line)) in lines_with_ranges(body).enumerate() {
            let Some(rest) = stamp_payload(line) else { continue };
            let (key, value) = parse_payload(rest).ok_or_else(|| StampError::Malformed {
                line_index,
                line: line.to_owned(),
            })?;
            if !REPEATABLE_KEYS.contains(&key) && stamps.iter().any(|s| s.key == key) {
                return Err(StampError::Duplicate { key: key.to_owned(), line_index });
            }
            stamps.push(Stamp {
                key: key.to_owned(),
                value: value.to_owned(),
                line_range: range,
                line_index,
            });
        }
        Ok(Stamps { stamps })
    }

    /// The value of a non-repeatable key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.stamps.iter().find(|s| s.key == key).map(|s| s.value.as_str())
    }

    /// All values of a key, in order (for repeatable keys such as `dep`).
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.stamps.iter().filter(move |s| s.key == key).map(|s| s.value.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.stamps.is_empty()
    }
}

/// Returns `body` with every stamp line removed and `stamps` appended at the end, one `#@ key: value`
/// line each. Non-stamp lines are kept byte for byte; trailing blank lines before the new stamps are
/// dropped so repeated rewrites are stable. The body's line ending (`\r\n` if it uses one) is used for
/// the new lines. An empty `stamps` just removes the stamp lines.
pub fn rewrite(body: &str, stamps: &[(&str, &str)]) -> String {
    let eol = if body.contains("\r\n") { "\r\n" } else { "\n" };
    let mut kept: Vec<&str> = lines_with_ranges(body)
        .map(|(_, line)| line)
        .filter(|line| stamp_payload(line).is_none())
        .collect();
    if stamps.is_empty() {
        return kept.concat();
    }
    // Drop whole trailing blank lines so the stamps follow the code directly; the first line (the rest
    // of the opening `<!--CODE` line) is never dropped. Other lines keep every byte.
    while kept.len() > 1 && kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.concat();
    // Stamps need a line of their own; an empty body (`<!--CODE-->`) also needs the separator after
    // the block name.
    if !out.ends_with('\n') {
        out.push_str(eol);
    }
    for (key, value) in stamps {
        out.push_str("#@ ");
        out.push_str(key);
        out.push_str(": ");
        out.push_str(value);
        out.push_str(eol);
    }
    out
}

/// `blake3:<hex>` of `bytes`.
pub fn blake3_digest(bytes: &[u8]) -> String {
    format!("{BLAKE3_PREFIX}{}", blake3::hash(bytes).to_hex())
}

/// The `intent` stamp value: hash of the INTENT text with whitespace runs collapsed to one space and
/// trimmed (spec §5.1).
pub fn intent_digest(intent_text: &str) -> String {
    let normalised = intent_text.split_whitespace().collect::<Vec<_>>().join(" ");
    blake3_digest(normalised.as_bytes())
}

/// The `generated` stamp value: `<length>:blake3:<hex>` of an output region.
///
/// Resilient to line endings and character encoding (D38): line endings are normalised to `\n`, the
/// length counts Unicode characters (not bytes), and the hash is over the UTF-8 of the normalised text.
/// A file converted between CRLF and LF, or re-saved in another encoding, keeps its stamp.
pub fn generated_digest(output: &str) -> String {
    let normalised = normalise_line_endings(output);
    format!("{}:{}", normalised.chars().count(), blake3_digest(normalised.as_bytes()))
}

/// `\r\n` and lone `\r` become `\n`.
pub fn normalise_line_endings(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\r') {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// Splits into lines, each with its terminator, together with its byte range.
fn lines_with_ranges(text: &str) -> impl Iterator<Item = (Range<usize>, &str)> {
    let mut start = 0;
    text.split_inclusive('\n').map(move |line| {
        let range = start..start + line.len();
        start = range.end;
        (range, line)
    })
}

/// If `line` is a stamp line, returns what follows `#@`.
fn stamp_payload(line: &str) -> Option<&str> {
    line.trim_start_matches([' ', '\t']).strip_prefix("#@")
}

/// Parses ` key: value` (after `#@`). Keys are lowercase ASCII letters, digits and `-`, starting with a
/// letter. The value is everything after `: `, without the line terminator or trailing blanks; it may be
/// empty.
fn parse_payload(rest: &str) -> Option<(&str, &str)> {
    let rest = rest.strip_prefix(' ')?;
    let (key, value) = rest.split_once(':')?;
    let mut chars = key.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    if !first_ok || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return None;
    }
    let value = value.trim_end_matches(['\n', '\r', ' ', '\t']);
    let value = match value.strip_prefix(' ') {
        Some(v) => v,
        None if value.is_empty() => value,
        None => return None,
    };
    Some((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "\nemit(\"x\")\n#@ intent: blake3:aa\n#@ params: ast1:blake3:bb\n#@ x-resolved-at: 09fe57\n";

    #[test]
    fn parses_stamps_in_order() {
        let s = Stamps::parse(BODY).unwrap();
        let keys: Vec<_> = s.stamps.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["intent", "params", "x-resolved-at"]);
        assert_eq!(s.get("params"), Some("ast1:blake3:bb"));
        assert_eq!(s.stamps[0].line_index, 2);
        assert_eq!(&BODY[s.stamps[0].line_range.clone()], "#@ intent: blake3:aa\n");
    }

    #[test]
    fn ordinary_comments_are_not_stamps() {
        let s = Stamps::parse("# intent: compute the TOC\nx = 1\n").unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn indented_and_crlf_stamps() {
        let s = Stamps::parse("x = 1\r\n  #@ intent: blake3:aa  \r\n").unwrap();
        assert_eq!(s.get("intent"), Some("blake3:aa"));
    }

    #[test]
    fn repeatable_dep_and_duplicate_error() {
        let s = Stamps::parse("#@ dep: read(\"a\") blake3:1\n#@ dep: read(\"b\")\n").unwrap();
        assert_eq!(s.get_all("dep").collect::<Vec<_>>(), ["read(\"a\") blake3:1", "read(\"b\")"]);
        let e = Stamps::parse("#@ intent: a\n#@ intent: b\n").unwrap_err();
        assert_eq!(e, StampError::Duplicate { key: "intent".into(), line_index: 1 });
    }

    #[test]
    fn malformed_stamps() {
        for bad in ["#@intent: a\n", "#@ Intent: a\n", "#@ intent a\n", "#@ intent:a\n", "#@\n", "#@ 1x: a\n"] {
            assert!(
                matches!(Stamps::parse(bad), Err(StampError::Malformed { .. })),
                "{bad:?} should be malformed"
            );
        }
        assert_eq!(Stamps::parse("#@ empty:\n").unwrap().get("empty"), Some(""));
    }

    #[test]
    fn classify_keys() {
        assert_eq!(classify("params"), KeyClass::Reserved);
        assert_eq!(classify("dep"), KeyClass::Reserved);
        assert_eq!(classify("x-resolved-at"), KeyClass::Agent);
        assert_eq!(classify("future"), KeyClass::Unknown);
    }

    #[test]
    fn rewrite_replaces_stamps_and_keeps_code_bytes() {
        let body = "\nemit(\"x\")   \n\n#@ intent: old\n\n";
        let out = rewrite(body, &[("intent", "new"), ("generated", "1:blake3:cc")]);
        assert_eq!(out, "\nemit(\"x\")   \n#@ intent: new\n#@ generated: 1:blake3:cc\n");
        // Stable: rewriting again with the same stamps changes nothing.
        assert_eq!(rewrite(&out, &[("intent", "new"), ("generated", "1:blake3:cc")]), out);
    }

    #[test]
    fn rewrite_stamps_only_body_and_crlf() {
        assert_eq!(rewrite("\n#@ intent: a\n", &[("intent", "b")]), "\n#@ intent: b\n");
        assert_eq!(rewrite("\r\nx = 1\r\n", &[("intent", "b")]), "\r\nx = 1\r\n#@ intent: b\r\n");
        assert_eq!(rewrite("\nx = 1\n#@ intent: a\n", &[]), "\nx = 1\n");
        // `<!--CODE-->` and `<!--CODE x = 1-->`: stamps still get lines of their own.
        assert_eq!(rewrite("", &[("intent", "b")]), "\n#@ intent: b\n");
        assert_eq!(rewrite(" x = 1", &[("intent", "b")]), " x = 1\n#@ intent: b\n");
    }

    #[test]
    fn digests() {
        assert_eq!(intent_digest("  a \n\t b "), intent_digest("a b"));
        assert_ne!(intent_digest("a b"), intent_digest("ab"));
        assert!(blake3_digest(b"").starts_with("blake3:"));
        assert!(generated_digest("héllo").starts_with("5:blake3:"));
        assert_eq!(generated_digest("a\r\nb\r\n"), generated_digest("a\nb\n"));
        assert_eq!(generated_digest("a\rb"), generated_digest("a\nb"));
        assert!(generated_digest("a\r\nb\r\n").starts_with("4:"));
    }
}
