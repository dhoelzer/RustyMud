//! Compile & load pipeline plus the driver backend: mudlib file resolution,
//! object loading with inheritance, master bootstrap, the interactive
//! backend loop (connections, commands, input_to, call_outs, heart beats).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;

use crate::parse::parse_file;
use crate::program::{flatten, ObjKind, Program};
use crate::value::*;
use crate::vm::{Ctrl, Vm};

pub struct Config {
    pub mudlib_dir: String,
    pub master_file: String, // e.g. "master.c" (relative to mudlib dir)
    pub port: u16,
}

pub fn canon(path: &str) -> String {
    let p = path.replace('\\', "/");
    let mut parts: Vec<String> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s.to_string()),
        }
    }
    format!("/{}", parts.join("/"))
}

// ----------------------------------------------------------------------
//  Compilation & object loading on the Vm
// ----------------------------------------------------------------------

impl Vm {
    pub fn fs_path(&self, canonical: &str) -> String {
        format!("{}{}", self.env.mudlib_dir, canonical)
    }

    fn read_src(&mut self, canonical: &str) -> Result<String, String> {
        // LDMud resolves <name> as <name>.c (simulate.c)
        if !canonical.ends_with(".c") {
            let with_c = format!("{}.c", canonical);
            let p = self.fs_path(&with_c);
            if let Ok(s) = std::fs::read_to_string(&p) {
                self.src_cache.insert(canonical.to_string(), s.clone());
                return Ok(s);
            }
        }
        if let Some(s) = self.src_cache.get(canonical) {
            return Ok(s.clone());
        }
        let p = self.fs_path(canonical);
        let content = std::fs::read_to_string(&p)
            .map_err(|e| format!("Could not open file '{}' for transfer.\n{}", p, e))?;
        self.src_cache.insert(canonical.to_string(), content.clone());
        Ok(content)
    }

    fn mk_pp(&self) -> crate::pp::Preprocessor {
        let mut pp = crate::pp::Preprocessor::new();
        for (k, v) in &self.defines {
            pp.defines.insert(k.clone(), crate::pp::MacroDef { params: None, body: v.clone() });
        }
        pp
    }

    /// Preprocess + parse + flatten a source file into a cached Program.
    pub fn compile(&mut self, path: &str) -> Result<Rc<Program>, String> {
        let canonical = canon(path);
        if let Some(p) = self.env.programs.get(&canonical) {
            return Ok(p.clone());
        }
        let src = self.read_src(&canonical)?;
        let mut pp = self.mk_pp();
        let mudlib = self.env.mudlib_dir.clone();
        let cache = std::mem::take(&mut self.src_cache);
        let cache = std::rc::Rc::new(std::cell::RefCell::new(cache));
        let mut included = Vec::new();
        let cache2 = cache.clone();
        let resolver = move |name: &str| -> Result<String, String> {
            let c = canon(name);
            if let Some(s) = cache2.borrow().get(&c) {
                return Ok(s.clone());
            }
            let p = format!("{}{}", mudlib, c);
            let content = std::fs::read_to_string(&p)
                .map_err(|e| format!("Could not open include file '{}': {}", p, e))?;
            cache2.borrow_mut().insert(c.clone(), content.clone());
            Ok(content)
        };
        let r = pp.run(&src, &resolver, 0, &mut included);
        self.src_cache = Rc::try_unwrap(cache).map(|c| c.into_inner()).unwrap_or_default();
        let expanded =
            r.map_err(|e| format!("Preprocessor error in {}: line {}: {}", canonical, e.line, e.msg))?;
        let compiled = parse_file(&canonical, &expanded)
            .map_err(|e| format!("Error in {}: line {}: {}", canonical, e.line, e.msg))?;
        // load inherits first so flatten can resolve them
        for inh in &compiled.inherits.clone() {
            let ipath = canon(inh);
            if !self.env.programs.contains_key(&ipath) {
                let p = self.compile(&ipath)?;
                self.env.programs.insert(ipath, p);
            }
        }
        let env_programs = self.env.programs.clone();
        let flat = flatten(compiled, &|p: &str| -> Result<Rc<Program>, String> {
            env_programs
                .get(&canon(p))
                .cloned()
                .ok_or_else(|| format!("Could not load inherit {}", p))
        })?;
        let rc = Rc::new(flat);
        self.env.programs.insert(canonical.clone(), rc.clone());
        Ok(rc)
    }

