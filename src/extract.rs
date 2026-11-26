//! Pure text extraction behind the host functions: `slice` (mdship INCLUDE semantics), `reindent`
//! (mdship `margin`), and regex helpers. No I/O here; the host passes text in.
//!
//! Every selector fails loudly: a `start` or `end` that never matches, a range outside the file, or a
//! missing section is an error, never "include nothing".

use regex::Regex;

use crate::markdown;

/// How to cut a piece out of a text. At most one of `range`, `start`/`end`, `section` may be used.
#[derive(Debug, Clone, Default)]
pub struct Slice<'a> {
    /// Regex; the piece starts after (or, with `include_start`, at) the first matching line.
    pub start: Option<&'a str>,
    /// Regex; the piece ends before (or, with `include_end`, at) the first matching line after `start`.
    pub end: Option<&'a str>,
    /// `"x..y"`: lines x through y, 1-based, inclusive.
    pub range: Option<&'a str>,
    /// A Markdown heading title (case-insensitive, numbering like `2.3.` ignored): the heading line and
    /// everything up to the next heading of the same or a higher level.
    pub section: Option<&'a str>,
    pub include_start: bool,
    pub include_end: bool,
    /// With `start` and `end`: concatenate every segment they bound, not just the first.
    pub repeat: bool,
}

/// Cuts the piece out of `text`. Lines keep their own line endings.
pub fn slice(text: &str, spec: &Slice) -> Result<String, String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let selectors = [spec.range.is_some(), spec.start.is_some() || spec.end.is_some(), spec.section.is_some()];
    if selectors.iter().filter(|&&s| s).count() > 1 {
        return Err("use only one of range, start/end, section".into());
    }
    if let Some(range) = spec.range {
        return slice_range(&lines, range);
    }
    if let Some(title) = spec.section {
        return slice_section(text, &lines, title);
    }
    if spec.start.is_none() && spec.end.is_none() {
        return Ok(text.to_owned());
    }
    slice_markers(&lines, spec)
}

fn slice_range(lines: &[&str], range: &str) -> Result<String, String> {
    let bad = || format!("range \"{range}\" must be \"x..y\" with 1 <= x <= y <= {}", lines.len());
    let (a, b) = range.split_once("..").ok_or_else(bad)?;
    let (a, b): (usize, usize) = (a.trim().parse().map_err(|_| bad())?, b.trim().parse().map_err(|_| bad())?);
    if a < 1 || a > b || b > lines.len() {
        return Err(bad());
    }
    Ok(lines[a - 1..b].concat())
}

fn slice_section(text: &str, lines: &[&str], title: &str) -> Result<String, String> {
    let wanted = strip_numbering(title).to_lowercase();
    let headings = markdown::headings(text);
    let (i, h) = headings
        .iter()
        .enumerate()
        .find(|(_, h)| strip_numbering(&h.text).to_lowercase() == wanted)
        .ok_or_else(|| format!("section \"{title}\" not found"))?;
    let end_line = headings[i + 1..]
        .iter()
        .find(|next| next.level <= h.level)
        .map_or(lines.len(), |next| next.line - 1);
    Ok(lines[h.line - 1..end_line].concat())
}

/// `2.3. Title` → `Title`.
fn strip_numbering(title: &str) -> &str {
    let t = title.trim();
    let digits_end = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    if digits_end > 0 && t[..digits_end].chars().next().is_some_and(|c| c.is_ascii_digit()) {
        t[digits_end..].trim_start()
    } else {
        t
    }
}

fn slice_markers(lines: &[&str], spec: &Slice) -> Result<String, String> {
    let start = spec.start.map(compile).transpose()?;
    let end = spec.end.map(compile).transpose()?;
    let content = |l: &str| l.trim_end_matches(['\n', '\r']).to_owned();
    let mut out = String::new();
    let mut i = 0;
    let mut found_any = false;
    loop {
        // Open a segment.
        let seg_start = match &start {
            Some(re) => match lines[i..].iter().position(|l| re.is_match(&content(l))) {
                Some(p) => {
                    let at = i + p;
                    if spec.include_start { at } else { at + 1 }
                }
                None if found_any => break,
                None => return Err(format!("start pattern {:?} matches no line", spec.start.unwrap_or(""))),
            },
            None => 0,
        };
        // Only `end` is tested while a segment is open.
        let search_from = match &start {
            Some(_) if !spec.include_start => seg_start,
            Some(_) => seg_start + 1,
            None => 0,
        };
        let (seg_end, next) = match &end {
            Some(re) => match lines.get(search_from..).unwrap_or(&[]).iter().position(|l| re.is_match(&content(l))) {
                Some(p) => {
                    let at = search_from + p;
                    (if spec.include_end { at + 1 } else { at }, at + 1)
                }
                None if start.is_some() => {
                    return Err(format!("end pattern {:?} matches no line after the matched start", spec.end.unwrap_or("")));
                }
                None => return Err(format!("end pattern {:?} matches no line", spec.end.unwrap_or(""))),
            },
            None => (lines.len(), lines.len()),
        };
        out.push_str(&lines[seg_start.min(seg_end)..seg_end].concat());
        found_any = true;
        if !spec.repeat || start.is_none() || end.is_none() || next >= lines.len() {
            break;
        }
        i = next;
    }
    Ok(out)
}

