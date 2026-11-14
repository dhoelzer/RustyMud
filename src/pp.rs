//! A practical C-preprocessor subset: #include (with #ifndef guards), #define
//! (object-like and function-like), #ifdef/#ifndef/#else/#endif, #undef,
//! #pragma passthrough. Mirrors what LDMud's preproc.c provides for mudlibs.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct MacroDef {
    pub params: Option<Vec<String>>, // None = object-like
    pub body: String,
}

pub struct Preprocessor {
    pub defines: HashMap<String, MacroDef>,
    pub inc_dirs: Vec<String>,
}

pub struct PpError {
    pub line: usize,
    pub msg: String,
}

pub type Resolver<'a> = (dyn Fn(&str) -> Result<String, String> + 'a);

impl Preprocessor {
    pub fn new() -> Preprocessor {
        let mut p = Preprocessor { defines: HashMap::new(), inc_dirs: vec!["/".into()] };
        // driver-provided defines (cf. LDMud config)
        p.defines.insert("__LDMUD__".into(), MacroDef { params: None, body: "1".into() });
        p.defines.insert("__LDMUD_MAJOR__".into(), MacroDef { params: None, body: "3".into() });
        p.defines.insert("__LDMUD_MINOR__".into(), MacroDef { params: None, body: "3".into() });
        p.defines.insert("__LDMUD_PATCH__".into(), MacroDef { params: None, body: "0".into() });
        p.defines.insert("__VERSION__".into(), MacroDef { params: None, body: "30300".into() });
        p
    }

    pub fn run(&mut self, src: &str, resolver: &Resolver<'_>, depth: usize, included: &mut Vec<String>) -> Result<String, PpError> {
        if depth > 32 {
            return Err(PpError { line: 0, msg: "#include nesting too deep".into() });
        }
        let raw: Vec<String> = src.lines().map(|l| l.to_string()).collect();
        // join backslash continuations
        let mut lines: Vec<(usize, String)> = Vec::new();
        let mut i = 0;
        while i < raw.len() {
            let ln = i + 1;
            let mut cur = raw[i].clone();
            while cur.ends_with('\\') && i + 1 < raw.len() {
                cur.pop();
                i += 1;
                cur.push_str(&raw[i]);
            }
            lines.push((ln, cur));
            i += 1;
        }

        let mut out = String::new();
        struct Frame {
            skipping: bool,
            parent_skip: bool,
            taken: bool,
        }
        let mut skip_stack: Vec<Frame> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let (ln, l) = &lines[i];
            let t = l.trim_start();
            if t.starts_with('#') {
                let d = t[1..].trim_start();
                let (kw, rest) = split_directive(d);
                let sk = skip_stack.iter().any(|f| f.skipping);
                match kw {
                    "ifdef" => {
                        let defined = self.defines.contains_key(rest.trim());
                        let take = !sk && defined;
                        skip_stack.push(Frame { skipping: !take, parent_skip: sk, taken: take });
                    }
                    "ifndef" => {
                        let defined = self.defines.contains_key(rest.trim());
                        let take = !sk && !defined;
                        skip_stack.push(Frame { skipping: !take, parent_skip: sk, taken: take });
                    }
                    "if" => {
                        let v = if sk { 0 } else { eval_pp_cond(self, rest).unwrap_or(0) };
                        let take = !sk && v != 0;
                        skip_stack.push(Frame { skipping: !take, parent_skip: sk, taken: take });
                    }
                    "elif" => {
                        if skip_stack.is_empty() {
                            return Err(PPEError_at(*ln, "#elif without #if".into()));
                        }
                        let f = skip_stack.last_mut().unwrap();
                        let take = !f.parent_skip && !f.taken && {
                            let v = eval_pp_cond(self, rest).unwrap_or(0);
                            v != 0
                        };
                        if take {
                            f.taken = true;
                            f.skipping = false;
                        } else {
                            f.skipping = true;
                        }
                    }
                    "else" => {
                        if skip_stack.is_empty() {
                            return Err(PPEError_at(*ln, "#else without #if".into()));
                        }
                        let f = skip_stack.last_mut().unwrap();
                        let take = !f.parent_skip && !f.taken;
                        if take {
                            f.taken = true;
                            f.skipping = false;
                        } else {
                            f.skipping = true;
                        }
                    }
                    "endif" => {
                        if skip_stack.is_empty() {
                            return Err(PPEError_at(*ln, "#endif without #if".into()));
                        }
                        skip_stack.pop();
                    }
                    "include" => {
                        if sk {
                            i += 1;
                            continue;
                        }
                        let f = extract_include_name(rest, &lines, *ln, self)?;
                        if !included.contains(&f) {
                            included.push(f.clone());
                            let content = resolver(&f).map_err(|e| PPEError_at(*ln, e))?;
                            let sub = self.run(&content, resolver, depth + 1, included)
                                .map_err(|e| PPEError_at(*ln, e.msg))?;
                            out.push_str(&sub);
                            out.push('\n');
                        }
                    }
                    "define" => {
                        if !sk {
                            let (name, def) = parse_define(rest);
                            self.defines.insert(name, def);
                        }
                    }
                    "undef" => {
                        if !sk {
                            self.defines.remove(rest.trim());
                        }
                    }
                    _ => {} // #pragma etc: ignore
                }
                i += 1;
                continue;
            }
            if skip_stack.iter().any(|f| f.skipping) {
                i += 1;
                continue;
            }
            let expanded = self.expand_line(l);
            out.push_str(&expanded);
            out.push('\n');
            i += 1;
        }
        Ok(out)
    }

    fn expand_line(&self, line: &str) -> String {
        // 1. expand macro invocations repeatedly (max 32 rounds)
        let mut s = line.to_string();
        // strings and comments inside a line are protected
        for _ in 0..32 {
            let e = self.expand_one(&s);
            if e == s {
                break;
            }
            s = e;
        }
        s
    }

    fn expand_one(&self, line: &str) -> String {
        let chars: Vec<char> = line.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '"' {
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i < chars.len() {
                    i += 1;
                }
                out.extend(chars[start..i.min(chars.len())].iter());
                continue;
            }
            if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
                out.extend(chars[i..].iter());
                break;
            }
            if c.is_alphabetic() || c == '_' {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if let Some(def) = self.defines.get(&word) {
                    if let Some(params) = &def.params {
                        // function-like: needs '('
                        let mut j = i;
                        while j < chars.len() && chars[j].is_whitespace() {
                            j += 1;
                        }
                        if j < chars.len() && chars[j] == '(' {
                            let (args, end) = split_args(&chars, j);
                            let body = subst_params(&def.body, params, &args);
                            out.push_str(&body);
                            i = end;
                            continue;
                        }
                    } else {
                        out.push_str(&def.body);
                        continue;
                    }
                }
                out.push_str(&word);
                continue;
            }
            out.push(c);
            i += 1;
        }
        out
    }
}

