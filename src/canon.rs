//! Version-independent canonical printer of the Starlark AST, used for the `params` hash (D20).
//!
//! The canonical form is deliberately not source code, so that nothing in it depends on how any
//! library formats Starlark:
//!
//! - prefix notation with parentheses: `(call (id 4:emit) (pos (str 1:x)))`;
//! - every string, identifier and name is length-prefixed (`<chars>:<text>`), so no escaping rules;
//! - integers in decimal, floats as their IEEE-754 bit pattern;
//! - only node values are read — never spans — so layout, comments and stamp lines do not count;
//! - nested statement lists are flattened, so `a; b` and two lines are the same.
//!
//! If the canonical form ever has to change, bump [`CANON_VERSION`]: stamps carry it (`ast1:`), so a
//! mismatch reads as "rehash needed", not as a hand edit. A change in `starlark_syntax`'s AST types
//! breaks this file's build instead of silently changing hashes.

use std::fmt::Write;

use starlark::syntax::ast::{
    ArgumentP, AssignOp, AssignTargetP, AstAssignTarget, AstExpr, AstLiteral, AstParameter, AstStmt, BinOp,
    ClauseP, ExprP, ForClauseP, ParameterP, StmtP,
};
use starlark_syntax::lexer::TokenInt;

use crate::lang::{self, ParseError};
use crate::stamps::blake3_digest;

/// The version tag of the canonical form, written into `params` stamps.
pub const CANON_VERSION: &str = "ast1";

/// The `params` stamp value of a CODE body: `ast1:blake3:<hex>`.
pub fn params_digest(body: &str) -> Result<String, ParseError> {
    Ok(format!("{CANON_VERSION}:{}", blake3_digest(canonical(body)?.as_bytes())))
}

/// The canonical form of a CODE body.
pub fn canonical(body: &str) -> Result<String, ParseError> {
    let module = lang::parse("CODE", body)?;
    let mut p = Printer::default();
    p.stmt(module.statement());
    Ok(p.out)
}

#[derive(Default)]
struct Printer {
    out: String,
}

impl Printer {
    fn open(&mut self, tag: &str) {
        self.out.push('(');
        self.out.push_str(tag);
    }

    fn close(&mut self) {
        self.out.push(')');
    }

    /// A length-prefixed string atom.
    fn s(&mut self, text: &str) {
        let _ = write!(self.out, " {}:{}", text.chars().count(), text);
    }

    fn atom(&mut self, text: &str) {
        self.out.push(' ');
        self.out.push_str(text);
    }

    fn sp(&mut self) {
        self.out.push(' ');
    }

    // ---- statements -----------------------------------------------------------------------------

    fn stmt(&mut self, stmt: &AstStmt) {
        self.open("block");
        self.stmt_items(stmt);
        self.close();
    }

    /// Prints the statements of `stmt` as items of the enclosing block, flattening nested lists.
    fn stmt_items(&mut self, stmt: &AstStmt) {
        match &stmt.node {
            StmtP::Statements(stmts) => {
                for s in stmts {
                    self.stmt_items(s);
                }
            }
            _ => {
                self.sp();
                self.stmt_one(stmt);
            }
        }
    }

    fn stmt_one(&mut self, stmt: &AstStmt) {
        match &stmt.node {
            StmtP::Break => self.out.push_str("(break)"),
            StmtP::Continue => self.out.push_str("(continue)"),
            StmtP::Pass => self.out.push_str("(pass)"),
            StmtP::Return(value) => {
                self.open("return");
                if let Some(v) = value {
                    self.sp();
                    self.expr(v);
                }
                self.close();
            }
            StmtP::Expression(e) => {
                self.open("expr");
                self.sp();
                self.expr(e);
                self.close();
            }
            StmtP::Assign(assign) => {
                self.open("assign");
                self.sp();
                self.target(&assign.lhs);
                self.sp();
                self.opt_type(assign.ty.as_ref().map(|t| &t.node.expr));
                self.sp();
                self.expr(&assign.rhs);
                self.close();
            }
            StmtP::AssignModify(lhs, op, rhs) => {
                self.open("assign-op");
                self.atom(assign_op(*op));
                self.sp();
                self.target(lhs);
                self.sp();
                self.expr(rhs);
                self.close();
            }
            StmtP::Statements(_) => self.stmt(stmt),
            StmtP::If(cond, then) => {
                self.open("if");
                self.sp();
                self.expr(cond);
                self.sp();
                self.stmt(then);
                self.close();
            }
            StmtP::IfElse(cond, branches) => {
                self.open("if-else");
                self.sp();
                self.expr(cond);
                self.sp();
                self.stmt(&branches.0);
                self.sp();
                self.stmt(&branches.1);
                self.close();
            }
            StmtP::For(f) => {
                self.open("for");
                self.sp();
                self.target(&f.var);
                self.sp();
                self.expr(&f.over);
                self.sp();
                self.stmt(&f.body);
                self.close();
            }
            StmtP::Def(def) => {
                self.open("def");
                self.s(&def.name.node.ident);
                self.sp();
                self.params(&def.params);
                self.sp();
                self.opt_type(def.return_type.as_ref().map(|t| &t.node.expr));
                self.sp();
                self.stmt(&def.body);
                self.close();
            }
            StmtP::Load(load) => {
                self.open("load");
                self.s(&load.module.node);
                for arg in &load.args {
                    self.out.push_str(" (as");
                    self.s(&arg.local.node.ident);
                    self.s(&arg.their.node);
                    self.close();
                }
                self.close();
            }
        }
    }

