//! The subset of the C preprocessor that device tree sources rely on: includes,
//! object- and function-like macros, and conditionals. Output carries cpp-style line
//! markers (`# 12 "file"`) so the parser can map every token back to its source.

use std::collections::HashMap;

use crate::lex::{Src, is_ident_char, is_ident_start};
use crate::vfs::{IncludeResolver, Vfs};
use crate::{Diagnostic, Severity};

const MAX_INCLUDE_DEPTH: usize = 64;
const MAX_EXPANSION_DEPTH: usize = 128;

#[derive(Debug, Clone)]
struct Macro {
    params: Option<Vec<String>>,
    variadic: bool,
    body: String,
}

pub struct Output {
    pub text: String,
    /// Every file that was read, in first-include order.
    pub files: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Preprocesses `main` (a path in `vfs`) with the given predefined macros.
pub fn preprocess(vfs: &dyn Vfs, main: &str, defines: &[(&str, &str)]) -> Output {
    let mut pp = Pp {
        vfs,
        resolver: IncludeResolver::new(vfs),
        macros: HashMap::new(),
        out: String::new(),
        files: Vec::new(),
        diags: Vec::new(),
        stack: Vec::new(),
    };
    pp.define_str("__DTS__", "1");
    for (name, value) in defines {
        pp.define_str(name, value);
    }
    if vfs.exists(main) {
        pp.run_file(main);
    } else {
        pp.diag(main, 0, 0, format!("file not found: {main}"), String::new());
    }
    Output {
        text: pp.out,
        files: pp.files,
        diagnostics: pp.diags,
    }
}

struct Cond {
    /// Whether lines in the current branch are emitted.
    active: bool,
    /// Whether some branch of this conditional has already been taken.
    taken: bool,
    /// Whether the enclosing context is active.
    parent: bool,
    seen_else: bool,
}

#[derive(Debug)]
enum ExpandError {
    /// A function-like macro's argument list runs past the end of the text.
    Unterminated,
    Other(String),
}

struct Pp<'a> {
    vfs: &'a dyn Vfs,
    resolver: IncludeResolver<'a>,
    macros: HashMap<String, Macro>,
    out: String,
    files: Vec<String>,
    diags: Vec<Diagnostic>,
    stack: Vec<String>,
}

