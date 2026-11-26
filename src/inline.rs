//! Inline values in running text (spec §7.7, D25), filled in after all CODE has run.
//!
//! - `<!--$path-->value` — the short form humans write. The value is a single word: it ends at the
//!   first whitespace (or the end of the line).
//! - `<!--= path-->value<!--/=-->` — the long form; the value may contain anything but the marker.
//!
//! `path` is a variable name followed by `.key` / `[index]` steps: `app.version`, `ports[0]`. A value
//! must be a string, number or bool. References inside code blocks, code spans, marklark block comments
//! and output regions are left alone (output regions belong to their CODE block and are guarded).

use std::fmt;
use std::ops::Range;

use crate::blocks::{self, Block};
use crate::host::Vars;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for InlineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for InlineError {}

/// Fills every inline value in `text` from `vars`. Returns the new text, or every error found.
pub fn apply(text: &str, blocks: &[Block], vars: &Vars) -> Result<String, Vec<InlineError>> {
    let skip = skipped_ranges(text, blocks);
    let line_of = |pos: usize| text[..pos].matches('\n').count() + 1;
    let mut out = String::with_capacity(text.len());
    let mut errors = Vec::new();
    let mut copied = 0;
    let mut pos = 0;
    while let Some(found) = text[pos..].find("<!--") {
        let start = pos + found;
        if let Some(r) = skip.iter().find(|r| r.contains(&start)) {
            pos = r.end;
            continue;
        }
        let after = &text[start + 4..];
        let (path, value_range) = if let Some(rest) = after.strip_prefix('$') {
            let Some(close) = rest.find("-->") else { break };
            let path = &rest[..close];
            let value_start = start + 5 + close + 3;
            let value_len = text[value_start..].find(char::is_whitespace).unwrap_or(text.len() - value_start);
            (path, value_start..value_start + value_len)
        } else if let Some(rest) = after.strip_prefix('=') {
            let Some(close) = rest.find("-->") else { break };
            let path = rest[..close].trim();
            let value_start = start + 5 + close + 3;
            match text[value_start..].find("<!--/=-->") {
                Some(end) => (path, value_start..value_start + end),
                None => {
                    errors.push(InlineError { line: line_of(start), message: "<!--= ...--> has no <!--/=--> marker".into() });
                    pos = value_start;
                    continue;
                }
            }
        } else {
            pos = start + 4;
            continue;
        };
        let short = after.starts_with('$');
        match render(path, vars) {
            Ok(value) if short && value.contains(char::is_whitespace) => errors.push(InlineError {
                line: line_of(start),
                message: format!(
                    "the value of `{path}` contains whitespace; use the long form <!--= {path}-->…<!--/=-->"
                ),
            }),
            Ok(value) => {
                out.push_str(&text[copied..value_range.start]);
                out.push_str(&value);
                copied = value_range.end;
            }
            Err(message) => errors.push(InlineError { line: line_of(start), message }),
        }
        pos = value_range.end.max(start + 4);
    }
    out.push_str(&text[copied..]);
    if errors.is_empty() { Ok(out) } else { Err(errors) }
}

/// Code blocks and spans, marklark block comments, and output regions.
fn skipped_ranges(text: &str, blocks: &[Block]) -> Vec<Range<usize>> {
    let mut ranges = blocks::code_ranges(text);
    for b in blocks {
        ranges.push(b.open.clone());
        if let Some(o) = &b.output {
            ranges.push(o.clone());
        }
    }
    ranges.sort_by_key(|r| r.start);
    ranges
}

/// Looks up `path` and renders a scalar.
fn render(path: &str, vars: &Vars) -> Result<String, String> {
    let steps = parse_path(path).ok_or_else(|| format!("`{path}` is not a variable path (e.g. app.version, ports[0])"))?;
    let (first, rest) = steps.split_first().ok_or_else(|| "empty variable path".to_owned())?;
    let Step::Key(name) = first else { return Err(format!("`{path}` must start with a variable name")) };
    let mut value = vars.get(*name).ok_or_else(|| format!("variable `{name}` is not defined"))?;
    for step in rest {
        value = match (step, value) {
            (Step::Key(k), serde_json::Value::Object(m)) => m.get(*k),
            (Step::Index(i), serde_json::Value::Array(a)) => a.get(*i),
            _ => None,
        }
        .ok_or_else(|| format!("`{path}`: no such element"))?;
    }
    match value {
        serde_json::Value::String(s) => Ok(s.clone()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::Bool(b) => Ok(b.to_string()),
        _ => Err(format!("`{path}` is not a string, number or bool")),
    }
}

enum Step<'a> {
    Key(&'a str),
    Index(usize),
}

fn parse_path(path: &str) -> Option<Vec<Step<'_>>> {
    let path = path.trim();
    let mut steps = Vec::new();
    let mut rest = path;
    let ident_end = |s: &str| s.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-')).unwrap_or(s.len());
    let n = ident_end(rest);
    if n == 0 {
        return None;
    }
    steps.push(Step::Key(&rest[..n]));
    rest = &rest[n..];
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let n = ident_end(r);
            if n == 0 {
                return None;
            }
            steps.push(Step::Key(&r[..n]));
            rest = &r[n..];
        } else {
            let r = rest.strip_prefix('[')?;
            let close = r.find(']')?;
            steps.push(Step::Index(r[..close].trim().parse().ok()?));
            rest = &r[close + 1..];
        }
    }
    Some(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        let mut v = Vars::new();
        v.insert("version".into(), "1.2.3".into());
        v.insert("app".into(), serde_json::json!({"name": "Demo App", "ports": [80, 443], "beta": true}));
        v
    }

    fn fill(text: &str) -> Result<String, Vec<InlineError>> {
        apply(text, &blocks::locate(text).unwrap(), &vars())
    }

    #[test]
    fn short_form_replaces_one_word() {
        assert_eq!(fill("Version <!--$version-->old, done.\n").unwrap(), "Version <!--$version-->1.2.3 done.\n");
        assert_eq!(fill("V <!--$version-->\n").unwrap(), "V <!--$version-->1.2.3\n");
        assert_eq!(fill("<!--$app.ports[1]-->0").unwrap(), "<!--$app.ports[1]-->443");
        assert_eq!(fill("<!--$app.beta-->x").unwrap(), "<!--$app.beta-->true");
    }

    #[test]
    fn long_form_and_whitespace_rule() {
        assert_eq!(fill("<!--= app.name-->old text<!--/=-->!").unwrap(), "<!--= app.name-->Demo App<!--/=-->!");
        let e = fill("<!--$app.name-->x").unwrap_err();
        assert!(e[0].message.contains("long form"), "{}", e[0]);
    }

    #[test]
    fn errors_name_the_problem() {
        let e = fill("a\n<!--$nope-->x\n<!--$app-->y\n<!--= version-->z\n").unwrap_err();
        let msgs: Vec<String> = e.iter().map(ToString::to_string).collect();
        assert_eq!(msgs.len(), 3, "{msgs:?}");
        assert!(msgs[0].starts_with("line 2: variable `nope`"));
        assert!(msgs[1].contains("not a string"));
        assert!(msgs[2].contains("no <!--/=-->"));
    }

    #[test]
    fn skips_code_blocks_and_output() {
        let text = "`<!--$version-->x`\n```\n<!--$version-->x\n```\n<!--CODE\nemit('')\n-->\n<!--$version-->y\n<!--/CODE-->\n";
        assert_eq!(fill(text).unwrap(), text);
        assert_eq!(fill("<!-- plain comment -->x").unwrap(), "<!-- plain comment -->x");
    }
}
