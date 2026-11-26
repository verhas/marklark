//! The evaluation loop: passes, aborts, block-local variable layers, dead ends (spec §7, D30).
//!
//! [`update`] takes a document held in memory and a [`FileSystem`], and returns the updated text. It
//! never touches storage itself, so the same code serves the CLI, the MCP server and library callers.
//!
//! Order of work:
//! 1. parse blocks and stamps; collect every CODE block's `tasks(...)` declaration and check the task
//!    graph — before anything runs (D15);
//! 2. check every output region against its `generated` stamp (spec §10);
//! 3. run passes: each pass runs, in document order, the blocks not run yet whose dependencies are
//!    complete. A block that aborts (an undefined `var`) leaves no trace and runs again later. A pass
//!    that runs nothing while blocks remain is a dead end;
//! 4. after each block that finishes, its output is spliced in, so later blocks see it (spec §7.6).

use std::collections::BTreeMap;
use std::fmt;

use starlark::environment::Module;
use starlark::eval::Evaluator;

use crate::blocks::Kind;
use crate::document::{DocError, Document, Parsed};
use crate::fs::{FileSystem, RecordedRead, Sandbox};
use crate::host::{self, Abort, BlockCtx, Vars};
use crate::lang;
use crate::splice::{self, IntegrityError};
use crate::tasks::{self, DeclError, GraphError, TaskDecl};

/// What an update produced.
#[derive(Debug, Clone)]
pub struct UpdateResult {
    /// The updated document text.
    pub text: String,
    /// One entry per CODE block, in document order.
    pub blocks: Vec<BlockRun>,
    /// The variables defined when the update finished.
    pub vars: Vars,
}

/// How one CODE block ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRun {
    /// 1-based line of the block's opening comment.
    pub line: usize,
    /// The pass (1-based) in which the block ran.
    pub pass: usize,
    /// Files the block read, with their hashes (D6).
    pub reads: Vec<RecordedRead>,
}

/// A block that could not run when the evaluation reached a dead end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stuck {
    pub line: usize,
    /// Tasks it depends on that never completed.
    pub unmet_tasks: Vec<String>,
    /// The variable whose absence aborted it, if it ran at all.
    pub missing_var: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    Document(Vec<DocError>),
    /// An invalid `tasks(...)` declaration (or a body that does not parse); `line` is the document line
    /// of the block.
    Declaration { line: usize, error: DeclError },
    TaskGraph(Vec<GraphError<usize>>),
    Integrity(IntegrityError),
    /// A Starlark error while running a block.
    Runtime { line: usize, message: String },
    /// A block called `emit` but has no `<!--/CODE-->` marker.
    EmitWithoutRegion { line: usize },
    /// A block's output would change the document's block structure.
    OutputBreaksBlocks { line: usize },
    DeadEnd(Vec<Stuck>),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::Document(errors) => join_lines(f, errors),
            UpdateError::Declaration { line, error } => write!(f, "CODE block at line {line}: {error}"),
            UpdateError::TaskGraph(errors) => join_lines(f, errors),
            UpdateError::Integrity(e) => e.fmt(f),
            UpdateError::Runtime { line, message } => write!(f, "CODE block at line {line} failed:\n{message}"),
            UpdateError::EmitWithoutRegion { line } => write!(
                f,
                "CODE block at line {line} calls emit() but has no <!--/CODE--> marker for its output"
            ),
            UpdateError::OutputBreaksBlocks { line } => write!(
                f,
                "CODE block at line {line}: its output contains a marklark block marker at the start of a line"
            ),
            UpdateError::DeadEnd(stuck) => {
                writeln!(f, "dead end: these CODE blocks can never run")?;
                for s in stuck {
                    write!(f, "  line {}:", s.line)?;
                    if !s.unmet_tasks.is_empty() {
                        write!(f, " waits for task(s) {}", s.unmet_tasks.join(", "))?;
                    }
                    if let Some(v) = &s.missing_var {
                        write!(f, " reads variable `{v}`, which no block defines")?;
                    }
                    writeln!(f)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for UpdateError {}

fn join_lines<T: fmt::Display>(f: &mut fmt::Formatter<'_>, items: &[T]) -> fmt::Result {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            writeln!(f)?;
        }
        item.fmt(f)?;
    }
    Ok(())
}

/// Everything `update` verifies before running anything: block structure, stamps, declarations, the
/// task graph, and the output guards. Runs no code and reads no files.
pub fn check(doc: &Document) -> Result<(), UpdateError> {
    prepare(doc).map(|_| ())
}

/// What [`prepare`] establishes before anything runs.
struct Prepared {
    parsed: Parsed,
    /// Indexes of the CODE blocks.
    code: Vec<usize>,
    /// Each CODE block's declaration (empty when it has none).
    decls: BTreeMap<usize, TaskDecl>,
}

