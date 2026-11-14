//! Expression evaluation — the Rust analogue of the expression part of
//! LDMud's interpret.c. Written as an impl block over Vm.

use std::rc::Rc;

use crate::ast::*;
use crate::efuns::{call_efun, is_efun, obj_id};
use crate::program::Program;
use crate::value::*;
use crate::vm::{Ctrl, EvResult, Vm};

impl Vm {
    pub(crate) fn eval_e(&mut self, prog: &Rc<Program>, e: ExprId) -> EvResult {
        let expr = prog.compiled.exprs[e].clone();
        match expr {
            Expr::IntConst(v) => Ok(Svalue::Int(v)),
            Expr::FloatConst(v) => Ok(Svalue::Float(v)),
            Expr::StrConst(s) => Ok(Svalue::Str(Rc::from(s.as_ref()))),
            Expr::LambdaStr(s) => {
                let lit = self.eval_e(prog, s)?;
                self.compile_lambda(&display_string(&lit))
            }
            Expr::Ident(name) => self.eval_ident(prog, &name),
            Expr::QuotedIdent(name) => Ok(Svalue::Closure(Rc::new(Closure::Func {
                oid: self.this_object(),
                name: Rc::from(name.as_str()),
            }))),
            Expr::ClosureFunc { name } => Ok(Svalue::Closure(Rc::new(Closure::Func {
                oid: self.this_object(),
                name: Rc::from(name.as_str()),
            }))),
            Expr::ClosureInline { body_id, nargs } => {
                // runtime inline `(: ... :)` closure
                Ok(Svalue::Closure(Rc::new(Closure::Lambda {
                    prog: prog.clone(),
                    body_id,
                    nargs,
                })))
            }
            Expr::ArrayLit(ids) => {
                let mut v = Vec::with_capacity(ids.len());
                for i in ids {
                    v.push(self.eval_e(prog, i)?);
                }
                Ok(mk_array(v))
            }
            Expr::MappingLit(pairs) => {
                let mut m = Mapping::new();
                for (k, v) in pairs {
                    let kv = self.eval_e(prog, k)?;
                    let vv = self.eval_e(prog, v)?;
                    m.set(kv, vv);
                }
                Ok(Svalue::Mapping(Rc::new(std::cell::RefCell::new(m))))
            }
            Expr::Pre(op, a) => self.eval_pre(prog, op, a),
            Expr::Post(op, a) => self.eval_post(prog, op, a),
            Expr::Bin(op, a, b) => self.eval_bin(prog, op, a, b),
            Expr::Ternary(c, a, b) => {
                let cv = self.eval_e(prog, c)?;
                if truthy(&cv) {
                    self.eval_e(prog, a)
                } else {
                    self.eval_e(prog, b)
                }
            }
            Expr::Index(base, idx) => {
                let bv = self.eval_e(prog, base)?;
                let iv = self.eval_e(prog, idx)?;
                self.index(&bv, &iv)
            }
            Expr::Slice(base, a, b) => {
                let bv = self.eval_e(prog, base)?;
                let av = self.eval_e(prog, a)?;
                let aidx = as_int(&av)?;
                let end = match b {
                    Some(x) => Some(as_int(&self.eval_e(prog, x)?)?),
                    None => None,
                };
                self.slice(&bv, aidx, end)
            }
            Expr::Call { name, args, inherit_path } => {
                self.eval_call(prog, name.as_ref(), inherit_path, &args)
            }
            Expr::DirectCall { name, args } => self.eval_named_call(prog, &name, &args),
            Expr::CallOther { ob, name, lfun_name, args } => {
                let obv = self.eval_e(prog, ob)?;
                let mut argv = Vec::with_capacity(args.len());
                for a in args {
                    argv.push(self.eval_e(prog, a)?);
                }
                let lfun = if let Some(n) = lfun_name {
                    n.clone()
                } else if let Some(n) = name {
                    let nv = self.eval_e(prog, n)?;
                    match nv {
                        Svalue::Str(s) => s.to_string(),
                        Svalue::Closure(_) => {
                            // ob->(closure)(args) unsupported
                            return Err(Ctrl::Error(mk_string(
                                "call other with closure lfun is not supported",
                            )));
                        }
                        _ => return Err(Ctrl::Error(mk_string("call other: bad lfun name"))),
                    }
                } else {
                    return Err(Ctrl::Error(mk_string("call other: missing lfun name")));
                };
                let oid = obj_id(&obv);
                match oid {
                    Some(o) => match self.call_function(o, &lfun, &argv) {
                        // ob->fun() is forgiving when the function is missing
                        Err(Ctrl::Error(msg)) => {
                            let m = display_string(&msg);
                            if m.contains("No function") {
                                Ok(Svalue::Int(0))
                            } else {
                                // propagate other runtime errors
                                self.report_error(&m);
                                Ok(Svalue::Int(0))
                            }
                        }
                        other => other,
                    },
                    None => Ok(Svalue::Int(0)), // call on no-object yields 0
                }
            }
            Expr::Cast(ty, a) => {
                let v = self.eval_e(prog, a)?;
                Ok(self.do_cast(&ty, v))
            }
            Expr::Catch(a) => {
                // catch(expr): errors in expr return the error string; they do
                // not invoke the master's runtime_error hook.
                match self.eval_e(prog, a) {
                    Ok(v) => Ok(v),
                    Err(Ctrl::Error(e)) => Ok(e),
                    Err(c) => Err(c),
                }
            }
            Expr::Throw(a) => {
                let v = self.eval_e(prog, a)?;
                Err(Ctrl::Error(v))
            }
            Expr::Comma(ids) => {
                let mut last = Svalue::Null;
                for i in ids {
                    last = self.eval_e(prog, i)?;
                }
                Ok(last)
            }
        }
    }

