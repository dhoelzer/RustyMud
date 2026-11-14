//! Lexer — mirrors the token set of LDMud's src/lex.c (reswords table).

use std::fmt;

pub type Spanned = (Tok, usize);

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Kw(String),
    Op(String),
    Eof,
}

pub const KEYWORDS: &[&str] = &[
    "async", "await", "break", "bytes", "case", "catch", "closure", "continue",
    "coroutine", "decltype", "default", "deprecated", "do", "else", "float",
    "for", "foreach", "function", "if", "in", "inherit", "int", "lpctype",
    "lwobject", "mapping", "mixed", "nomask", "nosave", "object", "private",
    "protected", "public", "return", "static", "status", "string", "struct",
    "switch", "symbol", "varargs", "virtual", "visible", "void", "while",
    "yield",
];

const PUNCT3: &[&str] = &["<<=", ">>=", "..."];
const PUNCT2: &[&str] = &[
    "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=",
    "&=", "|=", "^=", "<<", ">>", "->", "..", "::", "##", "(:", ":)",
];
const PUNCT1: &[char] = &[
    '+', '-', '*', '/', '%', '!', '~', '<', '>', '=', '(', ')', '[', ']', '{', '}',
    ',', ';', ':', '.', '&', '|', '^', '#', '?', '$', '@',
];

#[derive(Debug)]
pub struct LexError {
    pub line: usize,
    pub msg: String,
}

pub fn lex(src: &str) -> Result<Vec<Spanned>, LexError> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut out: Vec<Spanned> = Vec::new();

    macro_rules! err {
        ($($t:tt)*) => {
            return Err(LexError { line, msg: format!($($t)*) })
        };
    }

    while i < b.len() {
        let c = b[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                if b[i] == '/' && i + 1 < b.len() && b[i + 1] == '*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && i + 1 < b.len() && b[i + 1] == '/' {
                    depth -= 1;
                    i += 2;
                } else {
                    if b[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
            continue;
        }
        // a directive line starting with '#' (not #'function) shouldn't reach here
        if c == '#' && !(i + 1 < b.len() && b[i + 1] == '\'') {
            err!("unexpected '#' (run the preprocessor first?)");
        }
        // identifiers / keywords
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < b.len()
                && (b[i].is_alphanumeric() || b[i] == '_' || (b[i] == '\\' && i + 1 < b.len() && b[i + 1] == '\n'))
            {
                if b[i] == '\\' {
                    line += 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let w: String = b[start..i].iter().filter(|&ch| *ch != '\\').collect();
            if KEYWORDS.contains(&w.as_str()) {
                out.push((Tok::Kw(w), line));
            } else {
                out.push((Tok::Ident(w), line));
            }
            continue;
        }
        // numbers
        if c.is_ascii_digit() || (c == '.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let start = i;
            if c == '0' && i + 1 < b.len() && (b[i + 1] == 'x' || b[i + 1] == 'X') {
                i += 2;
                while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == '_') {
                    i += 1;
                }
                let digits: String = b[start + 2..i].iter().filter(|&c| *c != '_').collect();
                out.push((Tok::Int(u64::from_str_radix(&digits, 16).unwrap_or(0) as i64), line));
                continue;
            }
            if c == '0' && i + 1 < b.len() && (b[i + 1] == 'b' || b[i + 1] == 'B') {
                i += 2;
                while i < b.len() && (b[i] == '0' || b[i] == '1' || b[i] == '_') {
                    i += 1;
                }
                let digits: String = b[start + 2..i].iter().filter(|&c| *c != '_').collect();
                out.push((Tok::Int(u64::from_str_radix(&digits, 2).unwrap_or(0) as i64), line));
                continue;
            }
            let mut octal = c == '0' && i + 1 < b.len() && b[i + 1] >= '0' && b[i + 1] <= '7';
            let mut isfloat = false;
            while i < b.len() {
                let ch = b[i];
                if ch.is_ascii_digit() || ch == '_' {
                    if ch == '8' || ch == '9' {
                        octal = false;
                    }
                    i += 1;
                } else if ch == '.' && !isfloat && !(i + 1 < b.len() && b[i + 1] == '.') {
                    isfloat = true;
                    i += 1;
                } else {
                    break;
                }
            }
            if i < b.len() && (b[i] == 'e' || b[i] == 'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == '+' || b[j] == '-') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    isfloat = true;
                    i = j;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            // number suffixes
            if i < b.len() && (b[i] == 'l' || b[i] == 'L' || b[i] == 'u' || b[i] == 'U') {
                while i < b.len() && matches!(b[i], 'l' | 'L' | 'u' | 'U') {
                    i += 1;
                }
            }
            let text: String = b[start..i].iter().filter(|&c| *c != '_').collect();
            if isfloat {
                out.push((Tok::Float(text.parse::<f64>().unwrap_or(0.0)), line));
            } else if octal && text.len() > 1 {
                out.push((Tok::Int(i64::from_str_radix(&text[1..], 8).unwrap_or(0)), line));
            } else {
                out.push((Tok::Int(text.parse::<i64>().unwrap_or(i64::MAX)), line));
            }
            continue;
        }
        // strings with backslash-newline continuation
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' {
                    let (parts) = unescape_at(&b, i, line)?;
                    s.push_str(&parts.text);
                    line += parts.newlines;
                    i += parts.consumed;
                    continue;
                }
                if b[i] == '\n' {
                    err!("newline in string");
                }
                s.push(b[i]);
                i += 1;
            }
            if i >= b.len() {
                err!("unterminated string");
            }
            i += 1;
            out.push((Tok::Str(s), line));
            continue;
        }
        // char constants
        if c == '\'' {
            i += 1;
            let val = if b[i] == '\\' {
                let parts = unescape_at(&b, i, line)?;
                let v = parts.text.chars().next().map(|c| c as u32 as i64).unwrap_or(0);
                line += parts.newlines;
                i += parts.consumed;
                v
            } else {
                let v = b[i] as u32 as i64;
                i += 1;
                v
            };
            if i < b.len() && b[i] == '\'' {
                i += 1;
            }
            out.push((Tok::Int(val), line));
            continue;
        }
        // quoted function #' name (either order)
        if (c == '#' && i + 1 < b.len() && b[i + 1] == '\'')
            || (c == '\'' && i + 1 < b.len() && b[i + 1] == '#')
        {
            i += 2;
            let start = i;
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            if start == i {
                err!("expected function name after #'");
            }
            let w: String = b[start..i].iter().collect();
            out.push((Tok::Op("#'".into()), line));
            out.push((Tok::Ident(w), line));
            continue;
        }
        // lambda arguments $1 $2 and special $#, $$n
        if c == '$' {
            let start = i;
            i += 1;
            if i < b.len() && (b[i] == '$' || b[i] == '#' || b[i].is_ascii_digit()) {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let w: String = b[start..i].iter().collect();
            out.push((Tok::Ident(w), line));
            continue;
        }
        // punctuation: 3, 2, 1 chars, max munch
        if i + 3 <= b.len() {
            let r3: String = b[i..i + 3].iter().collect();
            if PUNCT3.contains(&r3.as_str()) {
                out.push((Tok::Op(r3), line));
                i += 3;
                continue;
            }
        }
        if i + 2 <= b.len() {
            let r2: String = b[i..i + 2].iter().collect();
            if PUNCT2.contains(&r2.as_str()) {
                out.push((Tok::Op(r2), line));
                i += 2;
                continue;
            }
        }
        if PUNCT1.contains(&c) {
            out.push((Tok::Op(c.to_string()), line));
            i += 1;
            continue;
        }
        err!("unexpected character '{}'", c);
    }
    out.push((Tok::Eof, line));
    Ok(out)
}

