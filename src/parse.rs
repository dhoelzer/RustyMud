//! Recursive-descent parser for the LPC subset — mirrors LDMud's parse.y
//! operator precedence table and declaration grammar.

use std::rc::Rc;

use crate::ast::*;
use crate::lex::{lex, Tok, Spanned};

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

pub struct Parser<'s> {
    toks: Vec<Spanned>,
    pos: usize,
    c: &'s mut Compiled,
}

fn is_type_kw(t: &Tok) -> bool {
    match t {
        Tok::Kw(k) => matches!(
            k.as_str(),
            "int" | "float" | "string" | "mixed" | "mapping" | "object" | "status"
                | "void" | "symbol" | "function" | "closure" | "bytes" | "struct" | "varargs"
        ),
        _ => false,
    }
}

pub fn parse_file(filename: &str, src: &str) -> Result<Compiled, ParseError> {
    let toks = lex(src).map_err(|e| ParseError { line: e.line, msg: e.msg })?;
    let mut c = Compiled::new(filename);
    let mut p = Parser { toks, pos: 0, c: &mut c };
    p.top_level()?;
    Ok(c)
}

impl<'s> Parser<'s> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }
    fn peek3(&self) -> &Tok {
        if self.pos + 2 < self.toks.len() {
            &self.toks[self.pos + 2].0
        } else {
            &Tok::Eof
        }
    }
    fn peek2(&self) -> &Tok {
        if self.pos + 1 < self.toks.len() {
            &self.toks[self.pos + 1].0
        } else {
            &Tok::Eof
        }
    }
    fn line(&self) -> usize {
        self.toks[self.pos].1
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn at_op(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if o == s)
    }
    fn at_kw(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Kw(k) if k == s)
    }
    fn eat_op(&mut self, s: &str) -> bool {
        if self.at_op(s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, s: &str) -> bool {
        if self.at_kw(s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_op(&mut self, s: &str) -> Result<(), ParseError> {
        if self.eat_op(s) {
            Ok(())
        } else {
            Err(ParseError {
                line: self.line(),
                msg: format!("expected '{}' but found {:?}", s, self.peek()),
            })
        }
    }
    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            t => Err(ParseError {
                line: self.line(),
                msg: format!("expected identifier but found {:?}", t),
            }),
        }
    }

    // ---------------- top level ----------------

    fn top_level(&mut self) -> Result<(), ParseError> {
        while !matches!(self.peek(), Tok::Eof) {
            if self.at_kw("inherit") {
                self.bump();
                let path = match self.peek().clone() {
                    Tok::Str(s) => {
                        self.bump();
                        s
                    }
                    _ => return Err(ParseError { line: self.line(), msg: "inherit expects a string".into() }),
                };
                self.expect_op(";")?;
                self.c.inherits.push(path);
                continue;
            }
            self.top_decl()?;
        }
        Ok(())
    }

    fn modifiers(&mut self) -> (Visibility, bool /*nomask*/, bool /*virtual*/, bool /*static kw*/) {
        let mut vis = Visibility::Public;
        let mut nomask = false;
        let mut virt = false;
        let mut static_kw = false;
        loop {
            if self.eat_kw("private") {
                vis = Visibility::Private;
            } else if self.eat_kw("protected") {
                vis = Visibility::Protected;
            } else if self.eat_kw("public") {
                vis = Visibility::Public;
            } else if self.eat_kw("static") {
                vis = Visibility::Static;
                static_kw = true;
            } else if self.eat_kw("nomask") {
                nomask = true;
            } else if self.eat_kw("virtual") {
                virt = true;
            } else {
                break;
            }
        }
        (vis, nomask, virt, static_kw)
    }

    fn type_ann(&mut self) -> Option<String> {
        if let Tok::Kw(k) = self.peek() {
            let k = k.clone();
            if matches!(
                k.as_str(),
                "int" | "float" | "string" | "mixed" | "mapping" | "object" | "status" | "void"
                    | "symbol" | "function" | "closure" | "bytes"
            ) {
                self.bump();
                return Some(k);
            }
        }
        if let Tok::Ident(s) = self.peek() {
            // object types like `object foo` handled above; user struct names: `Foo x`
            let s = s.clone();
            if matches!(self.peek2(), Tok::Ident(_)) {
                self.bump();
                return Some(s);
            }
        }
        None
    }

    /// Decide declaration vs statement/expression at block level.
    fn looks_like_decl(&self) -> bool {
        let mut p = self.pos;
        // skip modifiers
        while p < self.toks.len() {
            if let Tok::Kw(k) = &self.toks[p].0 {
                if matches!(k.as_str(), "private" | "protected" | "public" | "static" | "nomask" | "virtual") {
                    p += 1;
                    continue;
                }
            }
            break;
        }
        if p >= self.toks.len() {
            return false;
        }
        match &self.toks[p].0 {
            Tok::Kw(k) if matches!(k.as_str(), "int" | "float" | "string" | "mixed" | "mapping" | "object" | "status" | "symbol" | "function" | "closure" | "bytes" | "struct") => true,
            Tok::Ident(ty) => {
                // `Foo bar` two idents in a row => declaration
                matches!(self.toks.get(p + 1).map(|t| &t.0), Some(Tok::Ident(_)))
            }
            _ => false,
        }
    }

    fn top_decl(&mut self) -> Result<(), ParseError> {
        let (vis, nomask, virt, _) = self.modifiers();
        // varargs modifier on functions
        let varargs = self.eat_kw("varargs");
        let ret_ty = self.type_ann();
        // function definition? name '('  ...  ')'  '{'
        if let Tok::Ident(name) = self.peek().clone() {
            if matches!(self.peek2(), Tok::Op(o) if o == "(") {
                self.bump(); // name
                self.expect_op("(")?;
                let args = self.param_list()?;
                self.expect_op("{")?;
                let body_line = self.line();
                let body = self.block_stmts()?;
                let body_id = self.c.bodies.len();
                self.c.bodies.push(body);
                let info = FuncDefInfo {
                    name: name.clone(),
                    ret_ty,
                    args,
                    varargs,
                    vis,
                    nomask,
                    virtual_flag: virt,
                    body_id,
                    line: body_line,
                };
                if self.c.func_index.contains_key(&name) {
                    // later definitions (from inherits) fixed up by the loader
                    if let Some(&ix) = self.c.func_index.get(&name) {
                        self.c.funcs[ix] = info.clone();
                    }
                } else {
                    self.c.func_index.insert(name.clone(), self.c.funcs.len());
                    self.c.funcs.push(info);
                }
                return Ok(());
            }
        }
        // variable declaration list
        if ret_ty.is_none() && !matches!(self.peek(), Tok::Ident(_)) {
            return Err(ParseError {
                line: self.line(),
                msg: format!(
                    "unexpected token at top level: {:?} {:?}",
                    self.peek(),
                    self.peek2()
                ),
            });
        }
        loop {
            let is_ptr = self.eat_op("*");
            let name = self.expect_ident()?;
            let _ = is_ptr; // pointer types collapse into the runtime type model
            let init = if self.eat_op("=") {
                Some(self.assign_expr()?)
            } else {
                None
            };
            let nosave = self.eat_kw("nosave") || vis == Visibility::Static;
            self.c.vars.push(VarInit {
                name,
                ty: ret_ty.clone(),
                init,
                vis,
                nosave,
            });
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";")?;
        Ok(())
    }

    fn param_list(&mut self) -> Result<Vec<ArgInfo>, ParseError> {
        let mut args = Vec::new();
        if self.eat_op(")") {
            return Ok(args);
        }
        loop {
            let _ty = self.type_ann();
            let mut vararg_default = None;
            if self.at_ident_any() {
                let name = self.expect_ident()?;
                if self.eat_op("=") {
                    vararg_default = Some(self.assign_expr()?);
                }
                args.push(ArgInfo { name, ty: _ty, default: vararg_default });
            } else if self.eat_op("...") {
                // trailing varargs marker; nothing to bind
            } else {
                return Err(ParseError {
                    line: self.line(),
                    msg: format!("bad parameter {:?}", self.peek()),
                });
            }
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(")")?;
        Ok(args)
    }

    fn at_ident_any(&self) -> bool {
        matches!(self.peek(), Tok::Ident(_))
    }

    // ---------------- statements ----------------

    fn block_stmts(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut v = Vec::new();
        while !self.at_op("}") && !matches!(self.peek(), Tok::Eof) {
            v.push(self.stmt()?);
        }
        if !self.eat_op("}") {
            // if Eof: let caller produce a decent error (already consumed through)
        }
        Ok(v)
    }

    fn block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect_op("{")?;
        self.block_stmts()
    }

    fn stmt(&mut self) -> Result<Stmt, ParseError> {
        if self.eat_op(";") {
            return Ok(Stmt::Empty);
        }
        if self.at_op("{") {
            self.bump();
            return Ok(Stmt::Block(self.block_stmts()?));
        }
        if self.eat_kw("if") {
            self.expect_op("(")?;
            let cond = self.expr_seq()?;
            self.expect_op(")")?;
            let then = self.single_stmt_block()?;
            let els = if self.eat_kw("else") { self.single_stmt_block()? } else { vec![] };
            return Ok(Stmt::If { cond, then, els });
        }
        if self.eat_kw("while") {
            self.expect_op("(")?;
            let cond = self.expr_seq()?;
            self.expect_op(")")?;
            let body = self.single_stmt_block()?;
            return Ok(Stmt::While { cond, body });
        }
        if self.at_kw("do") {
            self.bump();
            let body = self.single_stmt_block()?;
            if !self.eat_kw("while") {
                return Err(ParseError { line: self.line(), msg: "do without while".into() });
            }
            self.expect_op("(")?;
            let cond = self.expr_seq()?;
            self.expect_op(")")?;
            self.expect_op(";")?;
            return Ok(Stmt::DoWhile { cond, body });
        }
        if self.eat_kw("for") {
            self.expect_op("(")?;
            let mut init = Vec::new();
            while !self.at_op(";") {
                if self.looks_like_decl() {
                    init.append(&mut self.decl_list(true)?);
                } else {
                    let e = self.expr_seq()?;
                    init.push(Stmt::Expr(e));
                    if !self.eat_op(",") {
                        break;
                    }
                }
            }
            self.expect_op(";")?;
            let cond = if self.at_op(";") { None } else { Some(self.expr_seq()?) };
            self.expect_op(";")?;
            let mut step = Vec::new();
            while !self.at_op(")") {
                let e = self.expr_seq()?;
                step.push(Stmt::Expr(e));
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(")")?;
            let body = self.single_stmt_block()?;
            return Ok(Stmt::For { init, cond, step, body });
        }
        if self.eat_kw("foreach") {
            self.expect_op("(")?;
            let mut vars = Vec::new();
            loop {
                let _ty = self.type_ann();
                vars.push(self.expect_ident()?);
                if !self.eat_op(",") {
                    break;
                }
            }
            if !(self.at_op(":") || matches!(self.peek(), Tok::Ident(s) if s == "in")) {
                return Err(ParseError { line: self.line(), msg: "foreach: expected ':' or 'in'".into() });
            }
            self.bump();
            let iter = self.assign_expr()?;
            let range_hi = if self.eat_op("..") { Some(self.assign_expr()?) } else { None };
            self.expect_op(")")?;
            let body = self.single_stmt_block()?;
            return Ok(Stmt::Foreach { vars, iter, range_hi, body });
        }
        if self.eat_kw("switch") {
            self.expect_op("(")?;
            let expr = self.expr_seq()?;
            self.expect_op(")")?;
            self.expect_op("{")?;
            let mut cases = Vec::new();
            let mut default = Vec::new();
            let mut cur: Option<(Vec<ExprId>, Vec<Stmt>)> = None;
            while !self.at_op("}") && !matches!(self.peek(), Tok::Eof) {
                if self.eat_kw("case") {
                    let mut labs = Vec::new();
                    loop {
                        labs.push(self.assign_expr()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    self.expect_op(":")?;
                    if let Some(prev) = cur.take() {
                        cases.push(prev);
                    }
                    cur = Some((labs, Vec::new()));
                    continue;
                }
                if self.eat_kw("default") {
                    if let Some(prev) = cur.take() {
                        cases.push(prev);
                    }
                    self.expect_op(":")?;
                    while !self.at_kw("case") && !self.at_kw("default") && !self.at_op("}") {
                        default.push(self.stmt()?);
                    }
                    continue;
                }
                if self.at_kw("break") {
                    if let Some(b) = cur.as_mut() {
                        b.1.push(self.stmt()?);
                    } else {
                        default.push(self.stmt()?);
                    }
                    continue;
                }
                if let Some(b) = cur.as_mut() {
                    b.1.push(self.stmt()?);
                } else {
                    return Err(ParseError { line: self.line(), msg: "statement outside case in switch".into() });
                }
            }
            if let Some(prev) = cur.take() {
                cases.push(prev);
            }
            self.eat_op("}");
            return Ok(Stmt::Switch { expr, cases, default });
        }
        if self.eat_kw("return") {
            if self.eat_op(";") {
                return Ok(Stmt::Return(None));
            }
            let e = self.expr_seq()?;
            self.expect_op(";")?;
            return Ok(Stmt::Return(Some(e)));
        }
        if self.eat_kw("break") {
            self.expect_op(";")?;
            return Ok(Stmt::Break);
        }
        if self.eat_kw("continue") {
            self.expect_op(";")?;
            return Ok(Stmt::Continue);
        }
        if self.looks_like_decl() {
            let ds = self.decl_stmt()?;
            if ds.len() == 1 {
                return Ok(ds.into_iter().next().unwrap());
            }
            // LDMud: comma declarators all bind in the same scope;
            // re-parse per-declaration when the statement position demands one.
            return Ok(Stmt::Block(ds));
        }
        let e = self.expr_seq()?;
        self.expect_op(";")?;
        Ok(Stmt::Expr(e))
    }

    /// Parses a `{ ... }` block or falls back to creating a synthetic scope
    /// for a single nested declaration.
    /// (The borrow checker problem with `Parser<'s>` re-borrow during
    /// expression parsing is handled by consuming `self` as `&mut`.)
    fn single_stmt_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        if self.at_op("{") {
            self.bump();
            return self.block_stmts();
        }
        if self.looks_like_decl() {
            // a lone declaration needs a block so it gets its own scope at runtime
            return self.decl_stmt();
        }
        let s = self.stmt()?;
        Ok(vec![s])
    }

    fn decl_stmt(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let ds = self.decl_list(false)?;
        self.expect_op(";")?;
        Ok(ds)
    }

    /// Declaration list without the trailing `;` (for-init uses the loop's `;`).
    fn decl_list(&mut self, _nosemi: bool) -> Result<Vec<Stmt>, ParseError> {
        let (vis, _nomask, _virt, static_kw) = self.modifiers();
        let ty = self.type_ann();
        let mut out = Vec::new();
        loop {
            let _ptr = self.eat_op("*");
            let name = self.expect_ident()?;
            let init = if self.eat_op("=") { Some(self.assign_expr()?) } else { None };
            let v = VarInit {
                name,
                ty: ty.clone(),
                init,
                vis,
                nosave: self.eat_kw("nosave") || static_kw,
            };
            out.push(Stmt::Decl(v));
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(out)
    }

    // ---------------- expressions ----------------

    fn expr_seq(&mut self) -> Result<ExprId, ParseError> {
        let mut v = vec![self.assign_expr()?];
        while self.eat_op(",") {
            v.push(self.assign_expr()?);
        }
        if v.len() == 1 {
            Ok(v[0])
        } else {
            Ok(self.c.add_expr(Expr::Comma(v)))
        }
    }

    fn assign_expr(&mut self) -> Result<ExprId, ParseError> {
        let lhs = self.ternary()?;
        let (op, _none): (BinOp, Option<ExprId>) = if self.eat_op("=") {
            (BinOp::Assign, None)
        } else if self.eat_op("+=") {
            (BinOp::AddAssign, None)
        } else if self.eat_op("-=") {
            (BinOp::SubAssign, None)
        } else if self.eat_op("*=") {
            (BinOp::MulAssign, None)
        } else if self.eat_op("/=") {
            (BinOp::DivAssign, None)
        } else if self.eat_op("%=") {
            (BinOp::ModAssign, None)
        } else if self.eat_op("&=") {
            (BinOp::AssignBitAnd, None)
        } else if self.eat_op("|=") {
            (BinOp::AssignBitOr, None)
        } else if self.eat_op("^=") {
            (BinOp::AssignBitXor, None)
        } else if self.eat_op("<<=") {
            (BinOp::AssignLsh, None)
        } else if self.eat_op(">>=") {
            (BinOp::AssignRsh, None)
        } else {
            return Ok(lhs);
        };
        let _ = _none;
        let rhs_e = self.assign_expr()?;
        Ok(self.c.add_expr(Expr::Bin(op, lhs, rhs_e)))
    }

    fn ternary(&mut self) -> Result<ExprId, ParseError> {
        let c = self.binary(0)?;
        if self.eat_op("?") {
            let a = self.assign_expr()?;
            self.expect_op(":")?;
            let b = self.assign_expr()?;
            return Ok(self.c.add_expr(Expr::Ternary(c, a, b)));
        }
        Ok(c)
    }

    // table-driven binary precedence (LPC)
    fn binary(&mut self, min_prec: u8) -> Result<ExprId, ParseError> {
        let mut lhs = self.unary()?;
        loop {
            let (op, prec) = match self.peek() {
                Tok::Op(o) => match o.as_str() {
                    "||" => (BinOp::Or, 1),
                    "&&" => (BinOp::And, 2),
                    "|" => (BinOp::BitOr, 3),
                    "^" => (BinOp::BitXor, 4),
                    "&" => (BinOp::BitAnd, 5),
                    "==" | "!=" => (if self.at_op("==") { BinOp::Eq } else { BinOp::Ne }, 6),
                    "<" | ">" | "<=" | ">=" => {
                        let op = match o.as_str() {
                            "<" => BinOp::Lt,
                            ">" => BinOp::Gt,
                            "<=" => BinOp::Le,
                            _ => BinOp::Ge,
                        };
                        (op, 7)
                    }
                    "<<" | ">>" => (if self.at_op("<<") { BinOp::Lsh } else { BinOp::Rsh }, 8),
                    "+" | "-" => (if self.at_op("+") { BinOp::Add } else { BinOp::Sub }, 9),
                    "*" | "/" | "%" => (
                        match o.as_str() {
                            "*" => BinOp::Mul,
                            "/" => BinOp::Div,
                            _ => BinOp::Mod,
                        },
                        10,
                    ),
                    _ => break,
                },
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            self.bump();
            let rhs = self.binary(prec + 1)?;
            lhs = self.c.add_expr(Expr::Bin(op, lhs, rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<ExprId, ParseError> {
        // cast expressions: (int) x, (string) x ...
        if self.at_op("(")
            && matches!(self.peek2(), Tok::Kw(k) if matches!(k.as_str(),
                "int"|"float"|"string"|"mixed"|"mapping"|"object"|"status"|"symbol"|"function"|"closure"|"bytes"|"void"))
            && matches!(self.peek3(), Tok::Op(o) if o == ")")
        {
            self.bump();
            let ty = match self.bump() {
                Tok::Kw(k) => k,
                _ => unreachable!(),
            };
            self.expect_op(")")?;
            let e = self.unary()?;
            return Ok(self.c.add_expr(Expr::Cast(ty, e)));
        }
        if self.eat_op("!") {
            let e = self.unary()?;
            return Ok(self.c.add_expr(Expr::Pre(UnOp::LNot, e)));
        }
        if self.eat_op("-") {
            // constant folding happens at eval; but fold int literals eagerly
            let e = self.unary()?;
            return Ok(self.c.add_expr(Expr::Pre(UnOp::Neg, e)));
        }
        if self.eat_op("+") {
            return self.unary();
        }
        if self.eat_op("~") {
            let e = self.unary()?;
            return Ok(self.c.add_expr(Expr::Pre(UnOp::BNot, e)));
        }
        if self.eat_op("++") {
            let e = self.unary()?;
            return Ok(self.c.add_expr(Expr::Pre(UnOp::PreIncr, e)));
        }
        if self.eat_op("--") {
            let e = self.unary()?;
            return Ok(self.c.add_expr(Expr::Pre(UnOp::PreDecr, e)));
        }
        if self.eat_kw("catch") {
            self.expect_op("(")?;
            let e = self.expr_seq()?;
            self.expect_op(")")?;
            return Ok(self.c.add_expr(Expr::Catch(e)));
        }
        if self.eat_kw("throw") {
            // `throw e` without parens too
            let e = self.unary()?;
            return Ok(self.c.add_expr(Expr::Throw(e)));
        }
        // array/mapping literals are primaries too: after building them the
        // ordinary postfix chain ([idx], (args), ->lfun) must still apply.
        if self.at_op("(") && matches!(self.peek2(), Tok::Op(o) if o == "{") {
            self.bump();
            self.bump();
            let lit = self.array_lit()?;
            return self.postfix_apply(lit);
        }
        if self.at_op("(") && matches!(self.peek2(), Tok::Op(o) if o == "[") {
            self.bump();
            self.bump();
            let lit = self.mapping_lit()?;
            return self.postfix_apply(lit);
        }

        if self.at_op("lambda") {
            // bare identifier used by lambda() efun — nothing special here.
        }
        let e = self.postfix()?;
        // postfix ++/--
        if self.eat_op("++") {
            return Ok(self.c.add_expr(Expr::Post(UnOp::PostIncr, e)));
        }
        if self.eat_op("--") {
            return Ok(self.c.add_expr(Expr::Post(UnOp::PostDecr, e)));
        }
        Ok(e)
    }

    fn close_seq(&mut self, a: &str, b: &str) -> bool {
        if self.at_op(a) && matches!(self.peek2(), Tok::Op(o) if o == b) {
            self.bump();
            self.bump();
            true
        } else {
            false
        }
    }

    fn array_lit(&mut self) -> Result<ExprId, ParseError> {
        let mut v = Vec::new();
        loop {
            if self.close_seq("}", ")") {
                break;
            }
            v.push(self.assign_expr()?);
            if !self.eat_op(",") {
                if !self.close_seq("}", ")") {
                    return Err(ParseError {
                        line: self.line(),
                        msg: format!("in array literal, expected , or }}) but found {:?}", self.peek()),
                    });
                }
                break;
            }
        }
        Ok(self.c.add_expr(Expr::ArrayLit(v)))
    }

    fn mapping_lit(&mut self) -> Result<ExprId, ParseError> {
        let mut v = Vec::new();
        loop {
            if self.close_seq("]", ")") {
                break;
            }
            let k = self.assign_expr()?;
            if !self.eat_op(":") {
                return Err(ParseError { line: self.line(), msg: "mapping literal: expected ':'".into() });
            }
            let val = self.assign_expr()?;
            v.push((k, val));
            // separator: , or ; optional before terminator
            if !(self.eat_op(",") || self.eat_op(";")) {
                if !self.close_seq("]", ")") {
                    return Err(ParseError {
                        line: self.line(),
                        msg: format!("in mapping literal, expect separator but found {:?}", self.peek()),
                    });
                }
                break;
            }
        }
        Ok(self.c.add_expr(Expr::MappingLit(v)))
    }

    #[allow(dead_code)]
    fn postfix(&mut self) -> Result<ExprId, ParseError> {
        let e = self.primary()?;
        self.postfix_apply(e)
    }

    fn postfix_apply(&mut self, mut e: ExprId) -> Result<ExprId, ParseError> {
        loop {
            if self.eat_op("[") {
                let idx = self.expr_seq()?;
                if self.eat_op("..") {
                    let b = if self.at_op("]") { None } else { Some(self.expr_seq()?) };
                    self.expect_op("]")?;
                    e = self.c.add_expr(Expr::Slice(e, idx, b));
                } else if self.eat_op("...") {
                    self.expect_op("]")?;
                    e = self.c.add_expr(Expr::Slice(e, idx, None));
                } else {
                    self.expect_op("]")?;
                    e = self.c.add_expr(Expr::Index(e, idx));
                }
                continue;
            }
            if self.at_op("(") {
                // direct call on a primary that was a (#') closure value or an identifier;
                // handled in primary for identifier calls, so this is a computed call.
                self.bump();
                let args = self.args()?;
                e = self.c.add_expr(Expr::Call { name: Some(e), args, inherit_path: None });
                continue;
            }
            if self.eat_op("->") {
                // ob->name(args) | ob->(closure args) | ob->name (implicit)
                if self.at_op("(") {
                    self.bump();
                    let callee = self.assign_expr()?; // closure expression
                    self.expect_op(")")?;
                    let args = if self.at_op("(") {
                        self.bump();
                        self.args()?
                    } else {
                        Vec::new()
                    };
                    e = self.c.add_expr(Expr::CallOther { ob: e, name: Some(callee), lfun_name: None, args });
                    continue;
                }
                let name = match self.peek().clone() {
                    Tok::Ident(s) => {
                        self.bump();
                        s
                    }
                    t => return Err(ParseError { line: self.line(), msg: format!("after -> expected lfun name, got {:?}", t) }),
                };
                let args = if self.eat_op("(") { self.args()? } else { Vec::new() };
                e = self.c.add_expr(Expr::CallOther { ob: e, name: None, lfun_name: Some(name), args });
                continue;
            }
            if self.eat_op("::") {
                // path:: or inherit:: — accept and treat like a normal (direct) call for now
                let mut iname = String::new();
                if let Tok::Ident(s) = self.peek().clone() {
                    if !self.at_path_double() {
                        iname = s;
                    }
                }
                let name = match self.peek().clone() {
                    Tok::Ident(s) => {
                        self.bump();
                        s
                    }
                    t => return Err(ParseError { line: self.line(), msg: format!("after :: expected name, got {:?}", t) }),
                };
                let args = if self.eat_op("(") { self.args()? } else { Vec::new() };
                let qid = self.c.add_expr(Expr::QuotedIdent(name.clone()));
                e = self.c.add_expr(Expr::Call {
                    name: Some(qid),
                    args,
                    inherit_path: if iname.is_empty() { None } else { Some(iname) },
                });
                continue;
            }
            break;
        }
        Ok(e)
    }

    fn at_path_double(&self) -> bool {
        matches!(self.peek(), Tok::Op(o) if o == "::")
    }

    fn inline_closure(&mut self) -> Result<ExprId, ParseError> {
        // (: [type name, ... :] expr :)  -- declared param names become aliases $1..$n
        self.bump(); // (:"
        // try to parse a parameter list terminated by Op(":") followed by expr until ":)"
        let mut params: Vec<String> = Vec::new();
        // heuristic: parameter list if we see ident ':' before ':)'
        let save = self.pos;
        let mut ok = true;
        loop {
            let _ = self.type_ann();
            match self.peek().clone() {
                Tok::Ident(n) => {
                    self.bump();
                    params.push(n);
                }
                _ => { ok = false; break; }
            }
            if self.eat_op(",") {
                continue;
            }
            if self.at_op(":") {
                self.bump();
                break;
            }
            ok = false;
            break;
        }
        if !ok {
            params.clear();
            self.pos = save;
        }
        let e = self.expr_seq()?;
        if !self.eat_op(":)") {
            return Err(ParseError { line: self.line(), msg: "inline closure: expected ':)'".into() });
        }
        let body = vec![Stmt::Return(Some(e))];
        let body_id = self.c.bodies.len();
        self.c.bodies.push(body);
        // record param aliases so the closure applies them
        for (i, p) in params.iter().enumerate() {
            self.c.inline_params.insert(body_id, self.c.inline_params.get(&body_id).cloned().unwrap_or_default());
            let v = self.c.inline_params.get_mut(&body_id).unwrap();
            v.push((p.clone(), i + 1));
        }
        // nargs: named params if present, else highest $N referenced in the body
        let mut nargs = params.len();
        if nargs == 0 {
            for ex in self.c.bodies[body_id].iter() {
                if let Stmt::Return(Some(e)) = ex {
                    nargs = nargs.max(dollar_rank(&self.c, *e));
                }
            }
        }
        Ok(self.c.add_expr(Expr::ClosureInline { body_id, nargs }))
    }

    fn args(&mut self) -> Result<Vec<ExprId>, ParseError> {
        let mut args = Vec::new();
        if self.eat_op(")") {
            return Ok(args);
        }
        loop {
            args.push(self.assign_expr()?);
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(")")?;
        Ok(args)
    }

    fn primary(&mut self) -> Result<ExprId, ParseError> {
        match self.peek().clone() {
            Tok::Int(v) => {
                let n1 = v;
                if let Tok::Int(w) = self.peek2().clone() {
                    // juxtaposed literal concatenation ("ab" "cd" style number adj: rare in LPC)
                    self.bump();
                    self.bump();
                    let joined = format!("{}{}", n1, w);
                    return Ok(self.c.add_expr(Expr::StrConst(Rc::from(joined.as_str()))));
                }
                self.bump();
                Ok(self.c.add_expr(Expr::IntConst(n1)))
            }
            Tok::Float(v) => {
                self.bump();
                Ok(self.c.add_expr(Expr::FloatConst(v)))
            }
            Tok::Str(s) => {
                self.bump();
                // juxtaposed string literals concatenate
                let mut s = s;
                while let Tok::Str(t) = self.peek().clone() {
                    self.bump();
                    s.push_str(&t);
                }
                Ok(self.c.add_expr(Expr::StrConst(s.into())))
            }
            Tok::Op(o) if o == "(" => {
                self.bump();
                let e = self.expr_seq()?;
                self.expect_op(")")?;
                Ok(e)
            }
            Tok::Op(o) if o == "(:" => {
                return self.inline_closure();
            }
            Tok::Op(o) if o == "#'" => {
                self.bump();
                let name = self.expect_ident()?;
                Ok(self.c.add_expr(Expr::ClosureFunc { name }))
            }
            Tok::Ident(name) => {
                self.bump();
                // cast: (int)expr is handled in unary? casts appear as (int) prefix —
                // handle `(type) cast` in postfix? Actually cast happens at unary stage.
                // lambda string form: handled through the lambda()/bind_lambda() efuns.
                if self.eat_op("(") {
                    let args = self.args()?;
                    return Ok(self.c.add_expr(Expr::DirectCall { name, args }));
                }
                Ok(self.c.add_expr(Expr::Ident(name)))
            }
            t => Err(ParseError {
                line: self.line(),
                msg: format!("unexpected token in expression: {:?}", t),
            }),
        }
    }
}

/// Cast support: `(type) expr` — we scan after `(` in primary? Simpler: preprocess in unary.
pub fn parse_expr_string(src: &str) -> Result<(Compiled, usize), ParseError> {
    let c = parse_file("<expr>", &format!("mixed __e__() {{ return ({}); }}", src))?;
    let entry = c.func_index["__e"];
    Ok((c, entry))
}

/// Highest $N positional parameter referenced by an expression tree.
fn dollar_rank(c: &crate::ast::Compiled, e: ExprId) -> usize {
    fn walk(c: &crate::ast::Compiled, e: ExprId, seen: &mut Vec<usize>) -> usize {
        if seen.contains(&e) {
            return 0;
        }
        seen.push(e);
        let x = match c.exprs.get(e) {
            Some(x) => x.clone(),
            None => return 0,
        };
        let sub: Vec<ExprId> = match &x {
            Expr::Ident(n) => {
                if let Some(rest) = n.strip_prefix('$') {
                    return rest.parse::<usize>().unwrap_or(0);
                }
                Vec::new()
            }
            Expr::ArrayLit(v) => v.clone(),
            Expr::MappingLit(v) => v.iter().flat_map(|(a, b)| [*a, *b]).collect(),
            Expr::Pre(_, a) | Expr::Post(_, a) => vec![*a],
            Expr::Bin(_, a, b) => vec![*a, *b],
            Expr::Ternary(a, b, cc) => vec![*a, *b, *cc],
            Expr::Index(a, b) => vec![*a, *b],
            Expr::Slice(a, b, c2) => {
                let mut v = vec![*a, *b];
                if let Some(x) = c2 {
                    v.push(*x);
                }
                v
            }
            Expr::Call { name, args, .. } => name.iter().copied().chain(args.iter().copied()).collect(),
            Expr::DirectCall { args, .. } => args.clone(),
            Expr::CallOther { ob, name, args, .. } => {
                vec![*ob].into_iter().chain(name.iter().copied()).chain(args.iter().copied()).collect()
            }
            Expr::Cast(_, a) => vec![*a],
            Expr::Catch(a) => vec![*a],
            Expr::Throw(a) => vec![*a],
            Expr::Comma(v) => v.clone(),
            _ => Vec::new(),
        };
        let mut m = 0;
        for s2 in sub {
            m = m.max(walk(c, s2, seen));
        }
        m
    }
    walk(c, e, &mut Vec::new())
}