    // ---- expressions ----------------------------------------------------------------------------

    fn expr(&mut self, e: &AstExpr) {
        match &e.node {
            ExprP::Tuple(items) => self.list("tuple", items),
            ExprP::List(items) => self.list("list", items),
            ExprP::Dict(pairs) => {
                self.open("dict");
                for (k, v) in pairs {
                    self.out.push_str(" (kv ");
                    self.expr(k);
                    self.sp();
                    self.expr(v);
                    self.close();
                }
                self.close();
            }
            ExprP::Dot(obj, name) => {
                self.open("dot");
                self.sp();
                self.expr(obj);
                self.s(&name.node);
                self.close();
            }
            ExprP::Call(f, args) => {
                self.open("call");
                self.sp();
                self.expr(f);
                for arg in &args.args {
                    self.sp();
                    match &arg.node {
                        ArgumentP::Positional(x) => self.tagged("pos", x),
                        ArgumentP::Named(name, x) => {
                            self.open("named");
                            self.s(&name.node);
                            self.sp();
                            self.expr(x);
                            self.close();
                        }
                        ArgumentP::Args(x) => self.tagged("star", x),
                        ArgumentP::KwArgs(x) => self.tagged("star2", x),
                    }
                }
                self.close();
            }
            ExprP::Index(pair) => {
                self.open("index");
                self.sp();
                self.expr(&pair.0);
                self.sp();
                self.expr(&pair.1);
                self.close();
            }
            ExprP::Index2(triple) => {
                self.open("index2");
                self.sp();
                self.expr(&triple.0);
                self.sp();
                self.expr(&triple.1);
                self.sp();
                self.expr(&triple.2);
                self.close();
            }
            ExprP::Slice(obj, start, stop, step) => {
                self.open("slice");
                self.sp();
                self.expr(obj);
                for part in [start, stop, step] {
                    self.sp();
                    self.opt_expr(part.as_deref());
                }
                self.close();
            }
            ExprP::Identifier(id) => {
                self.open("id");
                self.s(&id.node.ident);
                self.close();
            }
            ExprP::Lambda(lambda) => {
                self.open("lambda");
                self.sp();
                self.params(&lambda.params);
                self.sp();
                self.expr(&lambda.body);
                self.close();
            }
            ExprP::Literal(lit) => self.literal(lit),
            ExprP::Not(x) => self.tagged("not", x),
            ExprP::Minus(x) => self.tagged("neg", x),
            ExprP::Plus(x) => self.tagged("pos+", x),
            ExprP::BitNot(x) => self.tagged("bitnot", x),
            ExprP::Op(l, op, r) => {
                self.open("op");
                self.atom(bin_op(*op));
                self.sp();
                self.expr(l);
                self.sp();
                self.expr(r);
                self.close();
            }
            ExprP::If(triple) => {
                // Order in the AST: condition, value if true, value if false.
                self.open("cond");
                self.sp();
                self.expr(&triple.0);
                self.sp();
                self.expr(&triple.1);
                self.sp();
                self.expr(&triple.2);
                self.close();
            }
            ExprP::ListComprehension(item, first, clauses) => {
                self.open("list-comp");
                self.sp();
                self.expr(item);
                self.sp();
                self.for_clause(first);
                self.clauses(clauses);
                self.close();
            }
            ExprP::DictComprehension(kv, first, clauses) => {
                self.open("dict-comp");
                self.sp();
                self.expr(&kv.0);
                self.sp();
                self.expr(&kv.1);
                self.sp();
                self.for_clause(first);
                self.clauses(clauses);
                self.close();
            }
            ExprP::FString(fs) => {
                self.open("fstring");
                self.s(&fs.node.format.node);
                for x in &fs.node.expressions {
                    self.sp();
                    self.expr(x);
                }
                self.close();
            }
        }
    }

    fn tagged(&mut self, tag: &str, x: &AstExpr) {
        self.open(tag);
        self.sp();
        self.expr(x);
        self.close();
    }

