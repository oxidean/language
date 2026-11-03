use crate::ast::*;
use crate::diag::{Diag, Diags, Span};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Int,
    Bool,
    Str,
    CStr,
    Void,
    Err,
    Chan,
    Own(Box<Ty>),
    Ref(Box<Ty>),
    Mut(Box<Ty>),
    Struct(String),
    Unknown,
}

impl Ty {
    pub fn name(&self) -> String {
        match self {
            Ty::Int => "int".into(),
            Ty::Bool => "bool".into(),
            Ty::Str => "str".into(),
            Ty::CStr => "cstr".into(),
            Ty::Void => "void".into(),
            Ty::Err => "err".into(),
            Ty::Chan => "chan".into(),
            Ty::Own(t) => format!("own {}", t.name()),
            Ty::Ref(t) => format!("ref {}", t.name()),
            Ty::Mut(t) => format!("mut {}", t.name()),
            Ty::Struct(n) => n.clone(),
            Ty::Unknown => "?".into(),
        }
    }
    fn is_word(&self) -> bool {
        matches!(self, Ty::Int | Ty::Bool | Ty::Chan | Ty::Own(_))
    }
}

#[derive(Clone, Debug)]
pub enum Ret {
    Void,
    One(Ty),
    Pair(Ty),
}

#[derive(Clone)]
struct FnSig {
    params: Vec<Ty>,
    ret: Ret,
    is_extern: bool,
}

pub struct Checked {
    pub suspend: HashSet<String>,
}

struct Env {
    scopes: Vec<HashMap<String, Ty>>,
}
impl Env {
    fn new() -> Self {
        Env { scopes: vec![HashMap::new()] }
    }
    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop(&mut self) {
        self.scopes.pop();
    }
    fn get(&self, n: &str) -> Option<Ty> {
        self.scopes.iter().rev().find_map(|s| s.get(n).cloned())
    }
    fn set(&mut self, n: String, t: Ty) {
        self.scopes.last_mut().unwrap().insert(n, t);
    }
}

pub struct Checker<'a> {
    diags: &'a mut Diags,
    fns: HashMap<String, FnSig>,
    structs: HashMap<String, Vec<(String, Ty)>>,
    cur_ret: Ret,
    calls: HashMap<String, HashSet<String>>, // caller -> callee fn names
    cur_fn: String,
    in_let2: bool,
}

const BUILTINS: &[&str] = &[
    "print", "println", "mkchan", "send", "recv", "close", "sleep", "new",
    "assert", "itoa", "argc", "argv", "len", "byteat", "substr", "find", "atoi",
    "mkbuf", "bufstr", "poke8", "poke16", "poke32", "htons", "htonl",
    "cstr", "sqlite_open", "sqlite_exec", "sqlite_query", "sqlite_close",
];

pub fn check(c: &Crate, diags: &mut Diags) -> Checked {
    let mut ck = Checker {
        diags,
        fns: HashMap::new(),
        structs: HashMap::new(),
        cur_ret: Ret::Void,
        calls: HashMap::new(),
        cur_fn: String::new(),
        in_let2: false,
    };
    ck.register(c);
    ck.check_items(c);
    Checked { suspend: ck.suspend_set(c) }
}

