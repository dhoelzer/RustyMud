//! The interpreter and driver state: object table, call frames, master
//! hooks, input handling, command verbs, call_outs — the Rust analogue of
//! LDMud's interpret.c / otable.c / backend.c.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::*;
use crate::program::{LdObject, ObjKind, Program};
use crate::value::*;

pub type EvResult = Result<Svalue, Ctrl>;

#[derive(Debug)]
pub enum Ctrl {
    Return(Option<Svalue>),
    Break,
    Continue,
    Error(Svalue), // thrown error (a string in practice)
}

impl From<String> for Ctrl {
    fn from(s: String) -> Ctrl {
        Ctrl::Error(mk_string(s))
    }
}

#[derive(Clone)]
pub struct InputTo {
    pub closure: Svalue,
    pub flag: i64,
    pub prompt: Option<String>,
}

#[derive(Clone)]
pub struct ActionEntry {
    pub verb: String,
    pub lfun: String,
    pub object: ObjId,
    pub min_match_len: usize,
    pub flag: i64, // 0 == remove after one use, 1 == permanent
}

#[derive(Clone)]
pub struct NotifyFail {
    pub closure: Svalue,
    pub flag: i64,
    pub orig: Option<String>,
}

#[derive(Clone)]
pub struct CallOut {
    pub next_at_ms: i64,
    pub period_ms: Option<i64>,
    pub object: ObjId,
    pub closure: Svalue,
    pub arg: Svalue,
}

pub struct Frame {
    pub oid: ObjId,
    pub func_name: String,
    pub locals: Vec<Svalue>,
    pub local_ix: HashMap<String, usize>,
}

impl Frame {
    pub(crate) fn add_local(&mut self, name: &str, v: Svalue) {
        if let Some(&i) = self.local_ix.get(name) {
            self.locals[i] = v;
        } else {
            self.local_ix.insert(name.to_string(), self.locals.len());
            self.locals.push(v);
        }
    }
    pub(crate) fn slot(&mut self, name: &str) -> Option<usize> {
        self.local_ix.get(name).copied()
    }
}

pub struct Env {
    pub mudlib_dir: String,
    pub objects: HashMap<ObjId, Rc<RefCell<LdObject>>>,
    pub next_oid: ObjId,
    pub programs: HashMap<String, Rc<Program>>,
    pub master: ObjId,
    pub sefun: ObjId,
    pub command_giver: ObjId,
    pub last_command: String,
    pub last_command_origin: ObjId,
    pub interconnects: HashMap<ObjId, usize>, // oid -> connection slot
    pub input_to: HashMap<ObjId, Option<InputTo>>,
    pub actions: HashMap<ObjId, Vec<ActionEntry>>,
    pub commands_enabled: HashMap<ObjId, bool>,
    pub notify_fail: HashMap<ObjId, Option<NotifyFail>>,
    pub prompts: HashMap<ObjId, Svalue>,
    pub call_outs: Vec<CallOut>,
    pub heart_beats: Vec<ObjId>,
    pub driver_hooks: Vec<Svalue>,
    pub time_ms: i64,
    pub shutting_down: bool,
    pub shutdown_at_exit: bool,
    pub last_verb: Option<String>,
    pub error_log: Vec<String>,
}

impl Env {
    pub fn new() -> Env {
        Env {
            mudlib_dir: String::new(),
            objects: HashMap::new(),
            next_oid: 1,
            programs: HashMap::new(),
            master: 0,
            sefun: 0,
            command_giver: 0,
            last_command: String::new(),
            last_command_origin: 0,
            interconnects: HashMap::new(),
            input_to: HashMap::new(),
            actions: HashMap::new(),
            commands_enabled: HashMap::new(),
            notify_fail: HashMap::new(),
            prompts: HashMap::new(),
            call_outs: Vec::new(),
            heart_beats: Vec::new(),
            driver_hooks: vec![Svalue::Null; 32],
            time_ms: 0,
            shutting_down: false,
            shutdown_at_exit: false,
            last_verb: None,
            error_log: Vec::new(),
        }
    }

    pub fn object(&self, oid: ObjId) -> Option<Rc<RefCell<LdObject>>> {
        if oid == 0 {
            return None;
        }
        let o = self.objects.get(&oid)?;
        if o.borrow().destructed {
            return None;
        }
        Some(o.clone())
    }

    pub fn object_name(&self, oid: ObjId) -> Option<String> {
        self.objects.get(&oid).map(|o| o.borrow().name.clone())
    }

    pub fn interactive(&self, oid: ObjId) -> bool {
        self.interconnects.contains_key(&oid)
    }
}

pub struct CallInfo {
    pub object: ObjId,
    pub func: String,
}

pub struct Vm {
    pub env: Env,
    pub(crate) frames: Vec<Frame>,
    /// Pending output per connection slot (slot -> bytes).
    pub out_q: Vec<(usize, Vec<u8>)>,
    /// To whom write()/printf() go while executing.
    pub write_target: ObjId,
    pub rng: u64,
    pub anon_counter: u32,
    pub defines: HashMap<String, String>,
    pub(crate) src_cache: HashMap<String, String>,
    pub master_file: String,
}

