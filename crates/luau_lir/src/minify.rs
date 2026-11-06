//! Token-level Luau minifier for generated modules.
//!
//! - every *declared* identifier (locals, functions, parameters, loop variables) is renamed
//!   consistently to a short name, most frequent first; this is a bijection, so scoping is
//!   preserved. Fields (`a.b`, `a:b`), table-constructor keys, type annotations and globals
//!   are never renamed.
//! - comments and insignificant whitespace are removed (`--!` directives are kept)
//! - number literals take their shortest exact spelling
//! - string literals that repeat are stored once in a table

use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Name(String),
    Num(String),
    Str(String),
    Op(String),
}

const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil", "not",
    "or", "repeat", "return", "then", "true", "until", "while", "continue", "type", "export", "typeof",
];

/// Names that must never be produced by renaming (Luau/Roblox globals and type names).
const RESERVED: &[&str] = &[
    "buffer", "bit32", "math", "string", "table", "os", "utf8", "coroutine", "debug", "task", "error", "print", "warn",
    "tostring", "tonumber", "type", "typeof", "pcall", "xpcall", "select", "setmetatable", "getmetatable", "rawget",
    "rawset", "rawequal", "rawlen", "next", "pairs", "ipairs", "unpack", "assert", "require", "script", "game",
    "workspace", "DateTime", "Vector3", "CFrame", "Instance", "Enum", "_G", "_VERSION", "gcinfo", "newproxy",
    "number", "boolean", "any", "nil", "never", "unknown", "self", "vector", "loadstring", "getfenv", "setfenv",
];

fn tokenize(src: &str) -> (Vec<String>, Vec<Tok>) {
    let b = src.as_bytes();
    let mut i = 0;
    let mut directives = vec![];
    let mut toks = vec![];
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'-' && b.get(i + 1) == Some(&b'-') {
            // comment (keep --! directives)
            if b.get(i + 2) == Some(&b'[') && (b.get(i + 3) == Some(&b'[') || b.get(i + 3) == Some(&b'=')) {
                let end = src[i..].find("]]").map(|e| i + e + 2).unwrap_or(b.len());
                i = end;
            } else {
                let end = src[i..].find('\n').map(|e| i + e).unwrap_or(b.len());
                if b.get(i + 2) == Some(&b'!') {
                    directives.push(src[i..end].trim_end().to_string());
                }
                i = end;
            }
        } else if c == b'"' || c == b'\'' {
            let start = i;
            i += 1;
            while i < b.len() && b[i] != c {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            toks.push(Tok::Str(src[start..i].to_string()));
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            if c == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
                i += 2;
                while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
                    i += 1;
                }
            } else {
                while i < b.len() {
                    let d = b[i];
                    if d.is_ascii_digit() || d == b'.' || d == b'_' {
                        i += 1;
                    } else if (d == b'e' || d == b'E') && i + 1 < b.len() {
                        i += 1;
                        if b[i] == b'+' || b[i] == b'-' {
                            i += 1;
                        }
                    } else {
                        break;
                    }
                }
            }
            toks.push(Tok::Num(src[start..i].to_string()));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            toks.push(Tok::Name(src[start..i].to_string()));
        } else {
            const OPS: &[&str] = &[
                "...", "..=", "//=", "..", "==", "~=", "<=", ">=", "//", "+=", "-=", "*=", "/=", "%=", "^=", "::", "->",
            ];
            let mut op = None;
            for o in OPS {
                if src[i..].starts_with(o) {
                    op = Some(*o);
                    break;
                }
            }
            let o = op.map(|s| s.to_string()).unwrap_or_else(|| (c as char).to_string());
            i += o.len();
            toks.push(Tok::Op(o));
        }
    }
    (directives, toks)
}

