//! Locating INTENT, CODE and AI blocks in a document (spec §2).
//!
//! A block is an HTML comment that starts at the beginning of a line (leading blanks allowed), is not
//! inside a fenced or indented code block or an inline code span, and has the block name immediately
//! after `<!--`. A block comment ends at the first `-->`. Closing markers (`<!--/CODE-->`,
//! `<!--/AI-->`) follow the same line rule.
//!
//! Everything is reported as byte ranges into the document; nothing is re-rendered (spec §10). A
//! Markdown parser is used only to find code blocks and code spans by source position.

use std::fmt;
use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

/// The kinds of marklark block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Intent,
    Code,
    Ai,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Intent => "INTENT",
            Kind::Code => "CODE",
            Kind::Ai => "AI",
        }
    }

    const ALL: [Kind; 3] = [Kind::Intent, Kind::Code, Kind::Ai];
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One located block. All ranges are byte ranges into the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    /// 1-based line of the opening `<!--`.
    pub line: usize,
    /// The whole opening comment, from `<!--` to the end of `-->`.
    pub open: Range<usize>,
    /// The body: after the block name, up to (not including) `-->`.
    pub body: Range<usize>,
    /// The closing marker (`<!--/CODE-->` or `<!--/AI-->`), without its line terminator.
    pub close: Option<Range<usize>>,
    /// The output region: from after the opening comment (and its line terminator, if `-->` ends the
    /// line) to the start of the closing marker's line. Present exactly when `close` is.
    pub output: Option<Range<usize>>,
}

impl Block {
    pub fn body_text<'a>(&self, doc: &'a str) -> &'a str {
        &doc[self.body.clone()]
    }

    pub fn output_text<'a>(&self, doc: &'a str) -> Option<&'a str> {
        self.output.clone().map(|r| &doc[r])
    }

    /// End of the block: after its closing marker, or after the opening comment if it has none.
    pub fn end(&self) -> usize {
        self.close.as_ref().map_or(self.open.end, |c| c.end)
    }
}

/// A structural error in the document's blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockError {
    /// An opening comment with no `-->`.
    Unterminated { kind: Kind, line: usize },
    /// An AI block without `<!--/AI-->` (AI content always ends at the marker, D36).
    MissingClose { kind: Kind, line: usize },
    /// A closing marker that does not close the block directly before it.
    StrayClose { kind: Kind, line: usize },
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockError::Unterminated { kind, line } => {
                write!(f, "line {line}: <!--{kind} block is not closed with -->")
            }
            BlockError::MissingClose { kind, line } => {
                write!(f, "line {line}: <!--{kind} block has no <!--/{kind}--> marker")
            }
            BlockError::StrayClose { kind, line } => write!(
                f,
                "line {line}: <!--/{kind}--> does not follow a <!--{kind} block it could close"
            ),
        }
    }
}

impl std::error::Error for BlockError {}

/// Locates every block in `doc`. Returns all structural errors found, not just the first.
pub fn locate(doc: &str) -> Result<Vec<Block>, Vec<BlockError>> {
    let lines = LineIndex::new(doc);
    let (tokens, mut errors) = scan(doc, &lines);
    let blocks = pair(doc, tokens, &mut errors);
    if errors.is_empty() { Ok(blocks) } else { Err(errors) }
}

#[derive(Debug)]
enum Token {
    Open { kind: Kind, line: usize, open: Range<usize>, body: Range<usize> },
    Close { kind: Kind, line: usize, range: Range<usize> },
}

/// Finds every opening comment and closing marker, in document order.
fn scan(doc: &str, lines: &LineIndex) -> (Vec<Token>, Vec<BlockError>) {
    let code = code_ranges(doc);
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut pos = 0;
    while let Some(found) = doc[pos..].find("<!--") {
        let start = pos + found;
        if let Some(skip_to) = code.iter().find(|r| r.contains(&start)).map(|r| r.end) {
            pos = skip_to;
            continue;
        }
        let after = start + 4;
        let at_line_start = doc[lines.line_start(start)..start].bytes().all(|b| b == b' ' || b == b'\t');

        if at_line_start {
            if let Some((kind, len)) = closing_marker(&doc[after..]) {
                let range = start..after + len;
                if rest_of_line_is_blank(doc, range.end) {
                    tokens.push(Token::Close { kind, line: lines.line_of(start), range: range.clone() });
                    pos = range.end;
                    continue;
                }
            }
            if let Some(kind) = opening_name(&doc[after..]) {
                let name_end = after + kind.name().len();
                let line = lines.line_of(start);
                match doc[name_end..].find("-->") {
                    Some(rel) => {
                        let body = name_end..name_end + rel;
                        let open = start..body.end + 3;
                        pos = open.end;
                        tokens.push(Token::Open { kind, line, open, body });
                        continue;
                    }
                    None => {
                        errors.push(BlockError::Unterminated { kind, line });
                        break;
                    }
                }
            }
        }
        // An ordinary HTML comment: skip it whole, so its content is never mistaken for a block.
        pos = match doc[after..].find("-->") {
            Some(rel) => after + rel + 3,
            None => after,
        };
    }
    (tokens, errors)
}

