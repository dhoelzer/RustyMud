//! Efuns: the driver built-in functions callable from LPC — the Rust
//! analogue of LDMud's edriver.c plus internal/arrays, internal/ascii,
//! internal/mapping, internal/math, internal/strings, main_file,
//! main_package and internal/interpret builtins.

use std::cell::RefCell;
use std::rc::Rc;

use crate::value::*;
use crate::vm::{default_for, ActionEntry, Ctrl, EvResult, InputTo, NotifyFail, Vm};

/// Bind a string lfun name to a Closure::Func on the current object
/// (LDMud stores handler strings bound to the setting object).
fn bind_lfun_name(vm: &Vm, v: &Svalue) -> Svalue {
    match v {
        Svalue::Str(name) => Svalue::Closure(Rc::new(Closure::Func {
            oid: vm.this_object(),
            name: name.clone(),
        })),
        other => other.clone(),
    }
}

/// (name, min_argc); -1 = variadic.
pub const EFUNS: &[(&str, i32)] = &[
    ("write", 1), ("sprintf", -1), ("printf", -1), ("to_string", 1),
    ("sizeof", 1), ("strlen", 1), ("capitalize", 1), ("lower_case", 1),
    ("upper_case", 1), ("trim", -1), ("trim_ll", 1), ("trim_rr", 1),
    ("implode", -1), ("explode", -1), ("member_array", -1), ("filter", -1),
    ("map", -1), ("sort_array", -1), ("reorder", 2), ("unique_array", -1),
    ("allocate", -1), ("allocate_mapping", -1), ("keys", 1), ("values", 1),
    ("mapping_extract", 1), ("m_indices", 1), ("m_values", 1),
    ("m_delete", -1), ("m_add", -1), ("m_allocate", -1), ("m_contains", 2),
    ("abs", 1), ("min", -1), ("max", -1), ("random", 1),
    ("to_int", -1), ("to_float", -1), ("to_array", -1),
    ("this_object", 0), ("previous_object", -1), ("call_other", -1),
    ("query_privilege", 1), ("object_name", 1),
    ("load_object", 1), ("clone_object", 1), ("destruct", -1),
    ("objectp", 1), ("stringp", 1), ("intp", 1), ("floatp", 1),
    ("arrayp", 1), ("mappingp", 1), ("closurep", 1),
    ("find_object", 1), ("objects", -1), ("all_inventory", 1),
    ("environment", 1), ("all_environment", -1), ("inventory", 1),
    ("move_object", -1), ("this_environment", -1),
    ("this_interactive", 0), ("interactive", -1), ("input_to", -1),
    ("remove_interactive", -1),    ("query_verb", 0),
    ("set_prompt", 1), ("add_driver_hook", 2), ("query_driver_hook", 1),
    ("speak_to", 2), ("tell_object", 2), ("tell_room", -1),
    ("add_action", -1), ("remove_action", -1), ("enable_commands", 0),
    ("disable_commands", 0), ("notify_fail", -1), ("command", -1),
    ("set_driver_hook", -1), ("get_driver_hook", 1),
    ("call_out", -1), ("remove_call_out", -1), ("call_out_info", 0),
    ("set_heart_beat", -1), ("heart_beat_info", 0),
    ("time", 0), ("utime", 0), ("time_stamp", -1), ("ctime", -1),
    ("gc", 0), ("shallow", 1), ("deep_inventory", 1),
    ("file_name", 1), ("replace_string", -1),
    ("strstr", -1), ("sscanf", -1), ("index", -1), ("rindex", -1),
    ("compare", 2), ("compare_string", 2),
    ("lambda", -1), ("bind_lambda", -1), ("unbound_lambda", -1),
    ("apply", -1), ("apply_low", -1), ("funcall", -1),
    ("catch", -1), ("throw", -1), ("error", -1),
    ("member", -1),
    ("functions", -1), ("local_variables", -1), ("debug_info", -1),
    ("query_local_variables", -1),
    ("read_file", -1), ("write_file", -1), ("get_dir", 1), ("file_size", 1),
    ("mkdir", 1), ("remove_file", 1), ("rename", 2), ("file_time", 1),
    ("compile_string", 1), ("assert", -1),
    ("shutdown", -1), ("shutdown_at_exit", 1),
    ("set_every", -1), ("call_stack", 0),
    ("master", 0), ("simul_efun", 0), ("query_environment", -1),
    ("present", -1),
];

pub fn is_efun(name: &str) -> bool {
    EFUNS.iter().any(|(n, _)| *n == name)
}

pub fn efun_arity(name: &str) -> i32 {
    EFUNS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, a)| *a)
        .unwrap_or(0)
}

/// Resolve an object value to an object id.
pub fn obj_id(v: &Svalue) -> Option<ObjId> {
    match v {
        Svalue::Object(o) if *o != 0 => Some(*o),
        _ => None,
    }
}

pub fn object_value(oid: ObjId) -> Svalue {
    Svalue::Object(oid)
}

pub fn estr(m: String) -> Ctrl {
    Ctrl::Error(mk_string(m))
}