    fn do_cast(&mut self, ty: &str, v: Svalue) -> Svalue {
        match ty {
            "int" | "status" => match &v {
                Svalue::Float(f) => Svalue::Int(*f as i64),
                Svalue::Str(_) => Svalue::Int(0),
                Svalue::Object(_) => Svalue::Int(0),
                _ => v.clone(),
            },
            "float" => match &v {
                Svalue::Int(i) => Svalue::Float(*i as f64),
                _ => v.clone(),
            },
            "string" => match &v {
                Svalue::Int(i) => mk_string(i.to_string()),
                _ => v.clone(),
            },
            _ => v,
        }
    }

    fn eval_ident(&mut self, prog: &Rc<Program>, name: &str) -> EvResult {
        // local variable?
        if let Some(f) = self.frames.last_mut() {
            if let Some(i) = f.slot(name) {
                return Ok(f.locals[i].clone());
            }
        }
        // object variable?
        let oid = self.this_object();
        if oid != 0 {
            if let Some(o) = self.env.object(oid) {
                if let Some(ix) = o.borrow().prog.var_ix.get(name).copied() {
                    // clone the content to preserve value semantics? LDMud
                    // returns a reference to the lvalue; we clone for
                    // simplicity except when used as an lvalue (handled in
                    // assign/increment paths).
                    let v = o.borrow().vars[ix].clone();
                    return Ok(v);
                }
            }
        }
        Err(Ctrl::Error(mk_string(format!(
            "Undefined variable: {}",
            name
        ))))
    }

    // ----- lvalues -----

    fn locate_lvalue(&mut self, prog: &Rc<Program>, e: ExprId) -> Result<Option<LV>, Ctrl> {
        let ex = prog.compiled.exprs[e].clone();
        match ex {
            Expr::Ident(name) => {
                if self.frames.last().map(|f| f.local_ix.contains_key(&name)).unwrap_or(false) {
                    Ok(Some(LV::Local(name)))
                } else {
                    let oid = self.this_object();
                    if oid != 0 {
                        if let Some(o) = self.env.object(oid) {
                            if let Some(&ix) = o.borrow().prog.var_ix.get(&name) {
                                return Ok(Some(LV::Global(ix as u16)));
                            }
                        }
                    }
                    Ok(None)
                }
            }
            Expr::Index(b, i) => {
                let bv = self.eval_e(prog, b)?;
                let iv = self.eval_e(prog, i)?;
                Ok(Some(LV::Indexed(bv, iv)))
            }
            _ => Ok(None),
        }
    }

    fn store_lvalue(&mut self, lv: &LV, v: Svalue) -> EvResult {
        match lv {
            LV::Local(name) => {
                let fi = self.frames.len() - 1;
                if let Some(ix) = self.frames[fi].local_ix.get(name).copied() {
                    self.frames[fi].locals[ix] = v.clone();
                    Ok(v)
                } else {
                    Err(Ctrl::Error(mk_string("illegal assignment to local slot")))
                }
            }
            LV::Global(ix) => {
                let oid = self.this_object();
                if oid == 0 {
                    return Err(Ctrl::Error(mk_string("illegal assignment; no current object")));
                }
                let obj = self.env.object(oid).unwrap().clone();
                obj.borrow_mut().vars[*ix as usize] = v.clone(); // shallow store; deep handled by caller clone
                Ok(v)
            }
            LV::Indexed(base, idx) => {
                self.index_store(base, idx, v.clone())?;
                Ok(v)
            }
        }
    }

    fn read_located(&mut self, lv: &LV) -> EvResult {
        match lv {
            LV::Local(name) => {
                let ix = self.frames.last().unwrap().local_ix.get(name).copied().unwrap();
                Ok(self.frames.last().unwrap().locals[ix].clone())
            }
            LV::Global(ix) => {
                let oid = self.this_object();
                let obj = self.env.object(oid).unwrap();
                Ok(obj.borrow().vars[*ix as usize].clone())
            }
            LV::Indexed(base, idx) => self.index(base, idx),
        }
    }

    fn assign(&mut self, prog: &Rc<Program>, lhs: ExprId, rhs: Svalue) -> EvResult {
        match self.locate_lvalue(prog, lhs)? {
            Some(lv) => self.store_lvalue(&lv, rhs),
            None => Err(Ctrl::Error(mk_string("illegal lvalue in assignment"))),
        }
    }

