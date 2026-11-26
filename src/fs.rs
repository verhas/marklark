//! File abstraction: marklark works on documents and sources held in memory or on disk alike.
//!
//! - [`ProjectPath`] — a normalised path relative to the project root; it cannot leave the root.
//! - [`FileSystem`] — the minimal storage interface (`read`, `read_dir`). [`MemoryFs`] keeps files in
//!   memory (library use, tests); [`DiskFs`] reads a directory tree and refuses symlinks that escape it.
//! - [`Sandbox`] — what CODE actually reads through: applies `.gitignore` rules and the built-in deny
//!   list (D18, D34) and records every read with its hash (D6). The rules are read through the
//!   `FileSystem` itself, so they behave identically in memory and on disk, with or without git (D32).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::stamps::blake3_digest;

// ---------------------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------------------

/// A path relative to the project root: `/`-separated, no `.` or `..` components, no leading `/`.
/// The empty path is the root itself.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ProjectPath(String);

impl ProjectPath {
    pub fn root() -> Self {
        ProjectPath(String::new())
    }

    /// Parses a root-relative path. Backslashes are accepted as separators.
    pub fn new(path: &str) -> Result<Self, FsError> {
        Self::root().join(path)
    }

    /// Resolves `rel` against this path (taken as a directory), lexically. An absolute `rel` (leading
    /// `/`) is taken from the project root. Fails if `..` would leave the root.
    pub fn join(&self, rel: &str) -> Result<Self, FsError> {
        let rel_norm = rel.replace('\\', "/");
        let mut parts: Vec<&str> = if rel_norm.starts_with('/') {
            Vec::new()
        } else {
            self.components().collect()
        };
        for part in rel_norm.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    if parts.pop().is_none() {
                        return Err(FsError::Escapes { path: rel.to_owned() });
                    }
                }
                p => parts.push(p),
            }
        }
        Ok(ProjectPath(parts.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|p| !p.is_empty())
    }

    /// The last component, or `""` for the root.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or("")
    }

    /// The containing directory; the root's parent is the root.
    pub fn parent(&self) -> ProjectPath {
        match self.0.rfind('/') {
            Some(i) => ProjectPath(self.0[..i].to_owned()),
            None => ProjectPath::root(),
        }
    }

    /// The directories from the root down to (excluding) this path.
    pub fn ancestors_from_root(&self) -> Vec<ProjectPath> {
        let mut dirs = vec![ProjectPath::root()];
        let comps: Vec<&str> = self.components().collect();
        for i in 1..comps.len() {
            dirs.push(ProjectPath(comps[..i].join("/")));
        }
        dirs
    }

    /// This path relative to `dir`, if it lies inside it.
    pub fn strip_dir(&self, dir: &ProjectPath) -> Option<&str> {
        if dir.is_root() {
            return Some(&self.0);
        }
        self.0.strip_prefix(dir.as_str())?.strip_prefix('/')
    }
}

impl fmt::Display for ProjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() { f.write_str(".") } else { f.write_str(&self.0) }
    }
}

// ---------------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsError {
    NotFound { path: String },
    /// A path that leaves the project root (`..` or a symlink).
    Escapes { path: String },
    /// Refused by `.gitignore` or the built-in deny list.
    Denied { path: String, reason: String },
    NotUtf8 { path: String },
    Io { path: String, message: String },
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::NotFound { path } => write!(f, "{path}: no such file"),
            FsError::Escapes { path } => write!(f, "{path}: leaves the project root"),
            FsError::Denied { path, reason } => write!(f, "{path}: refused ({reason})"),
            FsError::NotUtf8 { path } => write!(f, "{path}: not valid UTF-8 text"),
            FsError::Io { path, message } => write!(f, "{path}: {message}"),
        }
    }
}

impl std::error::Error for FsError {}