fn prepare(doc: &Document) -> Result<Prepared, UpdateError> {
    let parsed = doc.parse().map_err(UpdateError::Document)?;
    let code: Vec<usize> = (0..parsed.blocks.len()).filter(|&i| parsed.blocks[i].kind == Kind::Code).collect();

    // 1. Declarations of every block, before anything runs.
    let mut decls: BTreeMap<usize, TaskDecl> = BTreeMap::new();
    for &i in &code {
        let block = &parsed.blocks[i];
        let decl = tasks::collect(block.body_text(&doc.text))
            .map_err(|error| UpdateError::Declaration { line: block.line, error })?;
        decls.insert(i, decl.unwrap_or_default());
    }
    let graph: Vec<(usize, TaskDecl)> = decls.iter().map(|(&i, d)| (parsed.blocks[i].line, d.clone())).collect();
    let graph_errors = tasks::check_graph(&graph);
    if !graph_errors.is_empty() {
        return Err(UpdateError::TaskGraph(graph_errors));
    }

    // 2. Guards.
    for &i in &code {
        splice::check_guard(&doc.text, &parsed.blocks[i], &parsed.stamps[i]).map_err(UpdateError::Integrity)?;
    }
    Ok(Prepared { parsed, code, decls })
}

/// Runs every CODE block of `doc` and returns the updated text.
pub fn update(doc: &Document, fs: &dyn FileSystem) -> Result<UpdateResult, UpdateError> {
    let Prepared { parsed, code, decls } = prepare(doc)?;

    // 3. Passes.
    let globals = host::globals();
    let sandbox = Sandbox::new(fs);
    let base_dir = doc.base_dir();
    let eol = splice::line_ending(&doc.text);
    let doc_name = doc.path.as_ref().map_or_else(|| "document".to_owned(), |p| p.to_string());

    let mut text = doc.text.clone();
    let mut current: Parsed = parsed;
    let mut vars = Vars::new();
    let mut runs: BTreeMap<usize, BlockRun> = BTreeMap::new();
    let mut missing: BTreeMap<usize, String> = BTreeMap::new();
    // Contributors still to run, per task.
    let mut remaining: BTreeMap<String, usize> = BTreeMap::new();
    for d in decls.values() {
        for t in &d.completes {
            *remaining.entry(t.clone()).or_default() += 1;
        }
    }
    let complete = |remaining: &BTreeMap<String, usize>, t: &str| remaining.get(t).copied().unwrap_or(0) == 0;

    let mut pass = 0;
    loop {
        pass += 1;
        let mut ran_any = false;
        for &i in &code {
            if runs.contains_key(&i) || !decls[&i].depends_on.iter().all(|t| complete(&remaining, t)) {
                continue;
            }
            let block = current.blocks[i].clone();
            let body = block.body_text(&text).to_owned();
            let ctx = BlockCtx::new(&vars, &sandbox, base_dir.clone(), &text);
            let outcome = run_block(&doc_name, &block.line, &body, &globals, &ctx);
            let reads = sandbox.take_reads();

            if let Some(Abort::MissingVar(name)) = host::take_abort(&ctx) {
                missing.insert(i, name);
                continue;
            }
            outcome?;

            // Finished: commit the block's layer and tasks (D30), then splice its output.
            let local = ctx.local.take();
            let emitted = *ctx.emitted.borrow();
            let output = ctx.output.take();
            drop(ctx);
            vars.extend(local);
            for t in &decls[&i].completes {
                if let Some(n) = remaining.get_mut(t) {
                    *n -= 1;
                }
            }
            missing.remove(&i);
            runs.insert(i, BlockRun { line: block.line, pass, reads });
            ran_any = true;

            if block.output.is_some() {
                let region = splice::region_text(&output, eol);
                let new_text = splice::write_output(&text, &block, &current.stamps[i], &region);
                let reparsed = Parsed::new(&new_text).map_err(|_| UpdateError::OutputBreaksBlocks { line: block.line })?;
                let same_shape = reparsed.blocks.len() == current.blocks.len()
                    && reparsed.blocks.iter().zip(&current.blocks).all(|(a, b)| a.kind == b.kind);
                if !same_shape {
                    return Err(UpdateError::OutputBreaksBlocks { line: block.line });
                }
                text = new_text;
                current = reparsed;
            } else if emitted {
                return Err(UpdateError::EmitWithoutRegion { line: block.line });
            }
        }
        if runs.len() == code.len() {
            break;
        }
        if !ran_any {
            let stuck = code
                .iter()
                .filter(|i| !runs.contains_key(i))
                .map(|&i| Stuck {
                    line: current.blocks[i].line,
                    unmet_tasks: decls[&i].depends_on.iter().filter(|t| !complete(&remaining, t)).cloned().collect(),
                    missing_var: missing.get(&i).cloned(),
                })
                .collect();
            return Err(UpdateError::DeadEnd(stuck));
        }
    }

    Ok(UpdateResult { text, blocks: runs.into_values().collect(), vars })
}

