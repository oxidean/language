use crate::ast::*;
use crate::diag::{Diag, Diags, Span};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Int,
    Float,
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
    Enum(String),
    Var(String), // generic type parameter (instantiated at call sites)
    Unknown,
}

impl Ty {
    pub fn name(&self) -> String {
        match self {
            Ty::Int => "int".into(),
            Ty::Float => "float".into(),
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
            Ty::Enum(n) => n.clone(),
            Ty::Var(n) => n.clone(),
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
    ty_params: Vec<(String, String)>, // [(T, constraint)]
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
    /// enum name -> [(variant, fields)]; fields f0..fn for positional
    /// payloads. Variant payload structs also live in `structs` as "E.V".
    enums: HashMap<String, Vec<(String, Vec<(String, Ty)>)>>,
    ifaces: HashMap<String, Vec<String>>,
    cur_ret: Ret,
    calls: HashMap<String, HashSet<String>>, // caller -> callee fn names
    cur_fn: String,
    in_let2: bool,
    ty_scope: HashSet<String>, // type params in scope (current generic fn)
    cur_ty_params: Vec<(String, String)>,
    loop_depth: i32,
    globals: HashMap<String, Ty>,
}

const BUILTINS: &[&str] = &[
    "print", "println", "mkchan", "send", "recv", "close", "sleep", "new",
    "assert", "itoa", "argc", "argv", "len", "byteat", "substr", "find", "atoi",
    "mkbuf", "bufstr", "poke8", "poke16", "poke32", "htons", "htonl",
    "cstr", "sqlite_open", "sqlite_exec", "sqlite_query", "sqlite_close",
    "readfile", "writefile", "exit", "mkshared", "sget", "sset", "scas", "sadd",
    "now", "now_ms", "usleep", "ftoi", "itof", "atof", "ftoa",
];

pub fn check(c: &Crate, diags: &mut Diags) -> Checked {
    let mut ck = Checker {
        diags,
        fns: HashMap::new(),
        structs: HashMap::new(),
        enums: HashMap::new(),
        ifaces: HashMap::new(),
        cur_ret: Ret::Void,
        calls: HashMap::new(),
        cur_fn: String::new(),
        in_let2: false,
        ty_scope: HashSet::new(),
        cur_ty_params: Vec::new(),
        loop_depth: 0,
        globals: HashMap::new(),
    };
    ck.register(c);
    ck.check_items(c);
    let checked = Checked { suspend: ck.suspend_set(c) };
    ck.check_suspend_rules(c, &checked.suspend);
    checked
}

impl<'a> Checker<'a> {
    fn register(&mut self, c: &Crate) {
        // pass 0: struct/enum names so self-references and forward use
        // resolve (recursive payloads like `own Expr` inside `Expr`)
        for it in &c.items {
            if let Item::Struct(s) = it {
                self.structs.insert(s.name.clone(), Vec::new());
            }
            if let Item::Enum(e) = it {
                self.enums.insert(e.name.clone(), Vec::new());
            }
            if let Item::Interface(i) = it {
                self.ifaces.insert(i.name.clone(), i.ops.clone());
            }
        }
        for it in &c.items {
            if let Item::Enum(e) = it {
                let mut seen = HashSet::new();
                let mut variants = Vec::new();
                for v in &e.variants {
                    if !seen.insert(v.name.clone()) {
                        self.diags.push(Diag::err(v.span, "check",
                            format!("duplicate variant `{}`", v.name)));
                        continue;
                    }
                    let fields: Vec<(String, Ty)> = match &v.payload {
                        VariantPayload::Unit => Vec::new(),
                        VariantPayload::Pos(tys) => tys.iter().enumerate()
                            .map(|(i, t)| (format!("f{i}"), self.resolve(t))).collect(),
                        VariantPayload::Named(fs) => fs.iter()
                            .map(|(n, t)| (n.clone(), self.resolve(t))).collect(),
                    };
                    // the `as` payload type: `E.V` resolves as a struct
                    self.structs.insert(format!("{}.{}", e.name, v.name), fields.clone());
                    variants.push((v.name.clone(), fields));
                }
                self.enums.insert(e.name.clone(), variants);
            }
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
                    self.ty_scope = f.ty_params.iter().map(|(t, _)| t.clone()).collect();
                    let params: Vec<Ty> = f.params.iter().map(|p| self.resolve(&p.ty)).collect();
                    let ret = self.resolve_ret(&f.ret);
                    self.ty_scope.clear();
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
                    for (t, c) in &f.ty_params {
                        if !c.is_empty() && c != "any" && c != "ordered" && c != "eq" && c != "add" && !self.ifaces.contains_key(c) {
                            self.diags.push(Diag::err(f.span, "check", format!("unknown constraint `{c}` for `{t}`")));
                        }
                    }
                    self.fns.insert(
                        f.name.clone(),
                        FnSig { params, ret, is_extern: matches!(it, Item::Extern(_)), ty_params: f.ty_params.clone() },
                    );
                }
                Item::Import(_) => {} // resolved by the driver before checking
                Item::Global(g) => {
                    let mut env = Env::new();
                    let it = self.expr(&g.init, &mut env);
                    let t = if let Some(tr) = &g.ty {
                        let want = self.resolve(tr);
                        self.assignable(&want, &it, g.span);
                        want
                    } else {
                        it
                    };
                    self.globals.insert(g.name.clone(), t);
                }
                Item::Struct(_) | Item::Interface(_) | Item::Enum(_) => {}
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
                "float" => Ty::Float,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "cstr" => Ty::CStr,
                "void" => Ty::Void,
                "err" => Ty::Err,
                s => {
                    if self.ty_scope.contains(s) {
                        Ty::Var(s.to_string())
                    } else if self.structs.contains_key(s) {
                        Ty::Struct(s.to_string())
                    } else if self.enums.contains_key(s) {
                        Ty::Enum(s.to_string())
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
                self.ty_scope = f.ty_params.iter().map(|(t, _)| t.clone()).collect();
                self.cur_ty_params = f.ty_params.clone();
                self.loop_depth = 0;
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
            Stmt::Defer { call, .. } => {
                self.expr(call, env);
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
                self.loop_depth += 1;
                self.block(body, env);
                self.loop_depth -= 1;
            }
            Stmt::Break { span } | Stmt::Continue { span } => {
                if self.loop_depth == 0 {
                    self.diags.push(Diag::err(*span, "check", "break/continue outside loop"));
                }
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
            Stmt::Select { arms, span } => {
                if arms.is_empty() {
                    self.diags.push(Diag::err(*span, "check", "`select` needs at least one arm"));
                }
                let mut has_else = false;
                let mut tmo_count = 0;
                for arm in arms {
                    env.push();
                    match &arm.kind {
                        SelKind::Recv { chan, bind, .. } => {
                            let ct = self.expr(chan, env);
                            self.assignable(&Ty::Chan, &ct, chan.span);
                            if let Some(n) = bind {
                                env.set(n.clone(), Ty::Int); // word payload (v0)
                            }
                        }
                        SelKind::Timeout { ms, .. } => {
                            tmo_count += 1;
                            let t = self.expr(ms, env);
                            self.assignable(&Ty::Int, &t, ms.span);
                        }
                        SelKind::Else { .. } => {
                            has_else = true;
                        }
                    }
                    for s in &arm.body.stmts {
                        self.stmt(s, env);
                    }
                    env.pop();
                }
                if has_else && tmo_count > 0 {
                    self.diags.push(Diag::err(*span, "check",
                        "`else` makes `timeout` unreachable — drop one"));
                }
                if tmo_count > 1 {
                    self.diags.push(Diag::err(*span, "check", "at most one `timeout` arm"));
                }
            }
        }
    }

    /// generic bodies may only use operators the constraint permits
    fn req_op(&mut self, var: &str, op: &str, binop: BinOp, sp: Span) {
        let c = self
            .cur_ty_params
            .iter()
            .find(|(t, _)| t == var)
            .map(|(_, c)| c.clone())
            .unwrap_or_default();
        let has = match c.as_str() {
            "" | "any" => false,
            "eq" => op == "eq",
            "ordered" => op == "ordered" || op == "eq",
            "add" => op == "add",
            name => self.ifaces.get(name).map(|ops| ops.iter().any(|o| o == op)).unwrap_or(false),
        };
        if !has {
            self.diags.push(Diag::err(sp, "type",
                format!("`{binop:?}` on `{var}` requires constraint `{op}` (has `{c}`)")));
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

    /// record Ty::Var bindings by matching an expected param type against the
    /// argument's type (own T binds T=own-inner? no — own T binds T fully)
    fn bind_vars(&self, want: &Ty, got: &Ty, bind: &mut HashMap<String, Ty>) {
        match (want, got) {
            (Ty::Var(v), g) => {
                bind.entry(v.clone()).or_insert_with(|| g.clone());
            }
            (Ty::Own(w), Ty::Own(g)) | (Ty::Ref(w), Ty::Ref(g)) | (Ty::Mut(w), Ty::Mut(g)) => {
                self.bind_vars(w, g, bind)
            }
            // own argument into ref/mut param still binds the inner var
            (Ty::Ref(w) | Ty::Mut(w), Ty::Own(g) | Ty::Mut(g) | Ty::Ref(g)) => self.bind_vars(w, g, bind),
            _ => {}
        }
    }

    /// does `t` satisfy constraint `c` (builtin word or interface name)?
    fn satisfies(&self, t: &Ty, c: &str) -> bool {
        let op_ok = |op: &str| match op {
            "eq" => matches!(t, Ty::Int | Ty::Bool | Ty::Str | Ty::Chan | Ty::Var(_)),
            "ordered" => matches!(t, Ty::Int | Ty::Var(_)),
            "add" => matches!(t, Ty::Int | Ty::Str | Ty::Var(_)),
            _ => false,
        };
        match c {
            "" | "any" => true,
            "eq" | "ordered" | "add" => op_ok(c),
            name => self
                .ifaces
                .get(name)
                .map(|ops| ops.iter().all(|o| op_ok(o)))
                .unwrap_or(false),
        }
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
            // nil (Ty::Err) is the null pointer for owned/borrowed refs
            || (*got == Ty::Err && matches!(want, Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_) | Ty::Chan))
            || matches!((want, got), (Ty::Ref(w), Ty::Own(g) | Ty::Mut(g) | Ty::Ref(g)) | (Ty::Mut(w), Ty::Own(g) | Ty::Mut(g)) if **w == **g);
        if !ok {
            self.diags.push(Diag::err(
                sp, "type",
                format!("expected `{}`, found `{}`", want.name(), got.name()),
            ));
        }
    }

    /// `E.V` — split at the last dot: E is the (possibly qualified) enum
    /// name, V the variant. Returns (enum, variant, fields).
    fn enum_variant(&self, path: &str) -> Option<(String, String, Vec<(String, Ty)>)> {
        let (e, v) = path.rsplit_once('.')?;
        let fields = self.enums.get(e)?
            .iter().find(|(n, _)| n == v)?.1.clone();
        Some((e.to_string(), v.to_string(), fields))
    }

    /// check variant-construction args (positional call or named lit)
    #[allow(clippy::too_many_arguments)]
    fn check_ctor(
        &mut self, e_name: &str, v_name: &str, want: &[(String, Ty)],
        given: &[(&String, &Expr)], sp: Span, env: &mut Env,
    ) {
        if given.len() != want.len() {
            self.diags.push(Diag::err(sp, "type",
                format!("`{e_name}.{v_name}` takes {} payload field(s), found {}",
                    want.len(), given.len())));
            return;
        }
        for ((wn, wt), (gn, ge)) in want.iter().zip(given.iter()) {
            if wn != *gn {
                self.diags.push(Diag::err(ge.span, "type",
                    format!("`{e_name}.{v_name}` field `{wn}` got `{gn}`")));
                continue;
            }
            let gt = self.expr(ge, env);
            let wt = wt.clone();
            self.assignable(&wt, &gt, ge.span);
        }
    }

    fn expr(&mut self, e: &Expr, env: &mut Env) -> Ty {
        match &e.kind {
            ExprKind::Int(_) => Ty::Int,
            ExprKind::Float(_) => Ty::Float,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Nil => Ty::Err, // nil is the empty err
            ExprKind::Ident(n) => match env.get(n).or_else(|| self.globals.get(n).cloned()) {
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
                // unit variant: `ExprKind.Nil` — obj is the enum name
                // (possibly a module path), not a value expression
                if let Some(on) = field_path(obj) {
                    if let Some(vars) = self.enums.get(&on).cloned() {
                        match vars.iter().find(|(n, _)| n == name) {
                            Some((_, fields)) if fields.is_empty() => {
                                return Ty::Enum(on.clone());
                            }
                            Some(_) => {
                                self.diags.push(Diag::err(e.span, "check",
                                    format!("`{on}.{name}` has a payload — construct it")));
                                return Ty::Unknown;
                            }
                            None => {
                                self.diags.push(Diag::err(e.span, "check",
                                    format!("enum `{on}` has no variant `{name}`")));
                                return Ty::Unknown;
                            }
                        }
                    }
                }
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
                // named variant construction: `ExprKind.Bin{op, l, r}`
                if let Some((en, vn, want)) = self.enum_variant(name) {
                    if want.iter().any(|(n, _)| !(n.starts_with('f') && n[1..].chars().all(|c| c.is_ascii_digit()))) || want.is_empty() {
                        let given: Vec<(&String, &Expr)> = want.iter()
                            .map(|(n, _)| n)
                            .filter_map(|n| fields.iter().find(|(g, _)| g == n).map(|(g, fe)| (g, fe)))
                            .collect();
                        let gv: HashSet<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
                        for (n, fe) in fields {
                            if !want.iter().any(|(w, _)| w == n) {
                                self.diags.push(Diag::err(fe.span, "type",
                                    format!("`{name}` has no field `{n}`")));
                            }
                        }
                        if given.len() != want.len() {
                            for (n, _) in &want {
                                if !gv.contains(n.as_str()) {
                                    self.diags.push(Diag::err(e.span, "type",
                                        format!("missing field `{n}` in `{name}` literal")));
                                }
                            }
                        }
                        self.check_ctor(&en, name, &want, &given, e.span, env);
                    } else {
                        self.diags.push(Diag::err(e.span, "check",
                            format!("`{name}` has positional payload — call it `{name}(...)`")));
                    }
                    let _ = vn;
                    return Ty::Enum(en);
                }
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
            ExprKind::As { e: inner, ty } => {
                let ot = self.expr(inner, env);
                let wt = self.resolve(ty);
                // `e as E.V` on an `own E` dereferences the handle
                let base = match &ot { Ty::Own(b) => &**b, t => t };
                match (base, &wt) {
                    (Ty::Enum(en), Ty::Struct(sn)) => {
                        if self.enum_variant(sn).map(|(e2, _, _)| e2) == Some(en.clone()) {
                            Ty::Struct(sn.clone())
                        } else {
                            self.diags.push(Diag::err(e.span, "type",
                                format!("`{sn}` is not a variant of `{en}`")));
                            Ty::Unknown
                        }
                    }
                    (Ty::Unknown, _) | (_, Ty::Unknown) => Ty::Unknown,
                    _ => {
                        self.diags.push(Diag::err(e.span, "type",
                            format!("`as` casts enum values to variant payloads — `{} as {}` invalid",
                                ot.name(), wt.name())));
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
                        if let Ty::Var(v) = &lt {
                            self.req_op(v, "ordered", *op, e.span);
                        } else if lt == Ty::Float || rt == Ty::Float {
                            self.assignable(&Ty::Float, &lt, l.span);
                            self.assignable(&Ty::Float, &rt, r.span);
                        } else {
                            self.assignable(&Ty::Int, &lt, l.span);
                            self.assignable(&Ty::Int, &rt, r.span);
                        }
                        Ty::Bool
                    }
                    BinOp::Add => {
                        if let Ty::Var(v) = &lt {
                            self.req_op(v, "add", *op, e.span);
                            Ty::Var(v.clone())
                        } else if lt == Ty::Str && rt == Ty::Str {
                            Ty::Str
                        } else if lt == Ty::Float || rt == Ty::Float {
                            self.assignable(&Ty::Float, &lt, l.span);
                            self.assignable(&Ty::Float, &rt, r.span);
                            Ty::Float
                        } else {
                            self.assignable(&Ty::Int, &lt, l.span);
                            self.assignable(&Ty::Int, &rt, r.span);
                            Ty::Int
                        }
                    }
                    _ => {
                        if lt == Ty::Float || rt == Ty::Float {
                            self.assignable(&Ty::Float, &lt, l.span);
                            self.assignable(&Ty::Float, &rt, r.span);
                            return Ty::Float;
                        }
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
                        if t == Ty::Float {
                            Ty::Float
                        } else {
                            self.assignable(&Ty::Int, &t, inner.span);
                            Ty::Int
                        }
                    }
                    UnOp::Not => {
                        self.assignable(&Ty::Bool, &t, inner.span);
                        Ty::Bool
                    }
                }
            }
            ExprKind::Match { e: scrut, arms } => {
                let st = self.expr(scrut, env);
                let en = match &st {
                    Ty::Enum(n) => Some(n.clone()),
                    Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match &**b {
                        Ty::Enum(n) => Some(n.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                let all_vars: Vec<String> = en.as_ref()
                    .map(|n| self.enums.get(n).cloned().unwrap_or_default()
                        .iter().map(|(v, _)| v.clone()).collect())
                    .unwrap_or_default();
                let mut covered: HashSet<String> = HashSet::new();
                let mut wild = false;
                let (mut saw_t, mut saw_f) = (false, false);
                let mut result: Option<Ty> = None;
                let (mut n_expr, mut n_block) = (0usize, 0usize);
                for arm in arms {
                    env.push();
                    match &arm.pat {
                        Pattern::Wild => wild = true,
                        Pattern::Bind(n) => {
                            wild = true;
                            env.set(n.clone(), st.clone());
                        }
                        Pattern::Lit(le) => {
                            let lt = self.expr(le, env);
                            match (&st, &lt) {
                                (Ty::Bool, Ty::Bool) => {
                                    if let ExprKind::Bool(v) = &le.kind {
                                        if *v { saw_t = true } else { saw_f = true }
                                    }
                                }
                                (Ty::Unknown, _) | (_, Ty::Unknown) => {}
                                _ => self.assignable(&st, &lt, arm.span),
                            }
                        }
                        Pattern::VarUnit { path } => match self.enum_variant(path) {
                            Some((en2, vn, fields)) => {
                                if en.as_deref() != Some(en2.as_str()) {
                                    self.diags.push(Diag::err(arm.span, "check",
                                        format!("`{path}` is a variant of `{en2}` — scrutinee is `{}`",
                                            en.as_deref().unwrap_or("non-enum"))));
                                } else if !fields.is_empty() {
                                    self.diags.push(Diag::err(arm.span, "check",
                                        format!("`{path}` has a payload — bind it `{path}(...)` or `{path}{{...}}`")));
                                } else {
                                    covered.insert(vn);
                                }
                            }
                            None => self.diags.push(Diag::err(arm.span, "check",
                                format!("unknown variant `{path}`"))),
                        },
                        Pattern::VarPos { path, binds } => match self.enum_variant(path) {
                            Some((en2, vn, fields)) => {
                                if en.as_deref() != Some(en2.as_str()) {
                                    self.diags.push(Diag::err(arm.span, "check",
                                        format!("`{path}` is a variant of `{en2}` — scrutinee is `{}`",
                                            en.as_deref().unwrap_or("non-enum"))));
                                } else {
                                    covered.insert(vn);
                                    if binds.len() != fields.len() {
                                        self.diags.push(Diag::err(arm.span, "check",
                                            format!("`{path}` takes {} binds, got {}",
                                                fields.len(), binds.len())));
                                    }
                                    for (b, (_, ft)) in binds.iter().zip(fields.iter()) {
                                        if b != "_" {
                                            env.set(b.clone(), ft.clone());
                                        }
                                    }
                                }
                            }
                            None => self.diags.push(Diag::err(arm.span, "check",
                                format!("unknown variant `{path}`"))),
                        },
                        Pattern::VarNamed { path, binds } => match self.enum_variant(path) {
                            Some((en2, vn, fields)) => {
                                if en.as_deref() != Some(en2.as_str()) {
                                    self.diags.push(Diag::err(arm.span, "check",
                                        format!("`{path}` is a variant of `{en2}` — scrutinee is `{}`",
                                            en.as_deref().unwrap_or("non-enum"))));
                                } else {
                                    covered.insert(vn);
                                    for b in binds {
                                        match fields.iter().find(|(n, _)| n == b) {
                                            Some((_, ft)) => env.set(b.clone(), ft.clone()),
                                            None => self.diags.push(Diag::err(arm.span, "check",
                                                format!("`{path}` has no field `{b}`"))),
                                        }
                                    }
                                }
                            }
                            None => self.diags.push(Diag::err(arm.span, "check",
                                format!("unknown variant `{path}`"))),
                        },
                    }
                    match &arm.body {
                        MatchBody::Expr(be) => {
                            n_expr += 1;
                            let bt = self.expr(be, env);
                            match &result {
                                None => result = Some(bt),
                                Some(rt) => self.assignable(rt, &bt, arm.span),
                            }
                        }
                        MatchBody::Block(b) => {
                            n_block += 1;
                            self.block(b, env);
                        }
                    }
                    env.pop();
                }
                if n_expr > 0 && n_block > 0 {
                    self.diags.push(Diag::err(e.span, "check",
                        "match arms must be all expressions or all blocks — not mixed"));
                }
                match &en {
                    Some(en) if !wild && st != Ty::Unknown => {
                        let missing: Vec<&String> = all_vars.iter()
                            .filter(|v| !covered.contains(*v)).collect();
                        if !missing.is_empty() {
                            self.diags.push(Diag::err(e.span, "check",
                                format!("non-exhaustive match on `{en}` — missing {}",
                                    missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))));
                        }
                    }
                    None if !wild && st == Ty::Bool => {
                        if !(saw_t && saw_f) {
                            self.diags.push(Diag::err(e.span, "check",
                                "non-exhaustive match on `bool` — cover `true`/`false` or add `_`"));
                        }
                    }
                    None if !wild && st != Ty::Unknown && st != Ty::Bool => {
                        self.diags.push(Diag::err(e.span, "check",
                            format!("`match` on `{}` needs a `_` arm", st.name())));
                    }
                    _ => {}
                }
                if n_expr > 0 {
                    result.unwrap_or(Ty::Void)
                } else {
                    Ty::Void
                }
            }
        }
    }

    fn call(&mut self, f: &Expr, args: &[Expr], sp: Span, env: &mut Env) -> Ty {
        if let ExprKind::Ident(name) = &f.kind {
            // enum variant construction: `ExprKind.Int(7)` qualifies to
            // an Ident call on the dotted path
            if let Some((en, vn, fields)) = self.enum_variant(name) {
                if fields.iter().all(|(n, _)| n.starts_with('f') && n[1..].chars().all(|c| c.is_ascii_digit())) {
                    // positional payload: args in declaration order
                    let given: Vec<(&String, &Expr)> = fields.iter().map(|(n, _)| n)
                        .zip(args.iter()).collect();
                    self.check_ctor(&en, &vn, &fields, &given, sp, env);
                } else {
                    self.diags.push(Diag::err(sp, "check",
                        format!("`{name}` has named payload — use `{name}{{...}}`")));
                }
                return Ty::Enum(en);
            }
            // `e.kind()` — enum tag accessor (flattens to Ident "e.kind")
            if let Some((obj, "kind")) = name.rsplit_once('.') {
                if args.is_empty() {
                    let ot = self.expr(&Expr {
                        kind: ExprKind::Ident(obj.to_string()), span: f.span,
                    }, env);
                    let otb = match &ot { Ty::Own(b) => &**b, t => t };
                    if matches!(otb, Ty::Enum(_)) {
                        return Ty::Int;
                    }
                    if ot != Ty::Unknown {
                        self.diags.push(Diag::err(sp, "type",
                            format!("`.kind()` on non-enum `{}`", ot.name())));
                    }
                    return Ty::Unknown;
                }
            }
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
                "mkchan" => {
                    if args.len() > 1 {
                        self.diags.push(Diag::err(sp, "check", "`mkchan` takes at most one capacity arg"));
                    }
                    if let Some(a) = args.first() {
                        let t = self.expr(a, env);
                        self.assignable(&Ty::Int, &t, a.span);
                    }
                    return Ty::Chan;
                }
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
                    // new(StructName) -> own Struct; new(expr) -> own <expr ty>
                    if let Some(a) = args.first() {
                        if let Expr { kind: ExprKind::Ident(n), .. } = a {
                            if self.structs.contains_key(n) {
                                return Ty::Own(Box::new(Ty::Struct(n.clone())));
                            }
                        }
                        let t = self.expr(a, env);
                        return Ty::Own(Box::new(t));
                    }
                    self.diags.push(Diag::err(sp, "type", "`new` needs a struct name or value"));
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
                "ftoi" => {
                    if let Some(a) = args.first() {
                        let t = self.expr(a, env);
                        self.assignable(&Ty::Float, &t, a.span);
                    }
                    return Ty::Int;
                }
                "itof" => {
                    if let Some(a) = args.first() {
                        let t = self.expr(a, env);
                        self.assignable(&Ty::Int, &t, a.span);
                    }
                    return Ty::Float;
                }
                "atof" => {
                    if let Some(a) = args.first() {
                        let t = self.expr(a, env);
                        self.assignable(&Ty::Str, &t, a.span);
                    }
                    return Ty::Float;
                }
                "ftoa" => {
                    if let Some(a) = args.first() {
                        let t = self.expr(a, env);
                        self.assignable(&Ty::Float, &t, a.span);
                    }
                    return Ty::Str;
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
                "sqlite_close" | "exit" | "sset" | "writefile" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return if name == &"writefile" { Ty::Int } else { Ty::Void };
                }
                "readfile" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Str;
                }
                "sget" | "scas" | "sadd" | "now" | "now_ms" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Int;
                }
                "usleep" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Void;
                }
                "mkshared" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Own(Box::new(Ty::Unknown));
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
            // generic: bind type params from argument types, check constraints
            let mut bind: HashMap<String, Ty> = HashMap::new();
            if !sig.ty_params.is_empty() {
                for (a, want) in args.iter().zip(sig.params.iter()) {
                    let got = self.expr(a, env);
                    self.bind_vars(want, &got, &mut bind);
                }
                for (tv, c) in &sig.ty_params {
                    match bind.get(tv) {
                        Some(t) => {
                            let t = t.clone();
                            if !self.satisfies(&t, c) {
                                self.diags.push(Diag::err(sp, "type",
                                    format!("`{}` does not satisfy constraint `{}`", t.name(), c)));
                            }
                        }
                        None => self.diags.push(Diag::err(sp, "type",
                            format!("cannot infer type parameter `{tv}`"))),
                    }
                }
            }
            for (a, want) in args.iter().zip(sig.params.iter()) {
                let got = self.expr(a, env);
                // extern fns get the raw C type; ref/mut params emit borrow checks
                self.assignable(&subst_ty(want, &bind), &got, a.span);
            }
            let ret = subst_ret(&sig.ret, &bind);
            return match ret {
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
                Stmt::Scope { .. } => true,
                // spawn doesn't suspend the caller — but its args might
                Stmt::Spawn { call, .. } => expr_susp(call),
                Stmt::Defer { call, .. } => expr_susp(call),
                Stmt::Expr(e) | Stmt::Assign { val: e, .. } | Stmt::Let { init: e, .. } => {
                    expr_susp(e)
                }
                Stmt::Let2 { call, .. } => expr_susp(call),
                Stmt::Select { .. } => true, // always a suspend point
                Stmt::Return { vals, .. } => vals.iter().any(expr_susp),
                Stmt::If { cond, then, els, .. } => {
                    expr_susp(cond) || block_has(then) || els.as_ref().map(block_has).unwrap_or(false)
                }
                Stmt::For { cond, body, .. } => expr_susp(cond) || block_has(body),
                Stmt::Unsafe { body, .. } => block_has(body),
                Stmt::Break { .. } | Stmt::Continue { .. } => false,
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
                ExprKind::As { e, .. } => expr_susp(e),
                ExprKind::StructLit { fields, .. } => fields.iter().any(|(_, e)| expr_susp(e)),
                ExprKind::Match { e, arms } => {
                    expr_susp(e)
                        || arms.iter().any(|a| match &a.body {
                            MatchBody::Expr(be) => expr_susp(be),
                            MatchBody::Block(b) => block_has(b),
                        })
                        || arms.iter().any(|a| match &a.pat {
                            Pattern::Lit(le) => expr_susp(le),
                            _ => false,
                        })
                }
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

    /// rules that need the computed suspend set:
    /// - `defer` may not capture a suspend point (it runs during unwind)
    /// - global initializers may not suspend (no task drives them)
    fn check_suspend_rules(&mut self, c: &Crate, susp: &HashSet<String>) {
        fn expr_has_susp(e: &Expr, susp: &HashSet<String>) -> bool {
            match &e.kind {
                ExprKind::Call { f, args } => {
                    let susp_callee = matches!(&f.kind,
                        ExprKind::Ident(n)
                            if susp.contains(n) || n == "recv" || n == "send" || n == "sleep");
                    susp_callee
                        || args.iter().any(|a| expr_has_susp(a, susp))
                        || matches!(&f.kind, ExprKind::Call { .. } | ExprKind::Field { .. })
                }
                ExprKind::Bin { l, r, .. } => {
                    expr_has_susp(l, susp) || expr_has_susp(r, susp)
                }
                ExprKind::Un { e, .. } => expr_has_susp(e, susp),
                ExprKind::Field { obj, .. } => expr_has_susp(obj, susp),
                ExprKind::As { e, .. } => expr_has_susp(e, susp),
                ExprKind::StructLit { fields, .. } => {
                    fields.iter().any(|(_, e)| expr_has_susp(e, susp))
                }
                ExprKind::Match { e, arms } => {
                    expr_has_susp(e, susp)
                        || arms.iter().any(|a| match &a.body {
                            MatchBody::Expr(be) => expr_has_susp(be, susp),
                            MatchBody::Block(b) => blk_susp(b, susp),
                        })
                        || arms.iter().any(|a| match &a.pat {
                            Pattern::Lit(le) => expr_has_susp(le, susp),
                            _ => false,
                        })
                }
                _ => false,
            }
        }
        fn blk_susp(b: &Block, susp: &HashSet<String>) -> bool {
            b.stmts.iter().any(|s| match s {
                Stmt::Scope { .. } | Stmt::Select { .. } => true,
                Stmt::Expr(e) | Stmt::Assign { val: e, .. } | Stmt::Let { init: e, .. }
                | Stmt::Defer { call: e, .. } | Stmt::Spawn { call: e, .. } => {
                    expr_has_susp(e, susp)
                }
                Stmt::Let2 { call, .. } => expr_has_susp(call, susp),
                Stmt::Return { vals, .. } => vals.iter().any(|v| expr_has_susp(v, susp)),
                Stmt::If { cond, then, els, .. } => {
                    expr_has_susp(cond, susp)
                        || blk_susp(then, susp)
                        || els.as_ref().map(|b| blk_susp(b, susp)).unwrap_or(false)
                }
                Stmt::For { cond, body, .. } => {
                    expr_has_susp(cond, susp) || blk_susp(body, susp)
                }
                Stmt::Unsafe { body, .. } => blk_susp(body, susp),
                Stmt::Break { .. } | Stmt::Continue { .. } => false,
            })
        }
        // `match` arm bodies emit as C stmt-exprs except when the whole match
        // is a bare Stmt::Expr — only there can suspending calls live inside
        // arms (block arms run through the frame stmt machinery)
        fn scan_match(e: &Expr, susp: &HashSet<String>, diags: &mut Diags, stmt_pos: bool) {
            if let ExprKind::Match { e: scrut, arms } = &e.kind {
                scan_match(scrut, susp, diags, false);
                for a in arms {
                    match &a.body {
                        MatchBody::Expr(be) => {
                            if expr_has_susp(be, susp) {
                                let top_call = matches!(&be.kind,
                                    ExprKind::Call { f, .. }
                                        if matches!(&f.kind, ExprKind::Ident(n)
                                            if susp.contains(n)
                                                || n == "recv" || n == "send" || n == "sleep"));
                                if !(stmt_pos && top_call) {
                                    diags.push(Diag::err(a.span, "check",
                                        "suspending call inside a match arm must be the whole arm body \
                                         of a statement-level match — use a block arm or hoist the call"));
                                }
                            }
                            scan_match(be, susp, diags,
                                stmt_pos && matches!(&be.kind, ExprKind::Match { .. }));
                        }
                        MatchBody::Block(b) => {
                            if !stmt_pos && blk_susp(b, susp) {
                                diags.push(Diag::err(a.span, "check",
                                    "suspending call inside a value-position match — \
                                     use `match` as a statement or hoist the call"));
                            }
                            walk_block(b, susp, diags);
                        }
                    }
                }
                return;
            }
            match &e.kind {
                ExprKind::Call { f, args } => {
                    scan_match(f, susp, diags, false);
                    for a in args {
                        scan_match(a, susp, diags, false);
                    }
                }
                ExprKind::Bin { l, r, .. } => {
                    scan_match(l, susp, diags, false);
                    scan_match(r, susp, diags, false);
                }
                ExprKind::Un { e, .. } | ExprKind::As { e, .. } => {
                    scan_match(e, susp, diags, false)
                }
                ExprKind::Field { obj, .. } => scan_match(obj, susp, diags, false),
                ExprKind::StructLit { fields, .. } => {
                    for (_, fe) in fields {
                        scan_match(fe, susp, diags, false);
                    }
                }
                _ => {}
            }
        }
        fn walk_block(b: &Block, susp: &HashSet<String>, diags: &mut Diags) {
            for s in &b.stmts {
                match s {
                    Stmt::Defer { call, span } => {
                        if expr_has_susp(call, susp) {
                            diags.push(Diag::err(*span, "check",
                                "`defer` cannot capture a suspending call — it runs during unwind"));
                        }
                        scan_match(call, susp, diags, false);
                    }
                    Stmt::Expr(e) => scan_match(e, susp, diags, true),
                    Stmt::Let { init, .. } | Stmt::Assign { val: init, .. } => {
                        scan_match(init, susp, diags, false)
                    }
                    Stmt::Let2 { call, .. } | Stmt::Spawn { call, .. } => {
                        scan_match(call, susp, diags, false)
                    }
                    Stmt::Return { vals, .. } => {
                        for v in vals {
                            scan_match(v, susp, diags, false);
                        }
                    }
                    Stmt::If { cond, then, els, .. } => {
                        scan_match(cond, susp, diags, false);
                        walk_block(then, susp, diags);
                        if let Some(e) = els {
                            walk_block(e, susp, diags);
                        }
                    }
                    Stmt::For { cond, body, .. } => {
                        scan_match(cond, susp, diags, false);
                        walk_block(body, susp, diags);
                    }
                    Stmt::Scope { body, .. } | Stmt::Unsafe { body, .. } => {
                        walk_block(body, susp, diags)
                    }
                    Stmt::Select { arms, .. } => {
                        for a in arms {
                            match &a.kind {
                                SelKind::Recv { chan, .. } => {
                                    scan_match(chan, susp, diags, false)
                                }
                                SelKind::Timeout { ms, .. } => {
                                    scan_match(ms, susp, diags, false)
                                }
                                SelKind::Else { .. } => {}
                            }
                            walk_block(&a.body, susp, diags);
                        }
                    }
                    Stmt::Break { .. } | Stmt::Continue { .. } => {}
                }
            }
        }
        for it in &c.items {
            match it {
                Item::Fn(f) => {
                    if let Some(b) = &f.body {
                        walk_block(b, susp, self.diags);
                    }
                }
                Item::Global(g) => {
                    if expr_has_susp(&g.init, susp) {
                        self.diags.push(Diag::err(g.span, "check",
                            "global initializer cannot suspend — move it into fn main"));
                    }
                }
                _ => {}
            }
        }
    }
}

/// flatten `a.b.c` Field chains to a dotted string (enum/module paths)
fn field_path(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field { obj, name } => Some(format!("{}.{}", field_path(obj)?, name)),
        _ => None,
    }
}

pub fn subst_ty(t: &Ty, bind: &HashMap<String, Ty>) -> Ty {
    match t {
        Ty::Var(v) => bind.get(v).cloned().unwrap_or_else(|| t.clone()),
        Ty::Own(b) => Ty::Own(Box::new(subst_ty(b, bind))),
        Ty::Ref(b) => Ty::Ref(Box::new(subst_ty(b, bind))),
        Ty::Mut(b) => Ty::Mut(Box::new(subst_ty(b, bind))),
        _ => t.clone(),
    }
}

fn subst_ret(r: &Ret, bind: &HashMap<String, Ty>) -> Ret {
    match r {
        Ret::One(t) => Ret::One(subst_ty(t, bind)),
        Ret::Pair(t) => Ret::Pair(subst_ty(t, bind)),
        Ret::Void => Ret::Void,
    }
}