// ---------------------------------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------------------------------

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Minimal storage interface. Paths are already confined lexically by [`ProjectPath`]; an
/// implementation must additionally refuse anything that resolves outside its root (e.g. symlinks).
pub trait FileSystem {
    fn read(&self, path: &ProjectPath) -> Result<Vec<u8>, FsError>;
    /// Entries of a directory, sorted by name. A missing directory is `NotFound`.
    fn read_dir(&self, dir: &ProjectPath) -> Result<Vec<DirEntry>, FsError>;
}

/// Files held in memory. Directories exist implicitly as prefixes of file paths.
#[derive(Debug, Clone, Default)]
pub struct MemoryFs {
    files: BTreeMap<ProjectPath, Vec<u8>>,
}

impl MemoryFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a file. Panics on a path that leaves the root (a programming error here).
    pub fn insert(&mut self, path: &str, content: impl Into<Vec<u8>>) -> &mut Self {
        let path = ProjectPath::new(path).expect("MemoryFs::insert: path leaves the root");
        self.files.insert(path, content.into());
        self
    }

    pub fn with(mut self, path: &str, content: impl Into<Vec<u8>>) -> Self {
        self.insert(path, content);
        self
    }
}

impl FileSystem for MemoryFs {
    fn read(&self, path: &ProjectPath) -> Result<Vec<u8>, FsError> {
        self.files.get(path).cloned().ok_or_else(|| FsError::NotFound { path: path.to_string() })
    }

    fn read_dir(&self, dir: &ProjectPath) -> Result<Vec<DirEntry>, FsError> {
        let mut entries = std::collections::BTreeSet::new();
        for path in self.files.keys() {
            if let Some(rest) = path.strip_dir(dir) {
                match rest.split_once('/') {
                    Some((name, _)) => entries.insert(DirEntry { name: name.to_owned(), is_dir: true }),
                    None => entries.insert(DirEntry { name: rest.to_owned(), is_dir: false }),
                };
            }
        }
        if entries.is_empty() && !dir.is_root() {
            return Err(FsError::NotFound { path: dir.to_string() });
        }
        Ok(entries.into_iter().collect())
    }
}

/// A directory tree on disk.
#[derive(Debug, Clone)]
pub struct DiskFs {
    root: PathBuf,
}

impl DiskFs {
    /// Uses `root` as the project root.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, FsError> {
        let root = root.into();
        let root = root.canonicalize().map_err(|e| io_error(&root, e))?;
        Ok(DiskFs { root })
    }

    /// The project root for a document (D22): the git repository root; outside git the nearest
    /// directory at or above the document containing `marklark.toml`; otherwise the document's
    /// directory.
    pub fn discover(document: &Path) -> Result<Self, FsError> {
        let doc = document.canonicalize().map_err(|e| io_error(document, e))?;
        let start = if doc.is_dir() { doc.as_path() } else { doc.parent().unwrap_or(Path::new("/")) };
        let root = start
            .ancestors()
            .find(|d| d.join(".git").exists())
            .or_else(|| start.ancestors().find(|d| d.join("marklark.toml").is_file()))
            .unwrap_or(start);
        DiskFs::new(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The project path of a file on disk, if it lies inside the root.
    pub fn project_path(&self, file: &Path) -> Result<ProjectPath, FsError> {
        let abs = file.canonicalize().map_err(|e| io_error(file, e))?;
        let rel = abs.strip_prefix(&self.root).map_err(|_| FsError::Escapes { path: file.display().to_string() })?;
        let rel = rel.to_str().ok_or_else(|| FsError::NotUtf8 { path: file.display().to_string() })?;
        ProjectPath::new(rel)
    }

    /// The on-disk path, refusing anything that resolves outside the root.
    fn resolve(&self, path: &ProjectPath) -> Result<PathBuf, FsError> {
        let joined = self.root.join(path.as_str());
        let real = joined.canonicalize().map_err(|e| io_error_for(path, e))?;
        if !real.starts_with(&self.root) {
            return Err(FsError::Escapes { path: path.to_string() });
        }
        Ok(real)
    }
}

impl FileSystem for DiskFs {
    fn read(&self, path: &ProjectPath) -> Result<Vec<u8>, FsError> {
        let real = self.resolve(path)?;
        std::fs::read(&real).map_err(|e| io_error_for(path, e))
    }

    fn read_dir(&self, dir: &ProjectPath) -> Result<Vec<DirEntry>, FsError> {
        let real = self.resolve(dir)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&real).map_err(|e| io_error_for(dir, e))? {
            let entry = entry.map_err(|e| io_error_for(dir, e))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            // Follow symlinks only while they stay inside the root.
            let Ok(target) = entry.path().canonicalize() else { continue };
            if !target.starts_with(&self.root) {
                continue;
            }
            entries.push(DirEntry { name, is_dir: target.is_dir() });
        }
        entries.sort();
        Ok(entries)
    }
}