fn PPEError_at(ln: usize, msg: String) -> PpError {
    PpError { line: ln, msg }
}

fn split_directive(d: &str) -> (&str, &str) {
    let idx = d.find(|c: char| c.is_whitespace() || c == '(').unwrap_or(d.len());
    (&d[..idx], &d[idx..])
}

fn split_args(chars: &[char], open: usize) -> (Vec<String>, usize) {
    let mut args = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    let mut i = open;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '(' => {
                depth += 1;
                if depth == 1 {
                    i += 1;
                    continue;
                }
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    if !cur.trim().is_empty() {
                        args.push(cur.trim().to_string());
                    }
                    return (args, i + 1);
                }
                cur.push(c);
            }
            ',' if depth == 1 => {
                args.push(cur.trim().to_string());
                cur.clear();
            }
            '"' => {
                cur.push(c);
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        cur.push(chars[i]);
                        i += 1;
                    }
                    cur.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    cur.push(chars[i]);
                }
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    (args, i)
}

fn subst_params(body: &str, params: &[String], args: &[String]) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.extend(chars[start..i.min(chars.len())].iter());
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let w: String = chars[start..i].iter().collect();
            if let Some(pos) = params.iter().position(|p| p == &w) {
                out.push_str(args.get(pos).map(|s| s.as_str()).unwrap_or(""));
                continue;
            }
            out.push_str(&w);
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn parse_define(rest: &str) -> (String, MacroDef) {
    let rest = rest.trim_start();
    let idx = rest.find(|c: char| c.is_whitespace() || c == '(').unwrap_or(rest.len());
    let name = rest[..idx].to_string();
    let mut params = None;
    let mut body_start = idx;
    if idx < rest.len() && rest.as_bytes().get(idx) == Some(&b'(') {
        // function-like macro
        let chars: Vec<char> = rest.chars().collect();
        let (ps, end) = split_args(&chars, idx);
        params = Some(ps);
        body_start = end;
    }
    let body = rest[body_start..].trim().to_string();
    (name, MacroDef { params, body })
}