/// Pairs opening comments with the closing marker that directly follows them.
fn pair(doc: &str, tokens: Vec<Token>, errors: &mut Vec<BlockError>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut iter = tokens.into_iter().peekable();
    while let Some(token) = iter.next() {
        match token {
            Token::Close { kind, line, .. } => errors.push(BlockError::StrayClose { kind, line }),
            Token::Open { kind, line, open, body } => {
                let mut close = None;
                if let Some(Token::Close { kind: ck, range, .. }) = iter.peek()
                    && *ck == kind
                    && kind != Kind::Intent
                {
                    close = Some(range.clone());
                    iter.next();
                }
                if close.is_none() && kind == Kind::Ai {
                    errors.push(BlockError::MissingClose { kind, line });
                }
                let output = close.as_ref().map(|c| output_start(doc, open.end)..line_start_of(doc, c.start));
                blocks.push(Block { kind, line, open, body, close, output });
            }
        }
    }
    blocks
}

/// `INTENT`, `CODE` or `AI` directly after `<!--`, followed by whitespace or `-->`.
fn opening_name(after: &str) -> Option<Kind> {
    Kind::ALL.into_iter().find(|k| {
        after.strip_prefix(k.name()).is_some_and(|rest| {
            rest.starts_with("-->") || rest.starts_with(|c: char| c.is_whitespace())
        })
    })
}

/// `/CODE-->` or `/AI-->` directly after `<!--`; returns the kind and the length after `<!--`.
fn closing_marker(after: &str) -> Option<(Kind, usize)> {
    [Kind::Code, Kind::Ai].into_iter().find_map(|k| {
        after.strip_prefix('/')?.strip_prefix(k.name())?.strip_prefix("-->")?;
        Some((k, 1 + k.name().len() + 3))
    })
}

fn rest_of_line_is_blank(doc: &str, pos: usize) -> bool {
    doc[pos..].split('\n').next().unwrap_or("").trim().is_empty()
}

/// Where an output region starts: after the opening comment, plus its line terminator if `-->` ends
/// the line.
fn output_start(doc: &str, open_end: usize) -> usize {
    let rest = &doc[open_end..];
    if rest.starts_with("\r\n") {
        open_end + 2
    } else if rest.starts_with('\n') {
        open_end + 1
    } else {
        open_end
    }
}

fn line_start_of(doc: &str, pos: usize) -> usize {
    doc[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Byte ranges of fenced and indented code blocks and of inline code spans.
fn code_ranges(doc: &str) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Parser::new_ext(doc, Options::empty())
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Start(Tag::CodeBlock(_)) | Event::Code(_) => Some(range),
            _ => None,
        })
        .collect();
    ranges.sort_by_key(|r| r.start);
    ranges
}