    fn list(&mut self, tag: &str, items: &[AstExpr]) {
        self.open(tag);
        for x in items {
            self.sp();
            self.expr(x);
        }
        self.close();
    }

    fn opt_expr(&mut self, e: Option<&AstExpr>) {
        match e {
            Some(e) => self.expr(e),
            None => self.out.push_str("(none)"),
        }
    }

    fn opt_type(&mut self, ty: Option<&AstExpr>) {
        match ty {
            Some(t) => self.tagged("type", t),
            None => self.out.push_str("(untyped)"),
        }
    }

    fn literal(&mut self, lit: &AstLiteral) {
        match lit {
            AstLiteral::Int(i) => {
                let text = match &i.node {
                    TokenInt::I32(v) => v.to_string(),
                    TokenInt::BigInt(v) => v.to_string(),
                };
                self.open("int");
                self.atom(&text);
                self.close();
            }
            AstLiteral::Float(f) => {
                let _ = write!(self.out, "(float {:016x})", f.node.to_bits());
            }
            AstLiteral::String(s) => {
                self.open("str");
                self.s(&s.node);
                self.close();
            }
            AstLiteral::Bytes(b) => {
                self.open("bytes");
                self.sp();
                for byte in &b.node {
                    let _ = write!(self.out, "{byte:02x}");
                }
                self.close();
            }
            AstLiteral::Ellipsis => self.out.push_str("(ellipsis)"),
        }
    }

    fn target(&mut self, t: &AstAssignTarget) {
        match &t.node {
            AssignTargetP::Tuple(items) => {
                self.open("t-tuple");
                for x in items {
                    self.sp();
                    self.target(x);
                }
                self.close();
            }
            AssignTargetP::Index(pair) => {
                self.open("t-index");
                self.sp();
                self.expr(&pair.0);
                self.sp();
                self.expr(&pair.1);
                self.close();
            }
            AssignTargetP::Dot(obj, name) => {
                self.open("t-dot");
                self.sp();
                self.expr(obj);
                self.s(&name.node);
                self.close();
            }
            AssignTargetP::Identifier(id) => {
                self.open("t-id");
                self.s(&id.node.ident);
                self.close();
            }
        }
    }

    fn params(&mut self, params: &[AstParameter]) {
        self.open("params");
        for p in params {
            self.sp();
            match &p.node {
                ParameterP::Slash => self.out.push_str("(slash)"),
                ParameterP::NoArgs => self.out.push_str("(star-only)"),
                ParameterP::Normal(id, ty, default) => {
                    self.open("param");
                    self.s(&id.node.ident);
                    self.sp();
                    self.opt_type(ty.as_ref().map(|t| &t.node.expr));
                    self.sp();
                    self.opt_expr(default.as_deref());
                    self.close();
                }
                ParameterP::Args(id, ty) => {
                    self.open("param*");
                    self.s(&id.node.ident);
                    self.sp();
                    self.opt_type(ty.as_ref().map(|t| &t.node.expr));
                    self.close();
                }
                ParameterP::KwArgs(id, ty) => {
                    self.open("param**");
                    self.s(&id.node.ident);
                    self.sp();
                    self.opt_type(ty.as_ref().map(|t| &t.node.expr));
                    self.close();
                }
            }
        }
        self.close();
    }

    fn for_clause(&mut self, f: &ForClauseP<starlark::syntax::ast::AstNoPayload>) {
        self.open("for");
        self.sp();
        self.target(&f.var);
        self.sp();
        self.expr(&f.over);
        self.close();
    }

    fn clauses(&mut self, clauses: &[ClauseP<starlark::syntax::ast::AstNoPayload>]) {
        for c in clauses {
            self.sp();
            match c {
                ClauseP::For(f) => self.for_clause(f),
                ClauseP::If(cond) => self.tagged("if", cond),
            }
        }
    }
}

fn bin_op(op: BinOp) -> &'static str {
    match op {
        BinOp::Or => "or",
        BinOp::And => "and",
        BinOp::Equal => "==",
        BinOp::NotEqual => "!=",
        BinOp::Less => "<",
        BinOp::Greater => ">",
        BinOp::LessOrEqual => "<=",
        BinOp::GreaterOrEqual => ">=",
        BinOp::In => "in",
        BinOp::NotIn => "not-in",
        BinOp::Subtract => "-",
        BinOp::Add => "+",
        BinOp::Multiply => "*",
        BinOp::Percent => "%",
        BinOp::Divide => "/",
        BinOp::FloorDivide => "//",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::LeftShift => "<<",
        BinOp::RightShift => ">>",
    }
}

