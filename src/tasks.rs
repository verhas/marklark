//! Collecting `tasks(depends_on=[...], completes=[...])` declarations before the first pass (spec §7.1).
//!
//! A CODE block may start with one `tasks(...)` call. Both parameters are keyword-only and optional;
//! their values must be list literals of string literals. A `tasks` reference anywhere else, a second
//! call, a positional argument or a non-literal value is an error. The declarations of every block are
//! collected before anything runs, then checked together: a task some block depends on but no block
//! completes, and a cycle between tasks, are errors.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use starlark::codemap::Span;
use starlark::syntax::AstModule;
use starlark::syntax::ast::{ArgumentP, AstExpr, AstLiteral, AstStmt, ExprP, StmtP};
use starlark_syntax::syntax::uniplate::Visit;

use crate::lang::{self, ParseError};

/// The name of the declaration function.
pub const TASKS: &str = "tasks";

/// One block's declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskDecl {
    pub depends_on: Vec<String>,
    pub completes: Vec<String>,
}

/// An invalid declaration; `line` is 1-based within the CODE body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclError {
    Parse(ParseError),
    /// `tasks` used anywhere but as the first statement's call.
    Misplaced { line: usize },
    Positional { line: usize },
    UnknownParameter { line: usize, name: String },
    RepeatedParameter { line: usize, name: String },
    /// A parameter value that is not a list literal of string literals.
    NotLiteral { line: usize, name: String },
}

impl fmt::Display for DeclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeclError::Parse(e) => e.fmt(f),
            DeclError::Misplaced { line } => write!(
                f,
                "body line {line}: `tasks(...)` may only appear once, as the block's first statement"
            ),
            DeclError::Positional { line } => write!(
                f,
                "body line {line}: `tasks(...)` takes only named arguments: depends_on=[...], completes=[...]"
            ),
            DeclError::UnknownParameter { line, name } => write!(
                f,
                "body line {line}: `tasks(...)` has no parameter `{name}` (only depends_on, completes)"
            ),
            DeclError::RepeatedParameter { line, name } => {
                write!(f, "body line {line}: `tasks(...)` parameter `{name}` given twice")
            }
            DeclError::NotLiteral { line, name } => write!(
                f,
                "body line {line}: `{name}` must be a list of string literals, e.g. {name}=[\"toc\"]"
            ),
        }
    }
}

impl std::error::Error for DeclError {}

/// Parses `body` and returns its declaration, if it has one.
pub fn collect(body: &str) -> Result<Option<TaskDecl>, DeclError> {
    let module = lang::parse("CODE", body).map_err(DeclError::Parse)?;
    collect_from(&module)
}

/// Like [`collect`], for an already parsed body.
pub fn collect_from(module: &AstModule) -> Result<Option<TaskDecl>, DeclError> {
    let line = |span: Span| module.file_span(span).resolve_span().begin.line + 1;
    let first = first_statement(module.statement());

    // The first statement's call, if it is `tasks(...)`.
    let decl_call = first.and_then(|s| match &s.node {
        StmtP::Expression(e) => match &e.node {
            ExprP::Call(callee, args) if is_tasks(callee) => Some((callee, args)),
            _ => None,
        },
        _ => None,
    });

    // Any other reference to `tasks` is misplaced.
    let mut misplaced = None;
    walk_stmt(module.statement(), &mut |e| {
        if misplaced.is_none() && is_tasks(e) && decl_call.is_none_or(|(callee, _)| !std::ptr::eq(callee.as_ref(), e)) {
            misplaced = Some(line(e.span));
        }
    });
    if let Some(line) = misplaced {
        return Err(DeclError::Misplaced { line });
    }

    let Some((_, args)) = decl_call else { return Ok(None) };
    let mut decl = TaskDecl::default();
    let mut seen = BTreeSet::new();
    for arg in &args.args {
        let l = line(arg.span);
        let (name, value) = match &arg.node {
            ArgumentP::Named(name, value) => (name.node.as_str(), value),
            _ => return Err(DeclError::Positional { line: l }),
        };
        let target = match name {
            "depends_on" => &mut decl.depends_on,
            "completes" => &mut decl.completes,
            other => return Err(DeclError::UnknownParameter { line: l, name: other.to_owned() }),
        };
        if !seen.insert(name) {
            return Err(DeclError::RepeatedParameter { line: l, name: name.to_owned() });
        }
        *target = string_list(value).ok_or_else(|| DeclError::NotLiteral { line: l, name: name.to_owned() })?;
    }
    Ok(Some(decl))
}

