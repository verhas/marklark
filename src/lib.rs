//! marklark — maintains Markdown documents from `<!--INTENT-->`, `<!--CODE-->` and `<!--AI-->` blocks.
//!
//! The specification lives in `docs/spec.md`; decisions in `docs/decisions.md`.

pub mod ai;
pub mod apply;
pub mod blocks;
pub mod canon;
pub mod data;
pub mod document;
pub mod eval;
pub mod extract;
pub mod fs;
pub mod guide;
pub mod host;
pub mod inline;
pub mod lang;
pub mod markdown;
pub mod splice;
pub mod stamps;
pub mod state;
pub mod tasks;