fn io_error(path: &Path, e: std::io::Error) -> FsError {
    if e.kind() == std::io::ErrorKind::NotFound {
        FsError::NotFound { path: path.display().to_string() }
    } else {
        FsError::Io { path: path.display().to_string(), message: e.to_string() }
    }
}

fn io_error_for(path: &ProjectPath, e: std::io::Error) -> FsError {
    if e.kind() == std::io::ErrorKind::NotFound {
        FsError::NotFound { path: path.to_string() }
    } else {
        FsError::Io { path: path.to_string(), message: e.to_string() }
    }
}

// ---------------------------------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------------------------------

/// File names always refused, whatever `.gitignore` says (D34). Matched against the last component.
const DENY_LIST: &[&str] = &[".env", ".env.*", "*.pem", "*.key", "id_rsa*", "id_dsa*", "id_ecdsa*", "id_ed25519*"];

/// Directories never read or listed.
const SKIPPED_DIRS: &[&str] = &[".git"];

/// A read recorded for dependency tracking (D6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRead {
    pub path: ProjectPath,
    /// `blake3:<hex>` of the bytes read.
    pub digest: String,
}

/// The confined, recording view of a [`FileSystem`] that host functions use.
pub struct Sandbox<'fs> {
    fs: &'fs dyn FileSystem,
    /// Compiled `.gitignore` per directory; `None` when the directory has none.
    ignores: RefCell<BTreeMap<ProjectPath, Option<Gitignore>>>,
    reads: RefCell<Vec<RecordedRead>>,
}

impl<'fs> Sandbox<'fs> {
    pub fn new(fs: &'fs dyn FileSystem) -> Self {
        Sandbox { fs, ignores: RefCell::new(BTreeMap::new()), reads: RefCell::new(Vec::new()) }
    }

    /// Reads a file after checking the deny list and `.gitignore`, and records the read.
    pub fn read(&self, path: &ProjectPath) -> Result<Vec<u8>, FsError> {
        self.check_allowed(path, false)?;
        let bytes = self.fs.read(path)?;
        self.reads.borrow_mut().push(RecordedRead { path: path.clone(), digest: blake3_digest(&bytes) });
        Ok(bytes)
    }

    /// Reads a UTF-8 text file (see [`Sandbox::read`]).
    pub fn read_text(&self, path: &ProjectPath) -> Result<String, FsError> {
        String::from_utf8(self.read(path)?).map_err(|_| FsError::NotUtf8 { path: path.to_string() })
    }

    /// Every readable file under the root, sorted. Ignored and denied files and `.git` are left out.
    pub fn list_files(&self) -> Result<Vec<ProjectPath>, FsError> {
        let mut out = Vec::new();
        self.walk(&ProjectPath::root(), &mut out)?;
        Ok(out)
    }

    /// The reads recorded so far, leaving the record empty.
    pub fn take_reads(&self) -> Vec<RecordedRead> {
        std::mem::take(&mut self.reads.borrow_mut())
    }

