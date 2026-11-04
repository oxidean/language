//! `oxl fmt` — canonical pretty-printer over the AST (E2). Comments are
//! collected as byte-range trivia and re-attached: on their own line
//! before the following element, or trailing when they shared the
//! previous construct's line in the source.
use crate::ast::*;
use crate::diag::{Diags, Span};
use crate::{lexer, parser};

/// a `//` or `/* */` comment's byte range in the source
#[derive(Clone, Copy)]
struct Comment {
    lo: u32,
    hi: u32,
}

/// scan comments outside string literals — lex() drops them, fmt keeps
/// their byte ranges and slices the original text back out (byte-exact)
fn collect_comments(src: &str) -> Vec<Comment> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut in_str = false;
    let mut esc = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_str = true;
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let lo = i;
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                out.push(Comment { lo: lo as u32, hi: i as u32 });
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let lo = i;
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(b.len());
                out.push(Comment { lo: lo as u32, hi: i.min(b.len()) as u32 });
            }
            _ => i += 1,
        }
    }
    out
}

/// format one source file; Err(()) after reporting when it doesn't parse
pub fn format_file(path: &str, src: &str) -> Result<String, ()> {
    let mut diags = Diags::new(path, src);
    let toks = lexer::lex(src, &mut diags);
    let krate = parser::parse(&toks, &mut diags);
    if diags.has_errors() {
        diags.report();
        return Err(());
    }
    let mut f = Fmt {
        src,
        toks: &toks,
        comments: collect_comments(src),
        ci: 0,
        line_starts: std::iter::once(0u32)
            .chain(src.bytes().enumerate().filter(|(_, c)| *c == b'\n').map(|(i, _)| i as u32 + 1))
            .collect(),
        out: String::new(),
        first_line: true,
    };
    f.krate(&krate);
    Ok(f.out)
}

struct Fmt<'a> {
    src: &'a str,
    toks: &'a [lexer::Token],
    comments: Vec<Comment>,
    ci: usize,
    line_starts: Vec<u32>,
    out: String,
    /// suppresses the leading newline before the first emitted line
    first_line: bool,
}

const IND: &str = "    ";
/// decls wider than this break one-member-per-line
const WIDE: usize = 80;

impl<'a> Fmt<'a> {
    fn line(&self, off: u32) -> usize {
        match self.line_starts.binary_search(&off) {
            Ok(l) => l,
            Err(l) => l - 1,
        }
    }

