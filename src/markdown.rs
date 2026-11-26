//! Read-only Markdown helpers for host functions: headings, anchors, code fences.
//!
//! The Markdown parser is used only to find things by source position (spec §10); nothing here
//! renders or rewrites a document.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// One heading of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    /// The heading's plain text (inline markup removed, code spans kept as their text).
    pub text: String,
    /// 1-based line.
    pub line: usize,
    /// GitHub-style anchor, unique within the document (`-1`, `-2`, … for repeats).
    pub anchor: String,
}

/// All headings, in document order.
pub fn headings(doc: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut current: Option<(u8, usize, String)> = None;
    for (event, range) in Parser::new_ext(doc, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let line = doc[..range.start].matches('\n').count() + 1;
                current = Some((level_number(level), line, String::new()));
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, _, text)) = current.as_mut() {
                    text.push_str(&t);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, _, text)) = current.as_mut() {
                    text.push(' ');
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, line, text)) = current.take() {
                    let base = slug(&text);
                    let n = seen.entry(base.clone()).or_insert(0);
                    let anchor = if *n == 0 { base.clone() } else { format!("{base}-{n}") };
                    *n += 1;
                    out.push(Heading { level, text: text.trim().to_owned(), line, anchor });
                }
            }
            _ => {}
        }
    }
    out
}

fn level_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// GitHub's heading anchor: lowercase; letters, digits, `_` and `-` kept; spaces become `-`; everything
/// else dropped.
pub fn slug(text: &str) -> String {
    text.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '_' || c == '-' => Some(c),
            _ => None,
        })
        .collect()
}

/// Wraps `text` in a backtick fence longer than any backtick run inside it.
pub fn fence(text: &str, lang: &str) -> String {
    let longest = text
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let ticks = "`".repeat(longest.max(2) + 1);
    let mut out = format!("{ticks}{lang}\n{text}");
    if !text.is_empty() && !text.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&ticks);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_with_lines_and_unique_anchors() {
        let doc = "# Title\n\ntext\n\n## Getting `started`!\n\n```\n# not a heading\n```\n\n## Getting started\n";
        let h = headings(doc);
        assert_eq!(
            h,
            vec![
                Heading { level: 1, text: "Title".into(), line: 1, anchor: "title".into() },
                Heading { level: 2, text: "Getting started!".into(), line: 5, anchor: "getting-started".into() },
                Heading { level: 2, text: "Getting started".into(), line: 11, anchor: "getting-started-1".into() },
            ]
        );
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("API v2.0 — Überblick"), "api-v20--überblick");
        assert_eq!(slug("snake_case and-dash"), "snake_case-and-dash");
    }

    #[test]
    fn fences() {
        assert_eq!(fence("x = 1", "python"), "```python\nx = 1\n```\n");
        assert_eq!(fence("a ```` b\n", ""), "`````\na ```` b\n`````\n");
    }
}