fn assign_op(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Add => "+=",
        AssignOp::Subtract => "-=",
        AssignOp::Multiply => "*=",
        AssignOp::Divide => "/=",
        AssignOp::FloorDivide => "//=",
        AssignOp::Percent => "%=",
        AssignOp::BitAnd => "&=",
        AssignOp::BitOr => "|=",
        AssignOp::BitXor => "^=",
        AssignOp::LeftShift => "<<=",
        AssignOp::RightShift => ">>=",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOC: &str = "\ntasks(depends_on=[\"h\"], completes=[\"toc\"])\nlines = []\nfor h in headings(min_level=2, max_level=3):\n    lines.append(\"  \" * (h.level - 2) + \"- [\" + h.text + \"]\")\nemit(\"\\n\".join(lines))\n#@ intent: blake3:aa\n#@ params: ast1:blake3:bb\n";

    #[test]
    fn layout_quotes_comments_and_stamps_do_not_count() {
        let reformatted = "# a comment\r\ntasks( depends_on = ['h'] , completes=[\"toc\",] )   \r\n\r\nlines=[]\r\nfor h in headings(min_level = 2,\r\n                  max_level = 3):\r\n  lines.append(('  ' * (h.level - 2)) + \"- [\" + h.text + ']')  # why\r\nemit('\\n'.join(lines))\r\n#@ intent: blake3:changed\r\n";
        assert_eq!(canonical(TOC).unwrap(), canonical(reformatted).unwrap());
        assert_eq!(params_digest(TOC).unwrap(), params_digest(reformatted).unwrap());
    }

    #[test]
    fn semantic_edits_count() {
        let base = params_digest(TOC).unwrap();
        for edit in [
            TOC.replace("max_level=3", "max_level=4"),
            TOC.replace("\"- [\"", "\"* [\""),
            TOC.replace("h.level - 2", "h.level + 2"),
            TOC.replace("lines.append", "lines.extend"),
        ] {
            assert_ne!(params_digest(&edit).unwrap(), base, "{edit}");
        }
    }

    #[test]
    fn precedence_is_explicit_and_parentheses_vanish() {
        assert_eq!(canonical("x = (a + b) * c").unwrap(), canonical("x = ((a + b)) * (c)").unwrap());
        assert_ne!(canonical("x = (a + b) * c").unwrap(), canonical("x = a + b * c").unwrap());
    }

    #[test]
    fn strings_are_length_prefixed() {
        // Without length prefixes these two could print alike.
        assert_ne!(canonical("f('a b', 'c')").unwrap(), canonical("f('a', 'b c')").unwrap());
        assert_eq!(canonical("'é'").unwrap(), "(block (expr (str 1:é)))");
    }

    #[test]
    fn statement_lists_are_flattened() {
        assert_eq!(canonical("a = 1; b = 2").unwrap(), canonical("a = 1\nb = 2\n").unwrap());
    }

    #[test]
    fn literals_and_constructs() {
        for (src, needle) in [
            ("def f(a, b=1, *args, c, **kw):\n    return a\n", "(def 1:f (params (param 1:a (untyped) (none)) (param 1:b (untyped) (int 1)) (param* 4:args (untyped)) (param 1:c (untyped) (none)) (param** 2:kw (untyped)))"),
            ("x = [y for y in a if y]", "(list-comp (id 1:y) (for (t-id 1:y) (id 1:a)) (if (id 1:y)))"),
            ("x = {k: v for k, v in d.items()}", "(dict-comp"),
            ("x = 1.5", "(float 3ff8000000000000)"),
            ("x = 12345678901234567890", "(int 12345678901234567890)"),
            ("x = a[1:2]", "(slice (id 1:a) (int 1) (int 2) (none))"),
            ("x = lambda y: -y", "(lambda (params (param 1:y (untyped) (none))) (neg (id 1:y)))"),
            ("x = a if b else c", "(cond (id 1:b) (id 1:a) (id 1:c))"),
            ("x += 1", "(assign-op += (t-id 1:x) (int 1))"),
            ("x = b'\\x00A'", "(bytes 0041)"),
            ("a, b = 1, 2", "(assign (t-tuple (t-id 1:a) (t-id 1:b)) (untyped) (tuple (int 1) (int 2)))"),
        ] {
            let c = canonical(src).unwrap();
            assert!(c.contains(needle), "{src:?}: missing {needle} in {c}");
        }
    }

    #[test]
    fn parse_errors_are_reported() {
        assert!(canonical("x = (").is_err());
        assert!(params_digest("emit(").is_err());
    }

    #[test]
    fn digest_format() {
        let d = params_digest("x = 1").unwrap();
        assert!(d.starts_with("ast1:blake3:"), "{d}");
        assert_eq!(d.len(), "ast1:blake3:".len() + 64);
    }
}