    fn text(&self, c: Comment) -> &'a str {
        &self.src[c.lo as usize..c.hi as usize]
    }

    fn nl(&mut self, ind: usize) {
        if self.first_line {
            self.first_line = false;
        } else {
            self.out.push('\n');
        }
        for _ in 0..ind {
            self.out.push_str(IND);
        }
    }

    fn blank(&mut self) {
        if !self.out.ends_with("\n\n") && !self.first_line {
            self.out.push('\n');
        }
    }

    /// end-exclusive offset of the last content char before `off` —
    /// AST span.hi values overshoot into the next token (the parser
    /// joins with the lookahead's span), so "what did the previous
    /// construct end on" is recovered by scanning back over whitespace
    fn prev_end(&self, off: u32) -> u32 {
        let b = self.src.as_bytes();
        let mut i = (off as usize).min(b.len());
        while i > 0 && (b[i - 1] as char).is_ascii_whitespace() {
            i -= 1;
        }
        i as u32
    }

    /// did the source leave a blank line between the previous content
    /// and offset `lo`? preserved (capped at one) between stmts
    fn gap(&mut self, lo: u32) {
        if self.line(lo) > self.line(self.prev_end(lo).saturating_sub(1)) + 1 {
            self.blank();
        }
    }

    /// flush comments ending before `lo` into `out`
    fn drain(&mut self, lo: u32, ind: usize) {
        let mut buf = std::mem::take(&mut self.out);
        self.drain_into(&mut buf, lo, ind, self.first_line);
        self.out = buf;
        if !self.out.is_empty() {
            self.first_line = false;
        }
    }

    /// same-line comments trail the previous construct, others take
    /// their own line at `ind` — writes into `buf` so expr-level
    /// constructs (match arms, closures) can attach comments inside
    /// the strings they build
    fn drain_into(&mut self, buf: &mut String, lo: u32, ind: usize, first: bool) {
        while let Some(&c) = self.comments.get(self.ci) {
            if c.hi > lo {
                break;
            }
            // trails when it shared the previous construct's line —
            // measured against the last content char before c.lo
            if !first
                && self.prev_end(c.lo) > 0
                && self.line(c.lo) == self.line(self.prev_end(c.lo) - 1)
            {
                buf.push_str(" ");
                buf.push_str(self.text(c));
            } else {
                // blank-line preservation, one max
                if !buf.is_empty()
                    && !buf.ends_with('\n')
                    && self.line(c.lo) > self.line(self.prev_end(c.lo).saturating_sub(1)) + 1
                {
                    buf.push('\n');
                }
                if !buf.is_empty() {
                    buf.push('\n');
                }
                for _ in 0..ind {
                    buf.push_str(IND);
                }
                buf.push_str(self.text(c));
            }
            self.ci += 1;
        }
    }

    /// flush only comments that trail the previous construct —
    /// leading comments stay queued for `drain` so item spacing can
    /// go between a decl's trailing comment and the next decl's own
    fn drain_trailing(&mut self, lo: u32) {
        while let Some(&c) = self.comments.get(self.ci) {
            if c.hi > lo {
                break;
            }
            let trails = !self.first_line
                && self.prev_end(c.lo) > 0
                && self.line(c.lo) == self.line(self.prev_end(c.lo) - 1);
            if !trails {
                break;
            }
            self.out.push_str(" ");
            self.out.push_str(self.text(c));
            self.ci += 1;
        }
    }

    /// hi of the `}` matching the first `{` at-or-after `lo` —
    /// depth-matched over the token stream so member lists inside
    /// braces (enum named payloads) don't fool it; strings/comments
    /// aren't tokens and can't fake a brace
    fn close_brace(&self, lo: u32) -> u32 {
        let mut depth = 0i32;
        let mut started = false;
        for t in self.toks {
            if t.span.lo < lo {
                continue;
            }
            match &self.src[t.span.lo as usize..t.span.hi as usize] {
                "{" => {
                    depth += 1;
                    started = true;
                }
                "}" => {
                    depth -= 1;
                    if started && depth == 0 {
                        return t.span.hi;
                    }
                }
                _ => {}
            }
        }
        lo
    }

    /// end of the token right before the one `span_hi` overshot to —
    /// parser joins stretch span.hi into the next token, so the real
    /// last token of a construct is `toks[i - 1]`
    fn prev_tok_end(&self, span_hi: u32) -> u32 {
        self.toks
            .iter()
            .rposition(|t| t.span.hi <= span_hi)
            .and_then(|i| i.checked_sub(1))
            .map(|i| self.toks[i].span.hi)
            .unwrap_or(span_hi)
    }

    fn krate(&mut self, k: &Crate) {
        for (i, it) in k.items.iter().enumerate() {
            self.drain_trailing(item_span(it).lo);
            if i > 0 {
                self.blank();
            }
            self.drain(item_span(it).lo, 0);
            self.nl(0);
            self.item(it);
        }
        // file tail: comments after the last item
        self.drain(u32::MAX, 0);
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn item(&mut self, it: &Item) {
        match it {
            Item::Import(i) => {
                let mut s = format!("import \"{}\"", i.path);
                if let Some(a) = &i.alias {
                    s += &format!(" as {a}");
                }
                if !i.selective.is_empty() {
                    s += &format!(".{{{}}}", i.selective.join(", "));
                }
                self.out.push_str(&s);
            }
            Item::Global(g) => {
                self.out.push_str(&format!("let {}", g.name));
                if let Some(t) = &g.ty {
                    self.out.push_str(&format!(": {}", self.ty(t)));
                }
                let init = self.expr(&g.init, 0, 0);
                self.out.push_str(&format!(" = {init}"));
            }
            Item::Struct(s) => {
                let members: Vec<(u32, String)> = s
                    .fields
                    .iter()
                    .map(|p| (p.span.lo, format!("{}: {}", p.name, self.ty(&p.ty))))
                    .collect();
                self.member_list(s.span.lo, &format!("struct {}", s.name), &members, false);
            }
            Item::Enum(e) => {
                let members: Vec<(u32, String)> = e
                    .variants
                    .iter()
                    .map(|v| (v.span.lo, match &v.payload {
                        VariantPayload::Unit => v.name.clone(),
                        VariantPayload::Pos(ts) => format!(
                            "{}({})",
                            v.name,
                            ts.iter().map(|t| self.ty(t)).collect::<Vec<_>>().join(", ")
                        ),
                        VariantPayload::Named(fs) => format!(
                            "{} {{ {} }}",
                            v.name,
                            fs.iter()
                                .map(|(n, t)| format!("{n}: {}", self.ty(t)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    }))
                    .collect();
                self.member_list(e.span.lo, &format!("enum {}", e.name), &members, true);
            }
            Item::Interface(i) => {
                let mut members: Vec<(u32, String)> = Vec::new();
                // ops carry no span — order them first, bound drains
                // by the first method's lo (they're decl-only anyway)
                members.extend(i.ops.iter().map(|o| (i.span.lo, o.clone())));
                members.extend(i.methods.iter().map(|m| {
                    (
                        m.span.lo,
                        format!("{}({}){}", m.name, self.params(&m.params), self.ret(&m.ret)),
                    )
                }));
                self.member_list(i.span.lo, &format!("interface {}", i.name), &members, false);
            }
            Item::Extern(f) => {
                self.out.push_str("extern ");
                self.fn_sig(f);
            }
            Item::Fn(f) => {
                self.fn_sig(f);
                if let Some(b) = &f.body {
                    self.out.push_str(" {");
                    self.block(b, 0);
                }
            }
        }
    }

    /// `{ a, b }` inline when it fits, else one member per line
    /// `head { m, m }` inline when narrow and comment-free, else one
    /// member per line — `head_lo` anchors the close-brace search,
    /// member `lo`s bound the per-member comment drains
    fn member_list(&mut self, head_lo: u32, head: &str, members: &[(u32, String)], trail: bool) {
        let close = self.close_brace(head_lo);
        let commented = !members.is_empty()
            && self.comments.get(self.ci).map(|c| c.hi <= close).unwrap_or(false);
        let inline = format!(
            "{head} {{ {}{} }}",
            members.iter().map(|(_, m)| m.clone()).collect::<Vec<_>>().join(", "),
            if trail { "," } else { "" }
        );
        if members.is_empty() {
            self.out.push_str(&format!("{head} {{}}"));
            return;
        }
        if !commented && inline.len() <= WIDE {
            self.out.push_str(&inline);
            return;
        }
        self.out.push_str(&format!("{head} {{"));
        for (lo, m) in members {
            self.drain(*lo, 1);
            self.nl(1);
            self.out.push_str(m);
            self.out.push(',');
        }
        self.drain(close, 1);
        self.nl(0);
        self.out.push('}');
    }

    fn fn_sig(&mut self, f: &Fn) {
        self.out.push_str("fn ");
        if let Some(r) = &f.recv {
            self.out.push_str(&format!("({} {}) ", r.name, self.ty(&r.ty)));
        }
        // strip module qualification back to the declared name for
        // methods (name arrives qualified `T.m` only after load_mod —
        // fmt runs on the single-file AST, so `name` is bare already)
        self.out.push_str(&f.name);
        if !f.ty_params.is_empty() {
            let ps: Vec<String> = f
                .ty_params
                .iter()
                .map(|(t, c)| if c.is_empty() { t.clone() } else { format!("{t} {c}") })
                .collect();
            self.out.push_str(&format!("[{}]", ps.join(", ")));
        }
        self.out.push_str(&format!("({})", self.params(&f.params)));
        self.out.push_str(&self.ret(&f.ret));
    }

    fn params(&self, ps: &[Param]) -> String {
        ps.iter()
            .map(|p| format!("{}: {}", p.name, self.ty(&p.ty)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn ret(&self, r: &RetTy) -> String {
        match r {
            RetTy::Void => String::new(),
            RetTy::One(t) => format!(" -> {}", self.ty(t)),
            RetTy::Pair(t) => format!(" -> [{}, err]", self.ty(t)),
        }
    }

    fn ty(&self, t: &TyRef) -> String {
        match &t.kind {
            TyKind::Named(n) => n.clone(),
            TyKind::Own(i) => format!("own {}", self.ty(i)),
            TyKind::Ref(i) => format!("ref {}", self.ty(i)),
            TyKind::Mut(i) => format!("mut {}", self.ty(i)),
            TyKind::Chan => "chan".into(),
            TyKind::Slice(i) => format!("[]{}", self.ty(i)),
            TyKind::Fn { params, ret } => {
                let ps = params.iter().map(|p| self.ty(p)).collect::<Vec<_>>().join(", ");
                match ret {
                    Some(r) => format!("fn({ps}) -> {}", self.ty(r)),
                    None => format!("fn({ps})"),
                }
            }
        }
    }

    /// `{` was just emitted — stmts at ind+1 then `}` at ind
    fn block(&mut self, b: &Block, ind: usize) {
        if b.stmts.is_empty() {
            // comments inside `{ }` stay inside — the `}` is the
            // token before the overshot span.hi
            let close = self.prev_tok_end(b.span.hi);
            if self.comments.get(self.ci).map_or(false, |c| c.hi <= close) {
                self.drain(close, ind + 1);
                self.nl(ind);
            }
            self.out.push('}');
            return;
        }
        for s in &b.stmts {
            self.drain(stmt_span(s).lo, ind + 1);
            self.gap(stmt_span(s).lo);
            self.nl(ind + 1);
            self.stmt(s, ind + 1);
        }
        // block spans overshoot by one lookahead token; the `}` is
        // the token right before it
        self.drain(self.prev_tok_end(b.span.hi), ind + 1);
        self.nl(ind);
        self.out.push('}');
    }

    fn stmt(&mut self, s: &Stmt, ind: usize) {
        match s {
            Stmt::Let { name, ty, init, .. } => {
                let t = ty.as_ref().map(|t| self.ty(t));
                let init = self.expr(init, 0, ind);
                match t {
                    Some(t) => self.out.push_str(&format!("let {name}: {t} = {init}")),
                    None => self.out.push_str(&format!("{name} := {init}")),
                }
            }
            Stmt::Let2 { a, b, call, .. } => {
                let bn = |b: &Bind| match b {
                    Bind::Name(n) => n.clone(),
                    Bind::Blank => "_".into(),
                };
                let call = self.expr(call, 0, ind);
                self.out.push_str(&format!("[{}, {}] = {call}", bn(a), bn(b)));
            }
            Stmt::Assign { target, val, .. } => {
                let t = self.expr(target, 0, ind);
                let v = self.expr(val, 0, ind);
                self.out.push_str(&format!("{t} = {v}"));
            }
            Stmt::Expr(e) => {
                let e = self.expr(e, 0, ind);
                self.out.push_str(&e);
            }
            Stmt::Return { vals, .. } => {
                if vals.is_empty() {
                    self.out.push_str("return");
                } else {
                    let vs: Vec<String> = vals.iter().map(|v| self.expr(v, 0, ind)).collect();
                    self.out.push_str(&format!("return {}", vs.join(", ")));
                }
            }
            Stmt::If { cond, then, els, .. } => {
                let c = self.expr(cond, 0, ind);
                self.out.push_str(&format!("if {c} {{"));
                self.block(then, ind);
                match els {
                    Some(b) if b.stmts.len() == 1 && matches!(b.stmts[0], Stmt::If { .. }) => {
                        // else-if chain stays on one line
                        self.out.push_str(" else ");
                        self.stmt(&b.stmts[0], ind);
                    }
                    Some(b) => {
                        self.out.push_str(" else {");
                        self.block(b, ind);
                    }
                    None => {}
                }
            }
            Stmt::For { cond, body, .. } => {
                let c = self.expr(cond, 0, ind);
                self.out.push_str(&format!("for {c} {{"));
                self.block(body, ind);
            }
            Stmt::Scope { body, .. } => {
                self.out.push_str("scope {");
                self.block(body, ind);
            }
            Stmt::Unsafe { body, .. } => {
                self.out.push_str("unsafe {");
                self.block(body, ind);
            }
            Stmt::Spawn { call, blocking, .. } => {
                let c = self.expr(call, 0, ind);
                self.out.push_str(&format!(
                    "{} {c}",
                    if *blocking { "spawnBlocking" } else { "spawn" }
                ));
            }
            Stmt::Defer { call, .. } => {
                let c = self.expr(call, 0, ind);
                self.out.push_str(&format!("defer {c}"));
            }
            Stmt::Select { arms, .. } => {
                self.out.push_str("select {");
                for a in arms {
                    self.nl(ind + 1);
                    match &a.kind {
                        SelKind::Recv { chan, bind, .. } => {
                            if let Some(b) = bind {
                                self.out.push_str(&format!("{b} := "));
                            }
                            let c = self.expr(chan, 0, ind);
                            self.out.push_str(&format!("recv({c})"));
                        }
                        SelKind::Timeout { ms, .. } => {
                            let ms = self.expr(ms, 0, ind);
                            self.out.push_str(&format!("timeout({ms})"));
                        }
                        SelKind::Else { .. } => self.out.push_str("else"),
                    }
                    self.out.push_str(" => {");
                    self.block(&a.body, ind + 1);
                }
                self.nl(ind);
                self.out.push('}');
            }
            Stmt::Break { .. } => self.out.push_str("break"),
            Stmt::Continue { .. } => self.out.push_str("continue"),
        }
    }

    // ---- expressions: precedence-aware, parens re-inserted ----

    fn expr(&mut self, e: &Expr, parent: u8, ind: usize) -> String {
        match &e.kind {
            ExprKind::Int(v) => v.to_string(),
            ExprKind::Float(v) => {
                // Display never uses exponent form (the lexer has none);
                // keep a `.` so the literal stays a float
                let s = format!("{v}");
                if s.contains('.') { s } else { format!("{s}.0") }
            }
            ExprKind::Str(s) => format!("\"{}\"", str_escape(s)),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Nil => "nil".into(),
            ExprKind::Ident(n) => n.clone(),
            ExprKind::Call { f, args } => {
                let fs = self.expr(f, POSTFIX, ind);
                let fs = if atomic(f) { fs } else { format!("({fs})") };
                let mut s = format!("{fs}(");
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    self.drain_into(&mut s, a.span.lo, ind, false);
                    s.push_str(&self.expr(a, 0, ind));
                }
                self.drain_into(&mut s, self.prev_tok_end(e.span.hi), ind, false);
                s.push(')');
                s
            }
            ExprKind::Field { obj, name } => {
                let o = self.expr(obj, POSTFIX, ind);
                let o = if atomic(obj) { o } else { format!("({o})") };
                format!("{o}.{name}")
            }
            ExprKind::Index { obj, idx } => {
                let o = self.expr(obj, POSTFIX, ind);
                let o = if atomic(obj) { o } else { format!("({o})") };
                format!("{o}[{}]", self.expr(idx, 0, ind))
            }
            ExprKind::Slice { obj, lo, hi } => {
                let o = self.expr(obj, POSTFIX, ind);
                let o = if atomic(obj) { o } else { format!("({o})") };
                let l = lo.as_ref().map(|e| self.expr(e, 0, ind)).unwrap_or_default();
                let h = hi.as_ref().map(|e| self.expr(e, 0, ind)).unwrap_or_default();
                format!("{o}[{l}:{h}]")
            }
            ExprKind::StructLit { name, fields } => {
                let mut s = format!("{name}{{");
                for (i, (n, fe)) in fields.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    self.drain_into(&mut s, fe.span.lo, ind, false);
                    s.push_str(&format!("{n}: {}", self.expr(fe, 0, ind)));
                }
                self.drain_into(&mut s, self.close_brace(e.span.lo), ind, false);
                s.push('}');
                s
            }
            ExprKind::Bin { op, l, r } => {
                let p = prec(*op);
                let ls = self.expr(l, 0, ind);
                let rs = self.expr(r, 0, ind);
                // left-assoc: parens on a looser left operand
                let ls = if expr_prec(l) < p { format!("({ls})") } else { ls };
                // right operand: parens when looser, or equal-binding
                // unless both ops re-associate (`a - (b + c)` needs
                // them, `a + (b + c)` flattens safely)
                let rs = match &r.kind {
                    ExprKind::Bin { op: rop, .. } if prec(*rop) == p => {
                        if assoc(*op) && assoc(*rop) { rs } else { format!("({rs})") }
                    }
                    _ if expr_prec(r) < p => format!("({rs})"),
                    _ => rs,
                };
                format!("{ls} {} {rs}", op_str(*op))
            }
            ExprKind::Un { op, e } => {
                let inner = self.expr(e, UNARY, ind);
                let inner = if expr_prec(e) < UNARY { format!("({inner})") } else { inner };
                format!("{}{inner}", match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                })
            }
            ExprKind::As { e, ty } => {
                let inner = self.expr(e, POSTFIX, ind);
                let inner = if expr_prec(e) < POSTFIX { format!("({inner})") } else { inner };
                format!("{inner} as {}", self.ty(ty))
            }
            ExprKind::Match { e, arms } => {
                let mut s = format!("match {} {{", self.expr(e, 0, ind));
                for a in arms {
                    // comments before the arm attach inside the match
                    self.drain_into(&mut s, a.span.lo, ind + 1, false);
                    s.push_str(&format!(
                        "\n{}{} => {}",
                        IND.repeat(ind + 1),
                        self.pat(&a.pat),
                        match &a.body {
                            MatchBody::Expr(e) => self.expr(e, 0, ind),
                            MatchBody::Block(b) => {
                                // block arm: `=> {` stmts `}` at arm ind
                                let mut t = String::from("{");
                                for st in &b.stmts {
                                    self.drain_into(&mut t, stmt_span(st).lo, ind + 2, false);
                                    t.push_str(&format!("\n{}", IND.repeat(ind + 2)));
                                    let mut sub = Fmt { // stmt printing shares the emitter
                                        src: self.src,
                                        toks: self.toks,
                                        comments: self.comments[self.ci..].to_vec(),
                                        ci: 0,
                                        line_starts: self.line_starts.clone(),
                                        out: String::new(),
                                        first_line: true,
                                    };
                                    sub.stmt(st, ind + 2);
                                    self.ci += sub.ci;
                                    t.push_str(&sub.out);
                                }
                                self.drain_into(&mut t, self.prev_tok_end(b.span.hi), ind + 2, false);
                                t.push_str(&format!("\n{}}}", IND.repeat(ind + 1)));
                                t
                            }
                        }
                    ));
                }
                self.drain_into(&mut s, self.prev_tok_end(e.span.hi), ind + 1, false);
                s.push_str(&format!("\n{}}}", IND.repeat(ind)));
                let _ = parent;
                s
            }
            ExprKind::Closure { params, body } => {
                let ps: Vec<String> = params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, self.ty(&p.ty)))
                    .collect();
                let head = format!("{{ |{}|", ps.join(", "));
                if body.stmts.len() == 1 && inline_stmt(&body.stmts[0]) {
                    let mut sub = Fmt {
                        src: self.src,
                        toks: self.toks,
                        comments: self.comments[self.ci..].to_vec(),
                        ci: 0,
                        line_starts: self.line_starts.clone(),
                        out: String::new(),
                        first_line: true,
                    };
                    sub.stmt(&body.stmts[0], 0);
                    self.ci += sub.ci;
                    format!("{head} {} }}", sub.out)
                } else {
                    let mut s = head;
                    for st in &body.stmts {
                        self.drain_into(&mut s, stmt_span(st).lo, ind + 1, false);
                        s.push_str(&format!("\n{}", IND.repeat(ind + 1)));
                        let mut sub = Fmt {
                            src: self.src,
                            toks: self.toks,
                            comments: self.comments[self.ci..].to_vec(),
                            ci: 0,
                            line_starts: self.line_starts.clone(),
                            out: String::new(),
                            first_line: true,
                        };
                        sub.stmt(st, ind + 1);
                        self.ci += sub.ci;
                        s.push_str(&sub.out);
                    }
                    self.drain_into(&mut s, self.prev_tok_end(body.span.hi), ind + 1, false);
                    s.push_str(&format!("\n{}}}", IND.repeat(ind)));
                    s
                }
            }
            ExprKind::TyLit(t) => self.ty(t),
        }
    }

    fn pat(&mut self, p: &Pattern) -> String {
        match p {
            Pattern::Wild => "_".into(),
            Pattern::VarUnit { path } => path.clone(),
            Pattern::VarPos { path, binds } => format!("{path}({})", binds.join(", ")),
            Pattern::VarNamed { path, binds } => format!("{path}{{{}}}", binds.join(", ")),
            Pattern::Lit(e) => self.expr(e, 0, 0),
            Pattern::Bind(n) => n.clone(),
        }
    }
}

const UNARY: u8 = 10;
const POSTFIX: u8 = 11;

/// binding-power table mirroring the parser's Pratt levels
fn prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::BitOr => 3,
        BinOp::Xor => 4,
        BinOp::BitAnd => 5,
        BinOp::Eq | BinOp::Ne => 6,
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 7,
        BinOp::Add | BinOp::Sub => 8,
        BinOp::Mul | BinOp::Div | BinOp::Mod => 9,
    }
}

