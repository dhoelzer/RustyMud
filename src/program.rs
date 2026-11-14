//! Programs, inheritance resolution, and object instances.
//! Rust analogue of LDMud's program_ob / object_t (otable.c, object.c).

use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::*;
use crate::value::*;

/// A loaded program: AST + flattened (inherit-merged) function & variable tables.
pub struct Program {
    pub filename: String, // canonical path "/std/room.c"
    pub compiled: Compiled,
    /// Flattened: index -> (function info, defining file)
    pub functions: Vec<(FuncDefInfo, String)>,
    pub func_ix: HashMap<String, usize>,
    /// Flattened variable slots in definition order (deepest inherit first).
    pub variables: Vec<(VarInit, String)>,
    pub var_ix: HashMap<String, usize>,
}

impl Program {
    pub fn lookup_func(&self, name: &str) -> Option<(usize, &FuncDefInfo, &str)> {
        self.func_ix.get(name).map(|&i| {
            let (info, def) = &self.functions[i];
            (i, info, def.as_str())
        })
    }

    pub fn from_compiled(compiled: Compiled) -> Program {
        let filename = compiled.filename.clone();
        let mut functions: Vec<(FuncDefInfo, String)> = Vec::new();
        let mut func_ix = HashMap::new();
        let mut variables: Vec<(VarInit, String)> = Vec::new();
        let mut var_ix = HashMap::new();
        for info in &compiled.funcs {
            func_ix.insert(info.name.clone(), functions.len());
            functions.push((info.clone(), filename.clone()));
        }
        for v in &compiled.vars {
            var_ix.insert(v.name.clone(), variables.len());
            variables.push((v.clone(), filename.clone()));
        }
        Program { filename, compiled, functions, func_ix, variables, var_ix }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObjKind {
    Master,
    SimulEfun,
    Object,
}

pub struct LdObject {
    pub id: ObjId,
    pub name: String, // canonical name; clones get "#N"
    pub prog: Rc<Program>,
    pub vars: Vec<Svalue>,
    pub destructed: bool,
    pub kind: ObjKind,
    pub env: ObjId, // environment (container), 0 = none
}

// ---- id remapping so inherited bodies live in one expression pool ----

fn shift_expr(e: &Expr, off_e: usize, off_b: usize) -> Expr {
    let r = |id: usize| id + off_e;
    match e {
        Expr::ArrayLit(v) => Expr::ArrayLit(v.iter().map(|&i| r(i)).collect()),
        Expr::MappingLit(v) => Expr::MappingLit(v.iter().map(|(a, b)| (r(*a), r(*b))).collect()),
        Expr::Pre(u, a) => Expr::Pre(*u, r(*a)),
        Expr::Post(u, a) => Expr::Post(*u, r(*a)),
        Expr::Bin(o, a, b) => Expr::Bin(*o, r(*a), r(*b)),
        Expr::Ternary(a, b, c) => Expr::Ternary(r(*a), r(*b), r(*c)),
        Expr::Index(a, b) => Expr::Index(r(*a), r(*b)),
        Expr::Slice(a, b, c) => Expr::Slice(r(*a), r(*b), c.map(r)),
        Expr::Call { name, args, inherit_path } => Expr::Call {
            name: name.map(r),
            args: args.iter().map(|&i| r(i)).collect(),
            inherit_path: inherit_path.clone(),
        },
        Expr::DirectCall { name, args } => Expr::DirectCall {
            name: name.clone(),
            args: args.iter().map(|&i| r(i)).collect(),
        },
        Expr::CallOther { ob, name, lfun_name, args } => Expr::CallOther {
            ob: r(*ob),
            name: name.map(r),
            lfun_name: lfun_name.clone(),
            args: args.iter().map(|&i| r(i)).collect(),
        },
        Expr::Cast(t, a) => Expr::Cast(t.clone(), r(*a)),
        Expr::Catch(a) => Expr::Catch(r(*a)),
        Expr::Throw(a) => Expr::Throw(r(*a)),
        Expr::Comma(v) => Expr::Comma(v.iter().map(|&i| r(i)).collect()),
        Expr::ClosureInline { body_id, nargs } => Expr::ClosureInline {
            body_id: body_id + off_b,
            nargs: *nargs,
        },
        x => x.clone(),
    }
}

fn shift_stmt(s: &Stmt, off_e: usize, off_b: usize) -> Stmt {
    let r = |id: usize| id + off_e;
    match s {
        Stmt::Expr(e) => Stmt::Expr(r(*e)),
        Stmt::Decl(v) => Stmt::Decl(VarInit {
            name: v.name.clone(),
            ty: v.ty.clone(),
            init: v.init.map(|i| r(i) + 0 * off_e),
            vis: v.vis,
            nosave: v.nosave,
        }),
        Stmt::If { cond, then, els } => Stmt::If {
            cond: r(*cond),
            then: then.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
            els: els.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: r(*cond),
            body: body.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
        },
        Stmt::DoWhile { cond, body } => Stmt::DoWhile {
            cond: r(*cond),
            body: body.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
        },
        Stmt::For { init, cond, step, body } => Stmt::For {
            init: init.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
            cond: cond.map(|i| r(i)),
            step: step.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
            body: body.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
        },
        Stmt::Foreach { vars, iter, range_hi, body } => Stmt::Foreach {
            vars: vars.clone(),
            iter: r(*iter),
            range_hi: range_hi.map(|i| r(i)),
            body: body.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
        },
        Stmt::Switch { expr, cases, default } => Stmt::Switch {
            expr: r(*expr),
            cases: cases
                .iter()
                .map(|(labs, body)| {
                    (
                        labs.iter().map(|&i| r(i)).collect(),
                        body.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
                    )
                })
                .collect(),
            default: default.iter().map(|s| shift_stmt(s, off_e, off_b)).collect(),
        },
        Stmt::Return(e) => Stmt::Return(e.map(|i| r(i))),
        Stmt::Block(v) => Stmt::Block(v.iter().map(|s| shift_stmt(s, off_e, off_b)).collect()),
        Stmt::Break | Stmt::Continue | Stmt::Empty => s.clone(),
    }
}

/// Merge inherited programs LDMud-style: base first, derived overrides
/// functions; variables concatenate (base vars first, derived override by name).
pub fn flatten(
    mut compiled: Compiled,
    loader: &dyn Fn(&str) -> Result<Rc<Program>, String>,
) -> Result<Program, String> {
    let mut functions: Vec<(FuncDefInfo, String)> = Vec::new();
    let mut func_ix: HashMap<String, usize> = HashMap::new();
    let mut variables: Vec<(VarInit, String)> = Vec::new();
    let mut var_ix: HashMap<String, usize> = HashMap::new();

    for path in compiled.inherits.clone() {
        let base = loader(&path)?;
        let off_e = compiled.exprs.len();
        let off_b = compiled.bodies.len();
        // copy base pools with shifted ids
        for e in &base.compiled.exprs {
            compiled.exprs.push(shift_expr(e, off_e, off_b));
        }
        for b in &base.compiled.bodies {
            compiled.bodies.push(b.iter().map(|s| shift_stmt(s, off_e, off_b)).collect());
        }
        // copy base function/variable tables with adjusted ids
        for (info, def) in base.functions.clone() {
            let mut info = info;
            info.body_id += off_b;
            if let Some(&ix) = func_ix.get(&info.name) {
                functions[ix] = (info, def);
            } else {
                func_ix.insert(info.name.clone(), functions.len());
                functions.push((info, def));
            }
        }
        for (mut v, def) in base.variables.clone() {
            v.init = v.init.map(|i| i + off_e);
            if let Some(&ix) = var_ix.get(&v.name) {
                variables[ix] = (v, def);
            } else {
                var_ix.insert(v.name.clone(), variables.len());
                variables.push((v, def));
            }
        }
        // inline param aliases need rebasing too
        let base_aliases: Vec<(usize, Vec<(String, usize)>)> = base
            .compiled
            .inline_params
            .iter()
            .map(|(k, v)| (k + off_b, v.clone()))
            .collect();
        for (k, v) in base_aliases {
            compiled.inline_params.insert(k, v);
        }
    }

    let fname = compiled.filename.clone();
    for info in compiled.funcs.clone() {
        if let Some(&ix) = func_ix.get(&info.name) {
            functions[ix] = (info, fname.clone());
        } else {
            func_ix.insert(info.name.clone(), functions.len());
            functions.push((info, fname.clone()));
        }
    }
    for v in compiled.vars.clone() {
        if let Some(&ix) = var_ix.get(&v.name) {
            variables[ix] = (v, fname.clone());
        } else {
            var_ix.insert(v.name.clone(), variables.len());
            variables.push((v, fname.clone()));
        }
    }

    Ok(Program { filename: fname, compiled, functions, func_ix, variables, var_ix })
}
