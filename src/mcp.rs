//! `marklark mcp`: the MCP server (spec §11), a thin layer over the library.
//!
//! Every tool takes a document path (absolute, or relative to the server's working directory). Tools
//! that change a document write it back the same way the CLI does, with the same backup rule.

use std::path::{Path, PathBuf};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use marklark::ai;
use marklark::apply;
use marklark::blocks::Kind;
use marklark::document::Document;
use marklark::eval;
use marklark::extract::{self, Slice};
use marklark::fs::DiskFs;
use marklark::markdown;
use marklark::stamps;
use marklark::state::{self, Finding};

use crate::{Backup, load, write_document};

const INSTRUCTIONS: &str = "marklark maintains Markdown documents. Humans write <!--INTENT--> blocks in \
prose; you satisfy them with <!--CODE--> blocks (hermetic Starlark run by marklark) or fill human-created \
<!--AI--> blocks with prose. Use intent_list / intent_check to find work, intent_context to see an \
intent and its current code, intent_apply to replace an intent's code (it is stamped and run before \
anything is written). For AI blocks: ai_resolve stores the dependencies you derive from the INTENT \
(read(...) / slice(...) calls), ai_context gives you their content, ai_update stores your prose. Never \
edit stamps (#@ lines) or generated output by hand.";