fn expr_prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Bin { op, .. } => prec(*op),
        ExprKind::Un { .. } => UNARY,
        ExprKind::Call { .. }
        | ExprKind::Field { .. }
        | ExprKind::Index { .. }
        | ExprKind::Slice { .. }
        | ExprKind::As { .. } => POSTFIX,
        _ => 12,
    }
}

/// is `e` safe bare as a postfix head (call/field/index/slice target)?
fn atomic(e: &Expr) -> bool {
    matches!(
        &e.kind,
        ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Nil
            | ExprKind::Ident(_)
            | ExprKind::Field { .. }
            | ExprKind::Call { .. }
            | ExprKind::Index { .. }
            | ExprKind::Slice { .. }
    )
}

/// ops where `a op (b op' c)` at equal precedence re-associates safely
fn assoc(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add | BinOp::Mul | BinOp::And | BinOp::Or | BinOp::BitAnd | BinOp::BitOr | BinOp::Xor
    )
}

fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::Xor => "^",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

/// stmts that can live inside a one-line `{ |args| stmt }` closure
fn inline_stmt(s: &Stmt) -> bool {
    matches!(
        s,
        Stmt::Return { .. }
            | Stmt::Expr(_)
            | Stmt::Assign { .. }
            | Stmt::Let { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. }
    )
}

fn str_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out
}

fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Let { span, .. }
        | Stmt::Let2 { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::Return { span, .. }
        | Stmt::If { span, .. }
        | Stmt::For { span, .. }
        | Stmt::Scope { span, .. }
        | Stmt::Spawn { span, .. }
        | Stmt::Select { span, .. }
        | Stmt::Defer { span, .. }
        | Stmt::Unsafe { span, .. }
        | Stmt::Break { span }
        | Stmt::Continue { span } => *span,
        Stmt::Expr(e) => e.span,
    }
}

fn item_span(it: &Item) -> Span {
    match it {
        Item::Fn(f) | Item::Extern(f) => f.span,
        Item::Struct(s) => s.span,
        Item::Enum(e) => e.span,
        Item::Interface(i) => i.span,
        Item::Import(i) => i.span,
        Item::Global(g) => g.span,
    }
}
