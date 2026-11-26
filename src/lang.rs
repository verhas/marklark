//! The Starlark language settings for CODE blocks.
//!
//! The dialect is pinned here, in one place (D23). CODE needs top-level `for`, which the standard
//! dialect rejects, so the extended dialect is the starting point; it is narrowed during development.

use starlark::syntax::{AstModule, Dialect};

/// The dialect CODE blocks are parsed and run with: the extended dialect without `load` (CODE gets
/// everything through host functions, never from other files).
pub fn code_dialect() -> Dialect {
    Dialect { enable_load: false, ..Dialect::Extended }
}

/// A CODE body that does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parses a CODE body. `name` appears in error messages.
pub fn parse(name: &str, body: &str) -> Result<AstModule, ParseError> {
    AstModule::parse(name, body.to_owned(), &code_dialect()).map_err(|e| ParseError { message: e.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_while_and_no_load() {
        assert!(parse("t", "while True:\n    pass\n").is_err());
        assert!(parse("t", "load('x.star', 'y')\n").is_err());
        assert!(parse("t", "for x in [1]:\n    pass\n").is_ok());
    }
}