fn extract_include_name(rest: &str, _lines: &[(usize, String)], _ln: usize, _pp: &Preprocessor) -> Result<String, PpError> {
    let r = rest.trim();
    let (f, quote) = if r.starts_with('"') {
        (r.trim_matches('"').to_string(), true)
    } else if r.starts_with('<') {
        (r.trim_matches('<').trim_matches('>').to_string(), false)
    } else {
        (r.trim_matches('"').to_string(), true)
    };
    if !f.starts_with('/') {
        return if quote { Ok(format!("/{}", f)) } else { Ok(f) };
    }
    Ok(f)
}

/// Very small #if evaluator: identifiers -> 0/1 (defined), numeric, && || ! == !=.
pub fn eval_pp_cond(pp: &Preprocessor, expr: &str) -> Result<i64, ()> {
    let expanded = pp.expand_line(expr);
    let mut p = CondParser { toks: tokenize_cond(&expanded), pos: 0 };
    p.or()
}

fn tokenize_cond(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let st = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(chars[st..i].iter().collect());
            continue;
        }
        if c.is_ascii_digit() {
            let st = i;
            let mut radix = 10;
            if c == '0' && i + 1 < chars.len() && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
                radix = 16;
                i += 2;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let text: String = chars[st..i].iter().collect();
            let v = i64::from_str_radix(text.trim_start_matches("0x").trim_start_matches("0X"), radix)
                .unwrap_or(0);
            out.push(format!("\u{1}{}", v)); // sentinel-prefixed number token
            continue;
        }
        if i + 1 < chars.len() {
            let two: String = chars[i..i + 2].iter().collect();
            if matches!(two.as_str(), "&&" | "||" | "==" | "!=" | "<=" | ">=") {
                out.push(two);
                i += 2;
                continue;
            }
        }
        out.push(c.to_string());
        i += 1;
    }
    out
}

struct CondParser {
    toks: Vec<String>,
    pos: usize,
}

impl CondParser {
    fn peek(&self) -> Option<&String> {
        self.toks.get(self.pos)
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.peek().map(|t| t == s).unwrap_or(false) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn or(&mut self) -> Result<i64, ()> {
        let mut a = self.and()?;
        while self.eat("||") {
            let b = self.and()?;
            a = if a != 0 || b != 0 { 1 } else { 0 };
        }
        Ok(a)
    }
    fn and(&mut self) -> Result<i64, ()> {
        let mut a = self.cmp()?;
        while self.eat("&&") {
            let b = self.cmp()?;
            a = if a != 0 && b != 0 { 1 } else { 0 };
        }
        Ok(a)
    }
    fn cmp(&mut self) -> Result<i64, ()> {
        let a = self.unary()?;
        if self.eat("==") {
            return Ok(if a == self.unary()? { 1 } else { 0 });
        }
        if self.eat("!=") {
            return Ok(if a != self.unary()? { 1 } else { 0 });
        }
        if self.eat("<") {
            return Ok(if a < self.unary()? { 1 } else { 0 });
        }
        if self.eat(">") {
            return Ok(if a > self.unary()? { 1 } else { 0 });
        }
        Ok(a)
    }
    fn unary(&mut self) -> Result<i64, ()> {
        if self.eat("!") {
            return Ok(if self.unary()? == 0 { 1 } else { 0 });
        }
        if self.eat("(") {
            let v = self.or()?;
            if !self.eat(")") {
                return Err(());
            }
            return Ok(v);
        }
        if let Some(t) = self.peek().cloned() {
            self.pos += 1;
            if let Some(num) = t.strip_prefix('\u{1}') {
                return num.parse::<i64>().map_err(|_| ());
            }
            return Ok(0); // bare identifier in #if after expansion: 0
        }
        Err(())
    }
}