    fn walk(&self, dir: &ProjectPath, out: &mut Vec<ProjectPath>) -> Result<(), FsError> {
        for entry in self.fs.read_dir(dir)? {
            let path = dir.join(&entry.name)?;
            if self.check_allowed(&path, entry.is_dir).is_err() {
                continue;
            }
            if entry.is_dir {
                self.walk(&path, out)?;
            } else {
                out.push(path);
            }
        }
        Ok(())
    }

    /// Refuses `.git`, denied file names, and paths ignored by any `.gitignore` from the root down.
    pub fn check_allowed(&self, path: &ProjectPath, is_dir: bool) -> Result<(), FsError> {
        if path.components().any(|c| SKIPPED_DIRS.contains(&c)) {
            return Err(FsError::Denied { path: path.to_string(), reason: "inside .git".into() });
        }
        let name = path.file_name();
        if !is_dir && DENY_LIST.iter().any(|p| glob_name_matches(p, name)) {
            return Err(FsError::Denied { path: path.to_string(), reason: "built-in deny list".into() });
        }
        // Deeper .gitignore files override shallower ones; within a directory, the last match wins.
        let mut ignored = false;
        for dir in path.ancestors_from_root() {
            let Some(rel) = path.strip_dir(&dir) else { continue };
            self.with_gitignore(&dir, |gi| match gi.matched_path_or_any_parents(rel, is_dir) {
                Match::Ignore(_) => ignored = true,
                Match::Whitelist(_) => ignored = false,
                Match::None => {}
            });
        }
        if ignored {
            return Err(FsError::Denied { path: path.to_string(), reason: "matched by .gitignore".into() });
        }
        Ok(())
    }

    fn with_gitignore(&self, dir: &ProjectPath, f: impl FnOnce(&Gitignore)) {
        let mut cache = self.ignores.borrow_mut();
        let entry = cache.entry(dir.clone()).or_insert_with(|| self.load_gitignore(dir));
        if let Some(gi) = entry {
            f(gi);
        }
    }

    fn load_gitignore(&self, dir: &ProjectPath) -> Option<Gitignore> {
        let mut sources = vec![dir.join(".gitignore").ok()?];
        if dir.is_root() {
            sources.push(ProjectPath::new(".git/info/exclude").ok()?);
        }
        // Root "." makes the matcher take candidate paths as already relative to `dir`.
        let mut builder = GitignoreBuilder::new(".");
        let mut any = false;
        for source in sources {
            let Ok(bytes) = self.fs.read(&source) else { continue };
            for line in String::from_utf8_lossy(&bytes).lines() {
                // A malformed pattern is skipped, as git does.
                let _ = builder.add_line(None, line);
                any = true;
            }
        }
        if !any {
            return None;
        }
        builder.build().ok()
    }
}

