//! The `marklark` command line: a thin shell around the library, which works on documents in memory.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod mcp;

use marklark::document::Document;
use marklark::eval::{self, UpdateError};
use marklark::fs::DiskFs;
use marklark::ai;
use marklark::state::{self, AcceptError, Finding, Severity};

/// Exit codes (D35): stable once released.
mod exit {
    /// Everything succeeded.
    pub const OK: u8 = 0;
    /// A document has a problem: an error in its blocks, a failed CODE block, a dead end, …
    pub const DOCUMENT: u8 = 1;
    /// marklark could not do its job: a file could not be read or written.
    pub const IO: u8 = 2;
}

#[derive(Parser)]
#[command(name = "marklark", version, about = "Maintain Markdown documents from INTENT and CODE blocks")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run all CODE blocks and splice their output into the documents.
    Update {
        files: Vec<PathBuf>,
        /// Treat every intent state other than verified as an error.
        #[arg(long)]
        strict: bool,
        /// Always write a .bak copy before changing a file.
        #[arg(long, conflicts_with = "no_bak")]
        bak: bool,
        /// Never write a .bak copy.
        #[arg(long)]
        no_bak: bool,
    },
    /// Check block structure, stamps, declarations, the task graph and output guards. Runs nothing,
    /// unless --determinism is given.
    Check {
        files: Vec<PathBuf>,
        /// Run the whole evaluation twice and fail on any difference.
        #[arg(long)]
        determinism: bool,
        /// Print a JSON report on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Report the intent states of each document; exit 1 if any is an error.
    IntentCheck {
        files: Vec<PathBuf>,
        /// Treat every state other than verified as an error.
        #[arg(long)]
        strict: bool,
        /// Print a JSON report on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Start the MCP server on stdin/stdout.
    Mcp,
    /// Confirm that the blocks governed by an INTENT still fit it: re-stamp them.
    IntentAccept {
        file: PathBuf,
        /// A line of the INTENT or of one of its blocks.
        #[arg(long)]
        line: usize,
        /// Instead, remove the `intent` and `params` stamps from the block at the line.
        #[arg(long)]
        drop_stamp: bool,
        /// Always write a .bak copy before changing the file.
        #[arg(long, conflicts_with = "no_bak")]
        bak: bool,
        /// Never write a .bak copy.
        #[arg(long)]
        no_bak: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut worst = exit::OK;
    match cli.command {
        Command::Update { files, strict, bak, no_bak } => {
            for file in &files {
                worst = worst.max(update_file(file, strict, backup_mode(bak, no_bak)));
            }
        }
        Command::Check { files, determinism, json } => {
            let mut reports = Vec::new();
            for file in &files {
                let (code, errors) = check_file(file, determinism);
                if !json {
                    for e in &errors {
                        eprintln!("{}: {e}", file.display());
                    }
                }
                reports.push(serde_json::json!({ "file": file.display().to_string(), "ok": errors.is_empty(), "errors": errors }));
                worst = worst.max(code);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::Value::Array(reports)).unwrap_or_default());
            }
        }
        Command::IntentCheck { files, strict, json } => {
            let mut reports = Vec::new();
            for file in &files {
                let (code, report) = intent_check_file(file, strict, !json);
                worst = worst.max(code);
                reports.push(report);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::Value::Array(reports)).unwrap_or_default());
            }
        }
        Command::Mcp => {
            if let Err(e) = mcp::serve() {
                worst = fail(exit::IO, &format!("MCP server: {e}"));
            }
        }
        Command::IntentAccept { file, line, drop_stamp, bak, no_bak } => {
            worst = intent_accept_file(&file, line, drop_stamp, backup_mode(bak, no_bak))
        }
    }
    ExitCode::from(worst)
}

#[derive(Clone, Copy)]
enum Backup {
    Always,
    Never,
    /// Unless git already holds the file's content (D17, D32).
    Auto,
}

fn backup_mode(bak: bool, no_bak: bool) -> Backup {
    match (bak, no_bak) {
        (true, _) => Backup::Always,
        (_, true) => Backup::Never,
        _ => Backup::Auto,
    }
}

/// Reads a document and the file system around it.
fn load(file: &Path) -> Result<(DiskFs, Document), String> {
    let fs = DiskFs::discover(file).map_err(|e| e.to_string())?;
    let path = fs.project_path(file).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{}: not valid UTF-8", file.display()))?;
    Ok((fs, Document::at(path, text)))
}

fn update_file(file: &Path, strict: bool, backup: Backup) -> u8 {
    let (fs, doc) = match load(file) {
        Ok(x) => x,
        Err(e) => return fail(exit::IO, &e),
    };
    // Intent drift never blocks an update (spec §9); some states warn, and --strict makes them errors.
    let findings = state::analyze(&doc).unwrap_or_default();
    if print_findings(file, &findings, state::Command::Update, strict) == Severity::Error {
        return exit::DOCUMENT;
    }
    let result = match eval::update(&doc, &fs) {
        Ok(r) => r,
        Err(e) => return report(file, &e),
    };
    if result.text == doc.text {
        return exit::OK;
    }
    write_document(file, &doc.text, &result.text, backup)
}

/// Writes a changed document, with a backup if wanted.
fn write_document(file: &Path, old: &str, new: &str, backup: Backup) -> u8 {
    let wants_backup = match backup {
        Backup::Always => true,
        Backup::Never => false,
        Backup::Auto => !git_holds_content(file),
    };
    if wants_backup {
        let mut bak = file.as_os_str().to_owned();
        bak.push(".bak");
        if let Err(e) = std::fs::write(&bak, old) {
            return fail(exit::IO, &format!("{}: cannot write backup: {e}", Path::new(&bak).display()));
        }
    }
    if let Err(e) = std::fs::write(file, new) {
        return fail(exit::IO, &format!("{}: {e}", file.display()));
    }
    eprintln!("{}: updated", file.display());
    exit::OK
}