fn is_tasks(e: &AstExpr) -> bool {
    matches!(&e.node, ExprP::Identifier(id) if id.node.ident == TASKS)
}

/// The first statement that is not a `load` (loads may precede the declaration).
fn first_statement(stmt: &AstStmt) -> Option<&AstStmt> {
    match &stmt.node {
        StmtP::Statements(stmts) => stmts.iter().find_map(first_statement),
        StmtP::Load(_) => None,
        _ => Some(stmt),
    }
}

fn string_list(e: &AstExpr) -> Option<Vec<String>> {
    match &e.node {
        ExprP::List(items) => items
            .iter()
            .map(|x| match &x.node {
                ExprP::Literal(AstLiteral::String(s)) => Some(s.node.clone()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn walk_stmt<'a>(stmt: &'a AstStmt, f: &mut impl FnMut(&'a AstExpr)) {
    stmt.node.visit_children(|v| match v {
        Visit::Stmt(s) => walk_stmt(s, f),
        Visit::Expr(e) => walk_expr(e, f),
    });
}

fn walk_expr<'a>(e: &'a AstExpr, f: &mut impl FnMut(&'a AstExpr)) {
    f(e);
    e.node.visit_expr(|child| walk_expr(child, f));
}

// ---------------------------------------------------------------------------------------------------
// Checks across all blocks
// ---------------------------------------------------------------------------------------------------

/// A problem with the task graph of a whole document. Blocks are identified by the caller's ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError<Id> {
    /// A task some block depends on that no block completes (usually a typo).
    NeverCompleted { task: String, needed_by: Vec<Id> },
    /// Tasks that depend on each other in a cycle; the first task is repeated at the end.
    Cycle { tasks: Vec<String> },
}

impl<Id: fmt::Display> fmt::Display for GraphError<Id> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::NeverCompleted { task, needed_by } => {
                let ids: Vec<String> = needed_by.iter().map(|i| i.to_string()).collect();
                write!(f, "task \"{task}\" is needed by {} but no block completes it", ids.join(", "))
            }
            GraphError::Cycle { tasks } => write!(f, "task cycle: {}", tasks.join(" -> ")),
        }
    }
}

/// Checks the collected declarations of all blocks before the first pass.
///
/// Edges: a block that depends on `a` and completes `b` makes `b` wait for `a`. A cycle means no
/// order can satisfy the declarations.
pub fn check_graph<Id: Clone + Ord>(decls: &[(Id, TaskDecl)]) -> Vec<GraphError<Id>> {
    let mut errors = Vec::new();
    let completed: BTreeSet<&str> = decls.iter().flat_map(|(_, d)| d.completes.iter().map(String::as_str)).collect();

    let mut missing: BTreeMap<&str, Vec<Id>> = BTreeMap::new();
    for (id, d) in decls {
        for t in &d.depends_on {
            if !completed.contains(t.as_str()) {
                missing.entry(t).or_default().push(id.clone());
            }
        }
    }
    for (task, needed_by) in missing {
        errors.push(GraphError::NeverCompleted { task: task.to_owned(), needed_by });
    }

    // Task b waits for task a.
    let mut edges: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (_, d) in decls {
        for b in &d.completes {
            for a in &d.depends_on {
                edges.entry(b.as_str()).or_default().insert(a.as_str());
            }
        }
    }
    if let Some(cycle) = find_cycle(&edges) {
        errors.push(GraphError::Cycle { tasks: cycle });
    }
    errors
}

/// Depth-first search for a cycle; deterministic (BTree order).
fn find_cycle(edges: &BTreeMap<&str, BTreeSet<&str>>) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Active,
        Done,
    }
    fn visit<'a>(
        node: &'a str,
        edges: &BTreeMap<&'a str, BTreeSet<&'a str>>,
        marks: &mut BTreeMap<&'a str, Mark>,
        stack: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        match marks.get(node) {
            Some(Mark::Done) => return None,
            Some(Mark::Active) => {
                let start = stack.iter().position(|n| *n == node).unwrap_or(0);
                let mut cycle: Vec<String> = stack[start..].iter().map(|s| s.to_string()).collect();
                cycle.push(node.to_owned());
                return Some(cycle);
            }
            None => {}
        }
        marks.insert(node, Mark::Active);
        stack.push(node);
        for next in edges.get(node).into_iter().flatten() {
            if let Some(c) = visit(next, edges, marks, stack) {
                return Some(c);
            }
        }
        stack.pop();
        marks.insert(node, Mark::Done);
        None
    }
    let mut marks = BTreeMap::new();
    for node in edges.keys() {
        let mut stack = Vec::new();
        if let Some(c) = visit(node, edges, &mut marks, &mut stack) {
            return Some(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(dep: &[&str], comp: &[&str]) -> TaskDecl {
        TaskDecl {
            depends_on: dep.iter().map(|s| s.to_string()).collect(),
            completes: comp.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn collects_the_declaration() {
        let body = "\n# leading comment\ntasks(depends_on=[\"headings\"], completes=[\"toc\", \"x\"])\nemit(1)\n#@ intent: a\n";
        assert_eq!(collect(body).unwrap(), Some(decl(&["headings"], &["toc", "x"])));
        assert_eq!(collect("tasks(completes=['a'])").unwrap(), Some(decl(&[], &["a"])));
        assert_eq!(collect("load('//l.star', 'f')\ntasks(completes=['a'])").unwrap(), Some(decl(&[], &["a"])));
        assert_eq!(collect("tasks()").unwrap(), Some(TaskDecl::default()));
    }

    #[test]
    fn no_declaration() {
        assert_eq!(collect("emit('x')\n").unwrap(), None);
        assert_eq!(collect("").unwrap(), None);
    }

    #[test]
    fn misplaced_declarations() {
        assert_eq!(collect("x = 1\ntasks(completes=['a'])").unwrap_err(), DeclError::Misplaced { line: 2 });
        assert_eq!(
            collect("tasks(completes=['a'])\ntasks(depends_on=['b'])").unwrap_err(),
            DeclError::Misplaced { line: 2 }
        );
        assert_eq!(collect("t = tasks\n").unwrap_err(), DeclError::Misplaced { line: 1 });
        assert_eq!(collect("def f():\n    tasks()\n").unwrap_err(), DeclError::Misplaced { line: 2 });
    }

    #[test]
    fn invalid_arguments() {
        assert_eq!(collect("tasks(['a'])").unwrap_err(), DeclError::Positional { line: 1 });
        assert!(matches!(collect("tasks(needs=['a'])").unwrap_err(), DeclError::UnknownParameter { .. }));
        assert!(matches!(
            collect("tasks(completes=['a'], completes=['b'])").unwrap_err(),
            DeclError::RepeatedParameter { .. } | DeclError::Parse(_)
        ));
        for bad in ["tasks(completes='a')", "tasks(completes=[x])", "tasks(completes=['a' + 'b'])", "tasks(completes=('a',))"] {
            assert!(matches!(collect(bad).unwrap_err(), DeclError::NotLiteral { .. }), "{bad}");
        }
    }

    #[test]
    fn graph_never_completed() {
        let errs = check_graph(&[(1, decl(&["headngs"], &["toc"])), (2, decl(&[], &["headings"]))]);
        assert_eq!(errs, vec![GraphError::NeverCompleted { task: "headngs".into(), needed_by: vec![1] }]);
    }

    #[test]
    fn graph_cycles() {
        let errs = check_graph(&[(1, decl(&["a"], &["b"])), (2, decl(&["b"], &["a"]))]);
        assert_eq!(errs, vec![GraphError::Cycle { tasks: vec!["a".into(), "b".into(), "a".into()] }]);
        let self_cycle = check_graph(&[(1, decl(&["a"], &["a"]))]);
        assert_eq!(self_cycle, vec![GraphError::Cycle { tasks: vec!["a".into(), "a".into()] }]);
        assert!(check_graph(&[(1, decl(&[], &["a"])), (2, decl(&["a"], &["b"])), (3, decl(&["a", "b"], &[]))]).is_empty());
    }
}