    // ----- indexing -----

    pub(crate) fn index(&mut self, base: &Svalue, idx: &Svalue) -> EvResult {
        match (base, idx) {
            (Svalue::Array(a), Svalue::Int(i)) => {
                let a = a.borrow();
                if *i < 0 || (*i as usize) >= a.len() {
                    Err(Ctrl::Error(mk_string(format!(
                        "Index {} out of range over array of size {}",
                        i,
                        a.len()
                    ))))
                } else {
                    Ok(a[*i as usize].clone())
                }
            }
            (Svalue::Array(a), k) => {
                // search for element, return index
                let a = a.borrow();
                let pos = a.iter().position(|e| eq(e, k));
                Ok(Svalue::Int(pos.map(|p| p as i64).unwrap_or(-1)))
            }
            (Svalue::Str(s), Svalue::Int(i)) => {
                let chars: Vec<char> = s.chars().collect();
                if *i < 0 || (*i as usize) >= chars.len() {
                    Err(Ctrl::Error(mk_string(format!(
                        "Index {} out of range over string of size {}",
                        i,
                        chars.len()
                    ))))
                } else {
                    Ok(Svalue::Int(chars[*i as usize] as i64))
                }
            }
            (Svalue::Mapping(m), k) => match m.borrow().get(k) {
                Some(v) => Ok(v),
                None => Ok(Svalue::Int(0)),
            },
            (Svalue::Object(o), Svalue::Int(_)) => {
                // object[int] indexes inventories — simplified to 0
                let _ = o;
                Ok(Svalue::Int(0))
            }
            (b, i) => Err(Ctrl::Error(mk_string(format!(
                "Indexing illegal on {} with {}",
                dbg(b),
                dbg(i)
            )))),
        }
    }

    fn index_store(&mut self, base: &Svalue, idx: &Svalue, v: Svalue) -> EvResult {
        match (base, idx) {
            (Svalue::Array(a), Svalue::Int(i)) => {
                let mut a = a.borrow_mut();
                if *i < 0 || (*i as usize) >= a.len() {
                    return Err(Ctrl::Error(mk_string(format!(
                        "Index {} out of range in assignment over array of size {}",
                        i,
                        a.len()
                    ))));
                }
                a[*i as usize] = v;
                Ok(Svalue::Null)
            }
            (Svalue::Mapping(m), k) => {
                m.borrow_mut().set(k.clone(), v);
                Ok(Svalue::Null)
            }
            (b, i) => Err(Ctrl::Error(mk_string(format!(
                "Illegal assignment to index of {} (index {})",
                dbg(b),
                dbg(i)
            )))),
        }
    }

