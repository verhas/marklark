//! The `marklark` command line: a thin shell around the library, which works on documents in memory.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use marklark::document::Document;
use marklark::eval::{self, UpdateError};
use marklark::fs::DiskFs;

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
        /// Always write a .bak copy before changing a file.
        #[arg(long, conflicts_with = "no_bak")]
        bak: bool,
        /// Never write a .bak copy.
        #[arg(long)]
        no_bak: bool,
    },
    /// Check block structure, stamps, declarations, the task graph and output guards. Runs nothing.
    Check { files: Vec<PathBuf> },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut worst = exit::OK;
    match cli.command {
        Command::Update { files, bak, no_bak } => {
            for file in &files {
                worst = worst.max(update_file(file, backup_mode(bak, no_bak)));
            }
        }
        Command::Check { files } => {
            for file in &files {
                worst = worst.max(check_file(file));
            }
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

fn update_file(file: &Path, backup: Backup) -> u8 {
    let (fs, doc) = match load(file) {
        Ok(x) => x,
        Err(e) => return fail(exit::IO, &e),
    };
    let result = match eval::update(&doc, &fs) {
        Ok(r) => r,
        Err(e) => return report(file, &e),
    };
    if result.text == doc.text {
        return exit::OK;
    }
    let wants_backup = match backup {
        Backup::Always => true,
        Backup::Never => false,
        Backup::Auto => !git_holds_content(file),
    };
    if wants_backup {
        let mut bak = file.as_os_str().to_owned();
        bak.push(".bak");
        if let Err(e) = std::fs::write(&bak, &doc.text) {
            return fail(exit::IO, &format!("{}: cannot write backup: {e}", Path::new(&bak).display()));
        }
    }
    if let Err(e) = std::fs::write(file, &result.text) {
        return fail(exit::IO, &format!("{}: {e}", file.display()));
    }
    eprintln!("{}: updated", file.display());
    exit::OK
}

fn check_file(file: &Path) -> u8 {
    let (_, doc) = match load(file) {
        Ok(x) => x,
        Err(e) => return fail(exit::IO, &e),
    };
    match eval::check(&doc) {
        Ok(()) => exit::OK,
        Err(e) => report(file, &e),
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
