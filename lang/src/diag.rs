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
    pub span: Span,
    pub sev: Sev,
    pub code: &'static str,
    pub msg: String,
}

impl Diag {
    pub fn err(span: Span, code: &'static str, msg: impl Into<String>) -> Self {
        Diag { span, sev: Sev::Error, code, msg: msg.into() }
    }
}

pub struct Diags {
    pub file: String,
    pub src: String,
    pub list: Vec<Diag>,
}

impl Diags {
    pub fn new(file: impl Into<String>, src: impl Into<String>) -> Self {
        Diags { file: file.into(), src: src.into(), list: Vec::new() }
    }
    pub fn push(&mut self, d: Diag) {
        self.list.push(d);
    }
    pub fn has_errors(&self) -> bool {
        self.list.iter().any(|d| d.sev == Sev::Error)
    }
    pub fn report(&self) {
        for d in &self.list {
            let (line, col) = self.line_col(d.span.lo);
            let sev = match d.sev {
                Sev::Error => "error",
                Sev::Warning => "warning",
            };
            eprintln!("{}:{}:{}: {} [{}] {}", self.file, line, col, sev, d.code, d.msg);
        }
    }
    fn line_col(&self, off: u32) -> (usize, usize) {
        let mut line = 1usize;
        let mut col = 1usize;
        for (i, c) in self.src.char_indices() {
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
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.lo, self.hi)
    }
}