    fn slice(&mut self, base: &Svalue, a: i64, end: Option<i64>) -> EvResult {
        let (lo, hi) = norm_range(base, a, end);
        match base {
            Svalue::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                if lo>=0 && hi >= 0 && (lo as usize) < (hi as usize) && (hi as usize) <= chars.len() {
                    let t: String = chars[lo as usize..hi as usize].iter().collect();
                    Ok(mk_string(t))
                } else {
                    Ok(mk_string(escape_str(s)))
                }
            }
            Svalue::Array(arr) => {
                let a = arr.borrow();
                if lo >= 0 && hi >= 0 && (lo as usize) <= (hi as usize) && (hi as usize) <= a.len() {
                    Ok(mk_array(a[lo as usize..hi as usize].to_vec()))
                } else {
                    Ok(base.clone())
                }
            }
            Svalue::Mapping(m) => {
                // mapping[range] selects keys
                let m2 = m.borrow();
                let out: Vec<(Svalue, Svalue)> = m2
                    .items()
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i >= lo as usize && *i < hi as usize)
                    .map(|(_, (k, v))| (k.clone(), v.clone()))
                    .collect();
                Ok(mk_mapping(out))
            }
            _ => Err(Ctrl::Error(mk_string(format!(
                "Slice illegal on {}",
                type_of(base)
            )))),
        }
    }

    // ----- operators -----

    fn eval_pre(&mut self, prog: &Rc<Program>, op: UnOp, a: ExprId) -> EvResult {
        match op {
            UnOp::Neg => {
                let v = self.eval_e(prog, a)?;
                match v {
                    Svalue::Int(i) => Ok(Svalue::Int(-i)),
                    Svalue::Float(f) => Ok(Svalue::Float(-f)),
                    x => Err(Ctrl::Error(mk_string(format!("Illegal type ({}) for unary '-'", type_of(&x))))),
                }
            }
            UnOp::Plus => self.eval_e(prog, a),
            UnOp::LNot => {
                let v = self.eval_e(prog, a)?;
                Ok(Svalue::Int(if truthy(&v) { 0 } else { 1 }))
            }
            UnOp::BNot => {
                let v = self.eval_e(prog, a)?;
                Ok(Svalue::Int(!as_int(&v)?))
            }
            UnOp::PreIncr | UnOp::PreDecr => {
                if let Some(lv) = self.locate_lvalue(prog, a)? {
                    let cur = self.read_located(&lv)?;
                    let s = add_values(&cur, if op == UnOp::PreIncr { 1 } else { -1 })?;
                    self.store_lvalue(&lv, s.clone())?;
                    Ok(s)
                } else {
                    Err(Ctrl::Error(mk_string("Illegal operand for ++/--")))
                }
            }
            UnOp::PostIncr | UnOp::PostDecr => {
                if let Some(lv) = self.locate_lvalue(prog, a)? {
                    let cur = self.read_located(&lv)?;
                    let s = add_values(&cur, if op == UnOp::PostIncr { 1 } else { -1 })?;
                    self.store_lvalue(&lv, s)?;
                    Ok(cur)
                } else {
                    Err(Ctrl::Error(mk_string("Illegal operand for ++/--")))
                }
            }
        }
    }

    fn eval_post(&mut self, prog: &Rc<Program>, op: UnOp, a: ExprId) -> EvResult {
        // parser may emit Post directly
        self.eval_pre(
            prog,
            match op {
                UnOp::PostIncr => UnOp::PostIncr,
                UnOp::PostDecr => UnOp::PostDecr,
                o => o,
            },
            a,
        )
    }

    fn eval_bin(&mut self, prog: &Rc<Program>, op: BinOp, a: ExprId, b: ExprId) -> EvResult {
        use BinOp::*;
        // short circuit
        match op {
            And => {
                let av = self.eval_e(prog, a)?;
                if !truthy(&av) {
                    return Ok(Svalue::Int(0));
                }
                let bv = self.eval_e(prog, b)?;
                return Ok(Svalue::Int(if truthy(&bv) { 1 } else { 0 }));
            }
            Or => {
                let av = self.eval_e(prog, a)?;
                if truthy(&av) {
                    return Ok(Svalue::Int(1));
                }
                let bv = self.eval_e(prog, b)?;
                return Ok(Svalue::Int(if truthy(&bv) { 1 } else { 0 }));
            }
            Assign => {
                let rhs = self.eval_e(prog, b)?;
                return self.assign(prog, a, rhs);
            }
            AddAssign | SubAssign | MulAssign | DivAssign | ModAssign | AssignBitAnd
            | AssignBitOr | AssignBitXor | AssignLsh | AssignRsh => {
                let rhs = self.eval_e(prog, b)?;
                if let Some(lv) = self.locate_lvalue(prog, a)? {
                    let cur = self.read_located(&lv)?;
                    let inner = match op {
                        AddAssign => Add,
                        SubAssign => Sub,
                        MulAssign => Mul,
                        DivAssign => Div,
                        ModAssign => Mod,
                        AssignBitAnd => BitAnd,
                        AssignBitOr => BitOr,
                        AssignBitXor => BitXor,
                        AssignLsh => Lsh,
                        _ => Rsh,
                    };
                    let merged = self.binop(prog, inner, cur, rhs)?;
                    return self.store_lvalue(&lv, merged.clone());
                }
                return Err(Ctrl::Error(mk_string("illegal lvalue in compound assignment")));
            }
            _ => {}
        }
        let av = self.eval_e(prog, a)?;
        let bv = self.eval_e(prog, b)?;
        self.binop(prog, op, av, bv)
    }

    fn binop(&mut self, _prog: &Rc<Program>, op: BinOp, a: Svalue, b: Svalue) -> EvResult {
        use BinOp::*;
        match op {
            Add => add2(&a, &b).map_err(serr).map(|v| deep_copy(&v)),
            Sub => sub2(&a, &b).map_err(serr).map(|v| deep_copy(&v)),
            Mul => mul2(&a, &b) .map_err(serr),
            Div => match (&a, &b) {
                (Svalue::Int(x), Svalue::Int(y)) => {
                    if *y == 0 {
                        return Err(Ctrl::Error(mk_string("Division by zero")));
                    }
                    Ok(Svalue::Int(x / y))
                }
                _ => {
                    let x = as_float(&a).map_err(serr)?;
                    let y = as_float(&b).map_err(serr)?;
                    if y == 0.0 {
                        return Err(Ctrl::Error(mk_string("Division by zero")));
                    }
                    // LPC: int/int = int; if either float => float
                    if matches!(a, Svalue::Int(_)) && matches!(b, Svalue::Int(_)) {
                        Ok(Svalue::Int((x / y) as i64))
                    } else {
                        Ok(Svalue::Float(x / y))
                    }
                }
            },
            Mod => {
                let x = as_int(&a).map_err(serr)?;
                let y = as_int(&b).map_err(serr)?;
                if y == 0 {
                    return Err(Ctrl::Error(mk_string("Modulo by zero")));
                }
                Ok(Svalue::Int(x % y))
            }
            Eq => Ok(Svalue::Int(if eq(&a, &b) { 1 } else { 0 })),
            Ne => Ok(Svalue::Int(if eq(&a, &b) { 0 } else { 1 })),
            Lt | Gt | Le | Ge => {
                let c = cmp(&a, &b);
                let v = match op {
                    Lt => c == std::cmp::Ordering::Less,
                    Gt => c == std::cmp::Ordering::Greater,
                    Le => c != std::cmp::Ordering::Greater,
                    _ => c != std::cmp::Ordering::Less,
                };
                Ok(Svalue::Int(if v { 1 } else { 0 }))
            }
            BitAnd | BitOr | BitXor | Lsh | Rsh => {
                let x = as_int(&a).map_err(serr)?;
                let y = as_int(&b).map_err(serr)?;
                Ok(Svalue::Int(match op {
                    BitAnd => x & y,
                    BitOr => x | y,
                    BitXor => x ^ y,
                    Lsh => x.wrapping_shl(y as u32),
                    _ => x.wrapping_shr(y as u32),
                }))
            }
            _ => Err(Ctrl::Error(mk_string("internal: unexpected operator"))),
        }
    }

    // ----- calls -----

    fn eval_call(
        &mut self,
        prog: &Rc<Program>,
        name: Option<&ExprId>,
        inherit_path: Option<String>,
        args: &[ExprId],
    ) -> EvResult {
        // computed call: closure value
        let direct_name: Option<String> = if let Some(n) = name {
            match &prog.compiled.exprs[*n] {
                Expr::QuotedIdent(s) => Some(s.clone()),
                _ => None,
            }
        } else {
            None
        };
        if let Some(n) = &direct_name {
            // inherit::name / path::name form
            let mut argv = Vec::with_capacity(args.len());
            for a in args {
                argv.push(self.eval_e(prog, *a)?);
            }
            if is_efun(n) {
                return call_efun(self, n, &argv);
            }
            let oid = self.this_object();
            let obj = self.env.object(oid);
            let has_global = obj
                .as_ref()
                .map(|o| o.borrow().prog.lookup_func(n).is_some())
                .unwrap_or(false);
            if has_global {
                return self.call_function(oid, n, &argv);
            }
            let _ = inherit_path;
            return Err(Ctrl::Error(mk_string(format!("No such function {}()", n))));
        }
        if let Some(n) = name {
            // Call computed value: (expr)(args)
            let cv = self.eval_e(prog, *n)?;
            let mut argv = Vec::with_capacity(args.len());
            for a in args {
                argv.push(self.eval_e(prog, *a)?);
            }
            return match cv {
                Svalue::Closure(_) => self.apply_closure(&cv, &argv),
                Svalue::Str(s) => {
                    let oid = self.this_object();
                    self.call_function(oid, &s, &argv)
                }
                _ => Err(Ctrl::Error(mk_string(format!(
                    "Tried to call non-closure {}",
                    dbg(&cv)
                )))),
            };
        }
        // unqualified identifier call: resolve ident at call position
        // (parser already made this call with name=None when ident followed
        // '(' — we lose the identifier if not re-scan; handle by storing the
        // identifier in Expr::QuotedIdent via a fallback below.)
        Err(Ctrl::Error(mk_string("Internal: call without callee")))
    }

    pub(crate) fn eval_named_call(
        &mut self,
        prog: &Rc<Program>,
        name: &str,
        args: &[ExprId],
    ) -> EvResult {
        // sscanf: destinations are lvalues, assigned by the parse.
        if name == "sscanf" {
            if args.len() < 2 {
                return Err(Ctrl::Error(mk_string("sscanf: too few arguments")));
            }
            let s = as_string(&self.eval_e(prog, args[0])?).map_err(serr)?;
            let fmt = as_string(&self.eval_e(prog, args[1])?).map_err(serr)?;
            let mut dests: Vec<Option<LV>> = Vec::new();
            for a in &args[2..] {
                dests.push(self.locate_lvalue(prog, *a)?);
            }
            let vals = sscanf_parse(&s, &fmt, dests.len());
            let n = vals.len() as i64;
            for (d, v) in dests.into_iter().zip(vals) {
                if let Some(lv) = d {
                    self.store_lvalue(&lv, v)?;
                }
            }
            return Ok(Svalue::Int(n));
        }
        // lambda()/bind_lambda string compilation
        if name == "bind_lambda" || name == "lambda" || name == "unbound_lambda" {
            if args.len() < 2 {
                return Err(Ctrl::Error(mk_string("lambda: too few arguments")));
            }
            let src = as_string(&self.eval_e(prog, args[args.len()-1])?).map_err(serr)?;
            let src = src.to_string();
            return self.compile_lambda(&src);
        }
        let mut argv: Vec<Svalue> = Vec::with_capacity(args.len());
        for a in args {
            argv.push(self.eval_e(prog, *a)?);
        }
        // efun?
        if is_efun(name) {
            return call_efun(self, name, &argv);
        }
        if name == "bind_lambda" {
            return Err(Ctrl::Error(mk_string("bind_lambda: lambda compilation unsupported")));
        }
        // sefun (simul efun)?
        if self.env.sefun != 0 {
            if let Some(sef) = self.env.object(self.env.sefun) {
                if sef.borrow().prog.lookup_func(name).is_some() {
                    return self.call_function(self.env.sefun, name, &argv);
                }
            }
        }
        // local or global function in this object?
        let oid = self.this_object();
        if oid != 0 {
            if let Some(o) = self.env.object(oid) {
                if o.borrow().prog.lookup_func(name).is_some() {
                    return self.call_function(oid, name, &argv);
                }
            }
        }
        // a variable (local or global) holding a closure: direct call applies it
        let by_name = {
            let mut found: Option<Svalue> = None;
            if let Some(f) = self.frames.last() {
                if let Some(&ix) = f.local_ix.get(name) {
                    found = Some(f.locals[ix].clone());
                }
            }
            if found.is_none() {
                let oid = self.this_object();
                if oid != 0 {
                    if let Some(o) = self.env.object(oid) {
                        if let Some(&ix) = o.borrow().prog.var_ix.get(name) {
                            found = Some(o.borrow().vars[ix as usize].clone());
                        }
                    }
                }
            }
            found
        };
        if let Some(v) = by_name {
            if matches!(v, Svalue::Closure(_)) {
                return self.apply_closure(&v, &argv);
            }
        }
        // implicit call_other on command_giver like objects is not done by the
        // driver; unresolved direct calls are runtime errors.
        Err(Ctrl::Error(mk_string(format!(
            "No function {} in the current object and it's no efun\n",
            name
        ))))
    }

    /// compile a lambda source string into an executable closure.
    pub(crate) fn compile_lambda(&mut self, src: &str) -> EvResult {
        // wrap into a function body and parse; reuse the driver's expression parser
        let (prog, entry, arity) = Vm::compile_lambda_unit(&src)
            .map_err(|e| Ctrl::Error(mk_string(format!("Error in lambda program: {}", e))))?;
        Ok(Svalue::Closure(Rc::new(Closure::Lambda { prog, body_id: entry, nargs: arity })))
    }
}

