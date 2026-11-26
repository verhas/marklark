//! Byte-range splicing of output regions, guarded by the `generated` stamp (spec §10).
//!
//! The document is never re-rendered: an update replaces exactly the bytes of an output region and of
//! the block body's stamp lines; everything else stays byte for byte.

use std::fmt;

use crate::blocks::Block;
use crate::stamps::{self, Stamps};

/// The output region no longer matches its `generated` stamp: a human edited generated text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityError {
    pub line: usize,
}

impl fmt::Display for IntegrityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}: the generated output was edited by hand; restore it, or delete the block's \
             `#@ generated:` stamp line to accept losing the edit",
            self.line
        )
    }
}

impl std::error::Error for IntegrityError {}

/// Checks that a block's output region still matches its `generated` stamp. A block without a region
/// or without the stamp passes.
pub fn check_guard(text: &str, block: &Block, stamps: &Stamps) -> Result<(), IntegrityError> {
    match (block.output_text(text), stamps.get("generated")) {
        (Some(output), Some(stamp)) if stamps::generated_digest(output) != stamp => {
            Err(IntegrityError { line: block.line })
        }
        _ => Ok(()),
    }
}

/// The line ending the document uses (that of its first line), for text marklark inserts.
pub fn line_ending(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// Prepares emitted text for a region: `\n` becomes the document's line ending, and non-empty output
/// ends with a line ending so the closing marker keeps its own line.
pub fn region_text(emitted: &str, eol: &str) -> String {
    let normalised = stamps::normalise_line_endings(emitted);
    let mut out = if eol == "\n" { normalised.into_owned() } else { normalised.replace('\n', eol) };
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out
}

/// Replaces a block's output region with `output` and sets its `generated` stamp, keeping every other
/// stamp in place and order. Returns the new document text. The block must have an output region.
pub fn write_output(text: &str, block: &Block, stamps: &Stamps, output: &str) -> String {
    let region = block.output.clone().expect("write_output: block has no output region");
    let digest = stamps::generated_digest(output);
    let mut pairs: Vec<(&str, &str)> = stamps.stamps.iter().map(|s| (s.key.as_str(), s.value.as_str())).collect();
    match pairs.iter_mut().find(|(k, _)| *k == "generated") {
        Some(pair) => pair.1 = &digest,
        None => pairs.push(("generated", &digest)),
    }
    let body = stamps::rewrite(block.body_text(text), &pairs);

    let mut out = String::with_capacity(text.len() + output.len());
    out.push_str(&text[..block.body.start]);
    out.push_str(&body);
    out.push_str(&text[block.body.end..region.start]);
    out.push_str(output);
    out.push_str(&text[region.end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::locate;

    fn first(text: &str) -> (Block, Stamps) {
        let b = locate(text).unwrap().remove(0);
        let s = Stamps::parse(b.body_text(text)).unwrap();
        (b, s)
    }

    #[test]
    fn writes_output_and_generated_stamp() {
        let text = "before\n<!--CODE\nemit('x')\n#@ intent: blake3:aa\n-->\nold\n<!--/CODE-->\nafter\n";
        let (b, s) = first(text);
        let out = write_output(text, &b, &s, "new\n");
        let expected_stamp = stamps::generated_digest("new\n");
        assert_eq!(
            out,
            format!("before\n<!--CODE\nemit('x')\n#@ intent: blake3:aa\n#@ generated: {expected_stamp}\n-->\nnew\n<!--/CODE-->\nafter\n")
        );
        // The guard accepts what was just written, and an idempotent rewrite changes nothing.
        let (b2, s2) = first(&out);
        assert!(check_guard(&out, &b2, &s2).is_ok());
        assert_eq!(write_output(&out, &b2, &s2, "new\n"), out);
    }

    #[test]
    fn guard_detects_hand_edits_but_not_line_ending_changes() {
        let text = "<!--CODE\n#@ generated: GEN\n-->\nout\n<!--/CODE-->\n".replace("GEN", &stamps::generated_digest("out\n"));
        let (b, s) = first(&text);
        assert!(check_guard(&text, &b, &s).is_ok());
        let edited = text.replace("\nout\n", "\nOUT\n");
        let (b, s) = first(&edited);
        assert_eq!(check_guard(&edited, &b, &s), Err(IntegrityError { line: 1 }));
        let crlf = text.replace('\n', "\r\n");
        let (b, s) = first(&crlf);
        assert!(check_guard(&crlf, &b, &s).is_ok());
    }

    #[test]
    fn region_text_line_endings() {
        assert_eq!(region_text("a\nb", "\n"), "a\nb\n");
        assert_eq!(region_text("a\nb\n", "\r\n"), "a\r\nb\r\n");
        assert_eq!(region_text("", "\n"), "");
        assert_eq!(line_ending("x\r\ny"), "\r\n");
        assert_eq!(line_ending("x\ny"), "\n");
    }
}
