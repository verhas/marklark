//! An in-memory Markdown document, its blocks and stamps, and how INTENTs bind blocks (spec §6).
//!
//! A [`Document`] is just text plus an optional project path (used to resolve relative paths in host
//! functions). Nothing here touches storage; the caller reads the text from wherever it lives.

use std::fmt;

use crate::blocks::{self, Block, BlockError, Kind};
use crate::fs::ProjectPath;
use crate::stamps::{StampError, Stamps};

/// A Markdown document held in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Where the document lives in the project, if anywhere. Relative paths in CODE resolve against
    /// its directory; without a path they resolve against the project root.
    pub path: Option<ProjectPath>,
    pub text: String,
}

impl Document {
    pub fn new(text: impl Into<String>) -> Self {
        Document { path: None, text: text.into() }
    }

    pub fn at(path: ProjectPath, text: impl Into<String>) -> Self {
        Document { path: Some(path), text: text.into() }
    }

    /// The directory relative paths resolve against.
    pub fn base_dir(&self) -> ProjectPath {
        self.path.as_ref().map_or_else(ProjectPath::root, ProjectPath::parent)
    }

    /// Locates blocks, parses stamps, and binds INTENTs to their blocks.
    pub fn parse(&self) -> Result<Parsed, Vec<DocError>> {
        Parsed::new(&self.text)
    }
}

/// A structural error, with the 1-based document line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocError {
    Block(BlockError),
    Stamp { line: usize, error: StampError },
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocError::Block(e) => e.fmt(f),
            DocError::Stamp { line, error } => match error {
                StampError::Malformed { line: text, .. } => {
                    write!(f, "line {line}: malformed stamp `{}` (expected `#@ key: value`)", text.trim_end())
                }
                StampError::Duplicate { key, .. } => write!(f, "line {line}: stamp `{key}` occurs more than once"),
            },
        }
    }
}

impl std::error::Error for DocError {}

/// The blocks of a document with their stamps, and the INTENT groups.
#[derive(Debug, Clone)]
pub struct Parsed {
    pub blocks: Vec<Block>,
    /// Stamps per block (same index); always empty for INTENT blocks, whose body is raw prose.
    pub stamps: Vec<Stamps>,
    pub groups: Vec<Group>,
}

/// The blocks an INTENT governs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// Index of the INTENT block.
    pub intent: usize,
    /// Indexes of the member blocks, in document order: CODE blocks, or a single AI block. Empty when
    /// the INTENT is not followed by any (it is then unresolved or detached, spec §9).
    pub members: Vec<usize>,
}

impl Parsed {
    pub fn new(text: &str) -> Result<Parsed, Vec<DocError>> {
        let blocks = blocks::locate(text).map_err(|es| es.into_iter().map(DocError::Block).collect::<Vec<_>>())?;
        let mut errors = Vec::new();
        let mut stamps = Vec::with_capacity(blocks.len());
        for block in &blocks {
            if block.kind == Kind::Intent {
                stamps.push(Stamps::default());
                continue;
            }
            match Stamps::parse(block.body_text(text)) {
                Ok(s) => stamps.push(s),
                Err(error) => {
                    let line = block.line + stamp_error_line(&error);
                    errors.push(DocError::Stamp { line, error });
                    stamps.push(Stamps::default());
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let groups = bind(text, &blocks, &stamps);
        Ok(Parsed { blocks, stamps, groups })
    }

    /// The group a block belongs to, as INTENT or member.
    pub fn group_of(&self, block: usize) -> Option<&Group> {
        self.groups.iter().find(|g| g.intent == block || g.members.contains(&block))
    }

    /// The text of an INTENT block (its body, raw).
    pub fn intent_text<'a>(&self, text: &'a str, group: &Group) -> &'a str {
        self.blocks[group.intent].body_text(text)
    }
}

fn stamp_error_line(e: &StampError) -> usize {
    match e {
        StampError::Malformed { line_index, .. } | StampError::Duplicate { line_index, .. } => *line_index,
    }
}

/// Binds each INTENT to the blocks after it (spec §6):
///
/// - The first member is the CODE or AI block right after the INTENT, with only blank lines between.
/// - An AI block is always a group of its own.
/// - Further CODE members follow while only blank lines separate them from the previous member and
///   their `intent` stamp equals the first member's (`G`). Two blocks without an `intent` stamp count as
///   sharing it, so an INTENT followed by several unstamped CODE blocks binds them all.
fn bind(text: &str, blocks: &[Block], stamps: &[Stamps]) -> Vec<Group> {
    let mut groups = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if block.kind != Kind::Intent {
            continue;
        }
        let mut members = Vec::new();
        let mut prev_end = block.end();
        let mut j = i + 1;
        if let Some(first) = blocks.get(j)
            && first.kind != Kind::Intent
            && only_blank(&text[prev_end..first.open.start])
        {
            members.push(j);
            prev_end = first.end();
            if first.kind == Kind::Code {
                let g = stamps[j].get("intent");
                j += 1;
                while let Some(next) = blocks.get(j)
                    && next.kind == Kind::Code
                    && only_blank(&text[prev_end..next.open.start])
                    && stamps[j].get("intent") == g
                {
                    members.push(j);
                    prev_end = next.end();
                    j += 1;
                }
            }
        }
        groups.push(Group { intent: i, members });
    }
    groups
}