impl Vm {
    pub fn new() -> Vm {
        Vm {
            env: Env::new(),
            frames: Vec::new(),
            out_q: Vec::new(),
            write_target: 0,
            rng: 0x243F6A8885A308D3,
            anon_counter: 0,
            defines: HashMap::new(),
            src_cache: HashMap::new(),
            master_file: "master.c".into(),
        }
    }

    // ------------------------------------------------------------------
    //  Output / connections
    // ------------------------------------------------------------------

    pub fn push_output(&mut self, slot: usize, s: &str) {
        self.out_q.push((slot, s.as_bytes().to_vec()));
    }

    pub fn slot_of(&self, oid: ObjId) -> Option<usize> {
        self.env.interconnects.get(&oid).copied()
    }

    pub fn write_out(&mut self, s: &str) {
        if let Some(slot) = self.slot_of(self.write_target) {
            self.push_output(slot, s);
        } else if self.write_target == 0 {
            print!("{}", s);
        }
    }

    pub fn tell_object(&mut self, oid: ObjId, s: &str) {
        if let Some(slot) = self.slot_of(oid) {
            self.push_output(slot, s);
        }
    }

    pub fn rnd(&mut self, n: u64) -> u64 {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        if n == 0 {
            0
        } else {
            (self.rng >> 33) % n
        }
    }

    // ------------------------------------------------------------------
    //  Frames
    // ------------------------------------------------------------------

    pub fn this_object(&self) -> ObjId {
        self.frames.last().map(|f| f.oid).unwrap_or(0)
    }

    pub fn previous_object(&self) -> ObjId {
        self.previous_n_object(1)
    }

    pub fn previous_n_object(&self, n: i64) -> ObjId {
        let n = n.max(0) as usize;
        if self.frames.len() > n {
            self.frames[self.frames.len() - 1 - n].oid
        } else {
            0
        }
    }

    pub fn current_prog(&self) -> Option<Rc<Program>> {
        self.env.object(self.this_object()).map(|o| o.borrow().prog.clone())
    }

    pub fn call_stack(&self) -> Vec<CallInfo> {
        self.frames
            .iter()
            .map(|f| CallInfo { object: f.oid, func: f.func_name.clone() })
            .collect()
    }

    pub fn runtime_trace(&self) -> String {
        let mut s = String::new();
        for (i, f) in self.frames.iter().enumerate().rev() {
            let name = self.env.object_name(f.oid).unwrap_or_else(|| "0".into());
            s.push_str(&format!("{}: {}.{}\n", i + 1, name, f.func_name));
        }
        s
    }

    pub fn frame_locals(&self) -> Vec<String> {
        let f = match self.frames.last() {
            Some(f) => f,
            None => return Vec::new(),
        };
        let mut names: Vec<(usize, String)> =
            f.local_ix.iter().map(|(k, v)| (*v, k.clone())).collect();
        names.sort_by_key(|x| x.0);
        names.into_iter().map(|(_, n)| n).collect()
    }

    pub fn frame_local_pairs(&self) -> Vec<(String, Svalue)> {
        let f = match self.frames.last() {
            Some(f) => f,
            None => return Vec::new(),
        };
        let mut out: Vec<(usize, String, Svalue)> = f
            .local_ix
            .iter()
            .map(|(k, v)| (*v, k.clone(), f.locals[*v].clone()))
            .collect();
        out.sort_by_key(|x| x.0);
        out.into_iter().map(|(_, n, v)| (n, v)).collect()
    }

    pub fn remove_interactive(&mut self, oid: ObjId) {
        self.env.interconnects.remove(&oid);
        self.env.input_to.remove(&oid);
        self.env.actions.remove(&oid);
        self.env.notify_fail.remove(&oid);
        if self.env.command_giver == oid {
            self.env.command_giver = 0;
        }
    }

    // ------------------------------------------------------------------
    //  Function application
    // ------------------------------------------------------------------

    pub fn call_function(&mut self, oid: ObjId, name: &str, args: &[Svalue]) -> EvResult {
        let obj = match self.env.object(oid) {
            Some(o) => o,
            None => {
                return Err(Ctrl::Error(mk_string(format!(
                    "Illegal object in call of '{}'\n",
                    name
                ))))
            }
        };
        let prog = obj.borrow().prog.clone();
        let (_ix, info, def_src) = match prog.lookup_func(name) {
            Some(x) => x,
            None => {
                return Err(Ctrl::Error(mk_string(format!(
                    "No function {} in object {}",
                    name,
                    obj.borrow().name
                ))))
            }
        };

        let body_id = info.body_id;
        let body = prog.compiled.bodies[body_id].clone();

        // frame: object globals first (copied out, synced back on return),
        // then parameters, then extra varargs as $1..$n.
        let mut frame =
            Frame { oid, func_name: name.to_string(), locals: Vec::new(), local_ix: HashMap::new() };
        {
            let gl: Vec<(String, Svalue)> = obj
                .borrow()
                .prog
                .variables
                .iter()
                .enumerate()
                .map(|(i, v)| (v.0.name.clone(), obj.borrow().vars[i].clone()))
                .collect();
            for (nm, val) in gl {
                frame.add_local(&nm, val);
            }
        }
        let arity = info.args.len();
        for (j, a) in info.args.iter().enumerate() {
            let v = if j < args.len() {
                args[j].clone()
            } else if let Some(d) = a.default {
                self.eval_e(&prog, d)?
            } else {
                default_for(&a.ty)
            };
            frame.add_local(&a.name, v);
        }
        if info.varargs {
            for (j, a) in args.iter().skip(arity).enumerate() {
                frame.add_local(&format!("${}", j + 1), a.clone());
            }
        }
        self.frames.push(frame);
        let r = self.exec_seq(&prog, oid, &body);
        let ret = match r {
            Ok(()) => default_for(&info.ret_ty),
            Err(Ctrl::Return(Some(v))) => v,
            Err(Ctrl::Return(None)) => default_for(&info.ret_ty),
            Err(e) => {
                self.sync_globals_from_frame(oid);
                self.frames.pop();
                return Err(e);
            }
        };
        self.sync_globals_from_frame(oid);
        self.frames.pop();
        Ok(ret)
    }