/// Re-indents `text` so its least indented non-blank line has exactly `margin` spaces, keeping relative
/// indentation. Blank lines become empty.
pub fn reindent(text: &str, margin: usize) -> String {
    let indent = |l: &str| l.len() - l.trim_start_matches([' ', '\t']).len();
    let min = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(indent)
        .min()
        .unwrap_or(0);
    let pad = " ".repeat(margin);
    text.split_inclusive('\n')
        .map(|l| {
            let body = l.trim_end_matches(['\n', '\r']);
            let eol = &l[body.len()..];
            if body.trim().is_empty() { eol.to_owned() } else { format!("{pad}{}{eol}", &body[min..]) }
        })
        .collect()
}

/// Compiles a pattern with optional flags: `i` case-insensitive, `m` multi-line, `s` dot matches
/// newline, `x` verbose.
pub fn compile_with_flags(pattern: &str, flags: &str) -> Result<Regex, String> {
    if let Some(bad) = flags.chars().find(|c| !"imsx".contains(*c)) {
        return Err(format!("unknown regex flag {bad:?} (use i, m, s, x)"));
    }
    let full = if flags.is_empty() { pattern.to_owned() } else { format!("(?{flags}){pattern}") };
    Regex::new(&full).map_err(|e| format!("invalid regex {pattern:?}: {e}"))
}

fn compile(pattern: &str) -> Result<Regex, String> {
    compile_with_flags(pattern, "")
}

/// Character offset of a byte offset (Starlark strings index by character).
pub fn char_offset(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "a\n// MARK\nb\nc\n// MARK\nd\n// MARK\ne\n// MARK\n";

    fn s(text: &str, spec: Slice) -> Result<String, String> {
        slice(text, &spec)
    }

    #[test]
    fn whole_text_and_range() {
        assert_eq!(s("x\ny\n", Slice::default()).unwrap(), "x\ny\n");
        assert_eq!(s("1\n2\n3\n", Slice { range: Some("2..3"), ..Default::default() }).unwrap(), "2\n3\n");
        assert!(s("1\n2\n", Slice { range: Some("2..3"), ..Default::default() }).is_err());
        assert!(s("1\n2\n", Slice { range: Some("x"), ..Default::default() }).is_err());
    }

    #[test]
    fn same_marker_opens_and_closes() {
        let spec = Slice { start: Some("// MARK"), end: Some("// MARK"), ..Default::default() };
        assert_eq!(s(SRC, spec.clone()).unwrap(), "b\nc\n");
        assert_eq!(s(SRC, Slice { repeat: true, ..spec.clone() }).unwrap(), "b\nc\ne\n");
        assert_eq!(
            s(SRC, Slice { include_start: true, include_end: true, ..spec }).unwrap(),
            "// MARK\nb\nc\n// MARK\n"
        );
    }

    #[test]
    fn start_or_end_alone() {
        assert_eq!(s("a\nb\nc\n", Slice { start: Some("^b"), ..Default::default() }).unwrap(), "c\n");
        assert_eq!(s("a\nb\nc\n", Slice { end: Some("^b"), ..Default::default() }).unwrap(), "a\n");
        assert_eq!(s("a\nb\nc\n", Slice { end: Some("^b"), include_end: true, ..Default::default() }).unwrap(), "a\nb\n");
    }

    #[test]
    fn markers_that_never_match_are_errors() {
        assert!(s("a\n", Slice { start: Some("zz"), ..Default::default() }).unwrap_err().contains("start pattern"));
        assert!(s("a\n", Slice { end: Some("zz"), ..Default::default() }).unwrap_err().contains("end pattern"));
        let odd = "// MARK\nb\n";
        assert!(s(odd, Slice { start: Some("// MARK"), end: Some("// MARK"), ..Default::default() }).is_err());
    }

    #[test]
    fn sections() {
        let md = "# Doc\n## 1. Intro\ntext\n### Detail\nmore\n## Usage\nuse\n";
        assert_eq!(s(md, Slice { section: Some("intro"), ..Default::default() }).unwrap(), "## 1. Intro\ntext\n### Detail\nmore\n");
        assert_eq!(s(md, Slice { section: Some("Usage"), ..Default::default() }).unwrap(), "## Usage\nuse\n");
        assert!(s(md, Slice { section: Some("Nope"), ..Default::default() }).is_err());
        assert!(s(md, Slice { section: Some("Usage"), range: Some("1..2"), ..Default::default() }).is_err());
    }

    #[test]
    fn reindenting() {
        assert_eq!(reindent("    def f():\n        pass\n\n", 2), "  def f():\n      pass\n\n");
        assert_eq!(reindent("x\n  y", 4), "    x\n      y");
    }

    #[test]
    fn regex_flags() {
        assert!(compile_with_flags("abc", "i").unwrap().is_match("ABC"));
        assert!(compile_with_flags("a", "q").is_err());
        assert_eq!(char_offset("éa", 2), 1);
    }
}