/// LPC sscanf (edrive semantics, simplified): backtracking matcher.
/// %s = greedy-with-backtracking string, %d = integer (optionally float if
/// the token has a fraction), %f float, %* suppresses assignment.
/// Returns the value list in format order; short when a field failed.
pub(crate) fn sscanf_parse(s: &str, fmt: &str, ndest: usize) -> Vec<Svalue> {
    #[derive(Clone, Copy, PartialEq)]
    enum Op {
        S,
        D,
        F,
        T,
        Lit,
    }
    // ---- parse fmt into pieces
    let mut pieces: Vec<(Op, String, bool)> = Vec::new(); // op, payload, store?
    let fc: Vec<char> = fmt.chars().collect();
    let mut i = 0usize;
    let mut lit = String::new();
    while i < fc.len() {
        let c = fc[i];
        if c != '%' {
            lit.push(c);
            i += 1;
            continue;
        }
        i += 1;
        if i < fc.len() && fc[i] == '%' {
            lit.push('%');
            i += 1;
            continue;
        }
        // flags
        let mut store = true;
        while i < fc.len() && matches!(fc[i], '+' | '!' | '~' | '*') {
            match fc[i] {
                '!' => store = false,
                '~' | '*' => store = store && fc[i] != '*',
                _ => {}
            }
            i += 1;
        }
        // modifiers: digits [. digits] (size / minmatch) - accepted, ignored
        while i < fc.len() && (fc[i].is_ascii_digit() || fc[i] == '.') {
            i += 1;
        }
        if i >= fc.len() {
            break;
        }
        let conv = fc[i];
        i += 1;
        match conv {
            's' => {
                if !lit.is_empty() {
                    pieces.push((Op::Lit, std::mem::take(&mut lit), true));
                }
                pieces.push((Op::S, String::new(), store));
            }
            'd' | 'D' | 'u' | 'U' => {
                if !lit.is_empty() {
                    pieces.push((Op::Lit, std::mem::take(&mut lit), true));
                }
                pieces.push((Op::D, String::new(), store));
            }
            'f' => {
                if !lit.is_empty() {
                    pieces.push((Op::Lit, std::mem::take(&mut lit), true));
                }
                pieces.push((Op::F, String::new(), store));
            }
            't' => {
                if !lit.is_empty() {
                    pieces.push((Op::Lit, std::mem::take(&mut lit), true));
                }
                pieces.push((Op::T, String::new(), false));
            }
            'O' | 'A' => {
                if !lit.is_empty() {
                    pieces.push((Op::Lit, std::mem::take(&mut lit), true));
                }
                pieces.push((Op::S, "o".into(), store));
            }
            _ => {}
        }
    }
    if !lit.is_empty() {
        pieces.push((Op::Lit, lit, true));
    }

    let sc: Vec<char> = s.chars().collect();

    fn lit_match(sc: &[char], mut si: usize, t: &str) -> Option<usize> {
        for c in t.chars() {
            if c == ' ' || c == '\t' || c == '\n' {
                let start = si;
                while si < sc.len()
                    && matches!(sc[si], ' ' | '\t' | '\n' | '\r')
                {
                    si += 1;
                }
                if si == start {
                    return None;
                }
            } else {
                if si >= sc.len() || sc[si] != c {
                    return None;
                }
                si += 1;
            }
        }
        Some(si)
    }

    fn num_at(sc: &[char], si: usize) -> Option<(usize, bool)> {
        // (end, had_frac)
        let mut j = si;
        if j < sc.len() && matches!(sc[j], '-' | '+') {
            j += 1;
        }
        let ds = j;
        while j < sc.len() && sc[j].is_ascii_digit() {
            j += 1;
        }
        if j == ds {
            return None;
        }
        let mut frac = false;
        if j + 1 < sc.len() && sc[j] == '.' && sc[j + 1].is_ascii_digit() {
            frac = true;
            j += 1;
            while j < sc.len() && sc[j].is_ascii_digit() {
                j += 1;
            }
        }
        Some((j, frac))
    }

    fn go(
        sc: &[char],
        pcs: &[(Op, String, bool)],
        pi: usize,
        mut si: usize,
        out: &mut Vec<Svalue>,
    ) -> bool {
        if pi >= pcs.len() {
            return true;
        }
        let op = pcs[pi].0;
        let payload: String = pcs[pi].1.clone();
        match op {
            Op::Lit => {
                if let Some(end) = lit_match(sc, si, &payload) {
                    if go(sc, pcs, pi + 1, end, out) {
                        return true;
                    }
                }
                false
            }
            Op::T => {
                let start = si;
                while si < sc.len() && matches!(sc[si], ' ' | '\t' | '\n' | '\r') {
                    si += 1;
                }
                if si == start {
                    return false;
                }
                go(sc, pcs, pi + 1, si, out)
            }
            Op::D => {
                let mut k = si;
                while k < sc.len() && matches!(sc[k], ' ' | '\t') {
                    k += 1;
                }
                if let Some((end, frac)) = num_at(sc, k) {
                    let t: String = sc[k..end].iter().collect();
                    let v = if frac && payload != "o" {
                        Svalue::Float(t.parse().unwrap_or(0.0))
                    } else {
                        match t.parse::<i64>() {
                            Ok(n) => Svalue::Int(n),
                            Err(_) => Svalue::Float(t.parse().unwrap_or(0.0)),
                        }
                    };
                    out.push(v);
                    if go(sc, pcs, pi + 1, end, out) {
                        return true;
                    }
                    out.pop();
                }
                false
            }
            Op::F => {
                let mut k = si;
                while k < sc.len() && matches!(sc[k], ' ' | '\t') {
                    k += 1;
                }
                if let Some((end, _)) = num_at(sc, k) {
                    let t: String = sc[k..end].iter().collect();
                    out.push(Svalue::Float(t.parse().unwrap_or(0.0)));
                    if go(sc, pcs, pi + 1, end, out) {
                        return true;
                    }
                    out.pop();
                }
                false
            }
            Op::S => {
                // greedy: longest first, backtracking down to empty match
                let max = sc.len() - si;
                for take in (0..=max).rev() {
                    let t: String = sc[si..si + take].iter().collect();
                    out.push(mk_string(t));
                    if go(sc, pcs, pi + 1, si + take, out) {
                        return true;
                    }
                    out.pop();
                }
                false
            }
        }
    }

    let mut out: Vec<Svalue> = Vec::new();
    if go(&sc, &pieces, 0, 0, &mut out) {
        // keep only the stored ones (in fmt order)
        let store_flags: Vec<bool> = pieces.iter().map(|(_, _, st)| *st).collect();
        let mut vals: Vec<Svalue> = Vec::new();
        let mut oi = 0usize;
        for st in store_flags {
            if oi >= out.len() {
                break;
            }
            let v = std::mem::replace(&mut out[oi], Svalue::Int(0));
            if st {
                vals.push(v);
            }
            oi += 1;
        }
        vals.truncate(ndest);
        vals
    } else {
        Vec::new()
    }
}