/// Maps byte offsets to line numbers.
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(doc: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(doc.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { starts }
    }

    /// 1-based line containing `pos`.
    fn line_of(&self, pos: usize) -> usize {
        self.starts.partition_point(|&s| s <= pos)
    }

    fn line_start(&self, pos: usize) -> usize {
        self.starts[self.line_of(pos) - 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(doc: &str) -> Vec<Kind> {
        locate(doc).unwrap().iter().map(|b| b.kind).collect()
    }

    #[test]
    fn intent_and_code_with_output() {
        let doc = "# Title\n\n<!--INTENT\nShow the version.\n-->\n<!--CODE\nemit(\"1.0\")\n-->\nold\n<!--/CODE-->\ntail\n";
        let blocks = locate(doc).unwrap();
        assert_eq!(blocks.len(), 2);
        let (intent, code) = (&blocks[0], &blocks[1]);
        assert_eq!(intent.kind, Kind::Intent);
        assert_eq!(intent.line, 3);
        assert_eq!(intent.body_text(doc), "\nShow the version.\n");
        assert_eq!(intent.close, None);
        assert_eq!(code.kind, Kind::Code);
        assert_eq!(code.line, 6);
        assert_eq!(code.body_text(doc), "\nemit(\"1.0\")\n");
        assert_eq!(code.output_text(doc), Some("old\n"));
        assert_eq!(&doc[code.close.clone().unwrap()], "<!--/CODE-->");
        assert_eq!(&doc[code.end()..], "\ntail\n");
    }

    #[test]
    fn code_without_output_region() {
        let doc = "<!--CODE\ndefine(\"a\", 1)\n-->\ntext\n<!--CODE\nemit(\"x\")\n-->\n<!--/CODE-->\n";
        let blocks = locate(doc).unwrap();
        assert_eq!(blocks[0].close, None);
        assert_eq!(blocks[0].output, None);
        assert_eq!(blocks[1].output_text(doc), Some(""));
    }

    #[test]
    fn ai_block_requires_close() {
        let doc = "<!--AI\n#@ intent: blake3:aa\n-->\nprose\n<!--/AI-->\n";
        let b = &locate(doc).unwrap()[0];
        assert_eq!(b.kind, Kind::Ai);
        assert_eq!(b.output_text(doc), Some("prose\n"));
        assert_eq!(
            locate("<!--AI\n-->\nprose\n").unwrap_err(),
            vec![BlockError::MissingClose { kind: Kind::Ai, line: 1 }]
        );
    }

    #[test]
    fn ignores_code_fences_spans_and_indented_code() {
        let doc = "```markdown\n<!--CODE\nx\n-->\n```\n\n`<!--CODE -->` inline\n\n    <!--CODE\n    -->\n\n<!--CODE-->\n";
        assert_eq!(kinds(doc), [Kind::Code]);
        assert_eq!(locate(doc).unwrap()[0].line, 12);
    }

    #[test]
    fn must_start_a_line_but_leading_blanks_are_allowed() {
        assert_eq!(kinds("text <!--CODE-->\n"), []);
        assert_eq!(kinds("  <!--CODE\n  -->\n"), [Kind::Code]);
    }

    #[test]
    fn name_must_end_at_whitespace_or_comment_end() {
        assert_eq!(kinds("<!--CODEX\n-->\n<!--INTENTION -->\n<!-- CODE -->\n"), []);
        assert_eq!(kinds("<!--CODE-->\n<!--INTENT\tx -->\n"), [Kind::Code, Kind::Intent]);
    }

    #[test]
    fn ordinary_comments_hide_their_content() {
        let doc = "<!-- note\n<!--CODE\n-->\n<!--CODE\n-->\n";
        let blocks = locate(doc).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].line, 4);
    }

    #[test]
    fn intent_body_may_mention_blocks() {
        let doc = "<!--INTENT\nAdd a <!--CODE block that...\n-->\n";
        assert_eq!(kinds(doc), [Kind::Intent]);
    }

    #[test]
    fn errors() {
        assert_eq!(
            locate("ok\n<!--CODE\nx = 1\n").unwrap_err(),
            vec![BlockError::Unterminated { kind: Kind::Code, line: 2 }]
        );
        assert_eq!(
            locate("<!--INTENT\nx\n-->\n<!--/CODE-->\n").unwrap_err(),
            vec![BlockError::StrayClose { kind: Kind::Code, line: 4 }]
        );
        // A CODE block followed by an AI closing marker: the marker is stray.
        assert_eq!(
            locate("<!--CODE\n-->\n<!--/AI-->\n").unwrap_err(),
            vec![BlockError::StrayClose { kind: Kind::Ai, line: 3 }]
        );
    }

    #[test]
    fn crlf_output_region() {
        let doc = "<!--CODE\r\nemit(\"x\")\r\n-->\r\nold\r\n<!--/CODE-->\r\n";
        assert_eq!(locate(doc).unwrap()[0].output_text(doc), Some("old\r\n"));
    }

    #[test]
    fn closing_marker_needs_its_own_line() {
        // Text after the marker on the same line: not a marker, so the CODE block has no region and the
        // line is an ordinary comment.
        let doc = "<!--CODE\n-->\n<!--/CODE--> trailing\n";
        let b = &locate(doc).unwrap()[0];
        assert_eq!(b.close, None);
    }
}