/// Parses and runs one block body. An abort shows up here as an error and is told apart by the caller
/// through the context.
///
/// The body is padded with the newlines that precede it in the document, so locations in Starlark's
/// messages are document lines (`guide.md:12:5`).
fn run_block(
    doc_name: &str,
    line: &usize,
    body: &str,
    globals: &starlark::environment::Globals,
    ctx: &BlockCtx,
) -> Result<(), UpdateError> {
    let padded = format!("{}{body}", "\n".repeat(line - 1));
    let ast = lang::parse(doc_name, &padded).map_err(|e| UpdateError::Runtime { line: *line, message: e.message })?;
    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        eval.extra = Some(ctx);
        eval.eval_module(ast, globals).map(|_| ())
    })
    .map_err(|e| UpdateError::Runtime { line: *line, message: e.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemoryFs;
    use crate::stamps;

    fn run(text: &str, fs: &MemoryFs) -> Result<UpdateResult, UpdateError> {
        update(&Document::new(text), fs)
    }

    fn region(text: &str, n: usize) -> String {
        let p = Parsed::new(text).unwrap();
        let blocks: Vec<_> = p.blocks.iter().filter(|b| b.output.is_some()).collect();
        blocks[n].output_text(text).unwrap().to_owned()
    }

    #[test]
    fn emits_into_the_region_and_stamps_it() {
        let text = "# Doc\n<!--CODE\nemit('hello')\n-->\nold\n<!--/CODE-->\n";
        let r = run(text, &MemoryFs::new()).unwrap();
        let digest = stamps::generated_digest("hello\n");
        assert_eq!(r.text, format!("# Doc\n<!--CODE\nemit('hello')\n#@ generated: {digest}\n-->\nhello\n<!--/CODE-->\n"));
        // A second update is a no-op.
        assert_eq!(run(&r.text, &MemoryFs::new()).unwrap().text, r.text);
    }

    #[test]
    fn variables_flow_in_any_order_through_passes() {
        // The reader comes first; it aborts in pass 1 and runs in pass 2.
        let text = "<!--CODE\nemit('v' + var('version'))\n-->\n<!--/CODE-->\n<!--CODE\ndefine('version', read('VERSION').strip())\n-->\n";
        let fs = MemoryFs::new().with("VERSION", "1.2.3\n");
        let r = run(text, &fs).unwrap();
        assert_eq!(region(&r.text, 0), "v1.2.3\n");
        assert_eq!(r.blocks[0].pass, 2);
        assert_eq!(r.blocks[1].pass, 1);
        assert_eq!(r.blocks[1].reads.len(), 1);
        assert_eq!(r.blocks[1].reads[0].path.as_str(), "VERSION");
        assert!(r.blocks[0].reads.is_empty());
    }

    #[test]
    fn aborted_block_leaves_no_trace() {
        // Block 1 defines `a` and then aborts on `b`. Its `a` must not leak: block 3 (which needs `a`)
        // aborts in pass 1 too. In pass 2 block 1 reruns and commits `a`, and block 3, later in document
        // order, runs in the same pass.
        let text = "<!--CODE\ndefine('a', 1)\nx = var('b')\n-->\n<!--CODE\ndefine('b', 2)\n-->\n<!--CODE\nemit(str(var('a')))\n-->\n<!--/CODE-->\n";
        let r = run(text, &MemoryFs::new()).unwrap();
        assert_eq!(r.vars.get("a"), Some(&serde_json::json!(1)));
        let passes: Vec<usize> = r.blocks.iter().map(|b| b.pass).collect();
        assert_eq!(passes, [2, 1, 2]);
    }

    #[test]
    fn redefinition_needs_lenient_and_block_sees_its_own_values() {
        let text = "<!--CODE\ndefine('x', 1)\ndefine('x', 2)\n-->\n";
        assert!(matches!(run(text, &MemoryFs::new()), Err(UpdateError::Runtime { .. })));
        let text = "<!--CODE\ndefine('x', [1])\n-->\n<!--CODE\ndefine('x', var('x') + [2], lenient=True)\nemit(str(var('x')))\n-->\n<!--/CODE-->\n";
        let r = run(text, &MemoryFs::new()).unwrap();
        assert_eq!(region(&r.text, 0), "[1, 2]\n");
    }

    #[test]
    fn var_default_does_not_abort() {
        let text = "<!--CODE\nemit(str(var('nope', default=[])))\n-->\n<!--/CODE-->\n";
        assert_eq!(region(&run(text, &MemoryFs::new()).unwrap().text, 0), "[]\n");
    }

    #[test]
    fn tasks_order_blocks_and_toc_sees_generated_headings() {
        let text = "<!--CODE\ntasks(depends_on=['sections'])\nemit('\\n'.join([h.text for h in headings(min_level=2)]))\n-->\n<!--/CODE-->\n<!--CODE\ntasks(completes=['sections'])\nemit('## Generated')\n-->\n<!--/CODE-->\n";
        let r = run(text, &MemoryFs::new()).unwrap();
        assert_eq!(region(&r.text, 0), "Generated\n");
        assert_eq!(r.blocks[0].pass, 2);
    }

    #[test]
    fn dead_end_names_missing_variable() {
        let text = "<!--CODE\nemit(var('verison'))\n-->\n<!--/CODE-->\n";
        match run(text, &MemoryFs::new()) {
            Err(UpdateError::DeadEnd(stuck)) => {
                assert_eq!(stuck, vec![Stuck { line: 1, unmet_tasks: vec![], missing_var: Some("verison".into()) }]);
            }
            other => panic!("expected dead end, got {other:?}"),
        }
    }

    #[test]
    fn static_errors_before_running() {
        let typo = "<!--CODE\ntasks(depends_on=['tocc'])\n-->\n<!--CODE\ntasks(completes=['toc'])\n-->\n";
        assert!(matches!(run(typo, &MemoryFs::new()), Err(UpdateError::TaskGraph(_))));
        let misplaced = "<!--CODE\nx = 1\ntasks()\n-->\n";
        assert!(matches!(run(misplaced, &MemoryFs::new()), Err(UpdateError::Declaration { line: 1, .. })));
    }

    #[test]
    fn guard_and_structure_errors() {
        let edited = "<!--CODE\nemit('a')\n#@ generated: 2:blake3:00\n-->\nhand edit\n<!--/CODE-->\n";
        assert!(matches!(run(edited, &MemoryFs::new()), Err(UpdateError::Integrity(_))));
        let no_region = "<!--CODE\nemit('a')\n-->\n";
        assert_eq!(run(no_region, &MemoryFs::new()).unwrap_err(), UpdateError::EmitWithoutRegion { line: 1 });
        let breaking = "<!--CODE\nemit('<!--CODE--' + '>')\n-->\n<!--/CODE-->\n";
        assert!(matches!(run(breaking, &MemoryFs::new()), Err(UpdateError::OutputBreaksBlocks { line: 1 })));
    }

    #[test]
    fn sandbox_applies_to_code() {
        let fs = MemoryFs::new().with(".env", "SECRET").with("docs/a.txt", "A").with("docs/b.md", "B");
        let leak = "<!--CODE\nemit(read('.env'))\n-->\n<!--/CODE-->\n";
        assert!(matches!(run(leak, &fs), Err(UpdateError::Runtime { .. })));
        let escape = "<!--CODE\nemit(read('../x'))\n-->\n<!--/CODE-->\n";
        assert!(matches!(run(escape, &fs), Err(UpdateError::Runtime { .. })));
        // Paths resolve against the document's directory.
        let doc = Document::at(crate::fs::ProjectPath::new("docs/guide.md").unwrap(), "<!--CODE\nemit(read('a.txt') + ','.join(glob('*.md')))\n-->\n<!--/CODE-->\n");
        let r = update(&doc, &fs).unwrap();
        assert_eq!(region(&r.text, 0), "Ab.md\n");
    }

    #[test]
    fn starlark_errors_are_reported_with_block_line() {
        let text = "x\n\n<!--CODE\nemit(1 + 'a')\n-->\n<!--/CODE-->\n";
        match run(text, &MemoryFs::new()) {
            Err(UpdateError::Runtime { line, .. }) => assert_eq!(line, 3),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn recursion_terminates() {
        // starlark-rust does not reject recursion, but unbounded recursion ends in a stack overflow error.
        let text = "<!--CODE\ndef f(n):\n    return f(n)\nf(1)\n-->\n";
        match run(text, &MemoryFs::new()) {
            Err(UpdateError::Runtime { message, .. }) => assert!(message.contains("call stack overflow"), "{message}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn crlf_documents_get_crlf_output() {
        let text = "<!--CODE\r\nemit('a\\nb')\r\n-->\r\n<!--/CODE-->\r\n";
        let r = run(text, &MemoryFs::new()).unwrap();
        assert!(r.text.contains("-->\r\na\r\nb\r\n<!--/CODE-->"), "{:?}", r.text);
        assert!(r.text.contains("#@ generated: "));
    }
}
