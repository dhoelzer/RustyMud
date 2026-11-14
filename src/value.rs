//! Runtime values: the Rust equivalent of LDMud's `svalue_t` / array_t / mapping_t.

use std::cmp::Ordering;
use std::cell::RefCell;
use std::rc::Rc;

use crate::program::Program;

pub type ObjId = u32;

impl std::fmt::Debug for Svalue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Svalue::Null => f.write_str("0"),
            Svalue::Int(v) => write!(f, "{}", v),
            Svalue::Float(v) => write!(f, "{}", v),
            Svalue::Str(s) => write!(f, "{:?}", s),
            Svalue::Object(o) => write!(f, "obj#{}", o),
            Svalue::Array(a) => write!(f, "array({})", a.borrow().len()),
            Svalue::Mapping(m) => write!(f, "mapping({})", m.borrow().len()),
            Svalue::Closure(_) => f.write_str("closure"),
        }
    }
}

impl std::fmt::Debug for Closure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Closure::Func { oid, name } => write!(f, "#'{} in obj#{}", name, oid),
            Closure::Lambda { nargs, .. } => write!(f, "lambda({})", nargs),
        }
    }
}

#[derive(Clone)]
pub enum Svalue {
    Null,
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Object(ObjId),
    Array(Rc<RefCell<Vec<Svalue>>>),
    Mapping(Rc<RefCell<Mapping>>),
    Closure(Rc<Closure>),
}

#[derive(Clone)]
pub enum Closure {
    /// `#'function` — bound to the object that created it.
    Func { oid: ObjId, name: Rc<str> },
    /// A compiled inline/lambda closure body.
    Lambda { prog: Rc<Program>, body_id: usize, nargs: usize },
}

/// An ordered mapping, like LDMud's mappings (insertion order preserved).
#[derive(Default, Clone)]
pub struct Mapping {
    entries: Vec<(Svalue, Svalue)>,
}

impl Mapping {
    pub fn new() -> Mapping {
        Mapping::default()
    }

    fn key_index(&self, k: &Svalue) -> Option<usize> {
        self.entries.iter().position(|(ek, _)| eq(ek, k))
    }

    pub fn get(&self, k: &Svalue) -> Option<Svalue> {
        self.key_index(k).map(|i| self.entries[i].1.clone())
    }

    pub fn set(&mut self, k: Svalue, v: Svalue) {
        match self.key_index(&k) {
            Some(i) => self.entries[i].1 = v,
            None => self.entries.push((k, v)),
        }
    }

