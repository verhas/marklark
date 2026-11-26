//! marklark — maintains Markdown documents from `<!--INTENT-->`, `<!--CODE-->` and `<!--AI-->` blocks.
//!
//! The specification lives in `docs/spec.md`; decisions in `docs/decisions.md`.

pub mod blocks;
pub mod canon;
pub mod eval;
pub mod host;
pub mod splice;
pub mod stamps;
pub mod tasks;