    pub fn canon_path(&self, path: &str) -> String {
        canon(path)
    }









    pub fn find_loaded(&self, canonical: &str) -> Option<ObjId> {
        let canonical = canon(canonical);
        for (oid, o) in &self.env.objects {
            let ob = o.borrow();
            if ob.destructed {
                continue;
            }
            if ob.name == canonical || ob.name.starts_with(&format!("{}#", canonical)) {
                return Some(*oid);
            }
        }
        None
    }

    pub fn objects_loaded(&self, canonical: &str) -> i64 {
        self.env
            .objects
            .values()
            .filter(|o| !o.borrow().destructed && o.borrow().name.starts_with(canonical))
            .count() as i64
    }

    /// load_object semantics: existing object or fresh load + create chain.
    pub fn load_object(&mut self, path: &str) -> Result<ObjId, String> {
        let canonical = canon(path);
        if let Some(o) = self.find_loaded(&canonical) {
            return Ok(o);
        }
        let prog = if let Some(p) = self.env.programs.get(&canonical) {
            p.clone()
        } else {
            if self.env.master != 0 && canonical != canon(&self.master_file) {
                let allow = self.apply_master_opt(
                    "compile_object",
                    &[mk_string(path.to_string()), Svalue::Int(1)],
                    Svalue::Int(1),
                );
                if !truthy(&allow) {
                    return Err(format!("The master object refused to compile {}", path));
                }
            }
            self.compile(&canonical)?
        };
        let oid = self.instantiate(prog, canonical.clone(), ObjKind::Object);
        self.init_vars(oid)?;
        self.apply_master_opt("create_object", &[Svalue::Object(oid)], Svalue::Int(0));
        if self.has_lfun(oid, "create") {
            let r = self.call_function(oid, "create", &[]);
            report_runtime(self, r);
        }
        Ok(oid)
    }

    pub fn load_object_cloned(&mut self, path: &str) -> Result<ObjId, String> {
        let canonical = canon(path);
        if !self.env.programs.contains_key(&canonical) {
            let _ = self.load_object(&canonical)?;
        }
        let prog = self
            .env
            .programs
            .get(&canonical)
            .cloned()
            .ok_or_else(|| format!("clone_object: could not load {}", path))?;
        let mut n = 0;
        let name = loop {
            let cand = format!("{}#{}", canonical, n);
            if self.env.objects.values().all(|o| o.borrow().name != cand) {
                break cand;
            }
            n += 1;
        };
        let oid = self.instantiate(prog, name, ObjKind::Object);
        self.init_vars(oid)?;
        self.apply_master_opt("create_object", &[Svalue::Object(oid)], Svalue::Int(0));
        let _ = self.call_function(oid, "create", &[]);
        Ok(oid)
    }

    pub fn compile_string(&mut self, src: &str) -> Result<ObjId, String> {
        self.anon_counter += 1;
        let name = format!("/Unnamed#{}", self.anon_counter);
        let expanded = {
            let mut pp = self.mk_pp();
            let mudlib = self.env.mudlib_dir.clone();
            let resolver = move |n: &str| -> Result<String, String> {
                let c = canon(n);
                let p = format!("{}{}", mudlib, c);
                std::fs::read_to_string(&p).map_err(|e| e.to_string())
            };
            pp.run(src, &resolver, 0, &mut Vec::new()).map_err(|e| e.msg)?
        };
        let compiled = parse_file(&name, &expanded).map_err(|e| format!("{}:{}", e.line, e.msg))?;
        let flat = flatten(compiled, &|_| Err("inherits not supported in compile_string".into()))?;
        let rc = Rc::new(flat);
        let oid = self.instantiate(rc, name, ObjKind::Object);
        self.init_vars(oid)?;
        Ok(oid)
    }