impl Pp<'_> {
    fn diag(&mut self, file: &str, line: u32, col: u32, message: String, snippet: String) {
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            file: file.to_string(),
            line,
            col,
            message,
            snippet,
        });
    }

    fn warn(&mut self, file: &str, line: u32, message: String, snippet: String) {
        self.diags.push(Diagnostic {
            severity: Severity::Warning,
            file: file.to_string(),
            line,
            col: 1,
            message,
            snippet,
        });
    }

    fn define_str(&mut self, name: &str, value: &str) {
        self.macros.insert(
            name.to_string(),
            Macro {
                params: None,
                variadic: false,
                body: value.to_string(),
            },
        );
    }

    fn marker(&mut self, line: usize, path: &str) {
        self.out.push_str(&format!("# {line} \"{path}\"\n"));
    }

    fn run_file(&mut self, path: &str) {
        let Some(data) = self.vfs.read(path) else {
            self.diag(path, 0, 0, format!("cannot read {path}"), String::new());
            return;
        };
        if !self.files.iter().any(|f| f == path) {
            self.files.push(path.to_string());
        }
        let text = clean(&String::from_utf8_lossy(&data));
        self.stack.push(path.to_string());
        self.marker(1, path);

        let mut conds: Vec<Cond> = Vec::new();
        // A line whose function-like macro call continues onto following lines.
        let mut pending: Option<(String, usize, usize)> = None; // (text, first line, extra lines)

        let lines: Vec<&str> = text.split('\n').collect();
        let last = lines.len().saturating_sub(1);
        for (i, raw) in lines.iter().enumerate() {
            if i == last && raw.is_empty() {
                break;
            }
            let lineno = i + 1;
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let active = conds.last().is_none_or(|c| c.active);

            if pending.is_none()
                && let Some((dir, args)) = directive(line)
            {
                self.directive(path, lineno, line, dir, args, active, &mut conds);
                continue;
            }
            if !active {
                self.out.push('\n');
                continue;
            }
            if pending.is_none()
                && let Some(rest) = line.trim_start().strip_prefix("/include/")
            {
                match parse_include_name(rest) {
                    Some((name, _)) => self.include(path, lineno, line, &name, true),
                    None => {
                        self.diag(
                            path,
                            lineno as u32,
                            1,
                            "expected \"file\" after /include/".into(),
                            line.trim().into(),
                        );
                        self.out.push('\n');
                    }
                }
                continue;
            }

            let (joined, first, extra) = match pending.take() {
                Some((prev, first, extra)) => (format!("{prev} {line}"), first, extra + 1),
                None => (line.to_string(), lineno, 0),
            };
            match self.expand(&joined, &mut Vec::new(), 0) {
                Ok(expanded) => {
                    self.out.push_str(&expanded);
                    for _ in 0..=extra {
                        self.out.push('\n');
                    }
                }
                Err(ExpandError::Unterminated) if i < last => {
                    pending = Some((joined, first, extra))
                }
                Err(e) => {
                    let msg = match e {
                        ExpandError::Unterminated => "unterminated macro argument list".to_string(),
                        ExpandError::Other(m) => m,
                    };
                    self.diag(path, first as u32, 1, msg, joined.trim().to_string());
                    self.out.push_str(&joined);
                    for _ in 0..=extra {
                        self.out.push('\n');
                    }
                }
            }
        }
        if let Some((joined, first, _)) = pending {
            self.diag(
                path,
                first as u32,
                1,
                "unterminated macro argument list".into(),
                joined.trim().into(),
            );
        }
        if !conds.is_empty() {
            self.diag(
                path,
                lines.len() as u32,
                1,
                "unterminated #if/#ifdef".into(),
                String::new(),
            );
        }
        self.stack.pop();
    }

    #[allow(clippy::too_many_arguments)]
    fn directive(
        &mut self,
        path: &str,
        lineno: usize,
        line: &str,
        dir: &str,
        args: &str,
        active: bool,
        conds: &mut Vec<Cond>,
    ) {
        let ln = lineno as u32;
        let snippet = || line.trim().to_string();
        match dir {
            "ifdef" | "ifndef" => {
                let name = args.split_whitespace().next().unwrap_or("");
                let defined = self.macros.contains_key(name);
                let cond = if dir == "ifdef" { defined } else { !defined };
                conds.push(Cond {
                    active: active && cond,
                    taken: cond,
                    parent: active,
                    seen_else: false,
                });
            }
            "if" => {
                let cond = active && self.eval_if(path, ln, line, args);
                conds.push(Cond {
                    active: cond,
                    taken: cond,
                    parent: active,
                    seen_else: false,
                });
            }
            "elif" => match conds.last() {
                Some(c) if c.seen_else => {
                    self.diag(path, ln, 1, "#elif after #else".into(), snippet())
                }
                Some(c) => {
                    let (taken, parent) = (c.taken, c.parent);
                    let cond = !taken && parent && self.eval_if(path, ln, line, args);
                    let c = conds.last_mut().unwrap();
                    c.active = cond;
                    c.taken |= cond;
                }
                None => self.diag(path, ln, 1, "#elif without #if".into(), snippet()),
            },
            "else" => match conds.last_mut() {
                Some(c) if c.seen_else => {
                    self.diag(path, ln, 1, "#else after #else".into(), snippet())
                }
                Some(c) => {
                    c.active = c.parent && !c.taken;
                    c.taken = true;
                    c.seen_else = true;
                }
                None => self.diag(path, ln, 1, "#else without #if".into(), snippet()),
            },
            "endif" => {
                if conds.pop().is_none() {
                    self.diag(path, ln, 1, "#endif without #if".into(), snippet());
                }
            }
            _ if !active => {}
            "include" | "include_next" => {
                let parsed = parse_include_name(args).or_else(|| {
                    let expanded = self.expand(args, &mut Vec::new(), 0).ok()?;
                    parse_include_name(&expanded)
                });
                match parsed {
                    Some((name, quoted)) => {
                        self.include(path, lineno, line, &name, quoted);
                        return;
                    }
                    None => self.diag(
                        path,
                        ln,
                        1,
                        "expected \"file\" or <file> after #include".into(),
                        snippet(),
                    ),
                }
            }
            "define" => match parse_define(args) {
                Some((name, m)) => {
                    self.macros.insert(name, m);
                }
                None => self.diag(path, ln, 1, "malformed #define".into(), snippet()),
            },
            "undef" => {
                let name = args.split_whitespace().next().unwrap_or("");
                self.macros.remove(name);
            }
            "error" => self.diag(path, ln, 1, format!("#error {}", args.trim()), snippet()),
            "warning" => self.warn(path, ln, format!("#warning {}", args.trim()), snippet()),
            _ => {} // pragma, line, ident: ignored
        }
        self.out.push('\n');
    }

    fn include(&mut self, path: &str, lineno: usize, line: &str, name: &str, quoted: bool) {
        match self.resolver.resolve(name, path, quoted) {
            Some(target) if self.stack.contains(&target) => {
                self.diag(
                    path,
                    lineno as u32,
                    1,
                    format!("recursive include of {target}"),
                    line.trim().into(),
                );
                self.out.push('\n');
            }
            Some(_) if self.stack.len() >= MAX_INCLUDE_DEPTH => {
                self.diag(
                    path,
                    lineno as u32,
                    1,
                    "includes nested too deeply".into(),
                    line.trim().into(),
                );
                self.out.push('\n');
            }
            Some(target) => {
                self.run_file(&target);
                self.marker(lineno + 1, path);
            }
            None => {
                self.diag(
                    path,
                    lineno as u32,
                    1,
                    format!("cannot find include file `{name}`; open the folder that contains it, or a compiled .dtb"),
                    line.trim().into(),
                );
                self.out.push('\n');
            }
        }
    }

    fn eval_if(&mut self, path: &str, ln: u32, line: &str, args: &str) -> bool {
        let replaced = self.replace_defined(args);
        let expanded = match self.expand(&replaced, &mut Vec::new(), 0) {
            Ok(e) => e,
            Err(_) => {
                self.diag(
                    path,
                    ln,
                    1,
                    "could not expand #if expression".into(),
                    line.trim().into(),
                );
                return false;
            }
        };
        // Identifiers left after expansion evaluate to 0, as in C.
        let zeroed = map_idents(&expanded, |_| Some("0".to_string()));
        let mut src = Src::new(&zeroed);
        let result = src.expr().and_then(|v| {
            src.skip_ws();
            if src.eof() {
                Ok(v)
            } else {
                Err(src.err("unexpected text after expression"))
            }
        });
        match result {
            Ok(v) => v != 0,
            Err(e) => {
                self.diag(
                    path,
                    ln,
                    1,
                    format!("bad #if expression: {}", e.msg),
                    line.trim().into(),
                );
                false
            }
        }
    }

    /// Replaces `defined(X)` / `defined X` with 1 or 0.
    fn replace_defined(&self, text: &str) -> String {
        let b = text.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            if is_ident_start(b[i]) {
                let j = ident_end(b, i);
                if &text[i..j] == "defined" {
                    let mut k = skip_spaces(b, j);
                    let paren = b.get(k) == Some(&b'(');
                    if paren {
                        k = skip_spaces(b, k + 1);
                    }
                    let e = ident_end(b, k);
                    let name = &text[k..e];
                    let mut end = e;
                    if paren {
                        end = skip_spaces(b, e);
                        if b.get(end) == Some(&b')') {
                            end += 1;
                        }
                    }
                    out.push_str(if self.macros.contains_key(name) {
                        "1"
                    } else {
                        "0"
                    });
                    i = end;
                    continue;
                }
                out.push_str(&text[i..j]);
                i = j;
            } else {
                let ch = text[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }
        out
    }

    fn expand(
        &self,
        text: &str,
        hidden: &mut Vec<String>,
        depth: usize,
    ) -> Result<String, ExpandError> {
        if depth > MAX_EXPANSION_DEPTH {
            return Err(ExpandError::Other(
                "macro expansion too deep (recursive macro?)".into(),
            ));
        }
        let b = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == b'"' {
                let j = skip_string(b, i);
                out.push_str(&text[i..j]);
                i = j;
            } else if c.is_ascii_digit() {
                // A pp-number: `0x1f`, `10UL`; never split into an identifier.
                let mut j = i + 1;
                while j < b.len() && (is_ident_char(b[j]) || b[j] == b'.') {
                    j += 1;
                }
                out.push_str(&text[i..j]);
                i = j;
            } else if is_ident_start(c) {
                let j = ident_end(b, i);
                let name = &text[i..j];
                i = j;
                let Some(m) = self
                    .macros
                    .get(name)
                    .filter(|_| !hidden.iter().any(|h| h == name))
                else {
                    out.push_str(name);
                    continue;
                };
                let Some(params) = &m.params else {
                    hidden.push(name.to_string());
                    let r = self.expand(&m.body, hidden, depth + 1);
                    hidden.pop();
                    out.push_str(&r?);
                    continue;
                };
                let k = skip_ws_all(b, j);
                if b.get(k) != Some(&b'(') {
                    out.push_str(name); // function-like macro name without a call
                    continue;
                }
                let (raw_args, end) = parse_args(text, k).ok_or(ExpandError::Unterminated)?;
                let raw_args = bind_args(name, params, m.variadic, raw_args)?;
                let expanded_args = raw_args
                    .iter()
                    .map(|a| self.expand(a, hidden, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?;
                let body = substitute(&m.body, params, m.variadic, &raw_args, &expanded_args);
                hidden.push(name.to_string());
                let r = self.expand(&body, hidden, depth + 1);
                hidden.pop();
                out.push_str(&r?);
                i = end;
            } else {
                let ch = text[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }
        Ok(out)
    }
}

/// Returns `(directive, args)` if `line` is a preprocessor directive we handle. Lines such
/// as `#address-cells = <1>;` are DTS, not directives.
fn directive(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim_start().strip_prefix('#')?;
    let rest = rest.trim_start();
    let end = rest
        .bytes()
        .position(|c| !c.is_ascii_alphabetic() && c != b'_')
        .unwrap_or(rest.len());
    let (word, args) = rest.split_at(end);
    const DIRECTIVES: [&str; 14] = [
        "include",
        "include_next",
        "define",
        "undef",
        "if",
        "ifdef",
        "ifndef",
        "elif",
        "else",
        "endif",
        "error",
        "warning",
        "pragma",
        "line",
    ];
    // `#address-cells`: a DTS property whose name merely starts with a directive word.
    if DIRECTIVES.contains(&word) && !args.starts_with(['-', ',']) {
        Some((word, args))
    } else {
        None
    }
}

fn parse_include_name(args: &str) -> Option<(String, bool)> {
    let args = args.trim();
    let (close, quoted) = match args.as_bytes().first()? {
        b'"' => ('"', true),
        b'<' => ('>', false),
        _ => return None,
    };
    let end = args[1..].find(close)?;
    Some((args[1..1 + end].to_string(), quoted))
}

fn parse_define(args: &str) -> Option<(String, Macro)> {
    let args = args.trim_start();
    let b = args.as_bytes();
    if !b.first().is_some_and(|&c| is_ident_start(c)) {
        return None;
    }
    let end = ident_end(b, 0);
    let name = args[..end].to_string();
    if b.get(end) == Some(&b'(') {
        let close = args[end..].find(')')? + end;
        let mut params = Vec::new();
        let mut variadic = false;
        for p in args[end + 1..close]
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            if p == "..." {
                variadic = true;
                params.push("__VA_ARGS__".to_string());
            } else if let Some(named) = p.strip_suffix("...") {
                variadic = true;
                params.push(named.trim().to_string());
            } else {
                params.push(p.to_string());
            }
        }
        let body = args[close + 1..].trim().to_string();
        Some((
            name,
            Macro {
                params: Some(params),
                variadic,
                body,
            },
        ))
    } else {
        Some((
            name,
            Macro {
                params: None,
                variadic: false,
                body: args[end..].trim().to_string(),
            },
        ))
    }
}

fn bind_args(
    name: &str,
    params: &[String],
    variadic: bool,
    mut args: Vec<String>,
) -> Result<Vec<String>, ExpandError> {
    if params.is_empty() && args.len() == 1 && args[0].trim().is_empty() {
        return Ok(Vec::new());
    }
    if variadic && args.len() >= params.len() {
        let rest = args.split_off(params.len() - 1);
        args.push(rest.join(","));
    } else if variadic && args.len() + 1 == params.len() {
        args.push(String::new());
    }
    if args.len() != params.len() {
        return Err(ExpandError::Other(format!(
            "macro `{name}` takes {} argument(s), {} given",
            params.len(),
            args.len()
        )));
    }
    Ok(args.into_iter().map(|a| a.trim().to_string()).collect())
}

/// Substitutes parameters into a function-like macro body, handling `#param` and `##`.
fn substitute(
    body: &str,
    params: &[String],
    _variadic: bool,
    raw: &[String],
    expanded: &[String],
) -> String {
    let b = body.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let param_index = |name: &str| params.iter().position(|p| p == name);
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            let j = skip_string(b, i);
            out.push_str(&body[i..j]);
            i = j;
        } else if c == b'#' && b.get(i + 1) == Some(&b'#') {
            out.push_str("##");
            i += 2;
        } else if c == b'#' {
            let k = skip_spaces(b, i + 1);
            let e = ident_end(b, k);
            if let Some(p) = param_index(&body[k..e]).filter(|_| e > k) {
                out.push('"');
                out.push_str(&raw[p].replace('\\', "\\\\").replace('"', "\\\""));
                out.push('"');
                i = e;
            } else {
                out.push('#');
                i += 1;
            }
        } else if is_ident_start(c) {
            let e = ident_end(b, i);
            let word = &body[i..e];
            match param_index(word) {
                Some(p) => {
                    // Operands of ## are not macro-expanded first.
                    let before_paste = out.trim_end().ends_with("##");
                    let after_paste = body[e..].trim_start().starts_with("##");
                    out.push_str(if before_paste || after_paste {
                        &raw[p]
                    } else {
                        &expanded[p]
                    });
                }
                None => out.push_str(word),
            }
            i = e;
        } else {
            let ch = body[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    paste(&out)
}

/// Applies token pasting: removes `##` and the whitespace around it.
fn paste(text: &str) -> String {
    if !text.contains("##") {
        return text.to_string();
    }
    let mut out = String::new();
    for (n, part) in text.split("##").enumerate() {
        if n == 0 {
            out.push_str(part.trim_end());
        } else {
            out = out.trim_end().to_string();
            out.push_str(part.trim_start());
        }
    }
    out
}

/// Parses a parenthesized argument list starting at `open`; returns the raw arguments and
/// the offset just past the closing paren, or `None` if unterminated.
fn parse_args(text: &str, open: usize) -> Option<(Vec<String>, usize)> {
    let b = text.as_bytes();
    let mut depth = 0;
    let mut args = Vec::new();
    let mut start = open + 1;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i = skip_string(b, i);
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    args.push(text[start..i].to_string());
                    return Some((args, i + 1));
                }
            }
            b',' if depth == 1 => {
                args.push(text[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Replaces each identifier (outside strings) for which `f` returns `Some`.
fn map_idents(text: &str, f: impl Fn(&str) -> Option<String>) -> String {
    let b = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let j = skip_string(b, i);
            out.push_str(&text[i..j]);
            i = j;
        } else if b[i].is_ascii_digit() {
            let mut j = i + 1;
            while j < b.len() && is_ident_char(b[j]) {
                j += 1;
            }
            out.push_str(&text[i..j]);
            i = j;
        } else if is_ident_start(b[i]) {
            let j = ident_end(b, i);
            match f(&text[i..j]) {
                Some(r) => out.push_str(&r),
                None => out.push_str(&text[i..j]),
            }
            i = j;
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn ident_end(b: &[u8], start: usize) -> usize {
    let mut j = start;
    while j < b.len() && is_ident_char(b[j]) {
        j += 1;
    }
    j
}

fn skip_spaces(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    i
}

fn skip_ws_all(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Skips a string literal starting at the opening quote; returns the offset past it.
fn skip_string(b: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            b'\n' => return i,
            _ => i += 1,
        }
    }
    b.len()
}

/// Splices backslash-newline continuations and strips comments, keeping the number of
/// lines unchanged so line numbers stay accurate.
fn clean(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut pending_newlines = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\\' && b.get(i + 1) == Some(&b'\n') {
            i += 2;
            pending_newlines += 1;
        } else if c == b'\\' && b.get(i + 1) == Some(&b'\r') && b.get(i + 2) == Some(&b'\n') {
            i += 3;
            pending_newlines += 1;
        } else if c == b'\n' {
            out.push(b'\n');
            out.extend(std::iter::repeat_n(b'\n', pending_newlines));
            pending_newlines = 0;
            i += 1;
        } else if c == b'"' {
            let j = skip_string(b, i);
            out.extend(&b[i..j]);
            i = j;
        } else if c == b'\''
            && let Some(len) = char_literal_len(&b[i..])
        {
            out.extend(&b[i..i + len]);
            i += len;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            out.push(b' ');
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                if b[i] == b'\n' {
                    out.push(b'\n');
                }
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else {
            out.push(c);
            i += 1;
        }
    }
    out.extend(std::iter::repeat_n(b'\n', pending_newlines));
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Length of a short character literal like `'a'` or `'\n'`, so an apostrophe inside it
/// isn't mistaken for anything else. Returns `None` for a lone apostrophe.
fn char_literal_len(b: &[u8]) -> Option<usize> {
    let end = b
        .iter()
        .skip(1)
        .take(6)
        .position(|&c| c == b'\'' || c == b'\n')?
        + 1;
    (b[end] == b'\'' && end > 1).then_some(end + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemVfs;

    fn pp(files: &[(&str, &str)]) -> Output {
        let mut vfs = MemVfs::new();
        for (p, t) in files {
            vfs.insert(p, *t);
        }
        preprocess(&vfs, files[0].0, &[])
    }

    /// Output with line markers removed and lines trimmed.
    fn body(o: &Output) -> Vec<String> {
        o.text
            .lines()
            .filter(|l| !l.starts_with("# "))
            .map(|l| l.trim().to_string())
            .collect()
    }

    #[test]
    fn object_and_function_macros() {
        let o = pp(&[(
            "a.dts",
            "#define GIC_SPI 0\n#define IRQ_TYPE_LEVEL_HIGH 4\n#define PIN(bank, n) ((bank) * 32 + (n))\n\
             interrupts = <GIC_SPI 12 IRQ_TYPE_LEVEL_HIGH>;\npin = <PIN(1, 3)>;\n",
        )]);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let b = body(&o);
        assert_eq!(b[3], "interrupts = <0 12 4>;");
        assert_eq!(b[4], "pin = <((1) * 32 + (3))>;");
    }

    #[test]
    fn stringify_paste_and_variadic() {
        let o = pp(&[(
            "a.dts",
            "#define S(x) #x\n#define CAT(a, b) a##b\n#define V(f, ...) f(__VA_ARGS__)\n#define ID(x) x\n\
             s = S(hi there);\nc = <CAT(0x, 10)>;\nv = <V(ID, 7)>;\n",
        )]);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let b = body(&o);
        assert_eq!(b[4], "s = \"hi there\";");
        assert_eq!(b[5], "c = <0x10>;");
        assert_eq!(b[6], "v = <7>;");
    }

    #[test]
    fn conditionals() {
        let o = pp(&[(
            "a.dts",
            "#define A 2\n#if A > 1 && defined(A)\nyes1\n#else\nno1\n#endif\n#ifndef B\nyes2\n#elif 1\nno2\n#endif\n\
             #if 0\n#if 1\nno3\n#endif\n#elif !defined B\nyes3\n#endif\n",
        )]);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let kept: Vec<String> = body(&o).into_iter().filter(|l| !l.is_empty()).collect();
        assert_eq!(kept, ["yes1", "yes2", "yes3"]);
    }

    #[test]
    fn includes_with_line_markers() {
        let o = pp(&[
            (
                "board.dts",
                "#include \"soc.dtsi\"\n#include <dt-bindings/x.h>\nX\n",
            ),
            ("soc.dtsi", "soc\n"),
            (
                "include/dt-bindings/x.h",
                "#ifndef X_H\n#define X_H\n#define X 42\n#endif\n",
            ),
        ]);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(
            o.text
                .contains("# 1 \"soc.dtsi\"\nsoc\n# 2 \"board.dts\"\n")
        );
        assert!(o.text.ends_with("# 3 \"board.dts\"\n42\n"));
        assert_eq!(
            o.files,
            ["board.dts", "soc.dtsi", "include/dt-bindings/x.h"]
        );
    }

    #[test]
    fn dts_hash_properties_are_not_directives() {
        let o = pp(&[(
            "a.dts",
            "#address-cells = <1>;\n#size-cells = <0>;\n#interrupt-cells = <2>;\n",
        )]);
        assert!(o.diagnostics.is_empty());
        assert_eq!(
            body(&o),
            [
                "#address-cells = <1>;",
                "#size-cells = <0>;",
                "#interrupt-cells = <2>;"
            ]
        );
    }

    #[test]
    fn comments_and_continuations_keep_line_count() {
        let o = pp(&[(
            "a.dts",
            "/* one\n two */ a\n#define M 1 + \\\n 2\nb = <(M)>; // c\n",
        )]);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let b = body(&o);
        assert_eq!(b.len(), 5);
        assert_eq!(b[1], "a");
        assert_eq!(b[4], "b = <(1 +  2)>;");
    }

    #[test]
    fn multi_line_macro_call() {
        let o = pp(&[("a.dts", "#define F(a, b) a b\nx = <F(1,\n 2)>;\ny;\n")]);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let b = body(&o);
        assert_eq!(b[1], "x = <1 2>;");
        assert_eq!(b[3], "y;");
    }

    #[test]
    fn missing_include_is_reported() {
        let o = pp(&[("a.dts", "#include <dt-bindings/gpio/gpio.h>\n")]);
        assert_eq!(o.diagnostics.len(), 1);
        assert_eq!(o.diagnostics[0].line, 1);
        assert!(o.diagnostics[0].message.contains("dt-bindings/gpio/gpio.h"));
    }

    #[test]
    fn recursive_macro_does_not_loop() {
        let o = pp(&[("a.dts", "#define A B\n#define B A\nA\n")]);
        assert!(o.diagnostics.is_empty());
        assert_eq!(body(&o)[2], "A");
    }
}
