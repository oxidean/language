use std::fmt;

#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub lo: u32,
    pub hi: u32,
}
impl Span {
    pub fn new(lo: u32, hi: u32) -> Self {
        Span { lo, hi }
    }
    pub fn join(a: Span, b: Span) -> Self {
        Span { lo: a.lo.min(b.lo), hi: a.hi.max(b.hi) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sev {
    Error,
    Warning,
}

#[derive(Debug)]
pub struct Diag {
    /// file the span indexes into — "" until push stamps it from the
    /// sink's file (or the checker's `pending`, walking a merged crate)
    pub file: String,
    pub span: Span,
    pub sev: Sev,
    pub code: &'static str,
    pub msg: String,
}

impl Diag {
    pub fn err(span: Span, code: &'static str, msg: impl Into<String>) -> Self {
        Diag { file: String::new(), span, sev: Sev::Error, code, msg: msg.into() }
    }
}

pub struct Diags {
    pub file: String,
    pub src: String,
    /// file `push` stamps on a diag arriving without one — the checker
    /// points it at the item's file as it walks a merged crate
    pub pending: String,
    pub list: Vec<Diag>,
}

impl Diags {
    pub fn new(file: impl Into<String>, src: impl Into<String>) -> Self {
        Diags { file: file.into(), src: src.into(), pending: String::new(), list: Vec::new() }
    }
    pub fn push(&mut self, mut d: Diag) {
        if d.file.is_empty() {
            d.file = if self.pending.is_empty() {
                self.file.clone()
            } else {
                self.pending.clone()
            };
        }
        self.list.push(d);
    }
    pub fn has_errors(&self) -> bool {
        self.list.iter().any(|d| d.sev == Sev::Error)
    }
    pub fn report(&self) {
        // each diag maps line/col against ITS file's text — a merged
        // crate's spans index into every loaded file, so the sink's src
        // alone would mis-position imported-file diagnostics
        let mut srcs: std::collections::HashMap<&str, Option<String>> =
            std::collections::HashMap::new();
        for d in &self.list {
            let f = if d.file.is_empty() { self.file.as_str() } else { d.file.as_str() };
            let (line, col) = if f == self.file {
                self.line_col(d.span.lo)
            } else {
                match srcs
                    .entry(f)
                    .or_insert_with(|| std::fs::read_to_string(f).ok())
                {
                    Some(s) => line_col_in(s, d.span.lo),
                    None => self.line_col(d.span.lo),
                }
            };
            let sev = match d.sev {
                Sev::Error => "error",
                Sev::Warning => "warning",
            };
            eprintln!("{}:{}:{}: {} [{}] {}", f, line, col, sev, d.code, d.msg);
        }
    }
    pub fn line_col(&self, off: u32) -> (usize, usize) {
        line_col_in(&self.src, off)
    }
}

/// byte offset -> 1-based (line, col) in `src`
pub fn line_col_in(src: &str, off: u32) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, c) in src.char_indices() {
        if i as u32 >= off {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.lo, self.hi)
    }
}