impl<'a> Checker<'a> {
    fn register(&mut self, c: &Crate) {
        // structs first so extern/fn signatures can name them regardless
        // of source order
        for it in &c.items {
            if let Item::Struct(s) = it {
                    let mut fields = Vec::new();
                    for f in &s.fields {
                        let t = self.resolve(&f.ty);
                        if matches!(t, Ty::Ref(_) | Ty::Mut(_)) {
                            self.diags.push(Diag::err(
                                f.span, "own",
                                "borrowed refs may not be stored in heap objects (v0)",
                            ));
                        }
                        fields.push((f.name.clone(), t));
                    }
                    self.structs.insert(s.name.clone(), fields);
            }
        }
        for it in &c.items {
            match it {
                Item::Fn(f) | Item::Extern(f) => {
                    let params: Vec<Ty> = f.params.iter().map(|p| self.resolve(&p.ty)).collect();
                    let ret = self.resolve_ret(&f.ret);
                    if matches!(ret, Ret::One(Ty::Ref(_) | Ty::Mut(_))) {
                        self.diags.push(Diag::err(
                            f.span, "own",
                            "borrowed refs may not be returned (v0)",
                        ));
                    }
                    // extern decls may share a builtin's name — they only
                    // declare the C prototype the builtin emits
                    if BUILTINS.contains(&f.name.as_str()) && !matches!(it, Item::Extern(_)) {
                        self.diags.push(Diag::err(f.span, "check", format!("`{}` is a builtin and cannot be redefined", f.name)));
                    }
                    self.fns.insert(
                        f.name.clone(),
                        FnSig { params, ret, is_extern: matches!(it, Item::Extern(_)) },
                    );
                }
                Item::Import(_, sp) => {
                    self.diags.push(Diag::err(*sp, "check", "imports are not supported yet (v0)"));
                }
                Item::Struct(_) => {}
            }
        }
    }

    fn resolve_ret(&mut self, r: &RetTy) -> Ret {
        match r {
            RetTy::Void => Ret::Void,
            RetTy::One(t) => Ret::One(self.resolve(t)),
            RetTy::Pair(t) => Ret::Pair(self.resolve(t)),
        }
    }

    fn resolve(&mut self, t: &TyRef) -> Ty {
        match &t.kind {
            TyKind::Named(n) => match n.as_str() {
                "int" => Ty::Int,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "cstr" => Ty::CStr,
                "void" => Ty::Void,
                "err" => Ty::Err,
                s => {
                    if self.structs.contains_key(s) {
                        Ty::Struct(s.to_string())
                    } else {
                        self.diags.push(Diag::err(t.span, "check", format!("unknown type `{s}`")));
                        Ty::Unknown
                    }
                }
            },
            TyKind::Own(t) => Ty::Own(Box::new(self.resolve(t))),
            TyKind::Ref(t) => Ty::Ref(Box::new(self.resolve(t))),
            TyKind::Mut(t) => Ty::Mut(Box::new(self.resolve(t))),
            TyKind::Chan => Ty::Chan,
        }
    }

    fn check_items(&mut self, c: &Crate) {
        for it in &c.items {
            if let Item::Fn(f) = it {
                self.cur_fn = f.name.clone();
                let sig = self.fns[&f.name].clone();
                self.cur_ret = sig.ret.clone();
                let mut env = Env::new();
                for (p, t) in f.params.iter().zip(sig.params.iter()) {
                    env.set(p.name.clone(), t.clone());
                }
                if let Some(b) = &f.body {
                    self.block(b, &mut env);
                }
            }
        }
    }

    fn block(&mut self, b: &Block, env: &mut Env) {
        env.push();
        for s in &b.stmts {
            self.stmt(s, env);
        }
        env.pop();
    }