    pub(crate) fn compile_lambda_unit(src: &str) -> Result<(Rc<Program>, usize, usize), String> {
        // compile a lambda body ("expr" or "(arg1, arg2) => expr") into a
        // standalone Program with one function.
        let (arity, body) = split_lambda_src(src);
        let params: Vec<String> = (1..=arity).map(|i| format!("mixed ${}", i)).collect();
        let ptext = params.join(", ");
        // a lambda body may be a statement sequence ("return x;") or a bare
        // expression ("$1 * 3") -- wrap the latter into a return.
        let b = body.trim();
        let stmts = if b.starts_with("return") {
            b.to_string()
        } else if b.ends_with(';') {
            format!("return ({});", b)
        } else {
            format!("return ({});", b)
        };
        let unit = format!("mixed __lambda__({}) {{ {} }}\n", ptext, stmts);
        let compiled = parse_file("<lambda>", &unit).map_err(|e| format!("{}:{}", e.line, e.msg))?;
        let prog = Program::from_compiled(compiled);
        let entry = *prog.func_ix.get("__lambda__").ok_or("internal: no lambda entry")?;
        Ok((Rc::new(prog), entry, arity))
    }

    // ---- boot ----

    pub fn boot_master(&mut self) -> Result<(), String> {
        let master_canon = canon(&self.master_file);
        let prog = self.compile(&master_canon)?;
        let oid = self.instantiate(prog.clone(), master_canon.clone(), ObjKind::Master);
        self.env.master = oid;
        self.init_vars(oid)?;
        if prog.lookup_func("create").is_some() {
            let r = self.call_function(oid, "create", &[]);
            report_runtime(self, r);
        }
        self.apply_master_opt("inaugurate_master", &[Svalue::Int(1)], Svalue::Int(0));
        // simul efun object
        let sef = self.apply_master("get_simul_efun", &[], Svalue::Int(0));
        if let Ok(Svalue::Str(p)) = sef {
            match self.load_object(&p) {
                Ok(oid2) => self.env.sefun = oid2,
                Err(e) => eprintln!("simul_efun error: {}", e),
            }
        }
        self.apply_master_opt("preload", &[], Svalue::Int(0));
        Ok(())
    }


    // ---- filesystem efuns support ----

    pub(crate) fn file_resolve(&mut self, path: &str) -> String {
        self.fs_path(&canon(path))
    }

    pub fn read_file(&mut self, path: &str) -> Result<String, String> {
        let p = self.file_resolve(path);
        if self.env.master != 0 {
            let ok = self.apply_master_opt(
                "valid_read",
                &[mk_string(p.clone()), Svalue::Object(self.this_object())],
                Svalue::Int(1),
            );
            if !truthy(&ok) {
                return Err(format!("read_file: No permission to read file '{}'.", p));
            }
        }
        std::fs::read_to_string(&p).map_err(|e| format!("read_file: {}: {}", p, e))
    }

    pub fn write_file(&mut self, path: &str, data: &str) -> Result<(), String> {
        let p = self.file_resolve(path);
        if self.env.master != 0 {
            let ok = self.apply_master_opt(
                "valid_write",
                &[mk_string(p.clone()), Svalue::Object(self.this_object())],
                Svalue::Int(1),
            );
            if !truthy(&ok) {
                return Err("No permission to write file".into());
            }
        }
        if let Some(dir) = std::path::Path::new(&p).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&p, data).map_err(|e| e.to_string())
    }

    pub fn get_dir(&mut self, path: &str) -> Result<Vec<String>, String> {
        let base = if path.ends_with('*') { &path[..path.rfind('*').unwrap()] } else { path };
        let p = self.file_resolve(base);
        let entries = std::fs::read_dir(&p).map_err(|e| format!("get_dir: {}: {}", p, e))?;
        let glob = match path.rfind('/') {
            Some(i) if path.ends_with('*') => path[i + 1..path.len() - 1].to_string(),
            _ if path.ends_with('*') => path[..path.len() - 1].to_string(),
            _ => String::new(),
        };
        let mut v = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !glob.is_empty() && !name.starts_with(&glob) {
                continue;
            }
            if e.path().is_dir() {
                v.push(format!("{}/", name));
            } else {
                v.push(name);
            }
        }
        v.sort();
        Ok(v)
    }

    pub fn file_size(&mut self, path: &str) -> i64 {
        let p = self.file_resolve(path);
        match std::fs::metadata(&p) {
            Ok(m) if m.is_dir() => -1,
            Ok(m) => m.len() as i64,
            Err(_) => -2,
        }
    }

    pub fn remove_file(&mut self, path: &str) -> Result<(), String> {
        let p = self.file_resolve(path);
        std::fs::remove_file(&p).map_err(|e| e.to_string())
    }

    pub fn rename_file(&mut self, f: &str, t: &str) -> Result<(), String> {
        let pf = self.file_resolve(f);
        let pt = self.file_resolve(t);
        if let Some(dir) = std::path::Path::new(&pt).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::rename(&pf, &pt).map_err(|e| e.to_string())
    }
}