    pub fn unset(&mut self, k: &Svalue) -> bool {
        match self.key_index(k) {
            Some(i) => {
                self.entries.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn keys(&self) -> Vec<Svalue> {
        self.entries.iter().map(|(k, _)| k.clone()).collect()
    }
    pub fn values(&self) -> Vec<Svalue> {
        self.entries.iter().map(|(_, v)| v.clone()).collect()
    }
    pub fn items(&self) -> &[(Svalue, Svalue)] {
        &self.entries
    }
    pub fn items_mut(&mut self) -> &mut Vec<(Svalue, Svalue)> {
        &mut self.entries
    }
}

impl Svalue {
    pub fn as_vec(&self) -> Option<Rc<RefCell<Vec<Svalue>>>> {
        match self {
            Svalue::Array(a) => Some(a.clone()),
            _ => None,
        }
    }
    /// Object value, or an lvalue path? For efun args, resolve to an id.
    pub fn as_object(&self, vm: &mut crate::vm::Vm) -> Option<ObjId> {
        match self {
            Svalue::Object(o) if *o != 0 => Some(*o),
            _ => None,
        }
    }
    pub fn as_string_simple(&self) -> String {
        match self {
            Svalue::Str(s) => s.to_string(),
            x => display_string(x),
        }
    }
}

pub fn mk_string<S: AsRef<str>>(s: S) -> Svalue {
    Svalue::Str(Rc::from(s.as_ref()))
}

pub fn mk_array(v: Vec<Svalue>) -> Svalue {
    Svalue::Array(Rc::new(RefCell::new(v)))
}

pub fn mk_mapping(entries: Vec<(Svalue, Svalue)>) -> Svalue {
    let mut m = Mapping::new();
    for (k, v) in entries {
        m.set(k, v);
    }
    Svalue::Mapping(Rc::new(RefCell::new(m)))
}

pub fn truthy(v: &Svalue) -> bool {
    match v {
        Svalue::Null => false,
        Svalue::Int(i) => *i != 0,
        Svalue::Float(f) => *f != 0.0,
        Svalue::Str(s) => !s.is_empty(),
        Svalue::Object(o) => *o != 0,
        Svalue::Array(a) => !a.borrow().is_empty(),
        Svalue::Mapping(m) => !m.borrow().is_empty(),
        Svalue::Closure(_) => true,
    }
}

/// Deep equality (LPC `==`).
pub fn eq(a: &Svalue, b: &Svalue) -> bool {
    match (a, b) {
        (Svalue::Null, Svalue::Null) => true,
        (Svalue::Int(x), Svalue::Int(y)) => x == y,
        (Svalue::Float(x), Svalue::Float(y)) => x == y,
        (Svalue::Int(x), Svalue::Float(y)) => (*x as f64) == *y,
        (Svalue::Float(x), Svalue::Int(y)) => *x == (*y as f64),
        (Svalue::Str(x), Svalue::Str(y)) => x == y,
        (Svalue::Object(x), Svalue::Object(y)) => x == y,
        (Svalue::Array(x), Svalue::Array(y)) => {
            let (x, y) = (x.borrow(), y.borrow());
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| eq(a, b))
        }
        (Svalue::Mapping(x), Svalue::Mapping(y)) => {
            let (x, y) = (x.borrow(), y.borrow());
            x.len() == y.len()
                && x.items()
                    .iter()
                    .all(|(k, v)| y.get(k).map(|v2| eq(v, &v2)).unwrap_or(false))
        }
        (Svalue::Closure(x), Svalue::Closure(y)) => match (&**x, &**y) {
            (Closure::Func { oid: o1, name: n1 }, Closure::Func { oid: o2, name: n2 }) => {
                o1 == o2 && n1 == n2
            }
            _ => Rc::ptr_eq(x, y),
        },
        _ => false,
    }
}

/// Deterministic ordering used for sorting and mapping-key ordering.
impl PartialEq for Svalue {
    fn eq(&self, other: &Svalue) -> bool {
        eq(self, other)
    }
}

pub fn cmp(a: &Svalue, b: &Svalue) -> Ordering {
    fn tag(v: &Svalue) -> u8 {
        match v {
            Svalue::Null => 0,
            Svalue::Int(_) => 1,
            Svalue::Float(_) => 2,
            Svalue::Str(_) => 3,
            Svalue::Object(_) => 5,
            Svalue::Array(_) => 6,
            Svalue::Mapping(_) => 7,
            Svalue::Closure(_) => 8,
        }
    }
    let t = tag(a).cmp(&tag(b));
    if t != Ordering::Equal {
        return t;
    }
    match (a, b) {
        (Svalue::Int(x), Svalue::Int(y)) => x.cmp(y),
        (Svalue::Float(x), Svalue::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Svalue::Str(x), Svalue::Str(y)) => x.cmp(y),
        (Svalue::Array(x), Svalue::Array(y)) => {
            let (x, y) = (x.borrow(), y.borrow());
            let n = x.len().min(y.len());
            for i in 0..n {
                let c = cmp(&x[i], &y[i]);
                if c != Ordering::Equal {
                    return c;
                }
            }
            x.len().cmp(&y.len())
        }
        _ => Ordering::Equal,
    }
}

pub fn deep_copy(v: &Svalue) -> Svalue {
    match v {
        Svalue::Array(a) => {
            let src = a.borrow();
            mk_array(src.iter().map(deep_copy).collect())
        }
        Svalue::Mapping(m) => {
            let src = m.borrow();
            mk_mapping(
                src.items()
                    .iter()
                    .map(|(k, e)| (deep_copy(k), deep_copy(e)))
                    .collect(),
            )
        }
        x => x.clone(),
    }
}

/// Number conversions used by arithmetic.
pub fn as_int(v: &Svalue) -> Result<i64, String> {
    match v {
        Svalue::Int(i) => Ok(*i),
        Svalue::Float(f) => Ok(*f as i64),
        Svalue::Null => Ok(0),
        _ => Err(format!("expected a number, got {}", type_of(v))),
    }
}

pub fn as_float(v: &Svalue) -> Result<f64, String> {
    match v {
        Svalue::Int(i) => Ok(*i as f64),
        Svalue::Float(f) => Ok(*f),
        Svalue::Null => Ok(0.0),
        _ => Err(format!("expected a number, got {}", type_of(v))),
    }
}

pub fn as_string(v: &Svalue) -> Result<Rc<str>, String> {
    match v {
        Svalue::Str(s) => Ok(s.clone()),
        _ => Err(format!("expected a string, got {}", type_of(v))),
    }
}

pub fn type_of(v: &Svalue) -> &'static str {
    match v {
        Svalue::Null => "int",
        Svalue::Int(_) => "int",
        Svalue::Float(_) => "float",
        Svalue::Str(_) => "string",
        Svalue::Object(_) => "object",
        Svalue::Array(_) => "array",
        Svalue::Mapping(_) => "mapping",
        Svalue::Closure(_) => "closure",
    }
}

/// Display for `%s` (unquoted strings, `0` for null pointers).
pub fn display_string(v: &Svalue) -> String {
    match v {
        Svalue::Null => "0".to_string(),
        Svalue::Int(i) => i.to_string(),
        Svalue::Float(f) => format_float(*f),
        Svalue::Str(s) => s.to_string(),
        Svalue::Object(o) => format!("object:{}", o),
        Svalue::Array(a) => {
            let parts: Vec<String> = a.borrow().iter().map(display_string).collect();
            parts.join(", ")
        }
        Svalue::Mapping(m) => {
            let parts: Vec<String> = m
                .borrow()
                .items()
                .iter()
                .map(|(k, e)| format!("{}->{}", display_string(k), display_string(e)))
                .collect();
            parts.join(", ")
        }
        Svalue::Closure(c) => match &**c {
            Closure::Func { name, .. } => format!("'#{}", name),
            Closure::Lambda { .. } => "<lambda>".to_string(),
        },
    }
}

/// Display for `%O` (LPC repr, strings quoted).
pub fn print_string(v: &Svalue) -> String {
    match v {
        Svalue::Str(s) => format!("\"{}\"", escape_str(s)),
        Svalue::Array(a) => {
            let parts: Vec<String> = a.borrow().iter().map(print_string).collect();
            format!("({{ {} }})", parts.join(", "))
        }
        Svalue::Mapping(m) => {
            let parts: Vec<String> = m
                .borrow()
                .items()
                .iter()
                .map(|(k, e)| format!("{} : {}", print_string(k), print_string(e)))
                .collect();
            format!("([ {} ])", parts.join("; "))
        }
        x => display_string(x),
    }
}

fn format_float(f: f64) -> String {
    if f.is_finite() && f == (f as i64) as f64 {
        format!("{:.1}", f)
    } else {
        format!("{}", f)
    }
}

pub fn escape_str(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

pub fn dbg(v: &Svalue) -> String {
    print_string(v)
}