    #[allow(clippy::needless_range_loop)]
    fn sync_globals_from_frame(&mut self, oid: ObjId) {
        let obj = match self.env.object(oid) {
            Some(o) => o.clone(),
            None => return,
        };
        let (locals, ix): (Vec<Svalue>, HashMap<String, usize>) = match self.frames.last() {
            Some(f) if f.oid == oid => (f.locals.clone(), f.local_ix.clone()),
            _ => return,
        };
        let vars: Vec<(usize, String)> = obj
            .borrow()
            .prog
            .variables
            .iter()
            .enumerate()
            .map(|(i, v)| (i, v.0.name.clone()))
            .collect();
        for i in 0..vars.len() {
            let (vi, nm) = vars[i].clone();
            if let Some(&slot) = ix.get(&nm) {
                if let Some(val) = locals.get(slot).cloned() {
                    obj.borrow_mut().vars[vi] = val;
                }
            }
        }
    }

    /// Run an inline closure body with $1..$n bound (and named aliases).
    pub(crate) fn run_func_body(
        &mut self,
        prog: Rc<Program>,
        oid: ObjId,
        body_id: usize,
        params: &[(String, usize)],
        args: &[Svalue],
    ) -> EvResult {
        let body = prog.compiled.bodies[body_id].clone();
        let mut frame = Frame {
            oid,
            func_name: "(inline)".into(),
            locals: Vec::new(),
            local_ix: HashMap::new(),
        };
        for (j, a) in args.iter().enumerate() {
            frame.add_local(&format!("${}", j + 1), a.clone());
        }
        for (nm, pos) in params {
            if *pos > 0 {
                if let Some(v) = args.get(pos - 1).cloned() {
                    frame.add_local(nm, v);
                }
            }
        }
        self.frames.push(frame);
        let r = self.exec_seq(&prog, oid, &body);
        let ret = match r {
            Ok(()) => Svalue::Int(0),
            Err(Ctrl::Return(Some(v))) => v,
            Err(Ctrl::Return(None)) => Svalue::Int(0),
            Err(e) => {
                self.frames.pop();
                return Err(e);
            }
        };
        self.frames.pop();
        Ok(ret)
    }

    pub(crate) fn run_inline_lambda(
        &mut self,
        prog: Rc<Program>,
        body_id: usize,
        args: &[Svalue],
    ) -> EvResult {
        let oid = self.this_object();
        let named = prog.compiled.inline_params.get(&body_id).cloned().unwrap_or_default();
        self.run_func_body(prog, oid, body_id, &named, args)
    }

    pub fn apply_closure(&mut self, cl: &Svalue, args: &[Svalue]) -> EvResult {
        match cl {
            Svalue::Closure(c) => match &**c {
                Closure::Func { oid, name } => self.call_function(*oid, name, args),
                Closure::Lambda { prog, body_id, nargs } => {
                    if *nargs != usize::MAX && args.len() != *nargs {
                        return Err(Ctrl::Error(mk_string(format!(
                            "apply: closure expects {} args, got {}",
                            nargs,
                            args.len()
                        ))));
                    }
                    self.run_inline_lambda(prog.clone(), *body_id, args)
                }
            },
            other => Err(Ctrl::Error(mk_string(format!(
                "Bad closure argument: {}",
                display_string(other)
            )))),
        }
    }

    /// Apply a master lfun if present.
    pub fn apply_master(&mut self, name: &str, args: &[Svalue], default: Svalue) -> EvResult {
        if self.env.master == 0 {
            return Ok(default);
        }
        let obj = match self.env.object(self.env.master) {
            Some(o) => o,
            None => return Ok(default),
        };
        if obj.borrow().prog.lookup_func(name).is_none() {
            return Ok(default);
        }
        self.call_function(self.env.master, name, args)
    }

    pub fn apply_master_opt(&mut self, name: &str, args: &[Svalue], default: Svalue) -> Svalue {
        match self.apply_master(name, args, default.clone()) {
            Ok(v) => v,
            Err(Ctrl::Error(e)) => {
                let line = format!("Error in master function {}: {}", name, display_string(&e));
                eprintln!("{}", line);
                self.env.error_log.push(line);
                default
            }
            Err(_) => default,
        }
    }