enum LV {
    Local(String),
    Global(u16),
    Indexed(Svalue, Svalue), // base value, index
}

// free helpers

pub(crate) fn serr(s: String) -> Ctrl {
    Ctrl::Error(mk_string(s))
}

pub(crate) fn add_values(v: &Svalue, delta: i64) -> Result<Svalue, String> {
    match v {
        Svalue::Int(i) => Ok(Svalue::Int(i + delta)),
        Svalue::Float(f) => Ok(Svalue::Float(f + delta as f64)),
        _ => Err(format!("Illegal type ({}) in increment/decrement", type_of(v))),
    }
}

fn add2(a: &Svalue, b: &Svalue) -> Result<Svalue, String> {
    match (a, b) {
        (Svalue::Null, x) | (x, Svalue::Null) => Ok(x.clone()),
        (Svalue::Int(x), Svalue::Int(y)) => Ok(Svalue::Int(x.wrapping_add(*y))),
        (Svalue::Int(x), Svalue::Float(y)) | (Svalue::Float(y), Svalue::Int(x)) => Ok(Svalue::Float(*x as f64 + *y)),
        (Svalue::Float(x), Svalue::Float(y)) => Ok(Svalue::Float(x + y)),
        (Svalue::Str(x), Svalue::Str(y)) => Ok(mk_string(format!("{}{}", x, y))),
        (Svalue::Str(x), Svalue::Int(y)) if *y >= 0 && *y < 0x110000 => {
            let c = char::from_u32(*y as u32).ok_or("illegal char code in string add")?;
            Ok(mk_string(format!("{}{}", x, c)))
        }
        (Svalue::Str(_), x) => Err(format!("Illegal type ({}) added to string", type_of(x))),
        (Svalue::Array(x), Svalue::Array(y)) => {
            let mut out = x.borrow().clone();
            out.extend(y.borrow().iter().cloned());
            Ok(mk_array(out))
        }
        (Svalue::Mapping(x), Svalue::Mapping(y)) => {
            let mut out = Mapping::new();
            for (k, v) in x.borrow().items() {
                out.set(k.clone(), v.clone());
            }
            for (k, v) in y.borrow().items() {
                out.set(k.clone(), v.clone());
            }
            Ok(mk_mapping(out.items().to_vec()))
        }
        (x, _) => Err(format!("illegal argument type ({}) in addition", type_of(x))),
    }
}