/// Positions of names that are declarations (locals, params, loop vars, function names).
fn declared(toks: &[Tok]) -> HashSet<String> {
    let mut out = HashSet::new();
    let name = |t: &Tok| match t {
        Tok::Name(n) if !KEYWORDS.contains(&n.as_str()) => Some(n.clone()),
        _ => None,
    };
    let is_op = |t: Option<&Tok>, s: &str| matches!(t, Some(Tok::Op(o)) if o == s);
    // skip a type annotation starting after ':' ; returns index of the token ending it
    let skip_type = |toks: &[Tok], mut j: usize| -> usize {
        let mut depth = 0i32;
        while j < toks.len() {
            match &toks[j] {
                Tok::Op(o) if o == "(" || o == "{" || o == "[" => depth += 1,
                Tok::Op(o) if (o == ")" || o == "}" || o == "]") && depth > 0 => depth -= 1,
                Tok::Op(o) if depth == 0 && (o == "," || o == "=" || o == ")") => return j,
                Tok::Name(n) if depth == 0 && KEYWORDS.contains(&n.as_str()) && n != "nil" => return j,
                Tok::Name(_) if depth == 0 && j > 0 && matches!(toks[j - 1], Tok::Name(_)) => return j,
                _ => {}
            }
            j += 1;
        }
        j
    };
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Name(k) if k == "local" => {
                let mut j = i + 1;
                if matches!(toks.get(j), Some(Tok::Name(f)) if f == "function") {
                    if let Some(n) = toks.get(j + 1).and_then(name) {
                        out.insert(n);
                    }
                    i = j + 1;
                    continue;
                }
                while let Some(n) = toks.get(j).and_then(name) {
                    out.insert(n);
                    j += 1;
                    if is_op(toks.get(j), ":") {
                        j = skip_type(toks, j + 1);
                    }
                    if is_op(toks.get(j), ",") {
                        j += 1;
                    } else {
                        break;
                    }
                }
                i = j;
                continue;
            }
            Tok::Name(k) if k == "function" => {
                // parameters
                let mut j = i + 1;
                while j < toks.len() && !is_op(toks.get(j), "(") {
                    j += 1;
                }
                j += 1;
                while let Some(n) = toks.get(j).and_then(name) {
                    out.insert(n);
                    j += 1;
                    if is_op(toks.get(j), ":") {
                        j = skip_type(toks, j + 1);
                    }
                    if is_op(toks.get(j), ",") {
                        j += 1;
                    } else {
                        break;
                    }
                }
            }
            Tok::Name(k) if k == "for" => {
                let mut j = i + 1;
                while let Some(n) = toks.get(j).and_then(name) {
                    out.insert(n);
                    j += 1;
                    if is_op(toks.get(j), ":") {
                        j = skip_type(toks, j + 1);
                    }
                    if is_op(toks.get(j), ",") {
                        j += 1;
                    } else {
                        break;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Which token positions are renameable name uses (not fields, keys or type annotations).
fn renameable(toks: &[Tok], decl: &HashSet<String>, strip: &mut Vec<bool>) -> Vec<bool> {
    let mut out = vec![false; toks.len()];
    *strip = vec![false; toks.len()];
    let mut strip_this_type = false;
    // track brace depth to recognize `{ key = value }`
    let mut brace = 0i32;
    // type-annotation regions: after ':' in local/param position until ',', '=', ')'
    let mut in_type = false;
    let mut type_depth = 0i32;
    let mut decl_ctx = 0i32; // >0 while inside a local/param/for declaration list
    for i in 0..toks.len() {
        let t = &toks[i];
        if in_type {
            match t {
                Tok::Op(o) if o == "(" || o == "{" || o == "[" => type_depth += 1,
                Tok::Op(o) if (o == ")" || o == "}" || o == "]") && type_depth > 0 => type_depth -= 1,
                Tok::Op(o) if type_depth == 0 && (o == "," || o == "=" || o == ")") => in_type = false,
                Tok::Name(n) if type_depth == 0 && KEYWORDS.contains(&n.as_str()) && n != "nil" => in_type = false,
                Tok::Name(_) if type_depth == 0 && i > 0 && matches!(toks[i - 1], Tok::Name(_)) => in_type = false,
                _ => {}
            }
            if in_type {
                if strip_this_type {
                    strip[i] = true;
                }
                continue;
            }
        }
        match t {
            Tok::Name(n) => {
                if n == "local" || n == "for" {
                    decl_ctx = 1;
                } else if n == "function" {
                    decl_ctx = 2;
                }
                let prev = if i > 0 { Some(&toks[i - 1]) } else { None };
                let next = toks.get(i + 1);
                let is_field = matches!(prev, Some(Tok::Op(o)) if o == "." || o == ":");
                let is_key = brace > 0
                    && matches!(next, Some(Tok::Op(o)) if o == "=")
                    && matches!(prev, Some(Tok::Op(o)) if o == "{" || o == "," || o == ";");
                if !is_field && !is_key && decl.contains(n) && !RESERVED.contains(&n.as_str()) {
                    out[i] = true;
                }
            }
            Tok::Op(o) => match o.as_str() {
                "{" => brace += 1,
                "}" => brace -= 1,
                ":" if decl_ctx > 0 => {
                    // type annotation in a declaration (local x: T / (a: T) / ): T )
                    in_type = true;
                    type_depth = 0;
                    // annotations on locals are optional; parameter annotations guide native codegen
                    strip_this_type = decl_ctx == 1;
                    if strip_this_type {
                        strip[i] = true;
                    }
                }
                "=" | "in" => decl_ctx = 0,
                ")" if decl_ctx == 2 => {
                    // return type annotation may follow
                    if matches!(toks.get(i + 1), Some(Tok::Op(c)) if c == ":") {
                        decl_ctx = 2;
                    } else {
                        decl_ctx = 0;
                    }
                }
                _ => {}
            },
            _ => {}
        }
        if let Tok::Name(n) = t {
            if KEYWORDS.contains(&n.as_str()) && n != "local" && n != "function" && n != "for" {
                decl_ctx = 0;
            }
        }
    }
    out
}

/// For each token: is it inside a `while`/`for`/`repeat` loop of the current function?
fn loop_mask(toks: &[Tok]) -> Vec<bool> {
    #[derive(PartialEq)]
    enum K {
        Func,
        Loop,
        Block,
    }
    let mut stack: Vec<K> = vec![];
    let mut skip_do = false;
    let mut out = vec![false; toks.len()];
    let inside = |stack: &Vec<K>| {
        for k in stack.iter().rev() {
            match k {
                K::Loop => return true,
                K::Func => return false,
                K::Block => {}
            }
        }
        false
    };
    for (i, t) in toks.iter().enumerate() {
        out[i] = inside(&stack);
        if let Tok::Name(n) = t {
            match n.as_str() {
                "function" => stack.push(K::Func),
                "while" | "for" => {
                    stack.push(K::Loop);
                    skip_do = true;
                }
                "repeat" => stack.push(K::Loop),
                "do" => {
                    if skip_do {
                        skip_do = false;
                    } else {
                        stack.push(K::Block);
                    }
                }
                "if" => stack.push(K::Block),
                "end" | "until" => {
                    stack.pop();
                }
                _ => {}
            }
        }
    }
    out
}

fn short_name(mut k: usize) -> String {
    const FIRST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_";
    const REST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_0123456789";
    let mut s = String::new();
    s.push(FIRST[k % FIRST.len()] as char);
    k /= FIRST.len();
    while k > 0 {
        k -= 1;
        s.push(REST[k % REST.len()] as char);
        k /= REST.len();
    }
    s
}

/// Shortest exact spelling of a numeric literal.
fn short_num(s: &str) -> String {
    let clean = s.replace('_', "");
    let v: Option<f64> = if let Some(h) = clean.strip_prefix("0x").or_else(|| clean.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok().map(|x| x as f64)
    } else {
        clean.parse().ok()
    };
    let Some(v) = v else { return s.to_string() };
    if v.fract() != 0.0 || v.abs() >= 9007199254740992.0 || v < 0.0 {
        // keep floats as written (already shortest round-trip from the emitter)
        return clean;
    }
    let n = v as u64;
    let mut best = n.to_string();
    let hex = format!("0x{n:X}");
    if hex.len() < best.len() {
        best = hex;
    }
    // m e k
    let mut m = n;
    let mut k = 0;
    while m != 0 && m % 10 == 0 {
        m /= 10;
        k += 1;
    }
    if k >= 3 {
        let e = format!("{m}e{k}");
        if e.len() < best.len() {
            best = e;
        }
    }
    best
}

fn needs_space(a: &str, b: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let (la, fb) = (a.chars().last().unwrap(), b.chars().next().unwrap());
    if word(la) && word(fb) {
        return true;
    }
    // number followed by '.' / '..' would extend the number
    if a.chars().next().unwrap().is_ascii_digit() && fb == '.' {
        return true;
    }
    if la == '.' && (fb == '.' || fb.is_ascii_digit()) {
        return true;
    }
    // "- -" would become a comment; "[ [" a long string
    (la == '-' && fb == '-') || (la == '[' && (fb == '[' || fb == '=')) || (la == '=' && fb == '=') || (la == '~' && fb == '=')
        || ((la == '<' || la == '>') && fb == '=')
        || (la == '/' && fb == '/')
}

/// Library members aliased to module locals (counts against Luau's 200-local limit; the
/// emitter reserves room for these).
pub const MAX_ALIASES: usize = 24;

#[derive(Clone, Copy, Default)]
pub struct Options {
    /// Drop type annotations on `local` declarations (smaller; can cost some native-code speed).
    pub strip_local_types: bool,
    /// Also alias library functions inside loops. Measured to be speed-neutral in native code,
    /// so `minify` enables it; `false` keeps loop bodies on direct `lib.fn` imports.
    pub alias_in_loops: bool,
}

pub fn minify(src: &str) -> String {
    minify_with(src, Options { alias_in_loops: true, ..Options::default() })
}

pub fn minify_with(src: &str, opts: Options) -> String {
    let (directives, toks0) = tokenize(src);
    let decl = declared(&toks0);
    let mut strip = vec![];
    let ren0 = renameable(&toks0, &decl, &mut strip);
    // drop stripped tokens up front
    let mut toks = vec![];
    let mut ren = vec![];
    for (i, t) in toks0.into_iter().enumerate() {
        if opts.strip_local_types && strip[i] {
            continue;
        }
        toks.push(t);
        ren.push(ren0[i]);
    }

    // repeated long string literals -> K[i]
    let mut str_count: HashMap<&str, usize> = HashMap::new();
    for t in &toks {
        if let Tok::Str(s) = t {
            if s.len() >= 12 {
                *str_count.entry(s.as_str()).or_default() += 1;
            }
        }
    }
    let mut interned: Vec<String> = str_count.iter().filter(|(_, c)| **c >= 2).map(|(s, _)| s.to_string()).collect();
    interned.sort();
    let intern_idx: HashMap<String, usize> = interned.iter().enumerate().map(|(i, s)| (s.clone(), i + 1)).collect();

    // frequency-ranked short names
    let mut freq: HashMap<String, usize> = HashMap::new();
    for (i, t) in toks.iter().enumerate() {
        if ren[i] {
            if let Tok::Name(n) = t {
                *freq.entry(n.clone()).or_default() += 1;
            }
        }
    }
    let mut names: Vec<(String, usize)> = freq.into_iter().collect();
    names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    // names that stay as-is (globals, fields, keys) must not be produced
    let mut taken: HashSet<String> = HashSet::new();
    for (i, t) in toks.iter().enumerate() {
        if let Tok::Name(n) = t {
            if !ren[i] {
                taken.insert(n.clone());
            }
        }
    }
    for k in KEYWORDS.iter().chain(RESERVED) {
        taken.insert(k.to_string());
    }
    let mut counter = 0usize;
    let mut next_name = |taken: &HashSet<String>| loop {
        let s = short_name(counter);
        counter += 1;
        if !taken.contains(&s) {
            return s;
        }
    };
    let ktab = if interned.is_empty() { None } else { Some(next_name(&taken)) };
    let mut map: HashMap<String, String> = HashMap::new();
    for (n, _) in &names {
        map.insert(n.clone(), next_name(&taken));
    }

    // optionally keep direct `buffer.readu32` imports inside loops
    let in_loop = if opts.alias_in_loops { vec![false; toks.len()] } else { loop_mask(&toks) };
    // alias frequently used library members (`buffer.readu32` -> `q`) elsewhere
    const LIBS: &[&str] = &["buffer", "bit32", "math", "string", "table"];
    let mut member_count: HashMap<(String, String), usize> = HashMap::new();
    for i in 0..toks.len().saturating_sub(2) {
        if let (Tok::Name(lib), Tok::Op(dot), Tok::Name(m)) = (&toks[i], &toks[i + 1], &toks[i + 2]) {
            let after_dot = i > 0 && matches!(&toks[i - 1], Tok::Op(o) if o == "." || o == ":");
            if dot == "." && LIBS.contains(&lib.as_str()) && !ren[i] && !after_dot && !in_loop[i] {
                *member_count.entry((lib.clone(), m.clone())).or_default() += 1;
            }
        }
    }
    let mut members: Vec<((String, String), usize)> = member_count.into_iter().filter(|(_, c)| *c >= 4).collect();
    members.sort_by(|a, b| (b.1 * (b.0 .0.len() + b.0 .1.len())).cmp(&(a.1 * (a.0 .0.len() + a.0 .1.len()))).then(a.0.cmp(&b.0)));
    members.truncate(MAX_ALIASES);
    let mut alias: HashMap<(String, String), String> = HashMap::new();
    for (k, _) in &members {
        alias.insert(k.clone(), next_name(&taken));
    }

    // rewrite tokens
    let mut i = 0;
    let mut merged: Vec<Tok> = Vec::with_capacity(toks.len());
    let mut merged_ren: Vec<bool> = Vec::with_capacity(toks.len());
    while i < toks.len() {
        if i + 2 < toks.len() {
            if let (Tok::Name(lib), Tok::Op(dot), Tok::Name(m)) = (&toks[i], &toks[i + 1], &toks[i + 2]) {
                let after_dot = i > 0 && matches!(&toks[i - 1], Tok::Op(o) if o == "." || o == ":");
                if dot == "." && !ren[i] && !after_dot && !in_loop[i] {
                    if let Some(a) = alias.get(&(lib.clone(), m.clone())) {
                        merged.push(Tok::Name(a.clone()));
                        merged_ren.push(false);
                        i += 3;
                        continue;
                    }
                }
            }
        }
        merged.push(toks[i].clone());
        merged_ren.push(ren[i]);
        i += 1;
    }
    let toks_len_before = toks.len();
    toks = merged;
    let ren = merged_ren;
    let _ = toks_len_before;
    for (i, t) in toks.iter_mut().enumerate() {
        match t {
            Tok::Name(n) if ren[i] => *n = map[n].clone(),
            Tok::Num(s) => *s = short_num(s),
            _ => {}
        }
    }

    let mut out = String::with_capacity(src.len() / 2);
    for d in &directives {
        out.push_str(d);
        out.push('\n');
    }
    if let Some(k) = &ktab {
        out.push_str(&format!("local {k}={{{}}}\n", interned.join(",")));
    }
    if !members.is_empty() {
        let names: Vec<&str> = members.iter().map(|(k, _)| alias[k].as_str()).collect();
        let vals: Vec<String> = members.iter().map(|(k, _)| format!("{}.{}", k.0, k.1)).collect();
        out.push_str(&format!("local {}={}\n", names.join(","), vals.join(",")));
    }
    let mut prev: Option<String> = None;
    for t in &toks {
        let s = match t {
            Tok::Str(s) => match (&ktab, intern_idx.get(s)) {
                (Some(k), Some(i)) => format!("{k}[{i}]"),
                _ => s.clone(),
            },
            Tok::Name(s) | Tok::Num(s) | Tok::Op(s) => s.clone(),
        };
        if let Some(p) = &prev {
            if needs_space(p, &s) {
                out.push(' ');
            }
        }
        out.push_str(&s);
        prev = Some(s);
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(short_num("0x100000000"), "4294967296");
        assert_eq!(short_num("1000000"), "1e6");
        assert_eq!(short_num("0xFFFFFFFF"), "4294967295");
        assert_eq!(short_num("255"), "255");
        assert_eq!(short_num("0.5"), "0.5");
    }

    #[test]
    fn renames_locals_not_fields() {
        let src = "--!native\nlocal MEM: buffer = buffer.create(10)\nlocal function add(a: number, b: number): number\n\treturn a + b\nend\nreturn { add = add, memory = function(): buffer return MEM end }\n";
        let m = minify(src);
        assert!(m.starts_with("--!native\n"));
        assert!(m.contains("buffer.create(10)"));
        assert!(m.contains("add="), "{m}");
        assert!(!m.contains("MEM"), "{m}");
        assert!(m.contains(": buffer") || m.contains(":buffer"), "{m}");
    }
}