fn lfun_name(_vm: &mut Vm, v: &Svalue) -> String {
    match v {
        Svalue::Str(s) => s.to_string(),
        Svalue::Closure(rc) => match &**rc {
            Closure::Func { name, .. } => name.to_string(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn as_int_ok(v: &Svalue) -> Option<i64> {
    as_int(v).ok()
}

fn to_seq(v: &Svalue) -> Option<Vec<Svalue>> {
    match v {
        Svalue::Array(a) => Some(a.borrow().clone()),
        Svalue::Str(s) => Some(s.chars().map(mk_char).collect()),
        Svalue::Mapping(m) => Some(m.borrow().keys()),
        _ => None,
    }
}

/// Does a closure value name the given lfun? (for remove_call_out)
pub fn closure_named(cl: &Svalue, fun: &str) -> bool {
    match cl {
        Svalue::Closure(rc) => match &**rc {
            Closure::Func { name, .. } => name.as_ref() == fun,
            _ => false,
        },
        Svalue::Str(s) => s.as_ref() == fun,
        _ => false,
    }
}

// ---------------------------------------------------------------------

pub fn call_efun(vm: &mut Vm, name: &str, args: &[Svalue]) -> EvResult {
    let need = |n: usize| -> Result<(), Ctrl> {
        if args.len() < n {
            Err(estr(format!(
                "Wrong number of arguments to {}. Expected at least {}, got {}.",
                name,
                n,
                args.len()
            )))
        } else {
            Ok(())
        }
    };
    fn arg_str(args: &[Svalue], i: usize) -> Result<String, Ctrl> {
        as_string(&args[i]).map_err(estr).map(|s| s.to_string())
    }
    fn arg_int(args: &[Svalue], i: usize) -> Result<i64, Ctrl> {
        as_int(&args[i]).map_err(estr)
    }

    match name {
        // ---------------- I/O ----------------
        "write" => {
            need(1)?;
            let s = display_string(&args[0]);
            vm.write_out(&s);
            if !s.ends_with('\n') {
                vm.write_out("\n");
            }
            Ok(Svalue::Int(0))
        }
        "printf" => {
            need(1)?;
            let s = do_sprintf(args).map_err(estr)?;
            vm.write_out(&s);
            Ok(Svalue::Int(0))
        }
        "sprintf" => {
            need(1)?;
            let s = do_sprintf(args).map_err(estr)?;
            Ok(mk_string(s))
        }
        "to_string" => {
            need(1)?;
            Ok(mk_string(print_string(&args[0])))
        }
        "debug_info" => Ok(mk_string("".to_string())),

        // ---------------- sized ----------------
        "sizeof" => {
            need(1)?;
            Ok(Svalue::Int(match &args[0] {
                Svalue::Str(s) => s.chars().count() as i64,
                Svalue::Array(a) => a.borrow().len() as i64,
                Svalue::Mapping(m) => m.borrow().len() as i64,
                Svalue::Null => -1,
                Svalue::Closure(rc) => match &**rc {
                    Closure::Lambda { nargs, .. } => *nargs as i64,
                    _ => 0,
                },
                _ => 0,
            }))
        }
        "strlen" => {
            need(1)?;
            Ok(Svalue::Int(as_string(&args[0]).map_err(estr)?.chars().count() as i64))
        }

        // ---------------- strings ----------------
        "capitalize" => {
            need(1)?;
            let s = arg_str(args, 0)?;
            let mut it = s.chars();
            let first: String = it.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            Ok(mk_string(format!("{}{}", first, it.as_str())))
        }
        "upper_case" => {
            need(1)?;
            if let Some(seq) = to_seq(&args[0]) {
                let joined: String = seq.iter().map(display_string).collect();
                Ok(mk_array(
                    joined.to_uppercase().chars().map(mk_char).collect(),
                ))
            } else {
                Ok(mk_string(arg_str(args, 0)?.to_uppercase()))
            }
        }
        "lower_case" => {
            need(1)?;
            if let Some(seq) = to_seq(&args[0]) {
                let joined: String = seq.iter().map(display_string).collect();
                Ok(mk_array(
                    joined.to_lowercase().chars().map(mk_char).collect(),
                ))
            } else {
                Ok(mk_string(arg_str(args, 0)?.to_lowercase()))
            }
        }
        "trim" => {
            need(1)?;
            Ok(mk_string(arg_str(args, 0)?.trim().to_string()))
        }
        "trim_ll" => {
            need(1)?;
            Ok(mk_string(arg_str(args, 0)?.trim_start().to_string()))
        }
        "trim_rr" => {
            need(1)?;
            Ok(mk_string(arg_str(args, 0)?.trim_end().to_string()))
        }
        "implode" => {
            need(2)?;
            let seq = to_seq(&args[0]).ok_or_else(|| estr("implode: not an array".into()))?;
            let glue = match &args[1] {
                Svalue::Str(g) => g.to_string(),
                _ => display_string(&args[1]),
            };
            let parts: Vec<String> = seq.iter().map(display_string).collect();
            Ok(mk_string(parts.join(&glue)))
        }
        "explode" => {
            need(2)?;
            let s = arg_str(args, 0)?;
            let parts: Vec<Svalue> = match &args[1] {
                Svalue::Str(g) if g.is_empty() => s.chars().map(mk_char).collect(),
                Svalue::Str(g) => s.split(g.as_ref()).map(mk_string).collect(),
                Svalue::Array(g) => {
                    let gs: Vec<String> = g.borrow().iter().map(display_string).collect();
                    let mut out = Vec::new();
                    for gl in &gs {
                        if s.contains(gl.as_str()) {
                            for sub in s.split(gl.as_str()) {
                                if !sub.is_empty() {
                                    out.push(mk_string(sub));
                                }
                            }
                            break;
                        }
                    }
                    if out.is_empty() {
                        out.push(mk_string(s.clone()));
                    }
                    out
                }
                _ => return Err(estr("explode: bad glue".into())),
            };
            Ok(mk_array(parts))
        }
        "replace_string" => {
            need(3)?;
            let s = arg_str(args, 0)?;
            let from: Vec<String> = match &args[1] {
                Svalue::Str(x) => vec![x.to_string()],
                Svalue::Array(a) => a.borrow().iter().map(display_string).collect(),
                _ => return Err(estr("replace_string: bad from".into())),
            };
            let to: Vec<String> = match &args[2] {
                Svalue::Str(x) => vec![x.to_string()],
                Svalue::Array(a) => a.borrow().iter().map(display_string).collect(),
                _ => return Err(estr("replace_string: bad to".into())),
            };
            let mut out = s;
            for (i, f) in from.iter().enumerate() {
                let r = to.get(i).cloned().unwrap_or_default();
                out = out.replace(f.as_str(), &r);
            }
            Ok(mk_string(out))
        }
        "strstr" => {
            need(2)?;
            let s = arg_str(args, 0)?;
            let sub = arg_str(args, 1)?;
            match s.find(sub.as_str()) {
                Some(b) => Ok(Svalue::Int(s[..b].chars().count() as i64)),
                None => Ok(Svalue::Int(-1)),
            }
        }
        "index" => {
            need(2)?;
            let idx = match &args[0] {
                Svalue::Str(s) => {
                    let sub = arg_str(args, 1)?;
                    s.find(sub.as_str()).map(|b| s[..b].chars().count() as i64)
                }
                Svalue::Array(a) => {
                    let pat = arg_str(args, 1)?;
                    a.borrow()
                        .iter()
                        .position(|e| display_string(e) == pat)
                        .map(|i| i as i64)
                }
                Svalue::Mapping(m) => {
                    if m.borrow().get(&args[1]).is_some() {
                        Some(0)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            Ok(Svalue::Int(idx.unwrap_or(-1)))
        }
        "rindex" => {
            need(2)?;
            let s = arg_str(args, 0)?;
            let sub = arg_str(args, 1)?;
            match s.rfind(sub.as_str()) {
                Some(b) => Ok(Svalue::Int(s[..b].chars().count() as i64)),
                None => Ok(Svalue::Int(-1)),
            }
        }
        "compare" | "compare_string" => {
            need(2)?;
            use std::cmp::Ordering;
            if let (Svalue::Str(a), Svalue::Str(b)) = (&args[0], &args[1]) {
                return Ok(Svalue::Int(match a.cmp(b) {
                    Ordering::Less => -1,
                    Ordering::Equal => 0,
                    Ordering::Greater => 1,
                }));
            }
            if let (Some(a), Some(b)) = (to_seq(&args[0]), to_seq(&args[1])) {
                let n = a.len().min(b.len());
                for i in 0..n {
                    match cmp(&a[i], &b[i]) {
                        Ordering::Less => return Ok(Svalue::Int(-1)),
                        Ordering::Greater => return Ok(Svalue::Int(1)),
                        Ordering::Equal => {}
                    }
                }
                return Ok(Svalue::Int(a.len() as i64 - b.len() as i64));
            }
            Ok(Svalue::Int(-1))
        }

        // ---------------- arrays & mappings ----------------
        "member" | "member_array" => {
            need(2)?;
            let seq = to_seq(&args[1]).ok_or_else(|| estr(format!("{}: bad argument", name)))?;
            for (i, e) in seq.iter().enumerate() {
                if eq(e, &args[0]) {
                    return Ok(Svalue::Int(i as i64));
                }
            }
            Ok(Svalue::Int(-1))
        }
        "filter" => {
            need(2)?;
            let seq = to_seq(&args[0]).ok_or_else(|| estr("filter: bad sequence".into()))?;
            let cl = args[1].clone();
            let mut out = Vec::new();
            for e in seq {
                match vm.apply_closure(&cl, &[e.clone()]) {
                    Ok(v) => {
                        if truthy(&v) {
                            out.push(e);
                        }
                    }
                    Err(c) => return Err(c),
                }
            }
            Ok(mk_array(out))
        }
        "map" => {
            need(2)?;
            let seq =
                to_seq(&args[0]).ok_or_else(|| estr("map: unsupported sequence type".into()))?;
            let cl = args[1].clone();
            let mut out = Vec::new();
            for e in seq {
                match vm.apply_closure(&cl, &[e.clone()]) {
                    Ok(v) => out.push(v),
                    Err(c) => return Err(c),
                }
            }
            Ok(mk_array(out))
        }
        "sort_array" => {
            need(1)?;
            match &args[0] {
                Svalue::Mapping(_) => Ok(args[0].clone()),
                v => {
                    let seq =
                        to_seq(v).ok_or_else(|| estr("sort_array: unsupported sequence".into()))?;
                    let mut vec = seq;
                    match args.get(1) {
                        Some(x) if matches!(x, Svalue::Closure(_)) => {
                            let cl = x.clone();
                            let mut sorted: Vec<Svalue> = Vec::new();
                            for item in vec.drain(..) {
                                let mut placed = false;
                                for (i, s) in sorted.iter().enumerate() {
                                    match vm.apply_closure(&cl, &[item.clone(), s.clone()]) {
                                        Ok(r) => {
                                            if truthy(&r) {
                                                sorted.insert(i, item.clone());
                                                placed = true;
                                                break;
                                            }
                                        }
                                        Err(c) => return Err(c),
                                    }
                                }
                                if !placed {
                                    sorted.push(item);
                                }
                            }
                            vec = sorted;
                        }
                        Some(Svalue::Int(2)) => {
                            vec.sort_by(|a, b| {
                                let ka = a.as_vec().and_then(|v| v.borrow().first().cloned());
                                let kb = b.as_vec().and_then(|v| v.borrow().first().cloned());
                                cmp(&ka.unwrap_or(Svalue::Int(0)), &kb.unwrap_or(Svalue::Int(0)))
                            });
                        }
                        _ => vec.sort_by(cmp),
                    }
                    Ok(mk_array(vec))
                }
            }
        }
        "reorder" => {
            need(2)?;
            let seq = to_seq(&args[0]).ok_or_else(|| estr("reorder: bad argument".into()))?;
            let ord = args[1].as_vec().ok_or_else(|| estr("reorder: bad order".into()))?;
            let mut out = Vec::new();
            for o in ord.borrow().iter() {
                let i = as_int(o).map_err(estr)?;
                let i = if i < 0 {
                    (seq.len() as i64 + i) as usize
                } else {
                    i as usize
                };
                out.push(seq.get(i).cloned().unwrap_or(Svalue::Int(0)));
            }
            Ok(mk_array(out))
        }
        "unique_array" => {
            need(1)?;
            let seq = to_seq(&args[0]).ok_or_else(|| estr("unique_array: bad argument".into()))?;
            let mut out: Vec<Svalue> = Vec::new();
            for e in seq {
                if !out.iter().any(|x| eq(x, &e)) {
                    out.push(e);
                }
            }
            Ok(mk_array(out))
        }
        "allocate" => {
            need(1)?;
            let n = arg_int(args, 0)?.max(0) as usize;
            let fill = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| default_for(&None));
            Ok(mk_array((0..n).map(|_| deep_copy(&fill)).collect()))
        }
        "allocate_mapping" => {
            let n = args.first().and_then(as_int_ok).unwrap_or(0).max(0) as usize;
            let fill = args.get(1).cloned().unwrap_or(Svalue::Int(0));
            let keys: Vec<Svalue> = (0..n).map(|i| Svalue::Int((i + 1) as i64)).collect();
            Ok(mk_mapping(keys.into_iter().map(|k| (k, deep_copy(&fill))).collect()))
        }
        "keys" => {
            need(1)?;
            match &args[0] {
                Svalue::Mapping(m) => Ok(mk_array(m.borrow().keys())),
                _ => Err(estr("keys: not a mapping".into())),
            }
        }
        "values" => {
            need(1)?;
            match &args[0] {
                Svalue::Mapping(m) => Ok(mk_array(m.borrow().values())),
                Svalue::Array(_) => Ok(args[0].clone()),
                _ => Err(estr("values: not a mapping".into())),
            }
        }
        "mapping_extract" => {
            need(1)?;
            match &args[0] {
                Svalue::Mapping(m) => {
                    let b = m.borrow();
                    let e = b.items();
                    Ok(mk_array(vec![
                        mk_array(e.iter().map(|(k, _)| k.clone()).collect()),
                        mk_array(e.iter().map(|(_, v)| v.clone()).collect()),
                    ]))
                }
                _ => Err(estr("mapping_extract: not a mapping".into())),
            }
        }
        "m_indices" => {
            need(1)?;
            match &args[0] {
                Svalue::Mapping(m) => Ok(mk_array(m.borrow().keys())),
                Svalue::Array(a) => {
                    Ok(mk_array((0..a.borrow().len()).map(|i| Svalue::Int(i as i64)).collect()))
                }
                _ => Err(estr("m_indices: not a mapping".into())),
            }
        }
        "m_values" => {
            need(1)?;
            match &args[0] {
                Svalue::Mapping(m) => Ok(mk_array(m.borrow().values())),
                Svalue::Array(a) => Ok(mk_array(a.borrow().clone())),
                _ => Err(estr("m_values: not a mapping".into())),
            }
        }
        "m_delete" => {
            need(2)?;
            match &args[0] {
                Svalue::Mapping(m) => {
                    let mut b = m.borrow_mut();
                    if b.unset(&args[1]) {
                        Ok(Svalue::Int(1))
                    } else {
                        Ok(Svalue::Int(0))
                    }
                }
                Svalue::Array(a) => {
                    let i = arg_int(args, 1)?;
                    let mut b = a.borrow_mut();
                    if i >= 0 && (i as usize) < b.len() {
                        b.remove(i as usize);
                        Ok(Svalue::Int(1))
                    } else {
                        Ok(Svalue::Int(0))
                    }
                }
                _ => Err(estr("m_delete: bad argument".into())),
            }
        }
        "m_contains" => {
            need(2)?;
            match &args[0] {
                Svalue::Mapping(m) => {
                    Ok(Svalue::Int(if m.borrow().get(&args[1]).is_some() { 1 } else { 0 }))
                }
                _ => Ok(Svalue::Int(0)),
            }
        }
        "m_add" => {
            if args.is_empty() {
                return Ok(mk_mapping(vec![]));
            }
            let base = match &args[0] {
                Svalue::Mapping(m) => m.clone(),
                _ => Rc::new(RefCell::new(Mapping::new())),
            };
            for a in args.iter().skip(1) {
                if let Svalue::Mapping(m) = a {
                    let adds: Vec<(Svalue, Svalue)> = m.borrow().items().to_vec();
                    {
                        let mut b = base.borrow_mut();
                        for (k, v) in adds {
                            b.set(k, v);
                        }
                    }
                }
            }
            Ok(Svalue::Mapping(base))
        }
        "m_allocate" => {
            if let (Some(ks), Some(vs)) = (
                args.first().and_then(|x| x.as_vec()),
                args.get(1).and_then(|x| x.as_vec()),
            ) {
                let k: Vec<Svalue> = ks.borrow().clone();
                let v: Vec<Svalue> = vs.borrow().clone();
                Ok(mk_mapping(k.into_iter().zip(v).collect()))
            } else {
                Ok(mk_mapping(vec![]))
            }
        }

        // ---------------- math ----------------
        "abs" => match &args[0] {
            Svalue::Int(i) => Ok(Svalue::Int(i.abs())),
            Svalue::Float(f) => Ok(Svalue::Float(f.abs())),
            v => Err(estr(format!("abs: bad argument {}", dbg(v)))),
        },
        "min" => {
            let v: Vec<Svalue> = match &args[0] {
                Svalue::Array(a) => a.borrow().clone(),
                _ => args.to_vec(),
            };
            Ok(v.into_iter().min_by(cmp).unwrap_or(Svalue::Int(0)))
        }
        "max" => {
            let v: Vec<Svalue> = match &args[0] {
                Svalue::Array(a) => a.borrow().clone(),
                _ => args.to_vec(),
            };
            Ok(v.into_iter().max_by(cmp).unwrap_or(Svalue::Int(0)))
        }
        "random" => {
            need(1)?;
            let n = arg_int(args, 0)?;
            if n <= 0 {
                return Ok(Svalue::Int(0));
            }
            Ok(Svalue::Int(vm.rnd(n as u64) as i64))
        }

        // ---------------- conversions ----------------
        "to_int" => Ok(Svalue::Int(match &args[0] {
            Svalue::Mapping(m) => m.borrow().len() as i64,
            Svalue::Str(s) => s.parse::<i64>().unwrap_or(0),
            v => as_int(v).unwrap_or(0),
        })),
        "to_float" => match &args[0] {
            Svalue::Str(s) => Ok(Svalue::Float(s.parse::<f64>().unwrap_or(0.0))),
            v => Ok(Svalue::Float(as_float(v).unwrap_or(0.0))),
        },
        "to_array" => {
            let seq = match args.get(0).cloned().unwrap_or(Svalue::Int(0)) {
                Svalue::Mapping(m) => m
                    .borrow()
                    .items()
                    .iter()
                    .map(|(k, v)| mk_array(vec![k.clone(), v.clone()]))
                    .collect(),
                Svalue::Array(a) => a.borrow().clone(),
                Svalue::Str(s) => s.chars().map(mk_char).collect(),
                x => vec![x],
            };
            Ok(mk_array(seq))
        }

        // ---------------- objects ----------------
        "this_object" => Ok(object_value(vm.this_object())),
        "previous_object" => {
            let depth = args.first().and_then(as_int_ok).unwrap_or(1);
            Ok(object_value(vm.previous_n_object(depth)))
        }
        "call_other" => {
            need(2)?;
            let oid = match obj_id(&args[0]) {
                Some(o) => o,
                None => {
                    if let Svalue::Str(p) = &args[0] {
                        match vm.load_object(p) {
                            Ok(o) => o,
                            Err(e) => return Err(estr(e)),
                        }
                    } else {
                        return Ok(Svalue::Int(0));
                    }
                }
            };
            let lfun = lfun_name(vm, &args[1]);
            if lfun.is_empty() {
                return Ok(Svalue::Int(0));
            }
            vm.call_function(oid, &lfun, &args[2..])
        }
        "query_privilege" => Ok(mk_string(format!(
            "priv-{}",
            vm.env
                .object_name(vm.this_object())
                .unwrap_or_else(|| "?".into())
        ))),
        "object_name" | "file_name" => {
            need(1)?;
            let oid = match &args[0] {
                Svalue::Object(o) if *o != 0 => *o,
                Svalue::Int(0) => vm.this_object(),
                _ => return Ok(Svalue::Int(0)),
            };
            match vm.env.object_name(oid) {
                Some(n) => Ok(mk_string(n)),
                None => Ok(Svalue::Int(0)),
            }
        }
        "load_object" => {
            need(1)?;
            let p = arg_str(args, 0)?;
            match vm.load_object(&p) {
                Ok(o) => Ok(object_value(o)),
                Err(e) => Err(estr(format!("{}: {}", name, e))),
            }
        }
        "clone_object" => {
            need(1)?;
            let p = arg_str(args, 0)?;
            match vm.load_object_cloned(&p) {
                Ok(o) => Ok(object_value(o)),
                Err(e) => Err(estr(format!("{}: {}", name, e))),
            }
        }
        "destruct" => {
            if let Some(o) = args.first().and_then(obj_id) {
                vm.destruct(o);
            } else if args.is_empty() || matches!(args.first(), Some(Svalue::Int(0))) {
                vm.destruct(vm.this_object());
            }
            Ok(Svalue::Int(1))
        }
        "objectp" => Ok(Svalue::Int(match &args[0] {
            Svalue::Object(o) if *o != 0 => 1,
            _ => 0,
        })),
        "stringp" => Ok(Svalue::Int(if matches!(args[0], Svalue::Str(_)) { 1 } else { 0 })),
        "intp" => Ok(Svalue::Int(if matches!(args[0], Svalue::Int(_)) { 1 } else { 0 })),
        "floatp" => Ok(Svalue::Int(if matches!(args[0], Svalue::Float(_)) { 1 } else { 0 })),
        "arrayp" => Ok(Svalue::Int(if matches!(args[0], Svalue::Array(_)) { 1 } else { 0 })),
        "mappingp" => Ok(Svalue::Int(if matches!(args[0], Svalue::Mapping(_)) { 1 } else { 0 })),
        "closurep" => Ok(Svalue::Int(if matches!(args[0], Svalue::Closure(_)) { 1 } else { 0 })),
        "find_object" => {
            need(1)?;
            let p = arg_str(args, 0)?;
            match vm.find_loaded(&p) {
                Some(o) => Ok(object_value(o)),
                None => Ok(Svalue::Int(0)),
            }
        }
        "objects" => {
            let filter: Option<String> = match args.first() {
                Some(Svalue::Str(p)) => Some(p.replace('*', "")),
                _ => None,
            };
            let mut v = Vec::new();
            for oid in vm.all_object_ids() {
                if let Some(n) = vm.env.object_name(oid) {
                    if filter.as_ref().map(|f| n.contains(f.as_str())).unwrap_or(true) {
                        v.push(object_value(oid));
                    }
                }
            }
            Ok(mk_array(v))
        }
        "inventory" => {
            need(1)?;
            let oid = match obj_id(&args[0]) {
                Some(o) => o,
                None => return Ok(mk_array(vec![])),
            };
            let v = vm.inventory(oid, false);
            Ok(mk_array(v.into_iter().map(object_value).collect()))
        }
        "all_inventory" | "deep_inventory" => {
            need(1)?;
            let oid = match obj_id(&args[0]) {
                Some(o) => o,
                None => return Ok(mk_array(vec![])),
            };
            let v = vm.inventory(oid, true);
            Ok(mk_array(v.into_iter().map(object_value).collect()))
        }
        "environment" => {
            let oid = match &args[0] {
                Svalue::Object(o) if *o != 0 => *o,
                _ => vm.this_object(),
            };
            match vm.env_of(oid) {
                0 => Ok(Svalue::Int(0)),
                e => Ok(object_value(e)),
            }
        }
        "query_environment" | "this_environment" => {
            let oid = match args.first().and_then(obj_id) {
                Some(o) => o,
                None => vm.this_object(),
            };
            match vm.env_of(oid) {
                0 => Ok(Svalue::Int(0)),
                e => Ok(object_value(e)),
            }
        }
        "all_environment" => {
            let oid = match args.first().and_then(obj_id) {
                Some(o) => o,
                None => vm.this_object(),
            };
            let mut v = Vec::new();
            let mut cur = oid;
            while cur != 0 {
                if let Some(o) = vm.env.object(cur) {
                    v.push(object_value(cur));
                    cur = o.borrow().env;
                } else {
                    break;
                }
            }
            Ok(mk_array(v))
        }
        "move_object" => {
            need(2)?;
            let ob = obj_id(&args[0]).unwrap_or_else(|| vm.this_object());
            let dest = args[1]
                .as_object(vm)
                .ok_or_else(|| estr("move_object: destination must be an object".into()))?;
            vm.move_object(ob, dest);
            Ok(Svalue::Int(0))
        }
        "present" => {
            need(1)?;
            let pat = arg_str(args, 0)?;
            let oid = match args.get(1) {
                Some(x) => obj_id(x).unwrap_or(0),
                None => vm.env.command_giver,
            };
            if oid == 0 {
                return Ok(Svalue::Int(0));
            }
            for inv in vm.inventory(oid, true) {
                if let Some(n) = vm.env.object_name(inv) {
                    if n.ends_with(pat.as_str()) || n == pat {
                        if vm.has_lfun(inv, "id") {
                            match vm.call_function(inv, "id", &[mk_string(pat.clone())]) {
                                Ok(r) if truthy(&r) => return Ok(object_value(inv)),
                                _ => {}
                            }
                        }
                        return Ok(object_value(inv));
                    }
                }
            }
            Ok(Svalue::Int(0))
        }

        // ---------------- interactivity ----------------
        "this_interactive" => Ok(object_value(vm.env.command_giver)),
        "interactive" => match args.first() {
            Some(Svalue::Object(o)) if vm.env.interactive(*o) => Ok(mk_string("this_player")),
            None if vm.env.interactive(vm.this_object()) => Ok(mk_string("this_player")),
            _ => Ok(Svalue::Int(0)),
        },
        "input_to" => {
            need(1)?;
            let oid = vm.this_object();
            let cl = match &args[0] {
                Svalue::Str(name) => {
                    // LDMud: a string lfun name is bound to the setting object
                    let name: Rc<str> = name.clone();
                    vm.env.objects.get(&oid); // no-op borrow guard
                    Svalue::Closure(Rc::new(Closure::Func { oid, name }))
                }
                Svalue::Closure(_) => args[0].clone(),
                _ => return Err(estr("input_to: bad argument".into())),
            };
            let flag = args.get(1).and_then(as_int_ok).unwrap_or(-1);
            let prompt = match args.get(2) {
                Some(Svalue::Str(p)) => Some(p.to_string()),
                Some(c @ Svalue::Closure(_)) => {
                    let c = c.clone();
                    match vm.apply_closure(&c, &[]) {
                        Ok(Svalue::Str(p)) => Some(p.to_string()),
                        Ok(v) if truthy(&v) => Some(display_string(&v)),
                        _ => None,
                    }
                }
                _ => None,
            };
            vm.env.input_to.insert(oid, Some(InputTo { closure: cl, flag, prompt }));
            Ok(Svalue::Int(0))
        }
        "query_verb" => Ok(match vm.env.last_verb.clone() {
            Some(v) => mk_string(v),
            None => Svalue::Int(0),
        }),
        "set_prompt" => {
            // set_prompt(closure|string|0) — LDMud semantics: 0 restores the
            // default prompt, closures print/return the prompt each round.
            let oid = vm.this_object();
            vm.set_prompt(oid, args[0].clone());
            Ok(Svalue::Int(1))
        }
        "add_driver_hook" => {
            let n = as_int(&args[0]).map_err(|e| Ctrl::Error(mk_string(e)))? as usize;
            if n >= vm.env.driver_hooks.len() {
                return Err(Ctrl::Error(mk_string(format!(
                    "add_driver_hook(): hook number {} out of range.\n",
                    n
                ))));
            }
            match &args[1] {
                Svalue::Str(s) => {
                    if !vm.has_lfun(vm.this_object(), s) {
                        return Err(Ctrl::Error(mk_string(format!(
                            "add_driver_hook(): invalid function or closure for hook {}\n",
                            n
                        ))));
                    }
                }
                Svalue::Closure(_) => {}
                _ => {
                    return Err(Ctrl::Error(mk_string(format!(
                        "add_driver_hook(): invalid argument for hook {}\n",
                        n
                    ))))
                }
            }
            vm.env.driver_hooks[n] = args[1].clone();
            Ok(Svalue::Int(1))
        }
        "query_driver_hook" => {
            let n = as_int(&args[0]).map_err(|e| Ctrl::Error(mk_string(e)))? as usize;
            Ok(vm.env.driver_hooks.get(n).cloned().unwrap_or(Svalue::Int(0)))
        }
        "remove_interactive" => {
            let oid = vm.this_object();
            vm.remove_interactive(oid);
            Ok(Svalue::Int(0))
        }
        "speak_to" | "tell_object" => {
            need(2)?;
            if let Some(o) = args[0].as_object(vm) {
                vm.tell_object(o, &display_string(&args[1]));
            }
            Ok(Svalue::Int(0))
        }
        "tell_room" => {
            need(2)?;
            let ob = obj_id(&args[0]).unwrap_or_else(|| vm.this_object());
            let room = vm.env_of(ob);
            let text = display_string(&args[1]);
            let from = args.get(2).and_then(obj_id);
            let inv = vm.inventory(room, false);
            for o in inv {
                if Some(o) == from {
                    continue;
                }
                let allow = if vm.has_lfun(o, "catch_tell") {
                    vm.call_function(o, "catch_tell", &[mk_string(text.clone())])
                        .map(|v| truthy(&v))
                        .unwrap_or(true)
                } else {
                    true
                };
                if allow {
                    vm.tell_object(o, &text);
                }
            }
            Ok(Svalue::Int(0))
        }
        "add_action" => {
            need(2)?;
            let lfun = lfun_name(vm, &args[0]);
            let verb = arg_str(args, 1)?;
            let min = args.get(2).and_then(as_int_ok).unwrap_or(verb.len() as i64) as usize;
            let flag = args.get(3).and_then(as_int_ok).unwrap_or(0);
            let oid = vm.this_object();
            if verb.chars().any(|c| !(c.is_alphabetic() || c == '_')) {
                return Err(estr("add_action: verb must be alphanumeric".into()));
            }
            vm.env
                .actions
                .entry(oid)
                .or_default()
                .push(ActionEntry { verb, lfun, object: oid, min_match_len: min, flag });
            vm.env.commands_enabled.entry(oid).or_insert(false);
            Ok(Svalue::Int(1))
        }
        "remove_action" => {
            need(1)?;
            let verb = arg_str(args, 0)?;
            let oid = args.get(1).and_then(obj_id).unwrap_or_else(|| vm.this_object());
            if let Some(entry) = vm.env.actions.get_mut(&oid) {
                entry.retain(|a| a.verb != verb);
            }
            Ok(Svalue::Int(1))
        }
        "enable_commands" => {
            vm.env.commands_enabled.insert(vm.this_object(), true);
            Ok(Svalue::Int(1))
        }
        "disable_commands" => {
            vm.env.commands_enabled.insert(vm.this_object(), false);
            Ok(Svalue::Int(0))
        }
        "notify_fail" => {
            let oid = vm.this_object();
            if args.is_empty() {
                vm.env.notify_fail.insert(oid, None);
                return Ok(Svalue::Int(1));
            }
            let nf = NotifyFail {
                closure: args[0].clone(),
                flag: args.get(1).and_then(as_int_ok).unwrap_or(0),
                orig: if args.len() > 1 && matches!(args[1], Svalue::Str(_)) {
                    Some(display_string(&args[1]))
                } else {
                    None
                },
            };
            vm.env.notify_fail.insert(oid, Some(nf));
            Ok(Svalue::Int(1))
        }
        "command" => {
            need(1)?;
            let cmd = arg_str(args, 0)?;
            let receiver = match args.get(1) {
                Some(x) => x.as_object(vm).unwrap_or(vm.env.command_giver),
                None => vm.env.command_giver,
            };
            Ok(Svalue::Int(if vm.execute_command(&cmd, receiver) { 1 } else { 0 }))
        }
        "set_driver_hook" => {
            need(2)?;
            let ix = arg_int(args, 0)? as usize;
            if ix >= vm.env.driver_hooks.len() {
                vm.env.driver_hooks.resize(ix + 1, Svalue::Null);
            }
            vm.env.driver_hooks[ix] = args[1].clone();
            Ok(Svalue::Int(1))
        }
        "get_driver_hook" => {
            need(1)?;
            let ix = arg_int(args, 0)? as usize;
            Ok(vm
                .env
                .driver_hooks
                .get(ix)
                .cloned()
                .unwrap_or(Svalue::Int(0)))
        }

        // ---------------- timers ----------------
        "call_out" => {
            need(1)?;
            let delay = args.get(1).and_then(as_int_ok).unwrap_or(0);
            let cl = bind_lfun_name(vm, &args[0]);
            let arg = args.get(2).cloned().unwrap_or(Svalue::Int(0));
            let oid = vm.this_object();
            vm.schedule_call_out(oid, delay * 1000, None, cl, arg);
            Ok(Svalue::Int(1))
        }
        "remove_call_out" => {
            need(1)?;
            let fun = lfun_name(vm, &args[0]);
            let vm_oid = args.get(1).and_then(obj_id).unwrap_or_else(|| vm.this_object());
            vm.env.call_outs.retain(|c| !(c.object == vm_oid && closure_named(&c.closure, &fun)));
            Ok(Svalue::Int(1))
        }
        "call_out_info" => {
            let mut out = Vec::new();
            for c in vm.env.call_outs.clone() {
                out.push(mk_mapping(vec![
                    (mk_string("function"), c.closure.clone()),
                    (mk_string("delay"), Svalue::Int((c.next_at_ms - vm.env.time_ms) / 1000)),
                    (mk_string("object"), object_value(c.object)),
                ]));
            }
            Ok(mk_array(out))
        }
        "set_heart_beat" => {
            let on = args.first().map(truthy).unwrap_or(true);
            let oid = args.get(1).and_then(obj_id).unwrap_or_else(|| vm.this_object());
            vm.env.heart_beats.retain(|x| *x != oid);
            if on {
                vm.env.heart_beats.push(oid);
            }
            Ok(Svalue::Int(1))
        }
        "set_every" => {
            need(2)?;
            let interval = arg_int(args, 1)?;
            let cl = bind_lfun_name(vm, &args[0]);
            vm.schedule_call_out(
                vm.this_object(),
                interval * 1000,
                Some(interval * 1000),
                cl,
                Svalue::Int(0),
            );
            Ok(Svalue::Int(1))
        }

        // ---------------- time ----------------
        "time" => Ok(Svalue::Int(vm.env.time_ms / 1000)),
        "utime" => Ok(Svalue::Int(vm.env.time_ms)),
        "time_stamp" => {
            let t = args.first().and_then(as_int_ok).unwrap_or(vm.env.time_ms / 1000);
            let fmt = args.get(1).map(display_string).unwrap_or_default();
            Ok(mk_string(time_stamp_str(t, &fmt)))
        }
        "ctime" => {
            let t = args.first().and_then(as_int_ok).unwrap_or(vm.env.time_ms / 1000);
            Ok(mk_string(ctime_str(t)))
        }

        // ---------------- housekeeping ----------------
        "gc" => Ok(Svalue::Int(0)),
        "shallow" => Ok(deep_copy(&args[0])),
        "call_stack" => {
            let mut out = Vec::new();
            for ci in vm.call_stack() {
                out.push(mk_array(vec![
                    object_value(ci.object),
                    mk_string(ci.func),
                    Svalue::Int(0),
                ]));
            }
            Ok(mk_array(out))
        }
        "master" => Ok(object_value(vm.env.master)),
        "simul_efun" => Ok(object_value(vm.env.sefun)),
        "shutdown" => {
            vm.shutdown_driver();
            Ok(Svalue::Int(0))
        }
        "shutdown_at_exit" => {
            if truthy(args.first().unwrap_or(&Svalue::Int(0))) {
                vm.env.shutdown_at_exit = true;
            }
            Ok(Svalue::Int(0))
        }

        // ---------------- apply ----------------
        "apply" | "apply_low" => {
            need(2)?;
            let oid = args[0].as_object(vm).unwrap_or(0);
            let lfun = lfun_name(vm, &args[1]);
            if lfun.is_empty() {
                return Ok(Svalue::Int(0));
            }
            vm.call_function(oid, &lfun, &args[2..])
        }
        "funcall" => {
            need(1)?;
            vm.apply_closure(&args[0], &args[1..])
        }
        "catch" => {
            // reached only if eval didn't intercept it (shouldn't normally happen)
            Err(estr("catch: internal form".into()))
        }
        "throw" => Err(Ctrl::Error(args.first().cloned().unwrap_or(Svalue::Int(0)))),
        "error" => Err(estr(match args.first() {
            Some(x) => display_string(x),
            None => "error".into(),
        })),
        "assert" => {
            let v = args.first().cloned().unwrap_or(Svalue::Int(0));
            if truthy(&v) {
                Ok(Svalue::Int(1))
            } else {
                let m = args
                    .get(1)
                    .map(display_string)
                    .map(|s| if s.is_empty() { "assertion failed".to_string() } else { s })
                    .unwrap_or_else(|| "assertion failed".to_string());
                Err(estr(m))
            }
        }

        // ---------------- object introspection ----------------
        "functions" => {
            let oid = args.first().and_then(obj_id).unwrap_or_else(|| vm.this_object());
            let o = match vm.env.object(oid) {
                Some(o) => o,
                None => return Ok(mk_array(vec![])),
            };
            let mut out = Vec::new();
            for (info, def) in o.borrow().prog.functions.clone() {
                out.push(mk_array(vec![
                    mk_string(info.ret_ty.clone().unwrap_or_else(|| "mixed".into())),
                    mk_string(def),
                    mk_string(info.name),
                ]));
            }
            Ok(mk_array(out))
        }
        "local_variables" => {
            let names = vm.frame_locals();
            Ok(mk_array(names.into_iter().map(mk_string).collect()))
        }
        "query_local_variables" => Ok(mk_array(
            vm.frame_local_pairs()
                .into_iter()
                .map(|(k, v)| mk_array(vec![mk_string(k), v]))
                .collect(),
        )),

        // ---------------- files ----------------
        "read_file" => {
            need(1)?;
            let path = display_string(&args[0]);
            match vm.read_file(&path) {
                Ok(c) => Ok(mk_string(c)),
                Err(e) => Err(estr(e)),
            }
        }
        "write_file" => {
            need(2)?;
            let p = arg_str(args, 0)?;
            let data = display_string(&args[1]);
            match vm.write_file(&p, &data) {
                Ok(()) => Ok(Svalue::Int(1)),
                Err(e) => Err(estr(e)),
            }
        }
        "get_dir" => {
            need(1)?;
            let p = arg_str(args, 0)?;
            match vm.get_dir(&p) {
                Ok(v) => Ok(mk_array(v.into_iter().map(mk_string).collect())),
                Err(e) => Err(estr(e)),
            }
        }
        "file_size" => {
            need(1)?;
            Ok(Svalue::Int(vm.file_size(&display_string(&args[0]))))
        }
        "file_time" => Ok(Svalue::Int(-2)),
        "mkdir" => {
            need(1)?;
            let p = vm.file_resolve(&display_string(&args[0]));
            match std::fs::create_dir_all(&p) {
                Ok(()) => Ok(Svalue::Int(1)),
                Err(e) => Err(estr(format!("{}", e))),
            }
        }
        "remove_file" => {
            need(1)?;
            let p = arg_str(args, 0)?;
            match vm.remove_file(&p) {
                Ok(()) => Ok(Svalue::Int(1)),
                Err(e) => Err(estr(e)),
            }
        }
        "rename" => {
            need(2)?;
            match vm.rename_file(&arg_str(args, 0)?, &arg_str(args, 1)?) {
                Ok(()) => Ok(Svalue::Int(1)),
                Err(e) => Err(estr(e)),
            }
        }

        // ---------------- compile ----------------
        "compile_string" => {
            need(1)?;
            let src = arg_str(args, 0)?;
            match vm.compile_string(&src) {
                Ok(o) => Ok(object_value(o)),
                Err(e) => Err(estr(e)),
            }
        }

        _ => Err(estr(format!("{}: unknown efun (not implemented)", name))),
    }
}

// ---------------------------------------------------------------------
//  shared small helpers
// ---------------------------------------------------------------------

pub fn mk_char(c: char) -> Svalue {
    mk_string(c.to_string())
}

// sprintf -----------------------------------------------------------------

pub fn do_sprintf(args: &[Svalue]) -> Result<String, String> {
    if args.is_empty() {
        return Err("sprintf: no format".into());
    }
    let fmt = as_string(&args[0])?.to_string();
    let mut out = String::new();
    let fc: Vec<char> = fmt.chars().collect();
    // conversions consume args[1..]
    let mut ai = 1usize;
    let mut i = 0usize;
    while i < fc.len() {
        if fc[i] != '%' {
            out.push(fc[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= fc.len() {
            break;
        }
        if fc[i] == '%' {
            out.push('%');
            i += 1;
            continue;
        }
        let start = i;
        while i < fc.len() && !fc[i].is_alphabetic() {
            i += 1;
        }
        if i >= fc.len() {
            break;
        }
        let flags: String = fc[start..i].iter().collect();
        let conv = fc[i];
        i += 1;
        if matches!(conv, 'O' | 'A' | 'M' | 'P') {
            let arg = args.get(ai).cloned().unwrap_or(Svalue::Int(0));
            ai += 1;
            out.push_str(&print_string(&arg));
            continue;
        }
        if conv == '*' {
            ai += 1;
            continue;
        }
        let arg = args.get(ai).cloned().unwrap_or(Svalue::Int(0));
        ai += 1;
        let mut s = match conv {
            'd' | 'i' => match &arg {
                Svalue::Str(s) => s.to_string(),
                Svalue::Float(f) => format!("{}", *f as i64),
                x => display_string(x),
            },
            's' => match &arg {
                Svalue::Array(arr) => {
                    let parts: Vec<String> = arr.borrow().iter().map(display_string).collect();
                    parts.join("")
                }
                Svalue::Mapping(m) => {
                    let b = m.borrow();
                    let parts: Vec<String> =
                        b.items().iter().map(|(_, v)| display_string(v)).collect();
                    parts.join("")
                }
                x => display_string(x),
            },
            'f' | 'g' | 'e' => format!("{:.6}", as_float(&arg).unwrap_or(0.0)),
            'c' => char::from_u32(as_int(&arg).unwrap_or(0) as u32)
                .map(|ch| ch.to_string())
                .unwrap_or_else(|| "*%c".into()),
            'x' => format!("{:x}", as_int(&arg).unwrap_or(0)),
            'o' => format!("{:o}", as_int(&arg).unwrap_or(0)),
            'H' => "*".repeat(display_string(&arg).chars().count().max(1)),
            _ => return Err(format!("sprintf: unknown conversion %{}", conv)),
        };
        let width: Option<usize> = {
            let d: String = flags.chars().take_while(|c| c.is_ascii_digit()).collect();
            if d.is_empty() {
                None
            } else {
                d.parse().ok()
            }
        };
        if let Some(w) = width {
            let n = s.chars().count();
            if n < w {
                if flags.contains('-') {
                    for _ in n..w {
                        s.push(' ');
                    }
                } else {
                    let pad = if flags.starts_with('0') { '0' } else { ' ' };
                    let pre: String = std::iter::repeat(pad).take(w - n).collect();
                    s = format!("{}{}", pre, s);
                }
            }
        }
        out.push_str(&s);
    }
    Ok(out)
}

// time helpers ------------------------------------------------------------

fn gm_offset(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    let mut days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let sec = rem % 60;
    let mut y = 1970i64;
    loop {
        let leap = (y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)) as i64;
        let year_days = 365 + leap;
        if days >= 0 && days < year_days {
            break;
        }
        if days < 0 {
            y -= 1;
            days += year_days;
        } else {
            y += 1;
            days -= year_days;
        }
    }
    let leap = (y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)) as i64;
    let months = [31, 28 + leap, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mo = 0usize;
    while days >= months[mo] {
        days -= months[mo];
        mo += 1;
    }
    (y, mo as i64 + 1, days + 1, h, m, sec)
}

pub fn ctime_str(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = gm_offset(secs);
    const WD: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MO: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = secs.div_euclid(86400);
    let wd = WD[days.rem_euclid(7) as usize];
    format!(
        "{} {} {:2} {:02}:{:02}:{:02} {:04} (UTC)",
        wd,
        MO[((mo - 1).rem_euclid(12)) as usize],
        d,
        h,
        mi,
        s,
        y
    )
}

pub fn time_stamp_str(secs: i64, fmtname: &str) -> String {
    let (y, mo, d, h, mi, s) = gm_offset(secs);
    match fmtname {
        "days" => format!("{}", secs / 86400),
        _ => format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, mi, s),
    }
}
