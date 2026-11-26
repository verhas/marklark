//! `marklark mcp`: the MCP server (spec §11), a thin layer over the library.
//!
//! Every tool takes a document path (absolute, or relative to the server's working directory). Tools
//! that change a document write it back the same way the CLI does, with the same backup rule.

use std::path::{Path, PathBuf};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::RoleServer;
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
use marklark::guide::{GUIDE, GUIDE_URI};
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
edit stamps (#@ lines) or generated output by hand. Write CODE by default; use an AI block only when the \
human asks for prose (in the INTENT or the request): ai_resolve on the INTENT creates it.

Before writing CODE, read the full guide: the `guide` tool or the resource marklark://guide (rules, \
workflow, the Starlark subset, every host function, patterns).

Every tool that writes takes `line` and `intent` (the intent_hash from intent_list / intent_context / \
ai_context). Line numbers shift after every write; if the line no longer belongs to that intent, the \
tool refuses and changes nothing — list the intents again.

Host functions for CODE (paths are relative to the document; file functions take a PATH and read the \
file themselves — pass \"data/app.toml\", not read(...)):
  emit(text)                         append to the block's output (needs <!--/CODE-->, which intent_apply adds)
  read(path) -> str                  whole text file
  slice(path, start=, end=, range=\"x..y\", section=\"Title\", include_start=, include_end=, repeat=) -> str
                                     a piece of a file: regex start/end lines, a line range, or a Markdown section;
                                     a selector that matches nothing is an error
  glob(pattern) -> [path]            matching files, sorted
  json(path) / yaml(path) / toml(path) / xml(path, xmlns={name: url}) -> data
  re_search(p, text, flags=) -> struct(text, groups, named, start, end) or None
  re_findall(p, text, flags=) -> list    re_sub(p, repl, text, count=, flags=) -> str   (repl uses $1)
  reindent(text, margin) -> str      fence(text, lang=) -> str (a code fence that fits the text)
  headings(min_level=, max_level=) -> [struct(level, text, line, anchor, path)]   anchor(text) -> str
  front_matter() -> data or None     here() -> struct(line, next_line)   document() -> str
  define(name, value, lenient=False) publish plain data for other blocks (dotted names nest)
  var(name, default=)                read it; without default, waits until some block defines it
  tasks(depends_on=[...], completes=[...])   first statement only: order blocks by named tasks
  load(\"//lib/x.star\", \"f\")      shared functions from a .star library (// = project root)
CODE must not contain the sequence --> anywhere (write \"--\" + \">\"). No width formatting (%3d, {:>3}): pad by hand.";

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
    /// The intent_hash of the INTENT you mean (from intent_list or intent_context). The tool refuses if
    /// the line belongs to another INTENT.
    pub intent: String,
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
    /// The intent_hash of the AI block's INTENT (from intent_list or ai_context).
    pub intent: String,
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
    /// The intent_hash of the AI block's INTENT (from intent_list or ai_context).
    pub intent: String,
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
    findings.extend(state::analyze_libs(doc, fs).map_err(|e| join(&e))?);
    let run = eval::update(doc, fs).map(|_| ());
    let mut findings = state::with_run_result(findings, &run);
    findings.sort_by_key(|f| f.line);
    Ok(findings)
}

/// `the INTENT at line 6 ("State the application's name …")`.
fn describe_target(target: Option<(usize, String)>) -> String {
    match target {
        Some((line, text)) => {
            let first = text.lines().next().unwrap_or("");
            let short: String = first.chars().take(80).collect();
            let more = if short.len() < text.len() { " …" } else { "" };
            format!("the INTENT at line {line} (\"{short}{more}\")")
        }
        None => "the INTENT".to_owned(),
    }
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
        findings.extend(state::analyze_libs(&doc, &fs).unwrap_or_default());
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
                    "intent_hash": stamps::intent_digest(block.body_text(&doc.text)),
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
        let target = state::intent_at(&doc, a.line);
        match apply::intent_apply(&doc, &fs, a.line, Some(&a.intent), &a.blocks, &extra) {
            Ok(text) => match save(&file, &doc.text, &text) {
                Ok(()) => ok_text(format!(
                    "{}: applied {} block(s) to {} and updated the document. Line numbers below it have shifted.",
                    a.file,
                    a.blocks.len(),
                    describe_target(target)
                )),
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
                "intent_hash": stamps::intent_digest(&c.intent),
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
        let target = state::intent_at(&doc, a.line);
        match ai::resolve(&doc, &fs, a.line, Some(&a.intent), &a.deps) {
            Ok(text) => match save(&file, &doc.text, &text) {
                Ok(()) => ok_text(format!(
                    "{}: stored {} dependencies for {}. Line numbers below it have shifted.",
                    a.file,
                    a.deps.len(),
                    describe_target(target)
                )),
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
        let target = state::intent_at(&doc, a.line);
        match ai::write(&doc, &fs, a.line, Some(&a.intent), &a.text) {
            Ok(text) => match save(&file, &doc.text, &text) {
                Ok(()) => ok_text(format!(
                    "{}: updated the AI block of {}. Line numbers below it have shifted.",
                    a.file,
                    describe_target(target)
                )),
                Err(e) => fail(e),
            },
            Err(e) => fail(e.to_string()),
        }
    }

    /// The full guide for agents: rules, workflow, the Starlark subset, every host function with its
    /// parameters, patterns and don'ts. Read it before writing CODE.
    #[tool]
    async fn guide(&self) -> Result<CallToolResult, ErrorData> {
        ok_text(GUIDE)
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
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(GUIDE_URI, "marklark-guide")
                .with_title("marklark — guide for agents")
                .with_description("Rules, workflow, the Starlark subset, every host function, patterns")
                .with_mime_type("text/markdown"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri == GUIDE_URI {
            let contents = ResourceContents::TextResourceContents {
                uri: request.uri,
                mime_type: Some("text/markdown".into()),
                text: GUIDE.to_owned(),
                meta: None,
            };
            Ok(ReadResourceResult::new(vec![contents]).into())
        } else {
            Err(ErrorData::resource_not_found(format!("no resource {}", request.uri), None))
        }
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
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
