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
    Slice(Box<Ty>),
    /// `fn(int, int) -> bool` — fn value ({env, fnptr}); ret Void = no `->`
    Fn(Vec<Ty>, Box<Ty>),
    Var(String), // generic type parameter (instantiated at call sites)
    /// atomic shared cell (mkshared) — copy/shareable, never move-tracked
    Shared,
    /// interface-typed value — {data, vtab} fat pair (D3)
    Iface(String),
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
            Ty::Slice(t) => format!("[]{}", t.name()),
            Ty::Fn(ps, r) => format!(
                "fn({}){}",
                ps.iter().map(|p| p.name()).collect::<Vec<_>>().join(", "),
                if **r == Ty::Void { String::new() } else { format!(" -> {}", r.name()) }),
            Ty::Var(n) => n.clone(),
            Ty::Shared => "shared".into(),
            Ty::Iface(n) => n.clone(),
            Ty::Unknown => "?".into(),
        }
    }
    fn is_word(&self) -> bool {
        matches!(self, Ty::Int | Ty::Bool | Ty::Chan | Ty::Own(_) | Ty::Shared)
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

/// one method decl inside an interface — signature only, no receiver
#[derive(Clone)]
struct IMethod {
    name: String,
    params: Vec<Ty>,
    ret: Ret,
}

#[derive(Clone, Default)]
struct IfaceDef {
    ops: Vec<String>,
    methods: Vec<IMethod>,
}

/// method-call resolution target (see `Checker::method_target`)
enum MTarget {
    /// direct call to the qualified `T.m` fn (receiver prepended)
    Fn(String),
    /// vtable dispatch — `I.m` key + the iface decl's params + ret
    Vt(String, Vec<Ty>, Ret),
    NoMethod,
}

/// one recorded use-site → its decl (E1 position index)
#[derive(Clone, Debug)]
pub struct UseRec {
    /// file the use-span indexes into
    pub file: String,
    pub span: Span,
    /// hover text (`fn f(x: int) -> str`, `x: own Point`, ...)
    pub sig: String,
    /// decl location — `None` for builtin/lang decls with no source
    pub def_file: String,
    pub def_span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    /// use-site records (use spans AND decl-name spans — hover works on both)
    pub uses: Vec<UseRec>,
}

pub struct Checked {
    pub suspend: HashSet<String>,
    pub index: Index,
}

struct Env {
    scopes: Vec<HashMap<String, Ty>>,
    /// shallow move tracking: a binding is marked moved when its `own`
    /// value flows somewhere ownership transfers (call arg, return,
    /// field store, channel send). Parallel to `scopes`.
    moved: Vec<HashMap<String, bool>>,
    /// E1: binding-site span per name — hover/goto on locals resolves
    /// to the visible binding's span. Parallel to `scopes`.
    decls: Vec<HashMap<String, Span>>,
}
impl Env {
    fn new() -> Self {
        Env {
            scopes: vec![HashMap::new()],
            moved: vec![HashMap::new()],
            decls: vec![HashMap::new()],
        }
    }
    fn push(&mut self) {
        self.scopes.push(HashMap::new());
        self.moved.push(HashMap::new());
        self.decls.push(HashMap::new());
    }
    fn pop(&mut self) {
        self.scopes.pop();
        self.moved.pop();
        self.decls.pop();
    }
    fn get(&self, n: &str) -> Option<Ty> {
        self.scopes.iter().rev().find_map(|s| s.get(n).cloned())
    }
    fn names(&self) -> HashSet<String> {
        self.scopes.iter().flat_map(|s| s.keys().cloned()).collect()
    }
    fn set(&mut self, n: String, t: Ty, span: Span) {
        self.scopes.last_mut().unwrap().insert(n.clone(), t);
        // a (re)binding starts un-moved — clears any stale bit on shadow
        self.moved.last_mut().unwrap().remove(&n);
        self.decls.last_mut().unwrap().insert(n, span);
    }
    /// span of the visible binding for `n` (innermost scope first)
    fn decl_span(&self, n: &str) -> Option<Span> {
        self.scope_of(n).and_then(|i| self.decls[i].get(n).copied())
    }
    fn scope_of(&self, n: &str) -> Option<usize> {
        (0..self.scopes.len()).rev().find(|&i| self.scopes[i].contains_key(n))
    }
    fn is_moved(&self, n: &str) -> bool {
        self.scope_of(n)
            .map(|i| self.moved[i].get(n).copied().unwrap_or(false))
            .unwrap_or(false)
    }
    fn mark_moved(&mut self, n: &str) {
        if let Some(i) = self.scope_of(n) {
            self.moved[i].insert(n.to_string(), true);
        }
    }
    fn clear_moved(&mut self, n: &str) {
        if let Some(i) = self.scope_of(n) {
            self.moved[i].remove(n);
        }
    }
    /// does control reach the end of this block? A trailing `return`,
    /// `break`/`continue`, or a trailing `if`/`scope` whose every path
    /// exits means the join below is unreachable via this block —
    /// moves on such paths must not merge into the joined state.
    fn block_falls_through(b: &Block) -> bool {
        b.stmts.last().map_or(true, Self::stmt_falls_through)
    }

    fn stmt_falls_through(s: &Stmt) -> bool {
        match s {
            Stmt::Return { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => false,
            // an `if` falls through when either branch can reach its end —
            // `if` without `else` always can (the not-taken path)
            Stmt::If { then, els, .. } => match els {
                Some(e) => Self::block_falls_through(then) || Self::block_falls_through(e),
                None => true,
            },
            Stmt::Scope { body, .. } => Self::block_falls_through(body),
            // `for`/`select`/`match`/`unsafe` bodies may run zero times or
            // exit via a path that still continues — treat as reachable
            _ => true,
        }
    }

    /// branch merge: keep a binding marked if it moved on ANY path —
    /// the join can't prove which branch ran, so a move in only one
    /// branch still forbids later reads. A branch that re-initializes
    /// the binding clears its mark on that path.
    fn moved_union(&mut self, other: &[HashMap<String, bool>]) {
        for (i, m) in self.moved.iter_mut().enumerate() {
            if let Some(o) = other.get(i) {
                for (k, v) in o.iter() {
                    if *v { m.insert(k.clone(), true); }
                }
            }
        }
    }
}

pub struct Checker<'a> {
    diags: &'a mut Diags,
    /// item-parallel origin files (spans index into their own file)
    files: &'a [String],
    /// file of the item currently being registered/checked
    cur_file: String,
    /// qualified decl name → (sig text, def file, def span)
    decls: HashMap<String, (String, String, Span)>,
    index: Index,
    fns: HashMap<String, FnSig>,
    structs: HashMap<String, Vec<(String, Ty)>>,
    /// enum name -> [(variant, fields)]; fields f0..fn for positional
    /// payloads. Variant payload structs also live in `structs` as "E.V".
    enums: HashMap<String, Vec<(String, Vec<(String, Ty)>)>>,
    ifaces: HashMap<String, IfaceDef>,
    /// qualified `T.m` names of method fns — for the susp-method ban
    method_spans: HashMap<String, Span>,
    cur_ret: Ret,
    calls: HashMap<String, HashSet<String>>, // caller -> callee fn names
    cur_fn: String,
    in_let2: bool,
    in_spawn: bool,
    ty_scope: HashSet<String>, // type params in scope (current generic fn)
    cur_ty_params: Vec<(String, String)>,
    loop_depth: i32,
    globals: HashMap<String, Ty>,
    // B4 deferred checks — susp isn't known until bodies are checked:
    /// named fns taken as values — validated against `suspend` post-pass
    fnval_uses: Vec<(String, Span)>,
    /// (span, callee idents inside closure, has structural susp seed)
    closure_checks: Vec<(Span, HashSet<String>, bool)>,
    /// Some(inferred) while checking a closure body — `return` feeds it
    closure_ret: Option<Option<Ty>>,
}

const BUILTINS: &[&str] = &[
    "print", "println", "mkchan", "send", "recv", "close", "sleep", "new",
    "mkslice", "push",
    "assert", "itoa", "argc", "argv", "len", "byteat", "substr", "find", "atoi",
    "mkbuf", "bufstr", "poke8", "poke16", "poke32", "htons", "htonl",
    "cstr", "str", "sqlite_open", "sqlite_exec", "sqlite_exec_bind",
    "sqlite_query", "sqlite_close", "set_rcvtimo",
    "readfile", "writefile", "exit", "mkshared", "sget", "sset", "scas", "sadd",
    "now", "now_ms", "usleep", "ftoi", "itof", "atof", "ftoa",
];

pub fn check(c: &Crate, files: &[String], diags: &mut Diags) -> Checked {
    let mut ck = Checker {
        diags,
        files,
        cur_file: String::new(),
        decls: HashMap::new(),
        index: Index::default(),
        fns: HashMap::new(),
        structs: HashMap::new(),
        enums: HashMap::new(),
        ifaces: HashMap::new(),
        method_spans: HashMap::new(),
        cur_ret: Ret::Void,
        calls: HashMap::new(),
        cur_fn: String::new(),
        in_let2: false,
        in_spawn: false,
        ty_scope: HashSet::new(),
        cur_ty_params: Vec::new(),
        loop_depth: 0,
        globals: HashMap::new(),
        fnval_uses: Vec::new(),
        closure_checks: Vec::new(),
        closure_ret: None,
    };
    ck.register(c);
    ck.check_items(c);
    let checked = Checked {
        suspend: ck.suspend_set(c),
        index: std::mem::take(&mut ck.index),
    };
    // B4 deferred: susp set only exists once call edges are collected
    for (n, sp) in &ck.fnval_uses {
        if checked.suspend.contains(n) {
            ck.diags.push(Diag::err(*sp, "type",
                format!("suspendable fn `{n}` can't be a value (v0.2)")));
        }
    }
    for (sp, callees, seeded) in &ck.closure_checks {
        if *seeded || callees.iter().any(|n| checked.suspend.contains(n)) {
            ck.diags.push(Diag::err(*sp, "type",
                "closures may not suspend — spawn/scope/select/recv/send/sleep inside `{ |..| }`".to_string()));
        }
    }
    // D3: methods can't suspend — vtable/indirect calls don't carry a
    // task frame. Checked post-hoc since `suspend` needs all bodies.
    for (m, sp) in &ck.method_spans {
        if checked.suspend.contains(m) {
            ck.diags.push(Diag::err(*sp, "check",
                format!("suspendable method `{m}` — methods may not suspend (v0.2)")));
        }
    }
    ck.check_suspend_rules(c, &checked.suspend);
    checked
}

impl<'a> Checker<'a> {
    /// file the i-th merged item's spans index into
    fn item_file(&self, i: usize) -> String {
        self.files.get(i).cloned().unwrap_or_else(|| self.diags.file.clone())
    }

    /// E1: record a decl — keyed for use-site lookups, plus a self-use so
    /// the decl name itself hovers/jumps (smallest-span wins at query time)
    fn rec_decl(&mut self, key: &str, span: Span, sig: String) {
        self.decls
            .insert(key.to_string(), (sig.clone(), self.cur_file.clone(), span));
        self.index.uses.push(UseRec {
            file: self.cur_file.clone(),
            span,
            sig,
            def_file: self.cur_file.clone(),
            def_span: span,
        });
    }

    /// the `.name` region of a callee/field expr (dotted uses point at the
    /// member, not the receiver): `x.m` → m's span, `f` → f's span.
    /// Call sites arrive flattened to `Ident("x.m")`; the source text at
    /// `f.span` IS the callee's name — the member starts after the last
    /// `.` and runs the last component's length. (span.hi can overrun
    /// into the call's `(`, so anchor at `lo`.)
    fn member_span(&self, f: &Expr) -> Span {
        match &f.kind {
            ExprKind::Field { obj, name } => {
                let lo = obj.span.hi + 1;
                Span::new(lo, lo + name.len() as u32)
            }
            ExprKind::Ident(n) => match n.rfind('.') {
                Some(d) => {
                    let lo = f.span.lo + (d + 1) as u32;
                    Span::new(lo, lo + (n.len() - d - 1) as u32)
                }
                None => f.span,
            },
            _ => f.span,
        }
    }

    /// E1: record a use-site → its decl (no-op for builtins/locals —
    /// those aren't `decls` keys)
    fn rec_use(&mut self, span: Span, key: &str) {
        if let Some((sig, f, sp)) = self.decls.get(key) {
            self.index.uses.push(UseRec {
                file: self.cur_file.clone(),
                span,
                sig: sig.clone(),
                def_file: f.clone(),
                def_span: *sp,
            });
        }
    }

    /// E1: record a local-binding use — sig is `name: ty`, def is the
    /// visible binding's span
    fn rec_local(&mut self, span: Span, name: &str, t: &Ty, env: &Env) {
        self.index.uses.push(UseRec {
            file: self.cur_file.clone(),
            span,
            sig: format!("{name}: {}", t.name()),
            def_file: self.cur_file.clone(),
            def_span: env.decl_span(name).unwrap_or(span),
        });
    }

    fn register(&mut self, c: &Crate) {
        // pass 0: struct/enum names so self-references and forward use
        // resolve (recursive payloads like `own Expr` inside `Expr`)
        for (i, it) in c.items.iter().enumerate() {
            self.cur_file = self.item_file(i);
            if let Item::Struct(s) = it {
                self.structs.insert(s.name.clone(), Vec::new());
            }
            if let Item::Enum(e) = it {
                self.enums.insert(e.name.clone(), Vec::new());
            }
            if let Item::Interface(i) = it {
                // method sigs resolve with an empty ty scope — interfaces
                // aren't generic
                let methods = i.methods.iter().map(|m| IMethod {
                    name: m.name.clone(),
                    params: m.params.iter().map(|p| self.resolve(&p.ty)).collect(),
                    ret: self.resolve_ret(&m.ret),
                }).collect();
                self.ifaces.insert(i.name.clone(), IfaceDef {
                    ops: i.ops.clone(), methods,
                });
                self.rec_decl(&i.name, i.span, format!("interface {}", i.name));
                for m in &i.methods {
                    let ps = m
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, ty_text(&p.ty)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let r = match &m.ret {
                        RetTy::Void => String::new(),
                        RetTy::One(t) => format!(" -> {}", ty_text(t)),
                        RetTy::Pair(t) => format!(" -> [{}, err]", ty_text(t)),
                    };
                    self.rec_decl(
                        &format!("{}.{}", i.name, m.name),
                        m.span,
                        format!("fn {}.{m_name}({ps}){r}", i.name, m_name = m.name),
                    );
                }
            }
        }
        for (i, it) in c.items.iter().enumerate() {
            self.cur_file = self.item_file(i);
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
                self.rec_decl(&e.name, e.span, format!("enum {}", e.name));
                for v in &e.variants {
                    let sig = match &v.payload {
                        VariantPayload::Unit => format!("{}.{}", e.name, v.name),
                        VariantPayload::Pos(ts) => format!(
                            "{}.{}({})",
                            e.name,
                            v.name,
                            ts.iter().map(ty_text).collect::<Vec<_>>().join(", ")
                        ),
                        VariantPayload::Named(_) => format!("{}.{} {{ .. }}", e.name, v.name),
                    };
                    self.rec_decl(&format!("{}.{}", e.name, v.name), v.span, sig);
                }
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
                    self.rec_decl(&s.name, s.span, format!("struct {}", s.name));
                    for f in &s.fields {
                        self.rec_decl(
                            &format!("{}.{}", s.name, f.name),
                            f.span,
                            format!("{}: {}", f.name, ty_text(&f.ty)),
                        );
                    }
                    self.structs.insert(s.name.clone(), fields);
            }
        }
        for (i, it) in c.items.iter().enumerate() {
            self.cur_file = self.item_file(i);
            match it {
                Item::Fn(f) | Item::Extern(f) => {
                    self.ty_scope = f.ty_params.iter().map(|(t, _)| t.clone()).collect();
                    let mut params: Vec<Ty> = Vec::new();
                    if let Some(r) = &f.recv {
                        let rt = self.resolve(&r.ty);
                        // the receiver base must be a declared struct/enum —
                        // `f.name` is already its `T.m` qualification
                        let base_ok = match &rt {
                            Ty::Struct(_) | Ty::Enum(_) => true,
                            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) =>
                                matches!(&**b, Ty::Struct(_) | Ty::Enum(_)),
                            _ => false,
                        };
                        if !base_ok {
                            self.diags.push(Diag::err(r.span, "check",
                                "method receiver must be a declared struct/enum type (optionally behind own/ref/mut)"));
                        }
                        self.method_spans.insert(f.name.clone(), r.span);
                        params.push(rt);
                    }
                    params.extend(f.params.iter().map(|p| self.resolve(&p.ty)));
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
                    self.rec_decl(&f.name, f.span, fn_sig_text(f));
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
                    self.rec_decl(&g.name, g.span, format!("{}: {}", g.name, t.name()));
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
                "shared" => Ty::Shared,
                s => {
                    if self.ty_scope.contains(s) {
                        Ty::Var(s.to_string())
                    } else if self.structs.contains_key(s) {
                        Ty::Struct(s.to_string())
                    } else if self.enums.contains_key(s) {
                        Ty::Enum(s.to_string())
                    } else if self.ifaces.contains_key(s) {
                        Ty::Iface(s.to_string())
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
            TyKind::Slice(t) => Ty::Slice(Box::new(self.resolve(t))),
            TyKind::Fn { params, ret } => Ty::Fn(
                params.iter().map(|p| self.resolve(p)).collect(),
                Box::new(ret.as_ref().map(|r| self.resolve(r)).unwrap_or(Ty::Void)),
            ),
        }
    }

    fn check_items(&mut self, c: &Crate) {
        for (i, it) in c.items.iter().enumerate() {
            self.cur_file = self.item_file(i);
            if let Item::Fn(f) = it {
                self.cur_fn = f.name.clone();
                let sig = self.fns[&f.name].clone();
                self.cur_ret = sig.ret.clone();
                self.ty_scope = f.ty_params.iter().map(|(t, _)| t.clone()).collect();
                self.cur_ty_params = f.ty_params.clone();
                self.loop_depth = 0;
                let mut env = Env::new();
                let pskip = if let Some(r) = &f.recv {
                    if let Some(t) = sig.params.first() {
                        env.set(r.name.clone(), t.clone(), r.span);
                    }
                    1
                } else { 0 };
                for (p, t) in f.params.iter().zip(sig.params.iter().skip(pskip)) {
                    env.set(p.name.clone(), t.clone(), p.span);
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
                env.set(name.clone(), t, *span);
            }
            Stmt::Let2 { a, b, call, span } => {
                self.in_let2 = true;
                self.expr(call, env);
                self.in_let2 = false;
                // call must produce [T, err]
                let val_ty = match self.call_ret(call, env) {
                    Some(Ret::Pair(t)) => t,
                    _ => {
                        self.diags.push(Diag::err(*span, "check",
                            "`[v, err] = f()` requires a call to a `[T, err]` function"));
                        Ty::Unknown
                    }
                };
                if let Bind::Name(n) = a {
                    env.set(n.clone(), val_ty, *span);
                }
                match b {
                    Bind::Name(n) => env.set(n.clone(), Ty::Err, *span),
                    Bind::Blank => self.diags.push(Diag::err(
                        *span, "check",
                        "cannot discard `err` — handle it or pass it up",
                    )),
                }
            }
            Stmt::Assign { target, val, span } => {
                // a bare-ident target is a write, not a use — assignment
                // to a moved-from binding re-initializes it
                let tt = match &target.kind {
                    ExprKind::Ident(n) => env
                        .get(n)
                        .or_else(|| self.globals.get(n).cloned())
                        .unwrap_or_else(|| self.expr(target, env)),
                    _ => self.expr(target, env),
                };
                let vt = self.expr(val, env);
                self.assignable(&tt, &vt, *span);
                // `x = y` re-initializes (aliasing walks like `s = s.next`
                // stay legal); `s.f = y`/`s[i] = y` hand the value away
                if !matches!(target.kind, ExprKind::Ident(_)) {
                    self.move_for(val, &tt, env);
                }
                if let ExprKind::Ident(n) = &target.kind {
                    env.clear_moved(n);
                }
            }
            Stmt::Expr(e) => {
                if let Some(Ret::Pair(_)) = self.call_ret(e, env) {
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
                // inside `{ |..| }` — feed the closure's inferred ret
                if self.closure_ret.is_some() {
                    if vals.len() > 1 {
                        self.diags.push(Diag::err(*span, "check",
                            "`[T, err]` returns aren't supported in closures"));
                        return;
                    }
                    let t = if vals.is_empty() {
                        Ty::Void
                    } else {
                        self.expr(&vals[0], env)
                    };
                    if matches!(t, Ty::Slice(_)) {
                        self.diags.push(Diag::err(vals[0].span, "check",
                            "`return` of `[]T` escapes its arena scope — return `own []T` (new(slice))"));
                    }
                    let prev = self.closure_ret.as_mut().unwrap();
                    match prev {
                        None => *prev = Some(t),
                        Some(w) => {
                            let w = w.clone();
                            self.assignable(&w, &t, *span);
                        }
                    }
                    return;
                }
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
                            self.move_for(&vals[0], want, env);
                            if matches!(want, Ty::Slice(_)) || matches!(t, Ty::Slice(_)) {
                                self.diags.push(Diag::err(vals[0].span, "check",
                                    "`return` of `[]T` escapes its arena scope — return `own []T` (new(slice))"));
                            }
                        }
                    }
                    Ret::Pair(want) => {
                        if vals.len() == 2 {
                            let t = self.expr(&vals[0], env);
                            self.assignable(want, &t, vals[0].span);
                            self.move_for(&vals[0], want, env);
                            if matches!(want, Ty::Slice(_)) || matches!(t, Ty::Slice(_)) {
                                self.diags.push(Diag::err(vals[0].span, "check",
                                    "`return` of `[]T` escapes its arena scope — return `own []T` (new(slice))"));
                            }
                            let e = self.expr(&vals[1], env);
                            self.assignable(&Ty::Err, &e, vals[1].span);
                        } else if vals.len() == 1 {
                            // `return f()` forwarding a [T, err] call
                            self.in_let2 = true;
                            self.expr(&vals[0], env);
                            self.in_let2 = false;
                            if let Some(Ret::Pair(t)) = self.call_ret(&vals[0], env) {
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
                // a move on either branch is possible after the join —
                // union the exits so only all-path re-init clears a mark.
                // Only exits that can REACH the join contribute: a branch
                // ending in return/break/continue can't have its moves
                // observed by later code
                let pre_moved = env.moved.clone();
                self.block(then, env);
                let then_moved = std::mem::replace(&mut env.moved, pre_moved);
                let then_ft = Env::block_falls_through(then);
                let els_ft = els.as_ref().map_or(true, Env::block_falls_through);
                if let Some(e) = els {
                    self.block(e, env);
                }
                if els.is_some() && !els_ft && then_ft {
                    env.moved = then_moved; // only the then path arrives
                } else if then_ft && (els.is_none() || els_ft) {
                    env.moved_union(&then_moved);
                }
                // else: only the else path (or nothing) arrives — the
                // else exit state is already in env.moved
            }
            Stmt::For { cond, body, span } => {
                // a move inside a loop leaves the next iteration reading a
                // moved value — unless the binding is re-initialized in the
                // same iteration (which clears the mark)
                let pre_moved = env.moved.clone();
                let t = self.expr(cond, env);
                self.assignable(&Ty::Bool, &t, cond.span);
                self.loop_depth += 1;
                self.block(body, env);
                self.loop_depth -= 1;
                for (i, m) in env.moved.iter().enumerate() {
                    for k in m.keys() {
                        if !pre_moved.get(i).and_then(|p| p.get(k)).copied().unwrap_or(false) {
                            self.diags.push(Diag::err(*span, "own",
                                format!("`{k}` moved inside `for` — re-assign it in the same iteration")));
                        }
                    }
                }
                env.moved = pre_moved;
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
                        self.in_spawn = true;
                        self.expr(call, env);
                        self.in_spawn = false;
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
                // arms are alternative paths — merge `moved` like if/else
                let pre_moved = env.moved.clone();
                let mut arm_moved = Vec::new();
                for arm in arms {
                    env.moved = pre_moved.clone();
                    env.push();
                    match &arm.kind {
                        SelKind::Recv { chan, bind, span } => {
                            let ct = self.expr(chan, env);
                            self.assignable(&Ty::Chan, &ct, chan.span);
                            if let Some(n) = bind {
                                env.set(n.clone(), Ty::Int, *span); // word payload (v0)
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
                    arm_moved.push((env.moved.clone(), Env::block_falls_through(&arm.body)));
                }
                // only arms that can reach the join contribute marks
                env.moved = pre_moved;
                for m in env.moved.iter_mut() {
                    m.clear();
                }
                for (st, ft) in &arm_moved {
                    if *ft {
                        env.moved_union(st);
                    }
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
            name => self.ifaces.get(name).map(|d| d.ops.iter().any(|o| o == op)).unwrap_or(false),
        };
        if !has {
            self.diags.push(Diag::err(sp, "type",
                format!("`{binop:?}` on `{var}` requires constraint `{op}` (has `{c}`)")));
        }
    }

    /// If `e` is a call to a declared fn, return its return type.
    fn call_ret(&mut self, e: &Expr, env: &mut Env) -> Option<Ret> {
        if let ExprKind::Call { f, args } = &e.kind {
            if let ExprKind::Ident(n) = &f.kind {
                if let Some(s) = self.fns.get(n) {
                    return Some(s.ret.clone());
                }
                // method / iface dispatch — same resolution as `call`
                if let Some((objp, mname)) = n.rsplit_once('.') {
                    if let Some(oe) = unflatten(objp, f.span) {
                        let ot = self.expr(&oe, env);
                        return match self.method_target(&ot, mname) {
                            MTarget::Fn(q) => self.fns.get(&q).map(|s| s.ret.clone()),
                            MTarget::Vt(_, _, r) => Some(r),
                            MTarget::NoMethod => None,
                        };
                    }
                } else if !BUILTINS.contains(&n.as_str()) {
                    if let Some(a0) = args.first() {
                        // speculative: the caller re-evaluates `e` after
                        // this — probe marks (e.g. a nested move) must
                        // not leak into the real check
                        let moved = env.moved.clone();
                        let t0 = self.expr(a0, env);
                        env.moved = moved;
                        if let MTarget::Fn(q) = self.method_target(&t0, n) {
                            return self.fns.get(&q).map(|s| s.ret.clone());
                        }
                    }
                }
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
            (Ty::Slice(w), Ty::Slice(g)) => self.bind_vars(w, g, bind),
            (Ty::Fn(wps, wr), Ty::Fn(gps, gr)) => {
                for (w, g) in wps.iter().zip(gps.iter()) {
                    self.bind_vars(w, g, bind);
                }
                self.bind_vars(wr, gr, bind);
            }
            _ => {}
        }
    }

    /// does `t` satisfy constraint `c` (builtin word or interface name)?
    /// A method-declaring interface requires the type's method set to
    /// cover it; ops keep their fixed vocabulary.
    fn satisfies(&self, t: &Ty, c: &str) -> bool {
        let op_ok = |op: &str, t: &Ty| match op {
            "eq" => matches!(t, Ty::Int | Ty::Bool | Ty::Str | Ty::Chan | Ty::Var(_)),
            "ordered" => matches!(t, Ty::Int | Ty::Var(_)),
            "add" => matches!(t, Ty::Int | Ty::Str | Ty::Var(_)),
            _ => false,
        };
        // a Var satisfies through its declared constraint only
        if let Ty::Var(v) = t {
            if let Some((_, vc)) = self.cur_ty_params.iter().find(|(n, _)| n == v) {
                if *vc == c { return true; }
                if self.ifaces.contains_key(vc) && self.ifaces.contains_key(c) {
                    return self.iface_covers(vc, c);
                }
                return self.ifaces.get(c)
                    .map(|d| d.methods.is_empty() && d.ops.iter().all(|o| op_ok(o, t)))
                    .unwrap_or(false);
            }
        }
        if let Ty::Iface(h) = t {
            if self.ifaces.contains_key(c) {
                return self.iface_covers(h, c);
            }
        }
        match c {
            "" | "any" => true,
            "eq" | "ordered" | "add" => op_ok(c, t),
            name => self.ifaces.get(name)
                .map(|d| {
                    d.ops.iter().all(|o| op_ok(o, t))
                        && (d.methods.is_empty()
                            || self.ty_base(t).map(|b| self.impl_satisfies(&b, name))
                                .unwrap_or(false))
                })
                .unwrap_or(false),
        }
    }

    /// base type name behind own/ref/mut — the name methods key on
    fn ty_base(&self, t: &Ty) -> Option<String> {
        match t {
            Ty::Struct(n) | Ty::Enum(n) => Some(n.clone()),
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => self.ty_base(b),
            _ => None,
        }
    }

    fn ret_eq(a: &Ret, b: &Ret) -> bool {
        match (a, b) {
            (Ret::Void, Ret::Void) => true,
            (Ret::One(x), Ret::One(y)) | (Ret::Pair(x), Ret::Pair(y)) => x == y,
            _ => false,
        }
    }

    /// iface `have` covers everything iface `want` declares
    fn iface_covers(&self, have: &str, want: &str) -> bool {
        let (Some(wdef), Some(hdef)) = (self.ifaces.get(want), self.ifaces.get(have)) else {
            return false;
        };
        wdef.ops.iter().all(|o| hdef.ops.contains(o))
            && wdef.methods.iter().all(|wm| hdef.methods.iter().any(|hm|
                hm.name == wm.name && hm.params == wm.params
                    && Self::ret_eq(&hm.ret, &wm.ret)))
    }

    /// concrete type `tname` carries every method iface `iname` declares —
    /// `T.m` must exist as a non-generic fn with matching sig behind the
    /// receiver slot
    fn impl_satisfies(&self, tname: &str, iname: &str) -> bool {
        let Some(def) = self.ifaces.get(iname) else { return false };
        def.methods.iter().all(|m| {
            let Some(sig) = self.fns.get(&format!("{tname}.{}", m.name)) else {
                return false;
            };
            if sig.is_extern || sig.params.len() != m.params.len() + 1 {
                return false;
            }
            if sig.ty_params.is_empty() {
                return sig.params[1..] == m.params[..] && Self::ret_eq(&sig.ret, &m.ret);
            }
            // generic impl method — the iface sig pins its ty params:
            // bind declared params onto the iface's, then require every
            // ty param bound and params+ret equal after subst
            let mut bind: HashMap<String, Ty> = HashMap::new();
            for (w, g) in sig.params[1..].iter().zip(m.params.iter()) {
                self.bind_vars(w, g, &mut bind);
            }
            if !sig.ty_params.iter().all(|(t, _)| bind.contains_key(t)) {
                return false;
            }
            sig.params[1..].iter().zip(m.params.iter())
                .all(|(w, g)| subst_ty(w, &bind) == *g)
                && match (&sig.ret, &m.ret) {
                    (Ret::Void, Ret::Void) => true,
                    (Ret::One(a), Ret::One(b)) | (Ret::Pair(a), Ret::Pair(b)) =>
                        subst_ty(a, &bind) == *b,
                    _ => false,
                }
        })
    }

    /// `want`/`got` where `got` is the receiver being adapted:
    /// value want derefs a pointer arg, pointer want refs a value arg
    fn recv_adapt_ty(&self, want: &Ty, got: &Ty) -> Ty {
        match (want, got) {
            (w @ (Ty::Struct(_) | Ty::Enum(_)), Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b))
                if self.ty_base(got).as_deref() == Some(w.name().as_str()) => w.clone(),
            (w @ (Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_)), Ty::Struct(_) | Ty::Enum(_))
                if self.ty_base(w).as_deref() == Some(got.name().as_str()) => w.clone(),
            _ => got.clone(),
        }
    }

    /// where a method lookup leads: a direct `T.m` fn, a vtable slot's
    /// declared sig (params + ret — receiver is the implicit data ptr),
    /// or nowhere
    fn method_target(&self, t: &Ty, m: &str) -> MTarget {
        match t {
            Ty::Struct(s) | Ty::Enum(s) => {
                let q = format!("{s}.{m}");
                if self.fns.contains_key(&q) {
                    MTarget::Fn(q)
                } else {
                    MTarget::NoMethod
                }
            }
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => self.method_target(b, m),
            Ty::Iface(w) => self.ifaces.get(w)
                .and_then(|d| d.methods.iter().find(|mm| mm.name == m))
                .map(|mm| MTarget::Vt(format!("{w}.{m}"), mm.params.clone(), mm.ret.clone()))
                .unwrap_or(MTarget::NoMethod),
            Ty::Var(v) => self.cur_ty_params.iter()
                .find(|(n, _)| n == v)
                .and_then(|(_, c)| self.ifaces.get(c).map(|d| (c, d)))
                .and_then(|(c, d)| d.methods.iter().find(|mm| mm.name == m)
                    .map(|mm| (c, mm)))
                .map(|(c, mm)| MTarget::Vt(format!("{c}.{m}"), mm.params.clone(), mm.ret.clone()))
                .unwrap_or(MTarget::NoMethod),
            _ => MTarget::NoMethod,
        }
    }

    /// field type of `name` on a struct(-ish) `t` — for the
    /// field-vs-method precedence check at call sites
    fn field_ty(&self, t: &Ty, name: &str) -> Option<Ty> {
        let sn = match t {
            Ty::Struct(n) => n,
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match &**b {
                Ty::Struct(n) => n,
                _ => return None,
            },
            _ => return None,
        };
        self.structs.get(sn)?.iter().find(|(n, _)| n == name).map(|(_, t)| t.clone())
    }

    /// `w.m(args)` through an iface vtable — the declared sig checks args
    /// (the receiver is the data ptr, not a visible arg)
    fn call_iface(&mut self, ps: &[Ty], ret: &Ret, args: &[Expr], sp: Span, env: &mut Env) -> Ty {
        if args.len() != ps.len() {
            self.diags.push(Diag::err(sp, "check",
                format!("method takes {} args, got {}", ps.len(), args.len())));
        }
        for (a, want) in args.iter().zip(ps.iter()) {
            let got = self.expr(a, env);
            self.assignable(want, &got, a.span);
            self.move_for(a, want, env);
        }
        match ret {
            Ret::Void => Ty::Void,
            Ret::One(t) => t.clone(),
            Ret::Pair(t) => {
                if !self.in_let2 {
                    self.diags.push(Diag::err(sp, "check",
                        "`[T, err]` result must be bound with `[v, err] =` or returned"));
                }
                t.clone()
            }
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
            // and the empty slice
            || (*got == Ty::Err && matches!(want, Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_) | Ty::Chan | Ty::Slice(_) | Ty::Shared))
            || matches!((want, got), (Ty::Ref(w), Ty::Own(g) | Ty::Mut(g) | Ty::Ref(g)) | (Ty::Mut(w), Ty::Own(g) | Ty::Mut(g)) if **w == **g)
            // an untyped owned cell (mkbuf, own ?) borrows into any ref/mut
            || (*got == Ty::Own(Box::new(Ty::Unknown)) && matches!(want, Ty::Ref(_) | Ty::Mut(_)))
            // interface-typed want: structural method-set satisfaction,
            // iface-to-iface coverage, a constrained Var, or nil
            || self.iface_ok(want, got);
        if !ok {
            self.diags.push(Diag::err(
                sp, "type",
                format!("expected `{}`, found `{}`", want.name(), got.name()),
            ));
        }
    }

    /// `got` flows into iface `want` — concrete impls need the method
    /// set, ifaces cover ifaces, constrained Vars forward their
    /// constraint, nil is the empty pair
    fn iface_ok(&self, want: &Ty, got: &Ty) -> bool {
        let Ty::Iface(w) = want else { return false };
        match got {
            Ty::Iface(g) => self.iface_covers(g, w),
            Ty::Var(v) => self.cur_ty_params.iter()
                .find(|(n, _)| n == v)
                .map(|(_, c)| c == w || self.iface_covers(c, w))
                .unwrap_or(false),
            Ty::Err => true,
            t => self.ty_base(t).map(|b| self.impl_satisfies(&b, w)).unwrap_or(false),
        }
    }

    /// `e` consumes an `own` binding — mark it moved. Reads were already
    /// use-checked by `expr()`, so a second use reports there, not here.
    fn move_own(&mut self, e: &Expr, env: &mut Env) {
        if let ExprKind::Ident(n) = &e.kind {
            if matches!(env.get(n), Some(Ty::Own(_))) {
                env.mark_moved(n);
            }
        }
    }
    /// move only when the destination wants ownership (param, field,
    /// ret) — an iface data ptr takes it too
    fn move_for(&mut self, e: &Expr, want: &Ty, env: &mut Env) {
        if matches!(want, Ty::Own(_) | Ty::Iface(_)) {
            self.move_own(e, env);
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
            self.move_for(ge, &wt, env);
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
                Some(t) => {
                    if env.get(n).is_some() {
                        self.rec_local(e.span, n, &t, env);
                    } else {
                        self.rec_use(e.span, n);
                    }
                    if env.is_moved(n) {
                        self.diags.push(Diag::err(e.span, "own",
                            format!("use of moved value `{n}`")));
                    }
                    t
                }
                None => {
                    if let Some(sig) = self.fns.get(n).cloned() {
                        self.rec_use(e.span, n);
                        // named fn as a value (B4) — restrictions:
                        // generics need a call site, [T,err] has no shared
                        // ret struct, susp fns can't cross the boundary
                        if !sig.ty_params.is_empty() {
                            self.diags.push(Diag::err(e.span, "check",
                                format!("generic fn `{n}` can't be a value — call it to instantiate")));
                        } else if matches!(sig.ret, Ret::Pair(_)) {
                            self.diags.push(Diag::err(e.span, "check",
                                format!("`{n}` returns `[T, err]` — pair returns can't be fn values (v0.2)")));
                        } else {
                            self.fnval_uses.push((n.clone(), e.span));
                            let r = match &sig.ret { Ret::One(t) => t.clone(), _ => Ty::Void };
                            return Ty::Fn(sig.params.clone(), Box::new(r));
                        }
                    } else if BUILTINS.contains(&n.as_str()) {
                        self.diags.push(Diag::err(e.span, "check", format!("`{n}` is a function — call it")));
                    } else {
                        self.diags.push(Diag::err(e.span, "check", format!("undefined `{n}`")));
                    }
                    Ty::Unknown
                }
            },
            ExprKind::Closure { params, body } => {
                let ptys: Vec<Ty> = params.iter().map(|p| self.resolve(&p.ty)).collect();
                let pnames: HashSet<String> =
                    params.iter().map(|p| p.name.clone()).collect();
                // copy-capture: a captured name may not be re-bound inside —
                // the generated env local would collide with the `let`
                let mut scan = BodyScan::default();
                scan_block(body, &mut scan);
                let outer = env.names();
                for n in scan.binds.iter() {
                    if !pnames.contains(n)
                        && outer.contains(n)
                        && scan.refs.contains(n)
                    {
                        self.diags.push(Diag::err(e.span, "check",
                            format!("closure local `{n}` shadows a captured variable — rename one")));
                    }
                }
                self.closure_checks.push((e.span, scan.callees, scan.seeded));
                let saved = std::mem::replace(&mut self.closure_ret, Some(None));
                env.push();
                for (p, t) in params.iter().zip(ptys.iter()) {
                    env.set(p.name.clone(), t.clone(), p.span);
                }
                self.block(body, env);
                env.pop();
                let inferred = std::mem::replace(&mut self.closure_ret, saved)
                    .flatten()
                    .unwrap_or(Ty::Void);
                Ty::Fn(ptys, Box::new(inferred))
            }
            ExprKind::Call { f, args } => self.call(f, args, e.span, env),
            ExprKind::Field { obj, name } => {
                // unit variant: `ExprKind.Nil` — obj is the enum name
                // (possibly a module path), not a value expression
                if let Some(on) = field_path(obj) {
                    if let Some(vars) = self.enums.get(&on).cloned() {
                        match vars.iter().find(|(n, _)| n == name) {
                            Some((_, fields)) if fields.is_empty() => {
                                self.rec_use(obj.span, &on);
                                self.rec_use(Span::new(obj.span.hi + 1, e.span.hi), &format!("{on}.{name}"));
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
                            .map(|(_, t)| {
                                self.rec_use(Span::new(obj.span.hi + 1, e.span.hi), &format!("{sn}.{name}"));
                                t.clone()
                            })
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
                self.rec_use(e.span, name);
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
                                    self.move_for(fe, &wt, env);
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
                if let TyKind::Named(n) = &ty.kind {
                    self.rec_use(ty.span, n);
                }
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
                        if matches!(lt, Ty::Fn(..)) || matches!(rt, Ty::Fn(..)) {
                            self.diags.push(Diag::err(e.span, "type",
                                "fn values aren't comparable (v0.2)"));
                        } else if (matches!(lt, Ty::Iface(_)) && rt != Ty::Err)
                            || (matches!(rt, Ty::Iface(_)) && lt != Ty::Err)
                        {
                            self.diags.push(Diag::err(e.span, "type",
                                "iface values aren't comparable except against nil (v0.2)"));
                        } else {
                            self.assignable(&lt, &rt, e.span);
                        }
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
                    BinOp::BitAnd | BinOp::BitOr | BinOp::Xor => {
                        // bit ops are int-only — no float promotion
                        self.assignable(&Ty::Int, &lt, l.span);
                        self.assignable(&Ty::Int, &rt, r.span);
                        Ty::Int
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
                // arms are alternative paths — merge `moved` like if/else
                let pre_moved = env.moved.clone();
                let mut arm_moved = Vec::new();
                for arm in arms {
                    env.moved = pre_moved.clone();
                    env.push();
                    match &arm.pat {
                        Pattern::Wild => wild = true,
                        Pattern::Bind(n) => {
                            wild = true;
                            env.set(n.clone(), st.clone(), arm.span);
                            // binding an `own` scrutinee moves it into the var
                            self.move_own(scrut, env);
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
                                            env.set(b.clone(), ft.clone(), arm.span);
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
                                            Some((_, ft)) => env.set(b.clone(), ft.clone(), arm.span),
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
                    let ft = match &arm.body {
                        MatchBody::Expr(_) => true,
                        MatchBody::Block(b) => Env::block_falls_through(b),
                    };
                    arm_moved.push((env.moved.clone(), ft));
                }
                // only arms that can reach the join contribute marks
                env.moved = pre_moved;
                for m in env.moved.iter_mut() {
                    m.clear();
                }
                for (st, ft) in &arm_moved {
                    if *ft {
                        env.moved_union(st);
                    }
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
            ExprKind::Index { obj, idx } => {
                let ot = self.expr(obj, env);
                let it = self.expr(idx, env);
                self.assignable(&Ty::Int, &it, idx.span);
                match &ot {
                    Ty::Slice(el) => (**el).clone(),
                    Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match &**b {
                        Ty::Slice(el) => (**el).clone(),
                        _ => {
                            self.diags.push(Diag::err(obj.span, "type",
                                format!("`[]` on `{}` — not a slice", ot.name())));
                            Ty::Unknown
                        }
                    },
                    Ty::Unknown => Ty::Unknown,
                    _ => {
                        self.diags.push(Diag::err(obj.span, "type",
                            format!("`[]` on `{}` — not a slice", ot.name())));
                        Ty::Unknown
                    }
                }
            }
            ExprKind::Slice { obj, lo, hi } => {
                let ot = self.expr(obj, env);
                for x in [lo, hi].into_iter().flatten() {
                    let xt = self.expr(x, env);
                    self.assignable(&Ty::Int, &xt, x.span);
                }
                match &ot {
                    Ty::Slice(_) => ot.clone(),
                    Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) if matches!(&**b, Ty::Slice(_)) => {
                        (**b).clone()
                    }
                    Ty::Unknown => Ty::Unknown,
                    _ => {
                        self.diags.push(Diag::err(obj.span, "type",
                            format!("`[..]` on `{}` — not a slice", ot.name())));
                        Ty::Unknown
                    }
                }
            }
            ExprKind::TyLit(_) => {
                self.diags.push(Diag::err(e.span, "type",
                    "type literal only allowed as `mkslice` element type"));
                Ty::Unknown
            }
        }
    }

    fn call(&mut self, f: &Expr, args: &[Expr], sp: Span, env: &mut Env) -> Ty {
        if let ExprKind::Ident(name) = &f.kind {
            // enum variant construction: `ExprKind.Int(7)` qualifies to
            // an Ident call on the dotted path
            if let Some((en, vn, fields)) = self.enum_variant(name) {
                self.rec_use(self.member_span(f), name);
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
            // a local fn value shadows a same-named fn or builtin
            if let Some(Ty::Fn(ps, r)) = env.get(name) {
                self.rec_local(f.span, name, &Ty::Fn(ps.clone(), r.clone()), env);
                return self.call_fnvalue(&ps, &r, args, sp, env);
            }
            // method dispatch (D3): `x.m(..)` resolves via the receiver's
            // type to `T.m` (a fn-typed field `x.m` wins); `w.m(..)` on an
            // iface or generic constraint vtab-dispatches; a bare `m(x,..)`
            // that isn't a fn retries as a method on args[0]'s type.
            let mut recv: Option<Expr> = None;
            let mut qname = name.clone();
            if !self.fns.contains_key(name) {
                if let Some((objp, mname)) = name.rsplit_once('.') {
                    if let Some(oe) = unflatten(objp, f.span) {
                        let ot = self.expr(&oe, env);
                        if ot == Ty::Unknown {
                            return Ty::Unknown;
                        }
                        if let Some(ft) = self.field_ty(&ot, mname) {
                            return match ft {
                                Ty::Fn(ps, r) => self.call_fnvalue(&ps, &r, args, sp, env),
                                _ => {
                                    self.diags.push(Diag::err(sp, "type",
                                        format!("`{ot0}.{mname}` is a field of `{ot}`, not callable",
                                            ot0 = self.ty_base(&ot).unwrap_or_else(|| ot.name()), ot = ot.name())));
                                    Ty::Unknown
                                }
                            };
                        }
                        match self.method_target(&ot, mname) {
                            MTarget::Fn(q) => {
                                recv = Some(oe);
                                qname = q;
                            }
                            MTarget::Vt(key, mps, mret) => {
                                self.rec_use(self.member_span(f), &key);
                                return self.call_iface(&mps, &mret, args, sp, env);
                            }
                            MTarget::NoMethod => {
                                self.diags.push(Diag::err(sp, "type",
                                    format!("no method `{mname}` on `{}`", ot.name())));
                                return Ty::Unknown;
                            }
                        }
                    }
                } else if !BUILTINS.contains(&name.as_str()) {
                    if let Some(a0) = args.first() {
                        // `m(x, ..)` — method sugar: receiver is args[0]
                        // already. Skip builtins: `new(Counter)`'s arg is
                        // a type name, not an expr — probing it would
                        // emit a spurious undefined-name diag. The arg
                        // is re-evaluated in the sig loop below, so its
                        // move marks must not leak out of the probe.
                        let moved = env.moved.clone();
                        let t0 = self.expr(a0, env);
                        env.moved = moved;
                        if let MTarget::Fn(q) = self.method_target(&t0, name) {
                            qname = q;
                        }
                    }
                }
            }
            let args2: Vec<Expr>;
            let args: &[Expr] = match &recv {
                Some(r) => {
                    args2 = std::iter::once(r.clone()).chain(args.iter().cloned()).collect();
                    &args2
                }
                None => args,
            };
            let name = qname.as_str();
            if self.fns.contains_key(name) {
                self.calls.entry(self.cur_fn.clone()).or_default().insert(name.to_string());
                self.rec_use(self.member_span(f), name);
            }
            match name {
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
                        self.move_own(&args[1], env);
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
                                self.rec_use(a.span, n);
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
                        match &t {
                            Ty::Str | Ty::Slice(_) => {}
                            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b)
                                if matches!(&**b, Ty::Slice(_)) => {}
                            Ty::Unknown => {}
                            _ => self.diags.push(Diag::err(a.span, "type",
                                format!("`len` takes str or slice — got `{}`", t.name()))),
                        }
                    }
                    return Ty::Int;
                }
                "mkslice" => {
                    // mkslice(T, n) — zeroed []T with len==cap==n
                    let et = match args.first() {
                        Some(a) => match &a.kind {
                            ExprKind::Ident(n) => self.resolve(&TyRef {
                                kind: TyKind::Named(n.clone()), span: a.span,
                            }),
                            ExprKind::TyLit(tr) => {
                                if let TyKind::Named(n) = &tr.kind {
                                    self.rec_use(tr.span, n);
                                }
                                self.resolve(tr)
                            }
                            _ => {
                                self.diags.push(Diag::err(a.span, "check",
                                    "`mkslice` takes a type name — mkslice(int, 8)"));
                                Ty::Unknown
                            }
                        },
                        None => {
                            self.diags.push(Diag::err(sp, "check",
                                "`mkslice` takes (T, n)"));
                            Ty::Unknown
                        }
                    };
                    match args.get(1) {
                        Some(n) => {
                            let nt = self.expr(n, env);
                            self.assignable(&Ty::Int, &nt, n.span);
                        }
                        None => self.diags.push(Diag::err(sp, "check",
                            "`mkslice` takes (T, n)")),
                    }
                    return Ty::Slice(Box::new(et));
                }
                "push" => {
                    // s = push(s, v) — returns the (possibly grown) header
                    let st = match args.first() {
                        Some(a) => self.expr(a, env),
                        None => Ty::Unknown,
                    };
                    match &st {
                        Ty::Slice(el) => {
                            if let Some(v) = args.get(1) {
                                let vt = self.expr(v, env);
                                self.assignable(el, &vt, v.span);
                                self.move_for(v, el, env);
                            } else {
                                self.diags.push(Diag::err(sp, "check",
                                    "`push` takes (slice, value)"));
                            }
                            return st.clone();
                        }
                        Ty::Unknown => return Ty::Unknown,
                        _ => {
                            self.diags.push(Diag::err(sp, "type",
                                format!("`push` on `{}` — not a slice", st.name())));
                            return Ty::Unknown;
                        }
                    }
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
                "str" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Str;
                }
                "sqlite_open" => {
                    for a in args {
                        self.expr(a, env);
                    }
                    return Ty::Own(Box::new(Ty::Unknown));
                }
                "sqlite_exec" | "sqlite_exec_bind" | "set_rcvtimo" => {
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
                    return if name == "writefile" { Ty::Int } else { Ty::Void };
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
                    return Ty::Shared;
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
                    if !matches!(t, Ty::Int | Ty::Bool | Ty::CStr | Ty::Str | Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_) | Ty::Chan | Ty::Shared | Ty::Fn(..) | Ty::Unknown) {
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
            for (i, (a, want)) in args.iter().zip(sig.params.iter()).enumerate() {
                let sp = self.in_spawn;
                self.in_spawn = false;
                let got = self.expr(a, env);
                self.in_spawn = sp;
                // extern fns get the raw C type; ref/mut params emit borrow checks
                let want = subst_ty(want, &bind);
                let got = if i == 0 && recv.is_some() {
                    // receiver slot: `*x`/`&x` adapt — `&` needs an lvalue
                    if matches!(want, Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_))
                        && matches!(got, Ty::Struct(_) | Ty::Enum(_))
                        && !is_lvalue(a)
                    {
                        self.diags.push(Diag::err(a.span, "type",
                            "pointer receiver on an rvalue — bind it first"));
                    }
                    self.recv_adapt_ty(&want, &got)
                } else {
                    got
                };
                self.assignable(&want, &got, a.span);
                self.move_for(a, &want, env);
                if sp && matches!(want, Ty::Slice(_)) {
                    self.diags.push(Diag::err(a.span, "check",
                        "`spawn` arg of `[]T` escapes into the task — arena backing dies with the scope; pass `own []T`"));
                }
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
        // non-Ident callee — indirect call through a fn value
        match self.expr(f, env) {
            Ty::Fn(ps, r) => self.call_fnvalue(&ps, &r, args, sp, env),
            Ty::Unknown => Ty::Unknown,
            t => {
                self.diags.push(Diag::err(sp, "type",
                    format!("call of non-function `{}`", t.name())));
                Ty::Unknown
            }
        }
    }

    /// `f(args)` where `f` is a `fn(...)` value — check arity + args
    fn call_fnvalue(&mut self, ps: &[Ty], r: &Ty, args: &[Expr], sp: Span, env: &mut Env) -> Ty {
        if args.len() != ps.len() {
            self.diags.push(Diag::err(sp, "check",
                format!("fn value takes {} args, got {}", ps.len(), args.len())));
        }
        for (a, want) in args.iter().zip(ps.iter()) {
            let sp = self.in_spawn;
            self.in_spawn = false;
            let got = self.expr(a, env);
            self.in_spawn = sp;
            self.assignable(want, &got, a.span);
            self.move_for(a, want, env);
            if sp && matches!(want, Ty::Slice(_)) {
                self.diags.push(Diag::err(a.span, "check",
                    "`spawn` arg of `[]T` escapes into the task — arena backing dies with the scope; pass `own []T`"));
            }
        }
        r.clone()
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
                ExprKind::Index { obj, idx } => expr_susp(obj) || expr_susp(idx),
                ExprKind::Slice { obj, lo, hi } => {
                    expr_susp(obj)
                        || lo.as_ref().map(|e| expr_susp(e)).unwrap_or(false)
                        || hi.as_ref().map(|e| expr_susp(e)).unwrap_or(false)
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
                ExprKind::Index { obj, idx } => {
                    expr_has_susp(obj, susp) || expr_has_susp(idx, susp)
                }
                ExprKind::Slice { obj, lo, hi } => {
                    expr_has_susp(obj, susp)
                        || lo.as_ref().map(|e| expr_has_susp(e, susp)).unwrap_or(false)
                        || hi.as_ref().map(|e| expr_has_susp(e, susp)).unwrap_or(false)
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
                ExprKind::Index { obj, idx } => {
                    scan_match(obj, susp, diags, false);
                    scan_match(idx, susp, diags, false);
                }
                ExprKind::Slice { obj, lo, hi } => {
                    scan_match(obj, susp, diags, false);
                    if let Some(x) = lo {
                        scan_match(x, susp, diags, false);
                    }
                    if let Some(x) = hi {
                        scan_match(x, susp, diags, false);
                    }
                }
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

/// can `&e` be taken? idents always; field/index chains when their
/// base is — anything else is an rvalue (`&(f().x)` isn't valid C)
fn is_lvalue(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ident(_) => true,
        ExprKind::Field { obj, .. } | ExprKind::Index { obj, .. } => is_lvalue(obj),
        _ => false,
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
        Ty::Slice(b) => Ty::Slice(Box::new(subst_ty(b, bind))),
        Ty::Fn(ps, r) => Ty::Fn(
            ps.iter().map(|p| subst_ty(p, bind)).collect(),
            Box::new(subst_ty(r, bind)),
        ),
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

/// one-pass body scan for closures (B4): every ident referenced
/// (`refs`), every name bound locally (`binds`), every ident used as a
/// call callee (`callees`), and whether a structural suspend seed
/// (scope/select/spawn or a susp-builtin call) appears (`seeded`).
#[derive(Default)]
pub(crate) struct BodyScan {
    pub refs: HashSet<String>,
    pub binds: HashSet<String>,
    pub callees: HashSet<String>,
    pub seeded: bool,
}

pub(crate) fn scan_block(b: &Block, s: &mut BodyScan) {
    for st in &b.stmts {
        scan_stmt(st, s);
    }
}

fn scan_stmt(st: &Stmt, s: &mut BodyScan) {
    match st {
        Stmt::Let { name, init, .. } => {
            s.binds.insert(name.clone());
            scan_expr(init, s);
        }
        Stmt::Let2 { a, b, call, .. } => {
            for bd in [a, b] {
                if let Bind::Name(n) = bd {
                    s.binds.insert(n.clone());
                }
            }
            scan_expr(call, s);
        }
        Stmt::Expr(e) => scan_expr(e, s),
        Stmt::Assign { target, val, .. } => {
            scan_expr(target, s);
            scan_expr(val, s);
        }
        Stmt::If { cond, then, els, .. } => {
            scan_expr(cond, s);
            scan_block(then, s);
            if let Some(e) = els {
                scan_block(e, s);
            }
        }
        Stmt::For { cond, body, .. } => {
            scan_expr(cond, s);
            scan_block(body, s);
        }
        Stmt::Scope { body, .. } => {
            s.seeded = true;
            scan_block(body, s);
        }
        Stmt::Unsafe { body, .. } => scan_block(body, s),
        Stmt::Spawn { call, .. } | Stmt::Defer { call, .. } => {
            s.seeded = true;
            scan_expr(call, s);
        }
        Stmt::Select { arms, .. } => {
            s.seeded = true;
            for a in arms {
                match &a.kind {
                    SelKind::Recv { chan, bind, .. } => {
                        scan_expr(chan, s);
                        if let Some(n) = bind {
                            s.binds.insert(n.clone());
                        }
                    }
                    SelKind::Timeout { ms, .. } => scan_expr(ms, s),
                    SelKind::Else { .. } => {}
                }
                scan_block(&a.body, s);
            }
        }
        Stmt::Return { vals, .. } => vals.iter().for_each(|v| scan_expr(v, s)),
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn scan_expr(e: &Expr, s: &mut BodyScan) {
    match &e.kind {
        ExprKind::Ident(n) => {
            s.refs.insert(n.clone());
        }
        ExprKind::Call { f, args } => {
            if let ExprKind::Ident(n) = &f.kind {
                s.callees.insert(n.clone());
                if matches!(n.as_str(), "recv" | "send" | "sleep" | "usleep") {
                    s.seeded = true;
                }
            } else {
                scan_expr(f, s);
            }
            args.iter().for_each(|a| scan_expr(a, s));
        }
        ExprKind::Bin { l, r, .. } => {
            scan_expr(l, s);
            scan_expr(r, s);
        }
        ExprKind::Un { e, .. } | ExprKind::As { e, .. } => scan_expr(e, s),
        ExprKind::Field { obj, .. } => scan_expr(obj, s),
        ExprKind::StructLit { fields, .. } => {
            fields.iter().for_each(|(_, fe)| scan_expr(fe, s))
        }
        ExprKind::Index { obj, idx } => {
            scan_expr(obj, s);
            scan_expr(idx, s);
        }
        ExprKind::Slice { obj, lo, hi } => {
            scan_expr(obj, s);
            if let Some(x) = lo {
                scan_expr(x, s);
            }
            if let Some(x) = hi {
                scan_expr(x, s);
            }
        }
        ExprKind::Match { e, arms } => {
            scan_expr(e, s);
            for a in arms {
                match &a.pat {
                    Pattern::Lit(le) => scan_expr(le, s),
                    Pattern::VarPos { binds, .. } | Pattern::VarNamed { binds, .. } => {
                        binds.iter().for_each(|n| {
                            s.binds.insert(n.clone());
                        })
                    }
                    Pattern::Bind(n) => {
                        s.binds.insert(n.clone());
                    }
                    _ => {}
                }
                match &a.body {
                    MatchBody::Expr(be) => scan_expr(be, s),
                    MatchBody::Block(b) => scan_block(b, s),
                }
            }
        }
        // nested closures contribute their own free refs — the outer
        // env must carry them so the inner env can be populated
        ExprKind::Closure { params, body } => {
            params.iter().for_each(|p| {
                s.binds.insert(p.name.clone());
            });
            scan_block(body, s);
        }
        _ => {}
    }
}