    fn stmt(&mut self, s: &Stmt, env: &mut Env) {
        match s {
            Stmt::Let { name, ty, init, span } => {
                let it = self.expr(init, env);
                let t = if let Some(tr) = ty {
                    let want = self.resolve(tr);
                    self.assignable(&want, &it, *span);
                    want
                } else {
                    it
                };
                env.set(name.clone(), t);
            }
            Stmt::Let2 { a, b, call, span } => {
                self.in_let2 = true;
                self.expr(call, env);
                self.in_let2 = false;
                // call must produce [T, err]
                let val_ty = match self.call_ret(call) {
                    Some(Ret::Pair(t)) => t,
                    _ => {
                        self.diags.push(Diag::err(*span, "check",
                            "`[v, err] = f()` requires a call to a `[T, err]` function"));
                        Ty::Unknown
                    }
                };
                if let Bind::Name(n) = a {
                    env.set(n.clone(), val_ty);
                }
                match b {
                    Bind::Name(n) => env.set(n.clone(), Ty::Err),
                    Bind::Blank => self.diags.push(Diag::err(
                        *span, "check",
                        "cannot discard `err` — handle it or pass it up",
                    )),
                }
            }
            Stmt::Assign { target, val, span } => {
                let tt = self.expr(target, env);
                let vt = self.expr(val, env);
                self.assignable(&tt, &vt, *span);
            }
            Stmt::Expr(e) => {
                if let Some(Ret::Pair(_)) = self.call_ret(e) {
                    self.diags.push(Diag::err(
                        e.span, "check",
                        "call returns `[T, err]` — bind it with `[v, err] =`",
                    ));
                }
                self.expr(e, env);
            }
            Stmt::Return { vals, span } => {
                let cur_ret = self.cur_ret.clone();
                match &cur_ret {
                    Ret::Void => {
                        if !vals.is_empty() {
                            self.diags.push(Diag::err(*span, "check", "function returns void"));
                        }
                    }
                    Ret::One(want) => {
                        if vals.len() != 1 {
                            self.diags.push(Diag::err(*span, "check", "expected one return value"));
                        } else {
                            let t = self.expr(&vals[0], env);
                            self.assignable(want, &t, vals[0].span);
                        }
                    }
                    Ret::Pair(want) => {
                        if vals.len() == 2 {
                            let t = self.expr(&vals[0], env);
                            self.assignable(want, &t, vals[0].span);
                            let e = self.expr(&vals[1], env);
                            self.assignable(&Ty::Err, &e, vals[1].span);
                        } else if vals.len() == 1 {
                            // `return f()` forwarding a [T, err] call
                            self.in_let2 = true;
                            self.expr(&vals[0], env);
                            self.in_let2 = false;
                            if let Some(Ret::Pair(t)) = self.call_ret(&vals[0]) {
                                self.assignable(want, &t, vals[0].span);
                            } else {
                                self.diags.push(Diag::err(*span, "check",
                                    "`[T, err]` function must `return v, err` or forward a `[T, err]` call"));
                            }
                        } else {
                            self.diags.push(Diag::err(*span, "check", "expected `return v, err`"));
                        }
                    }
                }
            }
            Stmt::If { cond, then, els, .. } => {
                let t = self.expr(cond, env);
                self.assignable(&Ty::Bool, &t, cond.span);
                self.block(then, env);
                if let Some(e) = els {
                    self.block(e, env);
                }
            }
            Stmt::For { cond, body, .. } => {
                let t = self.expr(cond, env);
                self.assignable(&Ty::Bool, &t, cond.span);
                self.block(body, env);
            }
            Stmt::Scope { body, .. } => self.block(body, env),
            Stmt::Spawn { call, .. } => {
                match &call.kind {
                    ExprKind::Call { f, .. } => {
                        // mark suspend-relevant edge
                        if let ExprKind::Ident(n) = &f.kind {
                            self.calls.entry(self.cur_fn.clone()).or_default().insert(n.clone());
                        }
                        self.expr(call, env);
                    }
                    _ => self.diags.push(Diag::err(call.span, "check", "`spawn` requires a call")),
                }
            }
            Stmt::Unsafe { body, .. } => self.block(body, env),
        }
    }

    /// If `e` is a call to a declared fn, return its return type.
    fn call_ret(&mut self, e: &Expr) -> Option<Ret> {
        if let ExprKind::Call { f, .. } = &e.kind {
            if let ExprKind::Ident(n) = &f.kind {
                return self.fns.get(n).map(|s| s.ret.clone());
            }
        }
        None
    }

    fn assignable(&mut self, want: &Ty, got: &Ty, sp: Span) {
        if *want == Ty::Unknown || *got == Ty::Unknown {
            return;
        }
        let ok = *want == *got
            || matches!((want, got), (Ty::Own(_), g) if *g == Ty::Own(Box::new(Ty::Unknown)))
            || (*want == Ty::CStr && *got == Ty::Str)
            || (*want == Ty::Err && *got == Ty::Int)
            || (*want == Ty::Int && *got == Ty::Err)
            || matches!((want, got), (Ty::Ref(w), Ty::Own(g) | Ty::Mut(g) | Ty::Ref(g)) | (Ty::Mut(w), Ty::Own(g) | Ty::Mut(g)) if **w == **g);
        if !ok {
            self.diags.push(Diag::err(
                sp, "type",
                format!("expected `{}`, found `{}`", want.name(), got.name()),
            ));
        }
    }

