//! Starlark libraries: `load("//lib/docs.star", "include")` in CODE (D40).
//!
//! - A library is a `.star` file in the project, read through the sandbox. A path starting with `//` is
//!   relative to the project root; any other path is relative to the file that loads it (the document,
//!   or another library). Paths given to host functions stay relative to the document.
//! - A library's top level may only define functions and constants: `emit`, `define`, `var` and `tasks`
//!   fail there. When a block calls a library function, it runs as part of that block, so those host
//!   functions work inside it.
//! - A library is loaded once per update and then reused (frozen); a block still names what it uses
//!   with its own `load`. A load cycle is an error.
//! - Blocks record the libraries they use, directly or through other libraries, as `#@ lib:` stamps, so
//!   a library change shows as `lib_changed` on exactly the blocks it affects.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use starlark::environment::{FrozenModule, Globals, Module};
use starlark::eval::{Evaluator, FileLoader};

use crate::document::Document;
use crate::fs::{FileSystem, ProjectPath, Sandbox};
use crate::host::{BlockCtx, Here, Vars};
use crate::lang;
use crate::stamps;

/// The file extension a library must have.
pub const EXTENSION: &str = ".star";

/// Resolves a `load` path: `//x.star` from the project root, anything else from `base`.
pub fn resolve(base: &ProjectPath, spec: &str) -> Result<ProjectPath, String> {
    if !spec.ends_with(EXTENSION) {
        return Err(format!("load(\"{spec}\"): libraries are {EXTENSION} files"));
    }
    let path = match spec.strip_prefix("//") {
        Some(rooted) => ProjectPath::new(rooted),
        None => base.join(spec),
    };
    path.map_err(|e| format!("load(\"{spec}\"): {e}"))
}

/// How a library is named in stamps and messages: `//lib/docs.star`.
pub fn display(path: &ProjectPath) -> String {
    format!("//{path}")
}

/// Loads libraries for one update, caching each one.
pub struct Libraries<'a> {
    sandbox: &'a Sandbox<'a>,
    globals: &'a Globals,
    /// The document's directory: host-function paths in a library's top level resolve against it.
    doc_dir: ProjectPath,
    document: &'a str,
    no_vars: Vars,
    cache: RefCell<HashMap<ProjectPath, FrozenModule>>,
    loading: RefCell<Vec<ProjectPath>>,
}

impl<'a> Libraries<'a> {
    pub fn new(sandbox: &'a Sandbox<'a>, globals: &'a Globals, doc_dir: ProjectPath, document: &'a str) -> Self {
        Libraries {
            sandbox,
            globals,
            doc_dir,
            document,
            no_vars: Vars::new(),
            cache: RefCell::new(HashMap::new()),
            loading: RefCell::new(Vec::new()),
        }
    }

    /// The loader for code in `base` (the document's directory, or a library's).
    pub fn loader_for(&'a self, base: ProjectPath) -> Scoped<'a> {
        Scoped { libraries: self, base }
    }

    fn load_path(&self, path: ProjectPath) -> Result<FrozenModule, String> {
        if let Some(m) = self.cache.borrow().get(&path) {
            return Ok(m.clone());
        }
        if self.loading.borrow().contains(&path) {
            let mut chain: Vec<String> = self.loading.borrow().iter().map(display).collect();
            chain.push(display(&path));
            return Err(format!("load cycle: {}", chain.join(" -> ")));
        }
        let source = self.sandbox.read_text(&path).map_err(|e| e.to_string())?;
        let name = display(&path);
        let ast = lang::parse(&name, &source).map_err(|e| e.message)?;
        self.loading.borrow_mut().push(path.clone());
        let ctx = BlockCtx::library(&self.no_vars, self.sandbox, self.doc_dir.clone(), self.document, Here::default());
        let scoped = self.loader_for(path.parent());
        let result = Module::with_temp_heap(|module| {
            {
                let mut eval = Evaluator::new(&module);
                eval.extra = Some(&ctx);
                eval.set_loader(&scoped);
                eval.eval_module(ast, self.globals)?;
            }
            Ok::<_, starlark::Error>(module.freeze()?)
        });
        self.loading.borrow_mut().pop();
        let frozen = result.map_err(|e| e.to_string())?;
        self.cache.borrow_mut().insert(path, frozen.clone());
        Ok(frozen)
    }
}

/// A [`FileLoader`] that resolves relative `load` paths against one directory.
pub struct Scoped<'a> {
    libraries: &'a Libraries<'a>,
    base: ProjectPath,
}

impl FileLoader for Scoped<'_> {
    fn load(&self, spec: &str) -> starlark::Result<FrozenModule> {
        let path = resolve(&self.base, spec).map_err(|e| starlark::Error::new_other(anyhow::anyhow!(e)))?;
        self.libraries.load_path(path).map_err(|e| starlark::Error::new_other(anyhow::anyhow!(e)))
    }
}

/// The libraries a CODE body uses, directly or through other libraries, with the hash of each file's
/// content (line endings normalised): `[("//lib/docs.star", "blake3:…")]`, sorted. This is what the
/// `#@ lib:` stamps record.
pub fn closure(doc: &Document, fs: &dyn FileSystem, body: &str) -> Result<Vec<(String, String)>, String> {
    let sandbox = Sandbox::new(fs);
    let mut found: BTreeMap<ProjectPath, String> = BTreeMap::new();
    let mut todo: Vec<(ProjectPath, String)> = Vec::new();
    let module = lang::parse("CODE", body).map_err(|e| e.message)?;
    for load in module.loads() {
        todo.push((doc.base_dir(), load.module_id.to_owned()));
    }
    while let Some((base, spec)) = todo.pop() {
        let path = resolve(&base, &spec)?;
        if found.contains_key(&path) {
            continue;
        }
        let source = sandbox.read_text(&path).map_err(|e| format!("load(\"{spec}\"): {e}"))?;
        found.insert(path.clone(), stamps::blake3_digest(stamps::normalise_line_endings(&source).as_bytes()));
        let lib = lang::parse(&display(&path), &source).map_err(|e| e.message)?;
        for load in lib.loads() {
            todo.push((path.parent(), load.module_id.to_owned()));
        }
    }
    Ok(found.into_iter().map(|(p, h)| (display(&p), h)).collect())
}

/// The `#@ lib:` stamp value of one library.
pub fn stamp_value(path: &str, digest: &str) -> String {
    format!("{path} {digest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolving_paths() {
        let docs = ProjectPath::new("docs").unwrap();
        assert_eq!(resolve(&docs, "//lib/a.star").unwrap().as_str(), "lib/a.star");
        assert_eq!(resolve(&docs, "fmt.star").unwrap().as_str(), "docs/fmt.star");
        assert_eq!(resolve(&docs, "../lib/a.star").unwrap().as_str(), "lib/a.star");
        assert!(resolve(&docs, "data.json").is_err());
        assert!(resolve(&docs, "../../x.star").is_err());
    }
}
