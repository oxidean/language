use crate::ast::*;
use crate::diag::{Diag, Diags, Span};
use crate::lexer::{Kw, P, Tok, Token};

pub struct Parser<'a> {
    toks: &'a [Token],
    i: usize,
    diags: &'a mut Diags,
    /// disallow struct literals in expression position (if/for conditions)
    no_lit: bool,
}

pub fn parse(toks: &[Token], diags: &mut Diags) -> Crate {
    let mut p = Parser { toks, i: 0, diags, no_lit: false };
    let mut items = Vec::new();
    while !p.at_eof() {
        let before = p.i;
        match p.item() {
            Some(it) => items.push(it),
            None => {
                p.skip_to_item_boundary();
                if p.i == before {
                    p.bump(); // guaranteed progress — never spin on a bad token
                }
            }
        }
    }
    Crate { items }
}

impl<'a> Parser<'a> {
    // ---- token plumbing ----
    fn cur(&self) -> &Token {
        &self.toks[self.i.min(self.toks.len() - 1)]
    }
    fn span(&self) -> Span {
        self.cur().span
    }
    fn at_eof(&self) -> bool {
        self.cur().tok == Tok::Eof
    }
    fn bump(&mut self) -> Token {
        let t = self.cur().clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }
    fn eat_p(&mut self, p: P) -> bool {
        if self.cur().tok == Tok::P(p) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, k: Kw) -> bool {
        if self.cur().tok == Tok::Kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_p(&mut self, p: P, what: &str) -> bool {
        if self.eat_p(p) {
            true
        } else {
            let sp = self.span();
            self.diags.push(Diag::err(sp, "parse", format!("expected {what}, found {}", self.show())));
            false
        }
    }
    fn ident(&mut self) -> Option<(String, Span)> {
        if let Tok::Ident(s) = self.cur().tok.clone() {
            let sp = self.bump().span;
            Some((s, sp))
        } else {
            let sp = self.span();
            self.diags.push(Diag::err(sp, "parse", format!("expected identifier, found {}", self.show())));
            None
        }
    }
    fn show(&self) -> String {
        match &self.cur().tok {
            Tok::Ident(s) => format!("`{s}`"),
            Tok::Int(v) => format!("`{v}`"),
            Tok::Str(s) => format!("`\"{s}\"`"),
            Tok::Kw(k) => format!("`{k:?}`"),
            Tok::P(p) => format!("`{p:?}`"),
            Tok::Eof => "end of file".into(),
        }
    }
    fn skip_to_item_boundary(&mut self) {
        let mut depth = 0i32;
        while !self.at_eof() {
            match self.cur().tok {
                Tok::P(P::LBrace) => depth += 1,
                Tok::P(P::RBrace) => {
                    if depth == 0 {
                        self.bump();
                        return;
                    }
                    depth -= 1;
                }
                _ if depth == 0 => return,
                _ => {}
            }
            self.bump();
        }
    }

    // ---- items ----
    fn item(&mut self) -> Option<Item> {
        if self.eat_kw(Kw::Struct) {
            return self.parse_struct().map(Item::Struct);
        }
        if self.eat_kw(Kw::Interface) {
            // interface Name { eq, ordered, add }
            let (name, sp) = self.ident()?;
            self.expect_p(P::LBrace, "`{`");
            let mut ops = Vec::new();
            while !self.eat_p(P::RBrace) && !self.at_eof() {
                if let Tok::Ident(op) = self.cur().tok.clone() {
                    self.bump();
                    ops.push(op);
                } else {
                    self.bump();
                }
                self.eat_p(P::Comma);
            }
            return Some(Item::Interface(Interface { name, ops, span: sp }));
        }
        if self.eat_kw(Kw::Let) {
            let (name, sp) = self.ident()?;
            let ty = if self.eat_p(P::Colon) { Some(self.ty()?) } else { None };
            self.expect_p(P::Assign, "`=`");
            let init = self.expr(0, false)?;
            return Some(Item::Global(Global { name, ty, init, span: sp }));
        }
        if self.eat_kw(Kw::Extern) {
            if !self.eat_kw(Kw::Fn) {
                let sp = self.span();
                self.diags.push(Diag::err(sp, "parse", "expected `fn` after `extern`"));
                return None;
            }
            let mut f = self.parse_fn_tail()?;
            f.body = None;
            self.eat_p(P::Semi);
            return Some(Item::Extern(f));
        }
        if self.eat_kw(Kw::Fn) {
            return self.parse_fn_tail().map(|mut f| {
                f.body = Some(self.block().unwrap_or(Block { stmts: vec![], span: f.span }));
                Item::Fn(f)
            });
        }
        if self.eat_kw(Kw::Import) {
            let sp = self.span();
            if let Tok::Str(s) = self.cur().tok.clone() {
                self.bump();
                // optional `as alias` and/or selective `.{a, b}`
                let mut alias = None;
                let mut selective = Vec::new();
                if let Tok::Ident(k) = &self.cur().tok {
                    if k == "as" {
                        self.bump();
                        if let Some((a, _)) = self.ident() {
                            alias = Some(a);
                        }
                    }
                }
                if self.eat_p(P::Dot) {
                    if self.expect_p(P::LBrace, "`{`") {
                        while !self.eat_p(P::RBrace) && !self.at_eof() {
                            if let Some((n, _)) = self.ident() {
                                selective.push(n);
                            }
                            if !self.eat_p(P::Comma) {
                                self.expect_p(P::RBrace, "`}`");
                                break;
                            }
                        }
                    }
                }
                self.eat_p(P::Semi);
                return Some(Item::Import(Import { path: s, alias, selective, span: sp }));
            }
            self.diags.push(Diag::err(sp, "parse", "expected string after `import`"));
            return None;
        }
        let sp = self.span();
        self.diags.push(Diag::err(sp, "parse", format!("expected item, found {}", self.show())));
        None
    }

    fn parse_struct(&mut self) -> Option<Struct> {
        let (name, sp) = self.ident()?;
        self.expect_p(P::LBrace, "`{`");
        let mut fields = Vec::new();
        while !self.eat_p(P::RBrace) && !self.at_eof() {
            let (fname, fsp) = self.ident()?;
            self.expect_p(P::Colon, "`:`");
            let ty = self.ty()?;
            fields.push(Param { name: fname, ty, span: fsp });
            self.eat_p(P::Comma);
            self.eat_p(P::Semi);
        }
        Some(Struct { name, fields, span: sp })
    }

    fn parse_fn_tail(&mut self) -> Option<Fn> {
        let (name, sp) = self.ident()?;
        let mut ty_params = Vec::new();
        // generics: fn f[T any, U ordered](...)
        if self.eat_p(P::LBracket) {
            while !self.eat_p(P::RBracket) && !self.at_eof() {
                let (t, _) = self.ident()?;
                let c = if let Tok::Ident(c) = self.cur().tok.clone() {
                    self.bump();
                    c
                } else {
                    "any".to_string()
                };
                ty_params.push((t, c));
                self.eat_p(P::Comma);
            }
        }
        self.expect_p(P::LParen, "`(`");
        let mut params = Vec::new();
        while !self.eat_p(P::RParen) && !self.at_eof() {
            let (pname, psp) = self.ident()?;
            self.expect_p(P::Colon, "`:`");
            let ty = self.ty()?;
            params.push(Param { name: pname, ty, span: psp });
            if !self.eat_p(P::Comma) {
                self.expect_p(P::RParen, "`)`");
                break;
            }
        }
        let ret = if self.eat_p(P::Arrow) {
            if self.eat_p(P::LBracket) {
                let t = self.ty()?;
                self.expect_p(P::Comma, "`,`");
                // err member is fixed
                if let Tok::Ident(s) = self.cur().tok.clone() {
                    if s == "err" {
                        self.bump();
                    } else {
                        let sp = self.span();
                        self.diags.push(Diag::err(sp, "parse", "second return member must be `err`"));
                    }
                }
                self.expect_p(P::RBracket, "`]`");
                RetTy::Pair(t)
            } else {
                RetTy::One(self.ty()?)
            }
        } else {
            RetTy::Void
        };
        Some(Fn { name, ty_params, params, ret, body: None, span: sp })
    }

    fn ty(&mut self) -> Option<TyRef> {
        let sp = self.span();
        // bare `own`/`ref`/`mut` = pointer to unknown (Shared cells, FFI handles)
        let void = || TyRef { kind: TyKind::Named("void".into()), span: sp };
        let starts_ty = |p: &Self| matches!(p.cur().tok, Tok::Ident(_)
            | Tok::Kw(Kw::Own) | Tok::Kw(Kw::Ref) | Tok::Kw(Kw::Mut) | Tok::Kw(Kw::Chan));
        if self.eat_kw(Kw::Own) {
            let inner = if starts_ty(self) { self.ty()? } else { void() };
            return Some(TyRef { kind: TyKind::Own(Box::new(inner)), span: sp });
        }
        if self.eat_kw(Kw::Ref) {
            let inner = if starts_ty(self) { self.ty()? } else { void() };
            return Some(TyRef { kind: TyKind::Ref(Box::new(inner)), span: sp });
        }
        if self.eat_kw(Kw::Mut) {
            let inner = if starts_ty(self) { self.ty()? } else { void() };
            return Some(TyRef { kind: TyKind::Mut(Box::new(inner)), span: sp });
        }
        if self.eat_kw(Kw::Chan) {
            return Some(TyRef { kind: TyKind::Chan, span: sp });
        }
        let (mut name, _) = self.ident()?;
        while self.cur().tok == Tok::P(P::Dot) {
            if let Some(Tok::Ident(n)) = self.toks.get(self.i + 1).map(|t| t.tok.clone()) {
                self.bump();
                self.bump();
                name.push('.');
                name.push_str(&n);
            } else {
                break;
            }
        }
        Some(TyRef { kind: TyKind::Named(name), span: sp })
    }

    // ---- statements ----
    fn block(&mut self) -> Option<Block> {
        let lo = self.span();
        if !self.expect_p(P::LBrace, "`{`") {
            return None;
        }
        let mut stmts = Vec::new();
        while !self.eat_p(P::RBrace) && !self.at_eof() {
            if let Some(s) = self.stmt() {
                stmts.push(s);
            }
            self.eat_p(P::Semi);
        }
        Some(Block { stmts, span: Span::new(lo.lo, self.span().hi) })
    }

    fn stmt(&mut self) -> Option<Stmt> {
        let sp = self.span();
        match self.cur().tok.clone() {
            Tok::Kw(Kw::Return) => {
                self.bump();
                let mut vals = Vec::new();
                // `return` / `return v` / `return v, err`
                if !matches!(self.cur().tok, Tok::P(P::RBrace | P::Semi) | Tok::Eof) {
                    vals.push(self.expr(0, false)?);
                    while self.eat_p(P::Comma) {
                        vals.push(self.expr(0, false)?);
                    }
                }
                Some(Stmt::Return { vals, span: sp })
            }
            Tok::Kw(Kw::Let) => {
                self.bump();
                let (name, _) = self.ident()?;
                let ty = if self.eat_p(P::Colon) { Some(self.ty()?) } else { None };
                self.expect_p(P::Assign, "`=`");
                let init = self.expr(0, false)?;
                Some(Stmt::Let { name, ty, init, span: sp })
            }
            Tok::Kw(Kw::If) => {
                self.bump();
                let cond = self.cond_expr()?;
                let then = self.block()?;
                let els = if self.eat_kw(Kw::Else) {
                    if self.cur().tok == Tok::Kw(Kw::If) {
                        // else-if chains as nested blocks
                        let inner = self.stmt()?;
                        Some(Block { stmts: vec![inner], span: then.span })
                    } else {
                        self.block()
                    }
                } else {
                    None
                };
                Some(Stmt::If { cond, then, els, span: sp })
            }
            Tok::Kw(Kw::For) => {
                self.bump();
                let cond = self.cond_expr()?;
                let body = self.block()?;
                Some(Stmt::For { cond, body, span: sp })
            }
            Tok::Kw(Kw::Scope) => {
                self.bump();
                let body = self.block()?;
                Some(Stmt::Scope { body, span: sp })
            }
            Tok::Kw(Kw::Spawn) | Tok::Kw(Kw::Go) => {
                self.bump();
                let call = self.expr(0, true)?;
                Some(Stmt::Spawn { call, pinned: false, blocking: false, span: sp })
            }
            Tok::Kw(Kw::SpawnB) => {
                self.bump();
                let call = self.expr(0, true)?;
                Some(Stmt::Spawn { call, pinned: false, blocking: true, span: sp })
            }
            Tok::Kw(Kw::Unsafe) => {
                self.bump();
                let body = self.block()?;
                Some(Stmt::Unsafe { body, span: sp })
            }
            Tok::Kw(Kw::Break) => {
                self.bump();
                Some(Stmt::Break { span: sp })
            }
            Tok::Kw(Kw::Continue) => {
                self.bump();
                Some(Stmt::Continue { span: sp })
            }
            Tok::P(P::LBracket) => {
                // [a, err] = f()
                self.bump();
                let a = self.bind()?;
                self.expect_p(P::Comma, "`,`");
                let b = self.bind()?;
                self.expect_p(P::RBracket, "`]`");
                self.expect_p(P::Assign, "`=`");
                let call = self.expr(0, false)?;
                Some(Stmt::Let2 { a, b, call, span: sp })
            }
            Tok::P(P::Underscore) => {
                self.diags.push(Diag::err(sp, "parse", "`_` is only allowed for the value in `[v, err]` bindings"));
                self.bump();
                None
            }
            _ => {
                let e = self.expr(0, false)?;
                if self.eat_p(P::ColonEq) {
                    // x := e2 — reparse: e must be Ident
                    if let ExprKind::Ident(name) = e.kind {
                        let init = self.expr(0, false)?;
                        return Some(Stmt::Let { name, ty: None, init, span: sp });
                    }
                    self.diags.push(Diag::err(e.span, "parse", "left of `:=` must be a name"));
                    return None;
                }
                if self.eat_p(P::Assign) {
                    let val = self.expr(0, false)?;
                    return Some(Stmt::Assign { target: e, val, span: sp });
                }
                Some(Stmt::Expr(e))
            }
        }
    }

    fn bind(&mut self) -> Option<Bind> {
        match self.cur().tok.clone() {
            Tok::P(P::Underscore) => {
                self.bump();
                Some(Bind::Blank)
            }
            Tok::Ident(s) => {
                self.bump();
                Some(Bind::Name(s))
            }
            _ => {
                let sp = self.span();
                self.diags.push(Diag::err(sp, "parse", "expected name or `_`"));
                None
            }
        }
    }

    fn cond_expr(&mut self) -> Option<Expr> {
        let old = self.no_lit;
        self.no_lit = true;
        let e = self.expr(0, false);
        self.no_lit = old;
        e
    }

    // ---- expressions (pratt) ----
    fn expr(&mut self, min_bp: u8, call_only: bool) -> Option<Expr> {
        let mut lhs = self.unary()?;
        if call_only {
            // spawn f(args) — require the head to be a call
            return Some(lhs);
        }
        loop {
            let (op, lbp, rbp) = match self.cur().tok {
                Tok::P(P::Or) => (BinOp::Or, 1, 2),
                Tok::P(P::And) => (BinOp::And, 3, 4),
                Tok::P(P::Eq) => (BinOp::Eq, 5, 6),
                Tok::P(P::Ne) => (BinOp::Ne, 5, 6),
                Tok::P(P::Lt) => (BinOp::Lt, 7, 8),
                Tok::P(P::Le) => (BinOp::Le, 7, 8),
                Tok::P(P::Gt) => (BinOp::Gt, 7, 8),
                Tok::P(P::Ge) => (BinOp::Ge, 7, 8),
                Tok::P(P::Plus) => (BinOp::Add, 9, 10),
                Tok::P(P::Minus) => (BinOp::Sub, 9, 10),
                Tok::P(P::Star) => (BinOp::Mul, 11, 12),
                Tok::P(P::Slash) => (BinOp::Div, 11, 12),
                Tok::P(P::Percent) => (BinOp::Mod, 11, 12),
                _ => break,
            };
            if lbp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.expr(rbp, false)?;
            let span = Span::join(lhs.span, rhs.span);
            lhs = Expr { kind: ExprKind::Bin { op, l: Box::new(lhs), r: Box::new(rhs) }, span };
        }
        Some(lhs)
    }

    fn unary(&mut self) -> Option<Expr> {
        let sp = self.span();
        match self.cur().tok {
            Tok::P(P::Minus) => {
                self.bump();
                let e = self.unary()?;
                Some(Expr { kind: ExprKind::Un { op: UnOp::Neg, e: Box::new(e) }, span: sp })
            }
            Tok::P(P::Bang) => {
                self.bump();
                let e = self.unary()?;
                Some(Expr { kind: ExprKind::Un { op: UnOp::Not, e: Box::new(e) }, span: sp })
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Option<Expr> {
        let mut e = self.primary()?;
        loop {
            match self.cur().tok {
                Tok::P(P::LParen) => {
                    self.bump();
                    let mut args = Vec::new();
                    while !self.eat_p(P::RParen) && !self.at_eof() {
                        args.push(self.expr(0, false)?);
                        if !self.eat_p(P::Comma) {
                            self.expect_p(P::RParen, "`)`");
                            break;
                        }
                    }
                    let span = Span::join(e.span, self.span());
                    e = Expr { kind: ExprKind::Call { f: Box::new(e), args }, span };
                }
                Tok::P(P::Dot) => {
                    self.bump();
                    let (name, _) = self.ident()?;
                    let span = Span::join(e.span, self.span());
                    e = Expr { kind: ExprKind::Field { obj: Box::new(e), name }, span };
                }
                _ => break,
            }
        }
        Some(e)
    }

    fn primary(&mut self) -> Option<Expr> {
        let t = self.cur().clone();
        let sp = t.span;
        match t.tok {
            Tok::Int(v) => {
                self.bump();
                Some(Expr { kind: ExprKind::Int(v), span: sp })
            }
            Tok::Str(s) => {
                self.bump();
                Some(Expr { kind: ExprKind::Str(s), span: sp })
            }
            Tok::Kw(Kw::True) => {
                self.bump();
                Some(Expr { kind: ExprKind::Bool(true), span: sp })
            }
            Tok::Kw(Kw::False) => {
                self.bump();
                Some(Expr { kind: ExprKind::Bool(false), span: sp })
            }
            Tok::Kw(Kw::Nil) => {
                self.bump();
                Some(Expr { kind: ExprKind::Nil, span: sp })
            }
            Tok::Ident(mut name) => {
                self.bump();
                // qualified struct literal: a.b.T{ ... } — only when the
                // dotted path is immediately followed by `{`
                if !self.no_lit {
                    let mut j = self.i;
                    while self.toks.get(j).map(|t| &t.tok) == Some(&Tok::P(P::Dot))
                        && matches!(self.toks.get(j + 1).map(|t| &t.tok), Some(Tok::Ident(_)))
                    {
                        j += 2;
                    }
                    if self.toks.get(j).map(|t| &t.tok) == Some(&Tok::P(P::LBrace)) && j > self.i {
                        while self.i < j {
                            name.push('.');
                            self.bump();
                            if let Tok::Ident(n) = self.bump().tok {
                                name.push_str(&n);
                            }
                        }
                    }
                }
                // struct literal: Name{ ... } (uppercase heuristic not required;
                // disambiguation is the caller's no_lit flag)
                if !self.no_lit && self.cur().tok == Tok::P(P::LBrace) {
                    self.bump();
                    let mut fields = Vec::new();
                    while !self.eat_p(P::RBrace) && !self.at_eof() {
                        let (fname, fsp) = self.ident()?;
                        self.expect_p(P::Colon, "`:`");
                        let fe = self.expr(0, false)?;
                        fields.push((fname.clone(), fe));
                        let _ = fsp;
                        if !self.eat_p(P::Comma) {
                            break;
                        }
                    }
                    self.expect_p(P::RBrace, "`}`");
                    return Some(Expr { kind: ExprKind::StructLit { name, fields }, span: sp });
                }
                Some(Expr { kind: ExprKind::Ident(name), span: sp })
            }
            Tok::P(P::LParen) => {
                self.bump();
                let e = self.expr(0, false)?;
                self.expect_p(P::RParen, "`)`");
                Some(e)
            }
            _ => {
                self.diags.push(Diag::err(sp, "parse", format!("expected expression, found {}", self.show_at(t))));
                self.bump();
                None
            }
        }
    }

    fn show_at(&self, t: Token) -> String {
        match t.tok {
            Tok::Ident(s) => format!("`{s}`"),
            Tok::Int(v) => format!("`{v}`"),
            Tok::Str(s) => format!("`\"{s}\"`"),
            Tok::Kw(k) => format!("`{k:?}`"),
            Tok::P(p) => format!("`{p:?}`"),
            Tok::Eof => "end of file".into(),
        }
    }
}