/// `*`-only glob match of a file name (enough for the deny list).
fn glob_name_matches(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || !name[first.len()..].ends_with(last) || name.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> ProjectPath {
        ProjectPath::new(s).unwrap()
    }

    #[test]
    fn project_paths() {
        assert_eq!(p("a/./b//c").as_str(), "a/b/c");
        assert_eq!(p("a\\b").as_str(), "a/b");
        assert_eq!(p("docs").join("../src/x.rs").unwrap().as_str(), "src/x.rs");
        assert_eq!(p("docs").join("/README.md").unwrap().as_str(), "README.md");
        assert!(matches!(p("docs").join("../../etc/passwd"), Err(FsError::Escapes { .. })));
        assert_eq!(p("a/b/c").parent().as_str(), "a/b");
        assert_eq!(p("a").parent(), ProjectPath::root());
        assert_eq!(p("a/b/c").ancestors_from_root(), vec![ProjectPath::root(), p("a"), p("a/b")]);
        assert_eq!(p("ab/c").strip_dir(&p("a")), None);
        assert_eq!(p("a/c").strip_dir(&p("a")), Some("c"));
    }

    #[test]
    fn memory_fs_read_and_list() {
        let fs = MemoryFs::new().with("a.txt", "A").with("src/x.rs", "X").with("src/sub/y.rs", "Y");
        assert_eq!(fs.read(&p("src/x.rs")).unwrap(), b"X");
        assert!(matches!(fs.read(&p("nope")), Err(FsError::NotFound { .. })));
        let names: Vec<_> = fs.read_dir(&p("src")).unwrap();
        assert_eq!(
            names,
            vec![DirEntry { name: "sub".into(), is_dir: true }, DirEntry { name: "x.rs".into(), is_dir: false }]
        );
    }

    #[test]
    fn sandbox_records_reads() {
        let fs = MemoryFs::new().with("a.txt", "hello");
        let sb = Sandbox::new(&fs);
        assert_eq!(sb.read_text(&p("a.txt")).unwrap(), "hello");
        let reads = sb.take_reads();
        assert_eq!(reads, vec![RecordedRead { path: p("a.txt"), digest: blake3_digest(b"hello") }]);
        assert!(sb.take_reads().is_empty());
    }

    #[test]
    fn sandbox_refuses_denied_and_ignored() {
        let fs = MemoryFs::new()
            .with(".gitignore", "target/\n*.log\n!keep.log\n")
            .with(".env", "SECRET=1")
            .with("certs/server.pem", "x")
            .with("target/out.txt", "x")
            .with("app.log", "x")
            .with("keep.log", "x")
            .with("sub/.gitignore", "local.txt\n")
            .with("sub/local.txt", "x")
            .with("sub/ok.txt", "x")
            .with("local.txt", "x")
            .with(".git/config", "x");
        let sb = Sandbox::new(&fs);
        for denied in [".env", "certs/server.pem", "target/out.txt", "app.log", "sub/local.txt", ".git/config"] {
            assert!(matches!(sb.read(&p(denied)), Err(FsError::Denied { .. })), "{denied} should be refused");
        }
        for ok in ["keep.log", "sub/ok.txt", "local.txt"] {
            assert!(sb.read(&p(ok)).is_ok(), "{ok} should be readable");
        }
        let listed: Vec<String> = sb.list_files().unwrap().iter().map(|p| p.to_string()).collect();
        assert_eq!(listed, [".gitignore", "keep.log", "local.txt", "sub/.gitignore", "sub/ok.txt"]);
    }

    #[test]
    fn deny_list_globs() {
        assert!(glob_name_matches("*.pem", "a.pem"));
        assert!(!glob_name_matches("*.pem", "a.pem.txt"));
        assert!(glob_name_matches(".env.*", ".env.local"));
        assert!(!glob_name_matches(".env.*", ".env"));
        assert!(glob_name_matches("id_rsa*", "id_rsa.pub"));
        assert!(!glob_name_matches("*.key", ".key2"));
    }

    #[test]
    fn disk_fs_confines_and_discovers_root() {
        let tmp = std::env::temp_dir().join(format!("marklark-fs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("proj/docs")).unwrap();
        std::fs::write(tmp.join("proj/marklark.toml"), "").unwrap();
        std::fs::write(tmp.join("proj/docs/a.md"), "# A").unwrap();
        std::fs::write(tmp.join("outside.txt"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.join("outside.txt"), tmp.join("proj/link.txt")).unwrap();

        let fs = DiskFs::discover(&tmp.join("proj/docs/a.md")).unwrap();
        assert_eq!(fs.root(), tmp.join("proj").canonicalize().unwrap());
        assert_eq!(fs.project_path(&tmp.join("proj/docs/a.md")).unwrap(), p("docs/a.md"));
        assert_eq!(fs.read(&p("docs/a.md")).unwrap(), b"# A");
        #[cfg(unix)]
        {
            assert!(matches!(fs.read(&p("link.txt")), Err(FsError::Escapes { .. })));
            assert!(!fs.read_dir(&ProjectPath::root()).unwrap().iter().any(|e| e.name == "link.txt"));
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