struct Unescaped {
    text: String,
    newlines: usize,
    consumed: usize,
}

/// b[i] == '\\' — decode the escape.
fn unescape_at(b: &[char], i: usize, _line: usize) -> Result<Unescaped, LexError> {
    let n = i + 1;
    if n >= b.len() {
        return Err(LexError { line: 0, msg: "unfinished escape at end of input".into() });
    }
    let c = b[n];
    match c {
        'n' => Ok(Unescaped { text: "\n".into(), newlines: 1, consumed: 2 }),
        'r' => Ok(Unescaped { text: "\r".into(), newlines: 0, consumed: 2 }),
        't' => Ok(Unescaped { text: "\t".into(), newlines: 0, consumed: 2 }),
        'b' => Ok(Unescaped { text: "\u{8}".into(), newlines: 0, consumed: 2 }),
        'f' => Ok(Unescaped { text: "\u{c}".into(), newlines: 0, consumed: 2 }),
        'v' => Ok(Unescaped { text: "\u{b}".into(), newlines: 0, consumed: 2 }),
        'a' => Ok(Unescaped { text: "\u{7}".into(), newlines: 0, consumed: 2 }),
        'e' => Ok(Unescaped { text: "\u{1b}".into(), newlines: 0, consumed: 2 }),
        '\\' => Ok(Unescaped { text: "\\".into(), newlines: 0, consumed: 2 }),
        '\'' => Ok(Unescaped { text: "'".into(), newlines: 0, consumed: 2 }),
        '"' => Ok(Unescaped { text: "\"".into(), newlines: 0, consumed: 2 }),
        '\n' => Ok(Unescaped { text: String::new(), newlines: 1, consumed: 2 }), // line continuation
        '0'..='7' => {
            let mut j = n;
            let mut val = 0u32;
            let mut cnt = 0;
            while j < b.len() && cnt < 3 && b[j] >= '0' && b[j] <= '7' {
                val = val * 8 + (b[j] as u32 - '0' as u32);
                j += 1;
                cnt += 1;
            }
            Ok(Unescaped {
                text: char::from_u32(val).unwrap_or('\0').to_string(),
                newlines: 0,
                consumed: j - i,
            })
        }
        'x' => {
            let mut j = n + 1;
            let mut digits = String::new();
            if j < b.len() && b[j] == '{' {
                j += 1;
                while j < b.len() && b[j] != '}' {
                    digits.push(b[j]);
                    j += 1;
                }
                if j < b.len() {
                    j += 1;
                }
            } else {
                while j < b.len() && digits.len() < 2 && b[j].is_ascii_hexdigit() {
                    digits.push(b[j]);
                    j += 1;
                }
            }
            // LDMud: if not valid hex, treat x literally
            if digits.is_empty() {
                return Ok(Unescaped { text: "x".into(), newlines: 0, consumed: 2 });
            }
            Ok(Unescaped {
                text: char::from_u32(u32::from_str_radix(&digits, 16).unwrap_or(0))
                    .unwrap_or('\0')
                    .to_string(),
                newlines: 0,
                consumed: j - i,
            })
        }
        _ => Ok(Unescaped { text: c.to_string(), newlines: 0, consumed: 2 }),
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}