fn only_blank(s: &str) -> bool {
    s.chars().all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(text: &str) -> Vec<(usize, Vec<usize>)> {
        Parsed::new(text).unwrap().groups.into_iter().map(|g| (g.intent, g.members)).collect()
    }

    const INTENT: &str = "<!--INTENT\nShow it.\n-->\n";

    #[test]
    fn intent_binds_following_code_sharing_the_stamp() {
        let doc = format!(
            "{INTENT}<!--CODE\n#@ intent: blake3:g\n-->\nout\n<!--/CODE-->\n\n<!--CODE\n#@ intent: blake3:g\n-->\n<!--CODE\n#@ intent: blake3:other\n-->\n"
        );
        assert_eq!(groups(&doc), [(0, vec![1, 2])]);
    }

    #[test]
    fn human_text_splits_the_group() {
        let doc = format!("{INTENT}<!--CODE\n#@ intent: a\n-->\nprose\n<!--CODE\n#@ intent: a\n-->\n");
        assert_eq!(groups(&doc), [(0, vec![1])]);
    }

    #[test]
    fn unstamped_code_blocks_bind_together() {
        let doc = format!("{INTENT}\n<!--CODE\nx = 1\n-->\n<!--CODE\ny = 2\n-->\n");
        assert_eq!(groups(&doc), [(0, vec![1, 2])]);
    }

    #[test]
    fn ai_block_is_a_group_of_one() {
        let doc = format!("{INTENT}<!--AI\n#@ intent: a\n-->\nprose\n<!--/AI-->\n<!--CODE\n#@ intent: a\n-->\n");
        assert_eq!(groups(&doc), [(0, vec![1])]);
    }

    #[test]
    fn intent_without_blocks_and_consecutive_intents() {
        let doc = format!("{INTENT}text\n{INTENT}{INTENT}<!--CODE-->\n");
        assert_eq!(groups(&doc), [(0, vec![]), (1, vec![]), (2, vec![3])]);
    }

    #[test]
    fn stamp_errors_carry_document_lines() {
        let doc = "a\n<!--CODE\nx = 1\n#@ bad\n-->\n";
        let errs = Document::new(doc).parse().unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].to_string().starts_with("line 4: malformed stamp"), "{}", errs[0]);
    }

    #[test]
    fn intent_body_is_not_parsed_for_stamps() {
        let doc = "<!--INTENT\n#@ not a stamp\n-->\n";
        assert!(Document::new(doc).parse().is_ok());
    }

    #[test]
    fn base_dir() {
        assert_eq!(Document::new("").base_dir(), ProjectPath::root());
        let d = Document::at(ProjectPath::new("docs/guide.md").unwrap(), "");
        assert_eq!(d.base_dir().as_str(), "docs");
    }
}