fn sub2(a: &Svalue, b: &Svalue) -> Result<Svalue, String> {
    match (a, b) {
        (Svalue::Int(x), Svalue::Int(y)) => Ok(Svalue::Int(x.wrapping_sub(*y))),
        (Svalue::Int(x), Svalue::Float(y)) => Ok(Svalue::Float(*x as f64 - *y)),
        (Svalue::Float(y), Svalue::Int(x)) => Ok(Svalue::Float(*y - *x as f64)),
        (Svalue::Float(x), Svalue::Float(y)) => Ok(Svalue::Float(x - y)),
        (Svalue::Array(x), Svalue::Array(y)) => {
            let ys = y.borrow();
            let out: Vec<Svalue> = x
                .borrow()
                .iter()
                .filter(|e| !ys.iter().any(|k| eq(e, k)))
                .cloned()
                .collect();
            Ok(mk_array(out))
        }
        (Svalue::Mapping(m), Svalue::Mapping(km)) => {
            let mut out = Mapping::new();
            let keys: Vec<Svalue> = km.borrow().keys();
            for (k, v) in m.borrow().items() {
                if !keys.iter().any(|x| eq(x, k)) {
                    out.set(k.clone(), v.clone());
                }
            }
            Ok(mk_mapping(out.items().to_vec()))
        }
        (x, _) => Err(format!("illegal argument type ({}) in subtraction", type_of(x))),
    }
}