    fn expr(&mut self, e: &Expr, env: &mut Env) -> Ty {
        match &e.kind {
            ExprKind::Int(_) => Ty::Int,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Nil => Ty::Err, // nil is the empty err
            ExprKind::Ident(n) => match env.get(n) {
                Some(t) => t,
                None => {
                    if self.fns.contains_key(n) || BUILTINS.contains(&n.as_str()) {
                        self.diags.push(Diag::err(e.span, "check", format!("`{n}` is a function — call it")));
                    } else {
                        self.diags.push(Diag::err(e.span, "check", format!("undefined `{n}`")));
                    }
                    Ty::Unknown
                }
            },
            ExprKind::Call { f, args } => self.call(f, args, e.span, env),
            ExprKind::Field { obj, name } => {
                let t = self.expr(obj, env);
                let base = match &t {
                    Ty::Struct(n) => Some(n.clone()),
                    Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => {
                        if let Ty::Struct(n) = &**b { Some(n.clone()) } else { None }
                    }
                    _ => None,
                };
                match base {
                    Some(sn) => {
                        let fields = self.structs.get(&sn).cloned().unwrap_or_default();
                        fields
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, t)| t.clone())
                            .unwrap_or_else(|| {
                                self.diags.push(Diag::err(e.span, "type",
                                    format!("`{sn}` has no field `{name}`")));
                                Ty::Unknown
                            })
                    }
                    None => {
                        if t != Ty::Unknown {
                            self.diags.push(Diag::err(e.span, "type",
                                format!("`.{name}` on non-struct `{}`", t.name())));
                        }
                        Ty::Unknown
                    }
                }
            }
            ExprKind::StructLit { name, fields } => {
                match self.structs.get(name).cloned() {
                    Some(want) => {
                        for (fname, fe) in fields {
                            match want.iter().find(|(n, _)| n == fname) {
                                Some((_, wt)) => {
                                    let gt = self.expr(fe, env);
                                    let wt = wt.clone();
                                    self.assignable(&wt, &gt, fe.span);
                                }
                                None => self.diags.push(Diag::err(fe.span, "type",
                                    format!("`{name}` has no field `{fname}`"))),
                            }
                        }
                        let given: HashSet<&String> = fields.iter().map(|(n, _)| n).collect();
                        for (n, _) in &want {
                            if !given.contains(n) {
                                self.diags.push(Diag::err(e.span, "type",
                                    format!("missing field `{n}` in `{name}` literal")));
                            }
                        }
                        Ty::Struct(name.clone())
                    }
                    None => {
                        self.diags.push(Diag::err(e.span, "type", format!("unknown struct `{name}`")));
                        Ty::Unknown
                    }
                }
            }
            ExprKind::Bin { op, l, r } => {
                let lt = self.expr(l, env);
                let rt = self.expr(r, env);
                match op {
                    BinOp::And | BinOp::Or => {
                        self.assignable(&Ty::Bool, &lt, l.span);
                        self.assignable(&Ty::Bool, &rt, r.span);
                        Ty::Bool
                    }
                    BinOp::Eq | BinOp::Ne => {
                        self.assignable(&lt, &rt, e.span);
                        Ty::Bool
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        self.assignable(&Ty::Int, &lt, l.span);
                        self.assignable(&Ty::Int, &rt, r.span);
                        Ty::Bool
                    }
                    BinOp::Add => {
                        if lt == Ty::Str && rt == Ty::Str {
                            Ty::Str
                        } else {
                            self.assignable(&Ty::Int, &lt, l.span);
                            self.assignable(&Ty::Int, &rt, r.span);
                            Ty::Int
                        }
                    }
                    _ => {
                        self.assignable(&Ty::Int, &lt, l.span);
                        self.assignable(&Ty::Int, &rt, r.span);
                        Ty::Int
                    }
                }
            }
            ExprKind::Un { op, e: inner } => {
                let t = self.expr(inner, env);
                match op {
                    UnOp::Neg => {
                        self.assignable(&Ty::Int, &t, inner.span);
                        Ty::Int
                    }
                    UnOp::Not => {
                        self.assignable(&Ty::Bool, &t, inner.span);
                        Ty::Bool
                    }
                }
            }
        }
    }

    fn call(&mut self, f: &Expr, args: &[Expr], sp: Span, env: &mut Env) -> Ty {
        if let ExprKind::Ident(name) = &f.kind {
            if self.fns.contains_key(name) {
                self.calls.entry(self.cur_fn.clone()).or_default().insert(name.clone());
            }
            match name.as_str() {
                "print" | "println" => {
                    if args.len() != 1 {
                        self.diags.push(Diag::err(sp, "check", format!("`{name}` takes 1 argument")));
                    } else {
                        let t = self.expr(&args[0], env);
                        if !matches!(t, Ty::Str | Ty::Int | Ty::Bool | Ty::Err | Ty::CStr | Ty::Unknown) {
                            self.diags.push(Diag::err(sp, "type", format!("cannot print `{}`", t.name())));
                        }
                    }
                    return Ty::Void;
                }
                "mkchan" => return Ty::Chan,
                "send" => {
                    if args.len() != 2 {
                        self.diags.push(Diag::err(sp, "check", "`send` takes (chan, value)"));
                    } else {
                        let ct = self.expr(&args[0], env);
                        self.assignable(&Ty::Chan, &ct, args[0].span);
                        let vt = self.expr(&args[1], env);
                        if !vt.is_word() {
                            self.diags.push(Diag::err(args[1].span, "own",
                                format!("channels carry word-sized owned values (v0), not `{}`", vt.name())));
                        }
                    }
                    return Ty::Void;
                }
                "recv" => return Ty::Int,
                "close" => {
                    // chan close OR libc close(fd) — dispatch on arg type
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Void;
                }
                "sleep" => {
                    if let Some(a) = args.first() {
                        let t = self.expr(a, env);
                        self.assignable(&Ty::Int, &t, a.span);
                    }
                    return Ty::Void;
                }
                "new" => {
                    // new(StructName) -> own Struct
                    if let Some(Expr { kind: ExprKind::Ident(n), .. }) = args.first() {
                        if self.structs.contains_key(n) {
                            return Ty::Own(Box::new(Ty::Struct(n.clone())));
                        }
                        self.diags.push(Diag::err(sp, "type", format!("`new` needs a struct name, got `{n}`")));
                    } else {
                        self.diags.push(Diag::err(sp, "type", "`new` needs a struct name"));
                    }
                    return Ty::Unknown;
                }
                "itoa" => {
                    if let Some(a) = args.first() {
                        let t = self.expr(a, env);
                        self.assignable(&Ty::Int, &t, a.span);
                    }
                    return Ty::Str;
                }
                "assert" => {
                    if let Some(a) = args.first() {
                        let t = self.expr(a, env);
                        self.assignable(&Ty::Bool, &t, a.span);
                    }
                    return Ty::Void;
                }
                "argc" => return Ty::Int,
                "argv" => return Ty::Str,
                "len" => {
                    if let Some(a) = args.first() {
                        let t = self.expr(a, env);
                        self.assignable(&Ty::Str, &t, a.span);
                    }
                    return Ty::Int;
                }
                "byteat" | "find" | "atoi" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Int;
                }
                "substr" | "bufstr" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Str;
                }
                "mkbuf" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Own(Box::new(Ty::Unknown));
                }
                "poke8" | "poke16" | "poke32" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Void;
                }
                "htons" | "htonl" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Int;
                }
                "cstr" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::CStr;
                }
                "sqlite_open" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Own(Box::new(Ty::Unknown));
                }
                "sqlite_exec" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Int;
                }
                "sqlite_query" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Str;
                }
                "sqlite_close" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Void;
                }
                _ => {}
            }
            let sig = match self.fns.get(name) {
                Some(s) => s.clone(),
                None => {
                    self.diags.push(Diag::err(sp, "check", format!("undefined `{name}`")));
                    return Ty::Unknown;
                }
            };
            if sig.is_extern {
                for a in args {
                    let t = self.expr(a, env);
                    if !matches!(t, Ty::Int | Ty::Bool | Ty::CStr | Ty::Str | Ty::Own(_) | Ty::Chan | Ty::Unknown) {
                        self.diags.push(Diag::err(a.span, "ffi",
                            format!("`{}` cannot cross the FFI boundary", t.name())));
                    }
                }
            }
            if args.len() != sig.params.len() {
                self.diags.push(Diag::err(sp, "check",
                    format!("`{name}` takes {} args, got {}", sig.params.len(), args.len())));
            }
            for (a, want) in args.iter().zip(sig.params.iter()) {
                let got = self.expr(a, env);
                // extern fns get the raw C type; ref/mut params emit borrow checks
                self.assignable(want, &got, a.span);
                if sig.is_extern && matches!(want, Ty::Ref(_) | Ty::Mut(_)) {
                    // handled in codegen
                }
            }
            return match sig.ret {
                Ret::Void => Ty::Void,
                Ret::One(t) => t,
                Ret::Pair(t) => {
                    if !self.in_let2 {
                        self.diags.push(Diag::err(sp, "check",
                            "`[T, err]` result must be bound with `[v, err] =` or returned"));
                    }
                    t
                }
            };
        }
        self.diags.push(Diag::err(sp, "check", "can only call named functions (v0)"));
        Ty::Unknown
    }

    /// Suspend-capable = reaches spawn/scope/recv/send/sleep transitively.
    fn suspend_set(&self, c: &Crate) -> HashSet<String> {
        let mut seeds: HashSet<String> = HashSet::new();
        fn block_has(b: &Block) -> bool {
            b.stmts.iter().any(|s| match s {
                Stmt::Spawn { .. } | Stmt::Scope { .. } => true,
                Stmt::Expr(e) | Stmt::Assign { val: e, .. } | Stmt::Let { init: e, .. } => {
                    expr_susp(e)
                }
                Stmt::Let2 { call, .. } => expr_susp(call),
                Stmt::Return { vals, .. } => vals.iter().any(expr_susp),
                Stmt::If { cond, then, els, .. } => {
                    expr_susp(cond) || block_has(then) || els.as_ref().map(block_has).unwrap_or(false)
                }
                Stmt::For { cond, body, .. } => expr_susp(cond) || block_has(body),
                Stmt::Unsafe { body, .. } => block_has(body),
            })
        }
        fn expr_susp(e: &Expr) -> bool {
            match &e.kind {
                ExprKind::Call { f, args } => {
                    let builtin_susp = matches!(
                        &f.kind,
                        ExprKind::Ident(n) if n == "recv" || n == "send" || n == "sleep"
                    );
                    builtin_susp
                        || args.iter().any(expr_susp)
                        || matches!(&f.kind, ExprKind::Call{..} | ExprKind::Field{..})
                }
                ExprKind::Bin { l, r, .. } => expr_susp(l) || expr_susp(r),
                ExprKind::Un { e, .. } => expr_susp(e),
                ExprKind::Field { obj, .. } => expr_susp(obj),
                ExprKind::StructLit { fields, .. } => fields.iter().any(|(_, e)| expr_susp(e)),
                _ => false,
            }
        }
        for it in &c.items {
            if let Item::Fn(f) = it {
                if f.body.as_ref().map(block_has).unwrap_or(false) {
                    seeds.insert(f.name.clone());
                }
            }
        }
        // reverse reachability: callers of suspend-capable fns are suspend-capable
        let mut susp = seeds;
        loop {
            let mut grew = false;
            for (caller, callees) in &self.calls {
                if !susp.contains(caller) && callees.iter().any(|c| susp.contains(c)) {
                    susp.insert(caller.clone());
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        susp
    }
}
