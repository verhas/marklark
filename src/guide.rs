//! The guide for agents (`docs/agent-guide.md`), compiled into the binary so the MCP server can serve
//! it and `marklark skill` can install it as a Claude Code skill. One source, three uses.

/// The guide, as Markdown.
pub const GUIDE: &str = include_str!("../docs/agent-guide.md");

/// The URI the MCP server serves the guide under.
pub const GUIDE_URI: &str = "marklark://guide";

/// The guide as a Claude Code skill (`SKILL.md`): YAML front matter, then the guide.
pub fn skill_md() -> String {
    format!(
        "---\nname: marklark\ndescription: \"Use when maintaining Markdown documents with marklark: resolving \
         <!--INTENT--> blocks, writing <!--CODE--> blocks (Starlark) or <!--AI--> blocks, using the marklark \
         MCP tools, or running marklark update / check / intent-check. Covers the rules, the workflow, the \
         Starlark subset, every host function, and patterns.\"\n---\n\n{GUIDE}"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn skill_has_front_matter_and_guide() {
        let s = super::skill_md();
        assert!(s.starts_with("---\nname: marklark\ndescription: \"Use when"));
        assert!(s.contains("\n---\n\n# marklark — guide for agents"));
        // The front matter must be valid YAML (the description contains `: `, so it is quoted).
        let fm = crate::data::front_matter_text(&s).unwrap();
        let parsed = crate::data::parse_yaml(fm).unwrap();
        assert_eq!(parsed["name"], "marklark");
        assert!(parsed["description"].as_str().unwrap().starts_with("Use when maintaining Markdown documents with marklark: "));
    }
}