pub(crate) fn report_runtime(vm: &mut Vm, r: Result<Svalue, Ctrl>) {
    if let Err(Ctrl::Error(e)) = r {
        let msg = display_string(&e);
        eprintln!("*Error: {}\n{}", msg, vm.runtime_trace());
        vm.env.error_log.push(msg);
    }
}

fn split_lambda_src(src: &str) -> (usize, String) {
    let s = src.trim();
    if let Some(pos) = find_arrow(s) {
        let left = s[..pos].trim();
        let body = s[pos + 2..].trim().to_string();
        let arity = if left.starts_with('(') && left.ends_with(')') {
            let inner = &left[1..left.len() - 1];
            if inner.trim().is_empty() {
                0
            } else {
                count_top_commas(inner) + 1
            }
        } else {
            1
        };
        (arity, body)
    } else {
        (1, s.to_string())
    }
}

fn find_arrow(s: &str) -> Option<usize> {
    let b: Vec<char> = s.chars().collect();
    let mut depth = 0i32;
    for i in 0..b.len() {
        match b[i] {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '=' if depth == 0 && i + 1 < b.len() && b[i + 1] == '>' => return Some(i),
            _ => {}
        }
    }
    None
}

fn count_top_commas(s: &str) -> usize {
    let mut depth = 0i32;
    let mut n = 0;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => n += 1,
            _ => {}
        }
    }
    n
}

// ----------------------------------------------------------------------
//  The interactive backend (TCP loop, single-threaded like LDMud's
//  backend.c)
// ----------------------------------------------------------------------

pub struct Backend {
    pub vm: Vm,
    pub cfg: Config,
}

impl Backend {
    pub fn new(cfg: Config) -> Backend {
        let mut vm = Vm::new();
        vm.env.mudlib_dir = cfg.mudlib_dir.clone();
        vm.master_file = cfg.master_file.clone();
        vm.defines.insert("__VERSION__".into(), "\"30300\"".into());
        Backend { vm, cfg }
    }

    pub fn boot_master(&mut self) -> Result<(), String> {
        self.vm.boot_master()
    }
}

struct Conn {
    id: usize,
    stream: TcpStream,
    reader: std::cell::RefCell<BufReader<TcpStream>>,
    buf: Vec<u8>,
    oid: ObjId, // 0 = not yet bound
}