/// Checks one document; returns the exit code and the error messages.
fn check_file(file: &Path, determinism: bool) -> (u8, Vec<String>) {
    let (fs, doc) = match load(file) {
        Ok(x) => x,
        Err(e) => return (exit::IO, vec![e]),
    };
    if let Err(e) = eval::check(&doc) {
        return (exit::DOCUMENT, error_lines(&e));
    }
    if determinism {
        let first = eval::update(&doc, &fs);
        let second = eval::update(&doc, &fs);
        let same = match (&first, &second) {
            (Ok(a), Ok(b)) => a.text == b.text && a.vars == b.vars,
            (Err(a), Err(b)) => a == b,
            _ => false,
        };
        if !same {
            return (exit::DOCUMENT, vec!["two runs of the same update gave different results".into()]);
        }
        if let Err(e) = first {
            return (exit::DOCUMENT, error_lines(&e));
        }
    }
    (exit::OK, Vec::new())
}

/// The messages of an error, one per problem.
fn error_lines(e: &UpdateError) -> Vec<String> {
    match e {
        UpdateError::Document(errors) => errors.iter().map(ToString::to_string).collect(),
        UpdateError::TaskGraph(errors) => errors.iter().map(ToString::to_string).collect(),
        UpdateError::Inline(errors) => errors.iter().map(ToString::to_string).collect(),
        other => vec![other.to_string()],
    }
}

/// Prints findings with their severity for `command`; returns the worst severity.
fn print_findings(file: &Path, findings: &[Finding], command: state::Command, strict: bool) -> Severity {
    let mut worst = Severity::Ok;
    for f in findings {
        let sev = state::severity(f.state, command, strict);
        let label = match sev {
            Severity::Ok => continue,
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        eprintln!("{}: {label}: {f}", file.display());
        worst = worst.max(sev);
    }
    worst
}

fn intent_check_file(file: &Path, strict: bool, print: bool) -> (u8, serde_json::Value) {
    let path = file.display().to_string();
    let (fs, doc) = match load(file) {
        Ok(x) => x,
        Err(e) => {
            if print {
                fail(exit::IO, &e);
            }
            return (exit::IO, serde_json::json!({ "file": path, "error": e }));
        }
    };
    let findings = match state::analyze(&doc).and_then(|mut f| {
        f.extend(ai::analyze(&doc, &fs)?);
        f.sort_by_key(|x| x.line);
        Ok(f)
    }) {
        Ok(f) => f,
        Err(errors) => {
            let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
            if print {
                eprintln!("{path}: {}", messages.join("\n"));
            }
            return (exit::DOCUMENT, serde_json::json!({ "file": path, "error": messages.join("\n") }));
        }
    };
    // `broken` needs a run; the result is discarded.
    let run = eval::update(&doc, &fs).map(|_| ());
    let findings = state::with_run_result(findings, &run);
    let worst = if print {
        print_findings(file, &findings, state::Command::IntentCheck, strict)
    } else {
        findings.iter().map(|f| state::severity(f.state, state::Command::IntentCheck, strict)).max().unwrap_or(Severity::Ok)
    };
    let items: Vec<serde_json::Value> = findings
        .iter()
        .map(|f| {
            let sev = match state::severity(f.state, state::Command::IntentCheck, strict) {
                Severity::Ok => "ok",
                Severity::Warning => "warning",
                Severity::Error => "error",
            };
            serde_json::json!({ "line": f.line, "state": f.state.name(), "severity": sev, "detail": f.detail })
        })
        .collect();
    let code = if worst == Severity::Error { exit::DOCUMENT } else { exit::OK };
    (code, serde_json::json!({ "file": path, "findings": items }))
}

fn intent_accept_file(file: &Path, line: usize, drop_stamp: bool, backup: Backup) -> u8 {
    let (fs, doc) = match load(file) {
        Ok(x) => x,
        Err(e) => return fail(exit::IO, &e),
    };
    let result = if drop_stamp {
        state::drop_stamps(&doc, line).map_err(|e| e.to_string())
    } else {
        match state::accept(&doc, line) {
            Err(AcceptError::AiBlock { line }) => ai::accept(&doc, &fs, line).map_err(|e| e.to_string()),
            other => other.map_err(|e| e.to_string()),
        }
    };
    match result {
        Ok(text) if text == doc.text => exit::OK,
        Ok(text) => write_document(file, &doc.text, &text, backup),
        Err(e) => {
            eprintln!("{}: {e}", file.display());
            exit::DOCUMENT
        }
    }
}

fn report(file: &Path, e: &UpdateError) -> u8 {
    eprintln!("{}: {e}", file.display());
    exit::DOCUMENT
}

fn fail(code: u8, message: &str) -> u8 {
    eprintln!("marklark: {message}");
    code
}

/// True when the file is tracked by git and has no staged or unstaged changes. Without git (or outside
/// a repository) this is false, so a backup is written (D32).
fn git_holds_content(file: &Path) -> bool {
    let dir = file.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = match file.file_name() {
        Some(n) => n,
        None => return false,
    };
    let run = |args: &[&std::ffi::OsStr]| {
        std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    run(&["ls-files".as_ref(), "--error-unmatch".as_ref(), "--".as_ref(), name])
        && run(&["diff".as_ref(), "--quiet".as_ref(), "--".as_ref(), name])
        && run(&["diff".as_ref(), "--cached".as_ref(), "--quiet".as_ref(), "--".as_ref(), name])
}
