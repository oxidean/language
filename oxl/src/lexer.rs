use crate::diag::{Diag, Diags, Span};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kw {
    Fn, Struct, Extern, Return, If, Else, For, Let, Scope, Spawn, Go,
    Own, Ref, Mut, Unsafe, Nil, True, False, Import, Chan,
    Break, Continue, Interface, SpawnB, Defer, Select, Enum, Match,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum P {
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Comma, Colon, Semi, Dot, Arrow, FatArrow,
    Assign, ColonEq, Underscore,
    Plus, Minus, Star, Slash, Percent,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or, Bang, Pipe, Amp, Caret,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Kw(Kw),
    P(P),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

pub fn lex(src: &str, diags: &mut Diags) -> Vec<Token> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    let at = |i: usize| -> u32 { i as u32 };
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let lo = i;
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 >= b.len() {
                    diags.push(Diag::err(Span::new(at(lo), at(b.len())), "lex", "unterminated block comment"));
                }
                i = (i + 2).min(b.len());
            }
            b'0'..=b'9' => {
                let lo = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
                // float: digits '.' digits ('.' not followed by digit stays punct)
                let mut is_float = false;
                if i < b.len() && b[i] == b'.'
                    && i + 1 < b.len() && b[i + 1].is_ascii_digit()
                {
                    i += 1;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    is_float = true;
                }
                let s: String = src[lo..i].chars().filter(|c| *c != '_').collect();
                if is_float {
                    match s.parse::<f64>() {
                        Ok(v) => out.push(Token { tok: Tok::Float(v), span: Span::new(at(lo), at(i)) }),
                        Err(_) => diags.push(Diag::err(Span::new(at(lo), at(i)), "lex", "float literal out of range")),
                    }
                } else {
                    match s.parse::<i64>() {
                        Ok(v) => out.push(Token { tok: Tok::Int(v), span: Span::new(at(lo), at(i)) }),
                        Err(_) => diags.push(Diag::err(Span::new(at(lo), at(i)), "lex", "integer literal out of range")),
                    }
                }
            }
            b'"' => {
                let lo = i;
                i += 1;
                let mut s = String::new();
                let mut closed = false;
                while i < b.len() {
                    match b[i] {
                        b'"' => {
                            i += 1;
                            closed = true;
                            break;
                        }
                        b'\\' if i + 1 < b.len() => {
                            let e = match b[i + 1] {
                                b'n' => '\n',
                                b't' => '\t',
                                b'r' => '\r',
                                b'0' => '\0',
                                b'\\' => '\\',
                                b'"' => '"',
                                x => x as char,
                            };
                            s.push(e);
                            i += 2;
                        }
                        _ => {
                            // source is valid UTF-8 — take the full
                            // char, not one byte (multi-byte literals
                            // must survive the lex round-trip)
                            let c = src[i..].chars().next().unwrap();
                            s.push(c);
                            i += c.len_utf8();
                        }
                    }
                }
                if !closed {
                    diags.push(Diag::err(Span::new(at(lo), at(i)), "lex", "unterminated string"));
                }
                out.push(Token { tok: Tok::Str(s), span: Span::new(at(lo), at(i)) });
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let lo = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let w = &src[lo..i];
                let kw = match w {
                    "fn" => Some(Kw::Fn),
                    "struct" => Some(Kw::Struct),
                    "extern" => Some(Kw::Extern),
                    "return" => Some(Kw::Return),
                    "if" => Some(Kw::If),
                    "else" => Some(Kw::Else),
                    "for" => Some(Kw::For),
                    "let" => Some(Kw::Let),
                    "scope" => Some(Kw::Scope),
                    "spawn" => Some(Kw::Spawn),
                    "go" => Some(Kw::Go),
                    "own" => Some(Kw::Own),
                    "ref" => Some(Kw::Ref),
                    "mut" => Some(Kw::Mut),
                    "unsafe" => Some(Kw::Unsafe),
                    "nil" => Some(Kw::Nil),
                    "true" => Some(Kw::True),
                    "false" => Some(Kw::False),
                    "import" => Some(Kw::Import),
                    "chan" => Some(Kw::Chan),
                    "break" => Some(Kw::Break),
                    "defer" => Some(Kw::Defer),
                    "continue" => Some(Kw::Continue),
                    "interface" => Some(Kw::Interface),
                    "spawnBlocking" => Some(Kw::SpawnB),
                    "select" => Some(Kw::Select),
                    "enum" => Some(Kw::Enum),
                    "match" => Some(Kw::Match),
                    _ => None,
                };
                // `_` alone is the blank punct, not an identifier
                if w == "_" {
                    out.push(Token { tok: Tok::P(P::Underscore), span: Span::new(at(lo), at(i)) });
                } else {
                    let tok = kw.map(Tok::Kw).unwrap_or_else(|| Tok::Ident(w.to_string()));
                    out.push(Token { tok, span: Span::new(at(lo), at(i)) });
                }
            }
            _ => {
                let two = |a: u8, c: u8, i: usize| b.get(i + 1) == Some(&c) && a == b[i];
                let (p, n) = if c == b':' && two(b':', b'=', i) {
                    (P::ColonEq, 2)
                } else if c == b'=' && two(b'=', b'=', i) {
                    (P::Eq, 2)
                } else if c == b'!' && two(b'!', b'=', i) {
                    (P::Ne, 2)
                } else if c == b'<' && two(b'<', b'=', i) {
                    (P::Le, 2)
                } else if c == b'>' && two(b'>', b'=', i) {
                    (P::Ge, 2)
                } else if c == b'&' && two(b'&', b'&', i) {
                    (P::And, 2)
                } else if c == b'&' {
                    (P::Amp, 1)
                } else if c == b'^' {
                    (P::Caret, 1)
                } else if c == b'|' && two(b'|', b'|', i) {
                    (P::Or, 2)
                } else if c == b'|' {
                    (P::Pipe, 1)
                } else if c == b'-' && two(b'-', b'>', i) {
                    (P::Arrow, 2)
                } else if c == b'=' && two(b'=', b'>', i) {
                    (P::FatArrow, 2)
                } else {
                    let p = match c {
                        b'(' => P::LParen,
                        b')' => P::RParen,
                        b'{' => P::LBrace,
                        b'}' => P::RBrace,
                        b'[' => P::LBracket,
                        b']' => P::RBracket,
                        b',' => P::Comma,
                        b':' => P::Colon,
                        b';' => P::Semi,
                        b'.' => P::Dot,
                        b'=' => P::Assign,
                        b'+' => P::Plus,
                        b'-' => P::Minus,
                        b'*' => P::Star,
                        b'/' => P::Slash,
                        b'%' => P::Percent,
                        b'<' => P::Lt,
                        b'>' => P::Gt,
                        b'!' => P::Bang,
                        _ => {
                            diags.push(Diag::err(Span::new(at(i), at(i + 1)), "lex", format!("unexpected byte {c:#04x}")));
                            i += 1;
                            continue;
                        }
                    };
                    (p, 1)
                };
                out.push(Token { tok: Tok::P(p), span: Span::new(at(i), at(i + n)) });
                i += n;
            }
        }
    }
    out.push(Token { tok: Tok::Eof, span: Span::new(at(b.len()), at(b.len())) });
    out
}