#[derive(Clone)]
pub struct MarklarkServer {
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
pub struct FileArg {
    /// Path of the Markdown document.
    pub file: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct LineArg {
    /// Path of the Markdown document.
    pub file: String,
    /// A line of the INTENT, or of one of the blocks it governs.
    pub line: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct ApplyArg {
    /// Path of the Markdown document.
    pub file: String,
    /// A line of the INTENT, or of one of the blocks it governs.
    pub line: usize,
    /// The bodies of the new CODE blocks (Starlark), in order. They replace every current block of the
    /// intent. A block that calls emit() gets an output region.
    pub blocks: Vec<String>,
    /// Optional agent-owned stamps; keys must start with `x-`.
    #[serde(default)]
    pub stamps: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ResolveArg {
    /// Path of the Markdown document.
    pub file: String,
    /// A line of the AI block or of its INTENT.
    pub line: usize,
    /// Dependencies derived from the INTENT: each a single read("path") or slice("path", ...) call with
    /// literal arguments; paths are relative to the document.
    pub deps: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AiUpdateArg {
    /// Path of the Markdown document.
    pub file: String,
    /// A line of the AI block or of its INTENT.
    pub line: usize,
    /// The new prose for the AI block.
    pub text: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SectionArg {
    /// Path of the Markdown document.
    pub file: String,
    /// Heading title (case-insensitive; numbering like "2.3." ignored).
    pub title: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct LinesArg {
    /// Path of the Markdown document.
    pub file: String,
    /// First line, 1-based.
    pub start: usize,
    /// Last line, inclusive.
    pub end: usize,
}

fn ok_json(value: Value) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

fn ok_text(text: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

fn fail(message: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::text(message)]))
}

fn findings_json(findings: &[Finding], command: state::Command) -> Value {
    Value::Array(
        findings
            .iter()
            .map(|f| {
                let severity = match state::severity(f.state, command, false) {
                    state::Severity::Ok => "ok",
                    state::Severity::Warning => "warning",
                    state::Severity::Error => "error",
                };
                json!({ "line": f.line, "state": f.state.name(), "severity": severity, "detail": f.detail })
            })
            .collect(),
    )
}

/// Static and AI findings, plus `broken` from an in-memory run.
fn all_findings(doc: &Document, fs: &DiskFs) -> Result<Vec<Finding>, String> {
    let mut findings = state::analyze(doc).map_err(|e| join(&e))?;
    findings.extend(ai::analyze(doc, fs).map_err(|e| join(&e))?);
    let run = eval::update(doc, fs).map(|_| ());
    let mut findings = state::with_run_result(findings, &run);
    findings.sort_by_key(|f| f.line);
    Ok(findings)
}

fn join<T: ToString>(items: &[T]) -> String {
    items.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
}

fn save(file: &Path, old: &str, new: &str) -> Result<(), String> {
    if old == new {
        return Ok(());
    }
    match write_document(file, old, new, Backup::Auto) {
        0 => Ok(()),
        _ => Err(format!("{}: could not write the document", file.display())),
    }
}

#[tool_router(router = tool_router)]
impl MarklarkServer {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    /// List every INTENT of a document: its text, what satisfies it (code, ai or none), the lines of
    /// its blocks, and its states. Runs nothing.
    #[tool]
    async fn intent_list(&self, Parameters(a): Parameters<FileArg>) -> Result<CallToolResult, ErrorData> {
        let file = PathBuf::from(&a.file);
        let (fs, doc) = match load(&file) {
            Ok(x) => x,
            Err(e) => return fail(e),
        };
        let parsed = match doc.parse() {
            Ok(p) => p,
            Err(e) => return fail(join(&e)),
        };
        let mut findings = state::analyze(&doc).unwrap_or_default();
        findings.extend(ai::analyze(&doc, &fs).unwrap_or_default());
        let intents: Vec<Value> = parsed
            .groups
            .iter()
            .map(|g| {
                let block = &parsed.blocks[g.intent];
                let kind = match g.members.first().map(|&m| parsed.blocks[m].kind) {
                    Some(Kind::Ai) => "ai",
                    Some(_) => "code",
                    None => "none",
                };
                let lines: Vec<usize> = g.members.iter().map(|&m| parsed.blocks[m].line).collect();
                let mine: Vec<Finding> =
                    findings.iter().filter(|f| f.line == block.line || lines.contains(&f.line)).cloned().collect();
                json!({
                    "line": block.line,
                    "intent": block.body_text(&doc.text).trim(),
                    "satisfied_by": kind,
                    "blocks": lines,
                    "states": findings_json(&mine, state::Command::IntentCheck),
                })
            })
            .collect();
        let orphans: Vec<Finding> = findings.iter().filter(|f| f.state == state::State::Orphan).cloned().collect();
        ok_json(json!({ "intents": intents, "orphans": findings_json(&orphans, state::Command::IntentCheck) }))
    }

    /// Everything about one INTENT: its text, each governed block (code without stamps, stamps,
    /// current output) and its states, including failures from a trial run.
    #[tool]
    async fn intent_context(&self, Parameters(a): Parameters<LineArg>) -> Result<CallToolResult, ErrorData> {
        let file = PathBuf::from(&a.file);
        let (fs, doc) = match load(&file) {
            Ok(x) => x,
            Err(e) => return fail(e),
        };
        let parsed = match doc.parse() {
            Ok(p) => p,
            Err(e) => return fail(join(&e)),
        };
        let Some(at) = (1..=a.line).rev().find_map(|l| {
            parsed.blocks.iter().position(|b| b.line == l).filter(|&i| parsed.group_of(i).is_some())
        }) else {
            return fail(format!("line {}: no INTENT here", a.line));
        };
        let group = parsed.group_of(at).expect("checked above");
        let intent_block = &parsed.blocks[group.intent];
        let findings = all_findings(&doc, &fs).unwrap_or_default();
        let member_lines: Vec<usize> = group.members.iter().map(|&m| parsed.blocks[m].line).collect();
        let blocks: Vec<Value> = group
            .members
            .iter()
            .map(|&m| {
                let b = &parsed.blocks[m];
                let st: serde_json::Map<String, Value> = parsed.stamps[m]
                    .stamps
                    .iter()
                    .map(|s| (s.key.clone(), Value::String(s.value.clone())))
                    .collect();
                json!({
                    "line": b.line,
                    "kind": b.kind.name(),
                    "code": stamps::rewrite(b.body_text(&doc.text), &[]).trim_matches('\n'),
                    "stamps": st,
                    "output": b.output_text(&doc.text),
                })
            })
            .collect();
        let mine: Vec<Finding> = findings
            .into_iter()
            .filter(|f| f.line == intent_block.line || member_lines.contains(&f.line))
            .collect();
        ok_json(json!({
            "line": intent_block.line,
            "intent": intent_block.body_text(&doc.text).trim(),
            "intent_hash": stamps::intent_digest(intent_block.body_text(&doc.text)),
            "blocks": blocks,
            "states": findings_json(&mine, state::Command::IntentCheck),
        }))
    }

    /// Replace all CODE blocks of an INTENT with new ones, atomically. The new blocks are checked,
    /// stamped as verified against the current INTENT, and the whole document is updated in memory;
    /// nothing is written unless that succeeds. Refuses if generated output was edited by hand.
    #[tool]
    async fn intent_apply(&self, Parameters(a): Parameters<ApplyArg>) -> Result<CallToolResult, ErrorData> {
        let file = PathBuf::from(&a.file);
        let (fs, doc) = match load(&file) {
            Ok(x) => x,
            Err(e) => return fail(e),
        };
        let extra: Vec<(String, String)> = a.stamps.into_iter().collect();
        match apply::intent_apply(&doc, &fs, a.line, &a.blocks, &extra) {
            Ok(text) => match save(&file, &doc.text, &text) {
                Ok(()) => ok_text(format!("{}: applied {} block(s) and updated the document", a.file, a.blocks.len())),
                Err(e) => fail(e),
            },
            Err(e) => fail(e.to_string()),
        }
    }

    /// Report the intent states of a document (as `marklark intent-check`).
    #[tool]
    async fn intent_check(&self, Parameters(a): Parameters<FileArg>) -> Result<CallToolResult, ErrorData> {
        let (fs, doc) = match load(Path::new(&a.file)) {
            Ok(x) => x,
            Err(e) => return fail(e),
        };
        match all_findings(&doc, &fs) {
            Ok(findings) => ok_json(findings_json(&findings, state::Command::IntentCheck)),
            Err(e) => fail(e),
        }
    }

    /// Run all CODE blocks and write the updated document (as `marklark update`).
    #[tool]
    async fn update(&self, Parameters(a): Parameters<FileArg>) -> Result<CallToolResult, ErrorData> {
        let file = PathBuf::from(&a.file);
        let (fs, doc) = match load(&file) {
            Ok(x) => x,
            Err(e) => return fail(e),
        };
        match eval::update(&doc, &fs) {
            Ok(r) if r.text == doc.text => ok_text(format!("{}: up to date", a.file)),
            Ok(r) => match save(&file, &doc.text, &r.text) {
                Ok(()) => ok_text(format!("{}: updated", a.file)),
                Err(e) => fail(e),
            },
            Err(e) => fail(e.to_string()),
        }
    }

    /// For an AI block: the prompt (its INTENT), the content of every dependency (marked `changed` if it
    /// differs from what the text was written from), the current text, and its states.
    #[tool]
    async fn ai_context(&self, Parameters(a): Parameters<LineArg>) -> Result<CallToolResult, ErrorData> {
        let (fs, doc) = match load(Path::new(&a.file)) {
            Ok(x) => x,
            Err(e) => return fail(e),
        };
        match ai::context(&doc, &fs, a.line) {
            Ok(c) => ok_json(json!({
                "line": c.line,
                "intent": c.intent,
                "deps": c.deps.iter().map(|d| json!({ "spec": d.spec, "content": d.content, "changed": d.changed })).collect::<Vec<_>>(),
                "text": c.content,
                "states": findings_json(&c.findings, state::Command::IntentCheck),
            })),
            Err(e) => fail(e.to_string()),
        }
    }

    /// Store the dependencies of an AI block, derived from its INTENT. Each is checked and test-read.
    #[tool]
    async fn ai_resolve(&self, Parameters(a): Parameters<ResolveArg>) -> Result<CallToolResult, ErrorData> {
        let file = PathBuf::from(&a.file);
        let (fs, doc) = match load(&file) {
            Ok(x) => x,
            Err(e) => return fail(e),
        };
        match ai::resolve(&doc, &fs, a.line, &a.deps) {
            Ok(text) => match save(&file, &doc.text, &text) {
                Ok(()) => ok_text(format!("{}: stored {} dependencies", a.file, a.deps.len())),
                Err(e) => fail(e),
            },
            Err(e) => fail(e.to_string()),
        }
    }

    /// Store new prose for an AI block, with the hashes of the inputs it was written from. Refuses to
    /// overwrite text a human edited.
    #[tool]
    async fn ai_update(&self, Parameters(a): Parameters<AiUpdateArg>) -> Result<CallToolResult, ErrorData> {
        let file = PathBuf::from(&a.file);
        let (fs, doc) = match load(&file) {
            Ok(x) => x,
            Err(e) => return fail(e),
        };
        match ai::write(&doc, &fs, a.line, &a.text) {
            Ok(text) => match save(&file, &doc.text, &text) {
                Ok(()) => ok_text(format!("{}: AI block updated", a.file)),
                Err(e) => fail(e),
            },
            Err(e) => fail(e.to_string()),
        }
    }

    /// The headings of a document: level, text, line, anchor.
    #[tool]
    async fn list_headings(&self, Parameters(a): Parameters<FileArg>) -> Result<CallToolResult, ErrorData> {
        match std::fs::read_to_string(&a.file) {
            Ok(text) => ok_json(Value::Array(
                markdown::headings(&text)
                    .into_iter()
                    .map(|h| json!({ "level": h.level, "text": h.text, "line": h.line, "anchor": h.anchor }))
                    .collect(),
            )),
            Err(e) => fail(format!("{}: {e}", a.file)),
        }
    }

    /// One section of a document: the heading and everything up to the next heading of the same or a
    /// higher level.
    #[tool]
    async fn get_section(&self, Parameters(a): Parameters<SectionArg>) -> Result<CallToolResult, ErrorData> {
        match std::fs::read_to_string(&a.file) {
            Ok(text) => match extract::slice(&text, &Slice { section: Some(&a.title), ..Default::default() }) {
                Ok(s) => ok_text(s),
                Err(e) => fail(e),
            },
            Err(e) => fail(format!("{}: {e}", a.file)),
        }
    }

    /// Lines `start` to `end` (1-based, inclusive) of a document.
    #[tool]
    async fn get_lines(&self, Parameters(a): Parameters<LinesArg>) -> Result<CallToolResult, ErrorData> {
        let range = format!("{}..{}", a.start, a.end);
        match std::fs::read_to_string(&a.file) {
            Ok(text) => match extract::slice(&text, &Slice { range: Some(&range), ..Default::default() }) {
                Ok(s) => ok_text(s),
                Err(e) => fail(e),
            },
            Err(e) => fail(format!("{}: {e}", a.file)),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MarklarkServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("marklark", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Serves MCP on stdin/stdout until the client disconnects.
pub fn serve() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let service = MarklarkServer::new().serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        anyhow::Ok(())
    })
}