    // ------------------------------------------------------------------
    //  Statements
    // ------------------------------------------------------------------

    pub(crate) fn eval(&mut self, prog: &Rc<Program>, e: ExprId) -> EvResult {
        self.eval_e(prog, e)
    }

    pub(crate) fn exec_seq(
        &mut self,
        prog: &Rc<Program>,
        oid: ObjId,
        body: &[Stmt],
    ) -> Result<(), Ctrl> {
        for st in body {
            self.exec_stmt(prog, oid, st)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_stmt(
        &mut self,
        prog: &Rc<Program>,
        oid: ObjId,
        st: &Stmt,
    ) -> Result<(), Ctrl> {
        match st {
            Stmt::Empty => Ok(()),
            Stmt::Expr(e) => {
                self.eval(prog, *e)?;
                Ok(())
            }
            Stmt::Decl(v) => {
                let init = match v.init {
                    Some(e) => self.eval(prog, e)?,
                    None => default_for(&v.ty),
                };
                self.frames.last_mut().unwrap().add_local(&v.name, init);
                Ok(())
            }
            Stmt::Block(b) => self.exec_seq(prog, oid, b),
            Stmt::If { cond, then, els } => {
                let c = self.eval(prog, *cond)?;
                if truthy(&c) {
                    self.exec_seq(prog, oid, then)
                } else {
                    self.exec_seq(prog, oid, els)
                }
            }
            Stmt::While { cond, body } => loop {
                let c = self.eval(prog, *cond)?;
                if !truthy(&c) || self.env.shutting_down {
                    break Ok(());
                }
                match self.exec_seq(prog, oid, body) {
                    Ok(()) => {}
                    Err(Ctrl::Break) => break Ok(()),
                    Err(Ctrl::Continue) => continue,
                    Err(x) => return Err(x),
                }
            },
            Stmt::DoWhile { cond, body } => loop {
                match self.exec_seq(prog, oid, body) {
                    Ok(()) => {}
                    Err(Ctrl::Break) => break Ok(()),
                    Err(Ctrl::Continue) => {}
                    Err(x) => return Err(x),
                }
                let c = self.eval(prog, *cond)?;
                if !truthy(&c) {
                    break Ok(());
                }
            },
            Stmt::For { init, cond, step, body } => {
                self.exec_seq(prog, oid, init)?;
                loop {
                    if let Some(c) = cond {
                        let cv = self.eval(prog, *c)?;
                        if !truthy(&cv) || self.env.shutting_down {
                            break;
                        }
                    }
                    match self.exec_seq(prog, oid, body) {
                        Ok(()) => {}
                        Err(Ctrl::Break) => break,
                        Err(Ctrl::Continue) => {}
                        Err(x) => return Err(x),
                    }
                    self.exec_seq(prog, oid, step)?;
                }
                Ok(())
            }
            Stmt::Foreach { vars, iter, range_hi, body } => {
                self.run_foreach(prog, oid, vars, *iter, *range_hi, body)
            }
            Stmt::Switch { expr, cases, default } => {
                let v = self.eval(prog, *expr)?;
                let mut chosen: Option<&Vec<Stmt>> = None;
                for (vals, body) in cases {
                    let mut hit = false;
                    for ve in vals {
                        let cv = self.eval(prog, *ve)?;
                        if eq(&v, &cv) {
                            hit = true;
                            break;
                        }
                    }
                    if hit {
                        chosen = Some(body);
                        break;
                    }
                }
                if chosen.is_none() {
                    let has_default = !default.is_empty();
                    let has_case_default = cases.iter().any(|(v, _)| v.is_empty());
                    if has_default {
                        chosen = Some(default);
                    } else if has_case_default {
                        for (vals, body) in cases {
                            if vals.is_empty() {
                                chosen = Some(body);
                                break;
                            }
                        }
                    }
                }
                if let Some(body) = chosen {
                    match self.exec_seq(prog, oid, body) {
                        Err(Ctrl::Break) => {}
                        x => x?,
                    }
                }
                Ok(())
            }
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => Some(self.eval(prog, *e)?),
                    None => None,
                };
                Err(Ctrl::Return(v))
            }
            Stmt::Break => Err(Ctrl::Break),
            Stmt::Continue => Err(Ctrl::Continue),
        }
    }