impl Backend {
    pub fn serve(mut self) -> std::io::Result<()> {
        let listener = TcpListener::bind(("0.0.0.0", self.cfg.port))?;
        listener.set_nonblocking(true)?;
        println!(
            "* ldmud-rs driver listening on port {} (mudlib: {})",
            self.cfg.port, self.cfg.mudlib_dir
        );
        let mut conns: Vec<Conn> = Vec::new();
        let mut next_id = 1usize;

        while !self.vm.env.shutting_down {
            // accept new connections (nonblocking)
            if let Ok((stream, _addr)) = listener.accept() {
                let _ = stream.set_nonblocking(true);
                let reader = BufReader::new(stream.try_clone()?);
                conns.push(Conn {
                    id: next_id,
                    stream,
                    reader: std::cell::RefCell::new(reader),
                    buf: Vec::new(),
                    oid: 0,
                });
                next_id += 1;
                // newly connected: ask master immediately
                let oid = self.vm.handle_new_connection(self.slot_next_id_minus_one(next_id));
                if let Some(c) = conns.iter_mut().find(|c| c.oid == 0) {
                    c.oid = oid;
                }
            }
            // read lines
            let mut i = 0;
            while i < conns.len() {
                let ev = {
                    let c = &mut conns[i];
                    match read_nonblocking_line(c.reader.get_mut(), &mut c.buf) {
                        Ok(ev) => ev,
                        Err(_) => LineEvent::Eof,
                    }
                };
                match ev {
                    LineEvent::Eof => {
                        let c = conns.remove(i);
                        let oid = c.oid;
                        if oid != 0 {
                            self.vm.disconnect(oid);
                        }
                        continue;
                    }
                    LineEvent::Nothing => {}
                    LineEvent::Line(l) => {
                        let oid0 = conns[i].oid;
                        if oid0 != 0 {
                            self.vm.receive_line(oid0, &l);
                        }
                    }
                }
                i += 1;
            }
            // timers
            self.vm.env.time_ms = now_ms();
            self.vm.do_call_outs();
            self.vm.do_heart_beats();
            // flush output
            for c in conns.iter_mut() {
                let q: Vec<(usize, Vec<u8>)> =
                    self.vm.out_q.iter().filter(|(s, _)| *s == c.id).cloned().collect();
                if !q.is_empty() {
                    for (_, b) in q {
                        let _ = c.stream.write_all(&b);
                        let _ = c.stream.flush();
                    }
                    self.vm.out_q.retain(|(s, _)| *s != c.id);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // final flush
        for c in conns.iter_mut() {
            for b in self.vm.out_q.iter().filter(|(s, _)| *s == c.id).map(|(_, b)| b.clone()) {
                let _ = c.stream.write_all(&b);
            }
        }
        eprintln!("* Driver down.");
        Ok(())
    }

    fn slot_next_id_minus_one(&self, next_id: usize) -> usize {
        next_id - 1
    }
}

impl Vm {
    /// New raw connection on `slot`: master connect() picks the object,
    /// then logon() runs.
    pub fn handle_new_connection(&mut self, slot: usize) -> ObjId {
        let ob = self.apply_master("connect", &[], Svalue::Int(0));
        let oid = match ob {
            Ok(Svalue::Object(o)) if self.env.object(o).is_some() => o,
            _ => 0,
        };
        if oid == 0 {
            self.push_output(slot, "Connection refused.\n");
            return 0;
        }
        self.env.interconnects.insert(oid, slot);
        self.env.command_giver = oid;
        self.write_target = oid;
        let r = self.call_function(oid, "logon", &[]);
        self.write_target = 0;
        match r {
            Ok(v) if truthy(&v) => {
                self.print_prompt_pub(oid);
                oid
            }
            _ => {
                self.env.interconnects.remove(&oid);
                self.destruct(oid);
                0
            }
        }
    }

    pub fn disconnect(&mut self, oid: ObjId) {
        if self.env.interconnects.contains_key(&oid) {
            self.write_target = oid;
            let _ = self.call_function(oid, "disconnect", &[]);
            self.write_target = 0;
            self.apply_master_opt("remove_player", &[Svalue::Object(oid)], Svalue::Int(0));
            self.env.interconnects.remove(&oid);
            self.push_output(self.slot_of(oid).unwrap_or(99999), "");
            if self.env.command_giver == oid {
                self.env.command_giver = 0;
            }
            self.destruct(oid);
        }
    }

    /// print_prompt for efuns (input_to path)
    pub fn print_prompt_pub(&mut self, oid: ObjId) {
        let _ = self.print_prompt(oid);
    }
}

pub enum LineEvent {
    Nothing,
    Line(String),
    Eof,
}

fn take_line(buf: &mut Vec<u8>) -> Option<String> {
    if let Some(pos) = buf.iter().position(|&c| c == b'\n' || c == b'\r') {
        let mut end = pos;
        if buf[pos] == b'\r' && buf.get(pos + 1) == Some(&b'\n') {
            end += 1;
        }
        let line_bytes: Vec<u8> = buf.drain(..=end).collect();
        let line = String::from_utf8_lossy(&line_bytes)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        return Some(line);
    }
    None
}

fn read_nonblocking_line(
    reader: &mut BufReader<TcpStream>,
    buf: &mut Vec<u8>,
) -> std::io::Result<LineEvent> {
    match reader.fill_buf() {
        Ok(b) if !b.is_empty() => {
            let b: Vec<u8> = b.to_vec();
            reader.consume(b.len());
            buf.extend_from_slice(&b);
        }
        Ok(_) => {
            return Ok(LineEvent::Eof);
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(e) => return Err(e),
    }
    if let Some(l) = take_line(buf) {
        return Ok(LineEvent::Line(l));
    }
    Ok(LineEvent::Nothing)
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