fn mul2(a: &Svalue, b: &Svalue) -> Result<Svalue, String> {
    match (a, b) {
        (Svalue::Int(x), Svalue::Int(y)) => Ok(Svalue::Int(x.wrapping_mul(*y))),
        (Svalue::Int(x), Svalue::Float(y)) | (Svalue::Float(y), Svalue::Int(x)) => Ok(Svalue::Float(*x as f64 * *y)),
        (Svalue::Float(x), Svalue::Float(y)) => Ok(Svalue::Float(x * y)),
        (Svalue::Array(x), Svalue::Array(y)) => {
            let ys = y.borrow();
            let out: Vec<Svalue> = x
                .borrow()
                .iter()
                .filter(|e| ys.iter().any(|k| eq(e, k)))
                .cloned()
                .collect();
            Ok(mk_array(out))
        }
        (Svalue::Mapping(x), Svalue::Mapping(y)) => {
            let mut out = Mapping::new();
            for (k, v) in x.borrow().items() {
                if let Some(v2) = y.borrow().get(k) {
                    if let Ok(sum) = add2(v, &v2) {
                        out.set(k.clone(), sum);
                    }
                }
            }
            Ok(mk_mapping(out.items().to_vec()))
        }
        (Svalue::Str(s), Svalue::Int(n)) | (Svalue::Int(n), Svalue::Str(s)) if *n >= 0 => {
            Ok(mk_string(s.repeat(*n as usize)))
        }
        (x, _) => Err(format!("illegal argument type ({}) in multiplication", type_of(x))),
    }
}

pub(crate) fn norm_range(seq: &Svalue, a: i64, end: Option<i64>) -> (i64, i64) {
    // LDMud slice semantics: a >= 0 with end = exclusive bound.
    // Negative `a` with no end: drop first -a elements... LDMud:
    //  str[0..] whole; str[a..] from a to end; str[0..b] first b;
    //  str[0] element (not slice, handled in index).
    //  Negative with two bounds follows (from len+a .. len+b);
    //  negative single index without second bound means "up to len+a".
    let len = match seq {
        Svalue::Str(s) => s.chars().count() as i64,
        Svalue::Array(a) => a.borrow().len() as i64,
        Svalue::Mapping(m) => m.borrow().len() as i64,
        _ => 0,
    };
    match (a, end) {
        (a, Some(b)) if a >= 0 && b >= 0 => (a, b),
        (a, Some(b)) => {
            let la = if a < 0 { len + a } else { a };
            let lb = if b < 0 { len + b } else { b };
            (la, lb)
        }
        (a, None) if a >= 0 => (a, len),
        (a, None) => (0, len + a), // negative-only: 0..len+a
    }
}