    fn run_foreach(
        &mut self,
        prog: &Rc<Program>,
        oid: ObjId,
        vars: &[String],
        iter: ExprId,
        range_hi: Option<ExprId>,
        body: &[Stmt],
    ) -> Result<(), Ctrl> {
        let seq: Vec<Svalue> = match range_hi {
            Some(hi) => {
                let lo = as_int(&self.eval(prog, iter)?).unwrap_or(0);
                let top = as_int(&self.eval(prog, hi)?).unwrap_or(0);
                if lo <= top {
                    (lo..=top).map(Svalue::Int).collect()
                } else {
                    Vec::new()
                }
            }
            None => {
                let v = self.eval(prog, iter)?;
                match &v {
                    Svalue::Array(a) => a.borrow().clone(),
                    Svalue::Str(s) => s.chars().map(|c| mk_string(c.to_string())).collect(),
                    Svalue::Mapping(m) => m.borrow().keys(),
                    Svalue::Null => Vec::new(),
                    other => {
                        return Err(Ctrl::Error(mk_string(format!(
                            "foreach over illegal iterable {}",
                            type_of(other)
                        ))))
                    }
                }
            }
        };
        for item in seq {
            {
                let f = self.frames.last_mut().unwrap();
                if vars.len() == 1 {
                    f.add_local(&vars[0], item);
                } else if let Some(a) = item.as_vec() {
                    let b = a.borrow();
                    f.add_local(&vars[0], b.first().cloned().unwrap_or(Svalue::Int(0)));
                    f.add_local(&vars[1], b.get(1).cloned().unwrap_or(Svalue::Int(0)));
                } else {
                    f.add_local(&vars[0], item.clone());
                    f.add_local(&vars[1], Svalue::Int(0));
                }
            }
            match self.exec_seq(prog, oid, body) {
                Ok(()) => {}
                Err(Ctrl::Break) => break,
                Err(Ctrl::Continue) => continue,
                Err(x) => return Err(x),
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    //  Object creation & destruction
    // ------------------------------------------------------------------

    pub fn instantiate(&mut self, prog: Rc<Program>, name: String, kind: ObjKind) -> ObjId {
        let oid = self.env.next_oid;
        self.env.next_oid += 1;
        let vars: Vec<Svalue> = prog.variables.iter().map(|(v, _)| default_for(&v.ty)).collect();
        let obj = LdObject { id: oid, name, prog, vars, destructed: false, kind, env: 0 };
        self.env.objects.insert(oid, Rc::new(RefCell::new(obj)));
        oid
    }

    /// Initialise global variables from their initialisers, in declaration
    /// order (base-inherit first).
    pub(crate) fn init_vars(&mut self, oid: ObjId) -> Result<(), String> {
        let obj = match self.env.object(oid) {
            Some(o) => o,
            None => return Ok(()),
        };
        let prog = obj.borrow().prog.clone();
        let inits: Vec<(String, Option<ExprId>)> =
            prog.variables.iter().map(|(v, _)| (v.name.clone(), v.init)).collect();
        if inits.iter().all(|(_, e)| e.is_none()) {
            return Ok(());
        }
        let vars_snapshot = obj.borrow().vars.clone();
        let mut frame = Frame {
            oid,
            func_name: "__INITVARS".into(),
            locals: Vec::new(),
            local_ix: HashMap::new(),
        };
        for (i, (name, _)) in inits.iter().enumerate() {
            frame.add_local(name, vars_snapshot[i].clone());
        }
        self.frames.push(frame);
        let mut errored = None;
        for (i, (name, e)) in inits.iter().enumerate() {
            if let Some(e) = e {
                match self.eval(&prog, *e) {
                    Ok(v) => {
                        let obj = self.env.object(oid).unwrap();
                        obj.borrow_mut().vars[i] = v.clone();
                        if let Some(ix) = self.frames.last_mut().unwrap().slot(name) {
                            self.frames.last_mut().unwrap().locals[ix] = v;
                        }
                    }
                    Err(Ctrl::Error(x)) => {
                        errored = Some(format!(
                            "Error initialising {}: {}",
                            name,
                            display_string(&x)
                        ));
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
        self.frames.pop();
        if let Some(e) = errored {
            return Err(e);
        }
        Ok(())
    }

    pub fn destruct(&mut self, oid: ObjId) {
        let o = match self.env.objects.get(&oid) {
            Some(o) => o.clone(),
            None => return,
        };
        self.env.heart_beats.retain(|x| *x != oid);
        self.env.call_outs.retain(|c| c.object != oid);
        if !o.borrow().destructed {
            o.borrow_mut().destructed = true;
            self.apply_master_opt("prepare_destruct", &[Svalue::Object(oid)], Svalue::Int(0));
            let has = self
                .env
                .objects
                .get(&oid)
                .map(|o| o.borrow().prog.lookup_func("destruct").is_some())
                .unwrap_or(false);
            if has {
                let _ = self.call_function(oid, "destruct", &[]);
            }
        }
        for entries in self.env.actions.values_mut() {
            entries.retain(|a| a.object != oid);
        }
        self.env.input_to.remove(&oid);
        self.env.notify_fail.remove(&oid);
        self.env.commands_enabled.remove(&oid);
        self.env.interconnects.remove(&oid);
        if self.env.command_giver == oid {
            self.env.command_giver = 0;
        }
    }

    pub fn set_environment(&mut self, ob: ObjId, env: ObjId) {
        if let Some(o) = self.env.objects.get(&ob) {
            o.borrow_mut().env = env;
        }
    }

    pub fn env_of(&self, oid: ObjId) -> ObjId {
        self.env.objects.get(&oid).map(|o| o.borrow().env).unwrap_or(0)
    }

    pub fn move_object(&mut self, ob: ObjId, dest: ObjId) {
        self.set_environment(ob, dest);
    }

    pub fn inventory(&self, oid: ObjId, deep: bool) -> Vec<ObjId> {
        if oid == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (_, o) in &self.env.objects {
            let ob = o.borrow();
            if !ob.destructed && ob.env == oid {
                out.push(ob.id);
            }
        }
        if deep {
            let mut i = 0;
            while i < out.len() {
                let cur = out[i];
                let mut sub = self.inventory(cur, false);
                out.append(&mut sub);
                i += 1;
            }
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn all_object_ids(&self) -> Vec<ObjId> {
        let mut v: Vec<ObjId> = self
            .env
            .objects
            .iter()
            .filter(|(_, o)| !o.borrow().destructed)
            .map(|(k, _)| *k)
            .collect();
        v.sort();
        v
    }

    pub fn has_lfun(&self, oid: ObjId, name: &str) -> bool {
        self.env
            .object(oid)
            .map(|o| o.borrow().prog.lookup_func(name).is_some())
            .unwrap_or(false)
    }

    // ------------------------------------------------------------------
    //  Timers
    // ------------------------------------------------------------------

    pub(crate) fn schedule_call_out(
        &mut self,
        oid: ObjId,
        delay: i64,
        period: Option<i64>,
        cl: Svalue,
        arg: Svalue,
    ) {
        self.env.call_outs.push(CallOut {
            next_at_ms: self.env.time_ms + delay,
            period_ms: period,
            object: oid,
            closure: cl,
            arg,
        });
    }

    pub fn do_call_outs(&mut self) {
        let due: Vec<usize> = (0..self.env.call_outs.len())
            .filter(|i| self.env.call_outs[*i].next_at_ms <= self.env.time_ms)
            .collect();
        for i in due.into_iter().rev() {
            let c = self.env.call_outs.remove(i);
            if self.env.object(c.object).is_none() {
                continue;
            }
            let slot = self.slot_of(c.object);
            let prevwt = self.write_target;
            if slot.is_some() {
                self.write_target = c.object;
            }
            let res = self.apply_closure(&c.closure, &[c.arg.clone()]);
            self.write_target = prevwt;
            if let Err(Ctrl::Error(e)) = res {
                let msg = format!("call_out error: {}", display_string(&e));
                eprintln!("{}", msg);
                self.env.error_log.push(msg);
            }
            if let Some(p) = c.period_ms {
                self.schedule_call_out(c.object, p, Some(p), c.closure, c.arg);
            }
        }
    }

    pub fn do_heart_beats(&mut self) {
        let list: Vec<ObjId> = self.env.heart_beats.clone();
        for oid in list {
            if self.env.object(oid).is_none() {
                self.env.heart_beats.retain(|x| *x != oid);
                continue;
            }
            if !self.has_lfun(oid, "heart_beat") {
                continue;
            }
            let prevwt = self.write_target;
            if self.slot_of(oid).is_some() {
                self.write_target = oid;
            }
            let r = self.call_function(oid, "heart_beat", &[]);
            self.write_target = prevwt;
            if let Err(Ctrl::Error(e)) = r {
                self.apply_master_opt(
                    "heart_beat_error",
                    &[Svalue::Object(oid), e],
                    Svalue::Int(0),
                );
            }
        }
    }

    pub fn shutdown_driver(&mut self) {
        if self.env.shutting_down {
            return;
        }
        let keep = self.apply_master_opt("notify_shutdown", &[], Svalue::Int(1));
        if truthy(&keep) {
            let slots: Vec<usize> = self.env.interconnects.values().cloned().collect();
            for slot in slots {
                self.push_output(slot, "Driver is shutting down. Goodbye.\n");
            }
        }
        self.apply_master_opt("shutdown", &[], Svalue::Int(0));
        self.env.shutting_down = true;
    }

    // ------------------------------------------------------------------
    //  Command loop / interactivity
    // ------------------------------------------------------------------

    /// Feed one line of entered text for an interactive object.
    pub fn receive_line(&mut self, oid: ObjId, line: &str) {
        if !self.env.interconnects.contains_key(&oid) {
            return;
        }
        self.env.command_giver = oid;
        let prev_wt = self.write_target;
        let outer_was_conn = self.slot_of(prev_wt).is_some();
        self.write_target = oid;
        let pending = self.env.input_to.get(&oid).cloned().flatten();
        if let Some(pending) = pending {
            // flag 0 == repeatable (stays armed); anything else is one-shot
            if pending.flag != 0 {
                self.env.input_to.remove(&oid);
            }
            let args = vec![mk_string(line.to_string())];
            let res = self.apply_closure(&pending.closure, &args);
            if let Err(Ctrl::Error(e)) = res {
                let msg = display_string(&e);
                self.report_error(&msg);
            }
            if !self.env.input_to.contains_key(&oid) {
                // handler consumed: arm command mode and show the prompt
                self.print_prompt(oid);
            }
        } else {
            self.execute_command(line, oid);
            self.print_prompt(oid);
        }
        // Only restore write_target if it wasn't already a connection sink
        if outer_was_conn {
            self.write_target = prev_wt;
        } else {
            self.write_target = 0;
        }
    }

    /// Apply a driver hook value (closure or master lfun name); None if unset.
    pub(crate) fn apply_hook(
        &mut self,
        hook: &Svalue,
        args: &[Svalue],
    ) -> Option<EvResult> {
        match hook {
            Svalue::Null => None,
            cl @ Svalue::Closure(_) => Some(self.apply_closure(cl, args)),
            Svalue::Str(name) => {
                let name = name.to_string();
                let target = if self.env.master != 0 {
                    self.env.master
                } else {
                    self.env.command_giver
                };
                Some(self.call_function(target, &name, args))
            }
            _ => None,
        }
    }

    pub fn set_prompt(&mut self, oid: ObjId, prompt: Svalue) {
        self.env.prompts.insert(oid, prompt);
    }

    /// print_prompt_string() from comm.c: pass the raw prompt through
    /// H_PRINT_PROMPT (22) if set, else write it directly.
    pub fn print_prompt_string(&mut self, s: &str) {
        let hook = self.env.driver_hooks.get(22).cloned().unwrap_or(Svalue::Null);
        match hook {
            Svalue::Null => self.write_out(s),
            cl @ (Svalue::Closure(_) | Svalue::Str(_)) => {
                if let Err(Ctrl::Error(e)) = self.apply_closure_or_name(&cl, &[mk_string(s)]) {
                    let msg = display_string(&e);
                    self.report_error(&msg);
                }
            }
            _ => self.write_out(s),
        }
    }

    fn apply_closure_or_name(&mut self, h: &Svalue, args: &[Svalue]) -> EvResult {
        match h {
            Svalue::Str(name) => {
                let name = name.to_string();
                let ob = self.env.command_giver;
                self.call_function(ob, &name, args)
            }
            other => self.apply_closure(other, args),
        }
    }

    /// print_prompt() from comm.c: input_to prompt, else interactive's own
    /// prompt (closure may print itself and return 0), else H_DEFAULT_PROMPT
    /// (21), else the default "> ".
    pub fn print_prompt(&mut self, oid: ObjId) {
        if !self.env.interconnects.contains_key(&oid) || self.env.shutting_down {
            return;
        }
        if let Some(Some(p)) = self.env.input_to.get(&oid).cloned() {
            if let Some(pr) = p.prompt {
                self.print_prompt_string(&pr);
                return;
            }
        }
        let p = self.env.prompts.get(&oid).cloned().unwrap_or(Svalue::Null);
        if !matches!(p, Svalue::Null) {
            self.print_one_prompt(&p, oid);
            return;
        }
        let dh = self.env.driver_hooks.get(21).cloned().unwrap_or(Svalue::Null);
        if !matches!(dh, Svalue::Null) {
            self.print_one_prompt(&dh, oid);
            return;
        }
        self.print_prompt_string("> ");
    }

    fn print_one_prompt(&mut self, p: &Svalue, oid: ObjId) {
        match p {
            Svalue::Str(s) => self.print_prompt_string(s),
            _ => {
                let oid = self.env.command_giver.max(oid);
                let r = match p {
                    Svalue::Closure(_) => self.apply_closure(p, &[]),
                    Svalue::Str(name) => {
                        let name = name.to_string();
                        self.call_function(oid, &name, &[])
                    }
                    _ => return,
                };
                match r {
                    Ok(v) => {
                        let s = if truthy(&v) { display_string(&v) } else { String::new() };
                        if truthy(&v) {
                            self.print_prompt_string(&s);
                        }
                    }
                    Err(Ctrl::Error(e)) => {
                        let msg = display_string(&e);
                        self.report_error(&msg);
                    }
                    Err(_) => {}
                }
            }
        }
    }

    pub(crate) fn print_prompt_inner(&mut self, oid: ObjId) {
        self.print_prompt(oid)
    }

    /// LDMud execute_command: try H_COMMAND (driver hook 15), then the
    /// registered verbs (actions), then notify_fail / master notify_fail.
    pub fn execute_command(&mut self, str: &str, ob: ObjId) -> bool {
        let str = str.to_string();
        if str.trim().is_empty() {
            return false;
        }
        self.env.last_command = str.clone();
        self.env.last_command_origin = ob;
        // H_COMMAND (17): hook lfun(str, ob) handles it, returns 1/0.
        let h = self.env.driver_hooks.get(17).cloned().unwrap_or(Svalue::Null);
        if !matches!(h, Svalue::Null) {
            if let Some(r) =
                self.apply_hook(&h, &[mk_string(str.clone()), Svalue::Object(ob)])
            {
                return matches!(r, Ok(v) if truthy(&v));
            }
        }
        self.parse_and_run(&str, ob)
    }

    #[allow(clippy::collapsible_if)]
    fn parse_and_run(&mut self, str: &str, ob: ObjId) -> bool {
        let first = str.split_whitespace().next().unwrap_or("").to_string();
        let rest = match str.split_once(char::is_whitespace) {
            Some((_, r)) => r.trim_start().to_string(),
            None => String::new(),
        };
        if first.is_empty() {
            return false;
        }
        // scan action tables of every object with commands enabled,
        // preferring the longest matching verb (LDMud verb matching)
        let actions: Vec<(ObjId, Vec<ActionEntry>)> =
            self.env.actions.clone().into_iter().collect();
        let mut best: Option<(usize, ActionEntry, String)> = None;
        for (oid, entries) in actions {
            if !self.env.commands_enabled.get(&oid).copied().unwrap_or(false) {
                continue;
            }
            for a in entries {
                let matched = if a.verb.is_empty() {
                    Some(0usize)
                } else if first == a.verb && first.len() >= a.min_match_len {
                    Some(first.len())
                } else if first.len() > a.verb.len()
                    && first.starts_with(&a.verb)
                    && a.verb.len() >= a.min_match_len
                {
                    Some(first.len())
                } else {
                    None
                };
                if let Some(m) = matched {
                    let arg = if first.len() > a.verb.len() && !a.verb.is_empty() {
                        if rest.is_empty() {
                            first[a.verb.len()..].to_string()
                        } else {
                            format!("{} {}", &first[a.verb.len()..], rest)
                        }
                    } else {
                        rest.clone()
                    };
                    let better = match &best {
                        Some((bl, _, _)) => m > *bl,
                        None => true,
                    };
                    if better {
                        best = Some((m, a, arg));
                    }
                }
            }
        }
        if let Some((_, action, arg)) = best {
            self.env.last_verb = Some(action.verb.clone());
            let r = self.call_function(
                action.object,
                &action.lfun,
                &[mk_string(arg), Svalue::Object(ob)],
            );
            match r {
                Ok(v) if truthy(&v) => {
                    // LDMud: a successful action is NOT removed; only
                    // remove_action() or destruct clears it.
                    self.env.last_verb = Some(action.verb.clone());
                    return true;
                }
                Ok(_) => {}
                Err(e) => {
                    self.handle_error(e);
                    return true;
                }
            }
        }
        self.env.last_verb = Some(first.clone());
        let mut msg: Option<Svalue> = None;
        // H_SEND_NOTIFY_FAIL (18): may redirect/fabricate failure handling
        if let Some(h) = self.env.driver_hooks.get(18).cloned() {
            if !matches!(h, Svalue::Null) {
                let res = self.apply_closure(&h, &[Svalue::Object(ob), mk_string(str.to_string())]);
                if let Ok(Svalue::Object(nob)) = res {
                    if let Some(Some(nf)) = self.env.notify_fail.get(&nob).cloned() {
                        let uc = nf.orig.clone().unwrap_or_else(|| str.to_string());
                        if let Svalue::Closure(_) = nf.closure {
                            match self.apply_closure(&nf.closure.clone(), &[mk_string(uc)]) {
                                Ok(v) => msg = Some(v),
                                Err(e) => self.handle_error(e),
                            }
                        }
                    }
                }
            }
        }
        // per-object notify_fail, then H_NOTIFY_FAIL (10), then "Huh?"
        if msg.is_none() && self.env.interactive(ob) {
            if let Some(Some(nf)) = self.env.notify_fail.remove(&ob) {
                let use_cmd = nf.orig.clone().unwrap_or_else(|| str.to_string());
                match &nf.closure {
                    cl @ Svalue::Closure(_) => {
                        match self.apply_closure(cl, &[mk_string(use_cmd)]) {
                            Ok(v) => msg = Some(v),
                            Err(e) => self.handle_error(e),
                        }
                    }
                    Svalue::Str(s) => {
                        msg = Some(mk_string(s.replace("%s", &first)));
                    }
                    other => msg = Some(other.clone()),
                }
            }
        }
        if msg.is_none() {
            let h = self.env.driver_hooks.get(10).cloned().unwrap_or(Svalue::Null);
            if let Some(r) = self.apply_hook(&h, &[mk_string(str.clone())]) {
                msg = match r {
                    Ok(v) => Some(v),
                    Err(e) => {
                        self.handle_error(e);
                        None
                    }
                };
            }
        }
        if msg.is_none() {
            msg = Some(mk_string("Huh?\n"));
        }
        if let Some(m) = msg {
            if truthy(&m) {
                let s = display_string(&m);
                self.write_out(&s);
                if !s.ends_with('\n') {
                    self.write_out("\n");
                }
            }
        }
        false
    }

    fn handle_error(&mut self, c: Ctrl) {
        if let Ctrl::Error(s) = c {
            let msg = display_string(&s);
            self.report_error(&msg);
        }
    }

    pub(crate) fn report_error(&mut self, msg: &str) {
        let m = msg.trim_end().to_string();
        let handled = if self.env.master != 0 {
            self.apply_master(
                "runtime_error",
                &[mk_string(m.clone()), Svalue::Object(self.this_object())],
                Svalue::Int(0),
            )
            .map(|v| truthy(&v))
            .unwrap_or(false)
        } else {
            false
        };
        if !handled {
            let t = m.strip_prefix('*').unwrap_or(&m).to_string();
            self.write_out(&format!("*Error:{}\n", t));
        }
        self.env.error_log.push(m);
    }

    pub fn query_verb(&self) -> Option<String> {
        self.env.last_verb.clone()
    }
}

pub fn default_for(ty: &Option<String>) -> Svalue {
    match ty.as_deref() {
        Some("float") => Svalue::Float(0.0),
        Some("string") => mk_string(""),
        Some("mapping") => mk_mapping(vec![]),
        _ => Svalue::Int(0),
    }
}