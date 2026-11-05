use crate::ast::*;
use crate::check::{subst_ty, Ty, Ret};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

const RT_H: &str = include_str!("../../runtime/rt.h");
const RT_C: &str = include_str!("../../runtime/rt.c");

pub fn runtime_files() -> (&'static str, &'static str) {
    (RT_H, RT_C)
}

/// flatten `a.b.c` Field chains to a dotted string (enum/module paths)
fn field_path(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field { obj, name } => Some(format!("{}.{}", field_path(obj)?, name)),
        _ => None,
    }
}

/// oxl `mod.name` -> C identifier `mod_name`
fn cname(n: &str) -> String {
    n.replace('.', "_")
}

/// vtab symbol fragment for an impl key — `iface:W2` rewraps sanitize
/// to `iface_W2` since ':' is not a C identifier char (.oxl gvtkey)
fn vtkey(implkey: &str) -> String {
    match implkey.strip_prefix("iface:") {
        Some(w2) => format!("iface_{}", cname(w2)),
        None => cname(implkey),
    }
}

fn c_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// one declared iface method — sig only; the receiver is the
/// interface's implicit data ptr
#[derive(Clone)]
struct IMeth {
    name: String,
    params: Vec<Ty>,
    ret: crate::check::Ret,
}

/// how a resolved call emits: a direct `T.m` (optional prepended
/// receiver) or an iface vtab slot call
enum CallPlan {
    Fn { q: String, recv: Option<Expr> },
    Vt { obj: Expr, w: String, midx: usize, mname: String,
         params: Vec<Ty>, ret: crate::check::Ret },
    None,
}

enum MT {
    Fn(String),
    /// (slot index, declared params, declared ret)
    Vt(usize, Vec<Ty>, crate::check::Ret),
}

struct Gen<'a> {
    out: String,
    trampolines: String,
    inst_out: String,
    inst_protos: String,
    // B4: fn-value wrappers + closures
    fnv_wraps: std::collections::BTreeSet<String>,
    closure_protos: String,
    closure_bodies: String,
    closure_n: usize,
    tmp: usize,
    scope_id: usize,
    env: Vec<HashMap<String, Ty>>,
    structs: HashMap<String, Vec<(String, Ty)>>,
    /// enum name -> ordered variant names (tag = index)
    enums: HashMap<String, Vec<String>>,
    /// "E.V" -> payload field types (also mirrored into `structs`)
    enum_fields: HashMap<String, Vec<(String, Ty)>>,
    /// iface name -> declared methods in order (= vtab slot order)
    ifaces: HashMap<String, Vec<IMeth>>,
    /// (iface, impl) pairs materialized at conversion sites — impl is a
    /// struct/enum name or `iface:{w2}` for iface-to-iface rewraps
    vtab_uses: std::collections::BTreeSet<(String, String)>,
    fns: HashMap<String, (Vec<Ty>, crate::check::Ret)>,
    ty_params: HashMap<String, Vec<(String, String)>>,
    ast_fns: HashMap<String, &'a Fn>,
    insts: Vec<(String, Vec<(String, Ty)>)>,
    inst_done: std::collections::HashSet<String>,
    subst: HashMap<String, Ty>,
    cur_tyvars: std::collections::HashSet<String>,
    externs: std::collections::HashSet<String>,
    globals: HashMap<String, Ty>,
    cur_ret_pair: bool,
    /// T of the current fn's `-> T` / `-> [T, err]` — conversion target
    /// for `return v` values
    cur_ret_ty: Ty,
    cur_fn: String,
    /// fn-scope defer stack — calls run LIFO at each return / fn end
    defers: Vec<Expr>,
    test_mode: bool,
    /// suspend set from the checker — these fns compile to frames
    susp: HashSet<String>,
    /// inside a suspendable fn's driver right now
    in_frame: bool,
    /// scoped map: local name -> frame field (parallel to env)
    fslots: Vec<HashMap<String, String>>,
    /// match-pattern bind scopes for &self type lookups (expr_ty)
    bind_env: std::cell::RefCell<Vec<HashMap<String, Ty>>>,
    /// collected local field decls for the frame struct
    frame_decls: Vec<String>,
    /// RtScope fields (one per `scope {}` site in this fn)
    scope_decls: Vec<String>,
    /// indices into scope_decls of scopes open at this emission point
    fscope_stack: Vec<usize>,
    /// resume states used (for the switch prologue)
    state_labels: Vec<usize>,
    /// set by Stmt::ExitBoundary — the next Return skips its own
    /// drain+defers (they already ran at the boundary)
    exit_drained: bool,
    state_ctr: usize,
    label_ctr: usize,
    /// (continue-label, break-label, open-scope depth) per enclosing loop
    loop_labels: Vec<(String, String, usize)>,
    hoist_ctr: usize,
    /// frame struct decls + poll prototypes — before all poll bodies
    /// (a poll body may spawn a fn whose frame is declared later)
    frame_head: String,
    /// frame structs + poll drivers, placed before all fn bodies
    frame_out: String,
}

pub fn generate(c: &Crate, susp: &HashSet<String>) -> String {
    generate_mode(c, false, susp)
}

fn mangle_ty(t: &Ty) -> String {
    t.name().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect()
}

pub fn generate_mode(c: &Crate, test_mode: bool, susp: &HashSet<String>) -> String {
    let mut g = Gen {
        out: String::new(),
        trampolines: String::new(),
        inst_out: String::new(),
        inst_protos: String::new(),
        fnv_wraps: std::collections::BTreeSet::new(),
        closure_protos: String::new(),
        closure_bodies: String::new(),
        closure_n: 0,
        tmp: 0,
        scope_id: 0,
        env: vec![HashMap::new()],
        structs: HashMap::new(),
        enums: HashMap::new(),
        enum_fields: HashMap::new(),
        ifaces: HashMap::new(),
        vtab_uses: std::collections::BTreeSet::new(),
        fns: HashMap::new(),
        ty_params: HashMap::new(),
        ast_fns: HashMap::new(),
        insts: Vec::new(),
        inst_done: std::collections::HashSet::new(),
        subst: HashMap::new(),
        cur_tyvars: std::collections::HashSet::new(),
        externs: std::collections::HashSet::new(),
        globals: HashMap::new(),
        cur_ret_pair: false,
        cur_ret_ty: Ty::Void,
        cur_fn: String::new(),
        defers: Vec::new(),
        test_mode,
        susp: susp.clone(),
        in_frame: false,
        fslots: Vec::new(),
        bind_env: std::cell::RefCell::new(Vec::new()),
        frame_decls: Vec::new(),
        scope_decls: Vec::new(),
        fscope_stack: Vec::new(),
        state_labels: Vec::new(),
        exit_drained: false,
        state_ctr: 1,
        label_ctr: 0,
        loop_labels: Vec::new(),
        hoist_ctr: 0,
        frame_head: String::new(),
        frame_out: String::new(),
    };
    // enum names first: `ty_of_ref`/`enum_variant` need them regardless of
    // declaration order relative to fns and structs; iface decls likewise —
    // a param `w: Writer` can appear before the interface item
    for it in &c.items {
        match it {
            Item::Enum(e) => {
                g.enums.insert(e.name.clone(), e.variants.iter().map(|v| v.name.clone()).collect());
            }
            Item::Interface(i) => {
                let methods = i.methods.iter().map(|m| IMeth {
                    name: m.name.clone(),
                    params: m.params.iter().map(|p| g.ty_of_ref(&p.ty)).collect(),
                    ret: match &m.ret {
                        RetTy::Void => crate::check::Ret::Void,
                        RetTy::One(t) => crate::check::Ret::One(g.ty_of_ref(t)),
                        RetTy::Pair(t) => crate::check::Ret::Pair(g.ty_of_ref(t)),
                    },
                }).collect();
                g.ifaces.insert(i.name.clone(), methods);
            }
            _ => {}
        }
    }
    for it in &c.items {
        match it {
            Item::Struct(s) => {
                g.structs.insert(
                    s.name.clone(),
                    s.fields.iter().map(|f| (f.name.clone(), g.ty_of_ref(&f.ty))).collect(),
                );
            }
            Item::Enum(e) => {
                let mut vnames = Vec::new();
                for v in &e.variants {
                    let fields: Vec<(String, Ty)> = match &v.payload {
                        VariantPayload::Unit => Vec::new(),
                        VariantPayload::Pos(tys) => tys.iter().enumerate()
                            .map(|(i, t)| (format!("f{i}"), g.ty_of_ref(t))).collect(),
                        VariantPayload::Named(fs) => fs.iter()
                            .map(|(n, t)| (n.clone(), g.ty_of_ref(t))).collect(),
                    };
                    let key = format!("{}.{}", e.name, v.name);
                    g.enum_fields.insert(key.clone(), fields.clone());
                    g.structs.insert(key, fields);
                    vnames.push(v.name.clone());
                }
                g.enums.insert(e.name.clone(), vnames);
            }
            Item::Fn(f) | Item::Extern(f) => {
                g.cur_tyvars = f.ty_params.iter().map(|(t, _)| t.clone()).collect();
                let mut params: Vec<Ty> = Vec::new();
                if let Some(r) = &f.recv {
                    params.push(g.ty_of_ref(&r.ty));
                }
                params.extend(f.params.iter().map(|p| g.ty_of_ref(&p.ty)));
                let ret = match &f.ret {
                    RetTy::Void => crate::check::Ret::Void,
                    RetTy::One(t) => crate::check::Ret::One(g.ty_of_ref(t)),
                    RetTy::Pair(t) => crate::check::Ret::Pair(g.ty_of_ref(t)),
                };
                g.cur_tyvars.clear();
                if matches!(it, Item::Extern(_)) {
                    g.externs.insert(f.name.clone());
                } else {
                    g.ast_fns.insert(f.name.clone(), f);
                }
                if !f.ty_params.is_empty() {
                    g.ty_params.insert(f.name.clone(), f.ty_params.clone());
                }
                g.fns.insert(f.name.clone(), (params, ret));
            }
            Item::Global(gl) => {
                let t = match &gl.ty {
                    Some(tr) => g.ty_of_ref(tr),
                    None => g.expr_ty(&gl.init),
                };
                g.globals.insert(gl.name.clone(), t.clone());
                g.env[0].insert(gl.name.clone(), t);
            }
            _ => {}
        }
    }

    let mut out = String::from("#include \"rt.h\"\n\n");

    // enum defs first: variant payload structs + the tagged union —
    // a user struct or another variant may embed an enum by value
    for it in &c.items {
        if let Item::Enum(en) = it {
            let mut members = String::new();
            for (i, v) in en.variants.iter().enumerate() {
                let key = format!("{}.{}", en.name, v.name);
                let fields = g.enum_fields.get(&key).cloned().unwrap_or_default();
                if fields.is_empty() {
                    continue;
                }
                let _ = writeln!(out, "struct o_s_{} {{", cname(&key));
                for (fname, fty) in &fields {
                    let _ = writeln!(out, "    {} f_{};", g.cty(fty), fname);
                }
                let _ = writeln!(out, "}};");
                members.push_str(&format!(" struct o_s_{} v{i};", cname(&key)));
            }
            if members.is_empty() {
                members.push_str(" char _pad;");
            }
            let _ = writeln!(out,
                "struct o_en_{} {{ int tag; union {{{} }} u; }};\n", cname(&en.name), members);
        }
    }

    // struct defs
    for it in &c.items {
        if let Item::Struct(s) = it {
            let _ = writeln!(out, "struct o_s_{} {{", cname(&s.name));
            for f in &s.fields {
                let _ = writeln!(out, "    {} f_{};", g.cty(&g.ty_of_ref(&f.ty)), f.name);
            }
            let _ = writeln!(out, "}};\n");
        }
    }

    // [T, err] result structs must precede fn prototypes
    for it in &c.items {
        if let Item::Fn(f) = it {
            if f.ty_params.is_empty() {
                if let RetTy::Pair(t) = &f.ret {
                    let _ = writeln!(
                        out, "struct o_ret_{} {{ {} val; int64_t err; }};",
                        cname(&f.name), g.cty(&g.ty_of_ref(t))
                    );
                }
            }
        }
    }

    // iface vtab call ret structs for `-> [T, err]` methods
    for (w, ms) in &g.ifaces {
        for m in ms {
            if let crate::check::Ret::Pair(t) = &m.ret {
                let _ = writeln!(
                    out, "struct o_vret_{}_{} {{ {} val; int64_t err; }};",
                    cname(w), cname(&m.name), g.cty(t));
            }
        }
    }

    // file-scope globals (initialized in oxl_main prologue)
    for it in &c.items {
        if let Item::Global(gl) = it {
            let t = g.globals.get(&gl.name).cloned().unwrap_or(Ty::Int);
            let _ = writeln!(out, "static {} o_g_{};", g.cty(&t), cname(&gl.name));
        }
    }

    // fn prototypes + extern passthroughs (generic fns get none — they
    // only exist as instantiations)
    for it in &c.items {
        match it {
            Item::Fn(f) => {
                if f.ty_params.is_empty() {
                    g.proto(&mut out, f, false);
                }
            }
            Item::Extern(f) => g.proto(&mut out, f, true),
            _ => {}
        }
    }

    g.out = out;
    let body_start = g.out.len();
    for it in &c.items {
        if let Item::Fn(f) = it {
            if f.ty_params.is_empty() {
                g.func(f);
            }
        }
    }
    // monomorphized instantiations + vtab-seeded method insts —
    // co-fixpoint: bodies create conv sites (vtab uses), vtab thunks
    // need generic method instances
    loop {
        let mut progress = false;
        while let Some((fname, bind)) = g.insts.pop() {
            let mangled = inst_name(&fname, &bind);
            if !g.inst_done.insert(mangled.clone()) {
                continue;
            }
            progress = true;
            if let Some(f) = g.ast_fns.get(&fname).cloned() {
                g.func_inst(f, &mangled, &bind);
            }
        }
        progress |= g.seed_vtab_insts();
        if !progress {
            break;
        }
    }
    // concrete vtabs + receiver-normalizing thunks — after every body
    // so vtab_uses is final
    let mut vtab_out = String::new();
    g.emit_vtabs(&mut vtab_out);
    // B4: zero-env wrappers giving named fns the (void* env, args...)
    // calling convention every fn value shares
    for n in g.fnv_wraps.clone() {
        if let Some((ps, ret)) = g.fns.get(&n).cloned() {
            let ret_c = match &ret {
                crate::check::Ret::One(t) => g.cty(t),
                _ => "void".into(),
            };
            let mut psig = String::new();
            for (i, p) in ps.iter().enumerate() {
                psig += &format!(", {} a{i}", g.cty(p));
            }
            let argl: Vec<String> = (0..ps.len()).map(|i| format!("a{i}")).collect();
            let callee = if g.externs.contains(&n) {
                n.clone()
            } else {
                format!("o_f_{}", cname(&n))
            };
            let ret_kw = if ret_c == "void" { "" } else { "return " };
            let _ = writeln!(g.inst_protos,
                "static {ret_c} o_fnv_wrap_{}(void* _e{psig}) {{ {ret_kw}{callee}({}); }}",
                cname(&n), argl.join(", "));
        }
    }
    // global initializers run first inside oxl_main (base scope exists)
    let mut global_inits = String::new();
    for it in &c.items {
        if let Item::Global(gl) = it {
            let gt = g.globals.get(&gl.name).cloned().unwrap_or(Ty::Unknown);
            let v = g.conv(&gl.init, &gt);
            let _ = writeln!(global_inits, "    o_g_{} = {v};", cname(&gl.name));
        }
    }
    if test_mode {
        let _ = writeln!(g.out, "void oxl_main(void) {{");
        g.out.push_str(&global_inits);
        let mut id = 0usize;
        for it in &c.items {
            if let Item::Fn(f) = it {
                if f.name.starts_with("test_") {
                    if g.susp.contains(&f.name) {
                        let _ = writeln!(g.out,
                            "    {{ RtScope _t{id}; rt_scope_enter(&_t{id});\n      struct o_frm_{cn} *_fr = (struct o_frm_{cn} *)rt_task_new(o_poll_{cn}, (int64_t)sizeof(struct o_frm_{cn}));\n      rt_run_root((RtFrame *)_fr);\n      int _e{id} = rt_scope_try(&_t{id});\n      rt_print_str(rt_str(\"  {n} ... \")); rt_println_str(rt_str(_e{id} ? \"FAIL\" : \"ok\")); }}",
                            cn = cname(&f.name), n = f.name, id = id);
                    } else {
                        let _ = writeln!(g.out,
                            "    {{ RtScope _t{id}; rt_scope_enter(&_t{id}); o_f_{cn}(); int _e{id} = rt_scope_try(&_t{id});\n      rt_print_str(rt_str(\"  {n} ... \")); rt_println_str(rt_str(_e{id} ? \"FAIL\" : \"ok\")); }}",
                            cn = cname(&f.name), n = f.name, id = id);
                    }
                    id += 1;
                }
            }
        }
        let _ = writeln!(g.out, "}}\n");
    }

    // trampolines + frame decls must precede the fn bodies that use them
    g.out = g.out.replace("/*GLOBALS*/", &global_inits);
    // inst protos precede every body (regular, inst, and frame poll fns);
    // closure protos likewise — closure wrapper bodies go last since
    // they only reference protos
    let mut final_out = g.out[..body_start].to_string();
    final_out.push_str(&g.trampolines);
    final_out.push_str(&g.frame_head);
    final_out.push_str(&g.inst_protos);
    final_out.push_str(&g.closure_protos);
    final_out.push_str(&vtab_out);
    final_out.push_str(&g.inst_out);
    final_out.push_str(&g.frame_out);
    final_out.push_str(&g.out[body_start..]);
    final_out.push_str(&g.closure_bodies);
    final_out
}

/// params with the method receiver prepended — `fn (r T) m` emits its
/// C fn with `r` as arg 0
fn all_params(f: &Fn) -> Vec<&Param> {
    f.recv.iter().chain(f.params.iter()).collect()
}

/// env-bound idents referenced by a deferred call, first-seen order —
/// these get captured by value into the site's env struct. Globals,
/// fn names and type params are not captured (they outlive the call).
fn defer_caps(g: &Gen, call: &Expr) -> Vec<(String, Ty)> {
    fn walk(e: &Expr, g: &Gen, out: &mut Vec<(String, Ty)>) {
        match &e.kind {
            ExprKind::Ident(n) => {
                if g.env.iter().skip(1).any(|s| s.contains_key(n))
                    || g.fslots.iter().any(|s| s.contains_key(n))
                {
                    if !g.globals.contains_key(n)
                        && !out.iter().any(|(m, _)| m == n)
                    {
                        out.push((n.clone(), g.lookup(n)));
                    }
                }
            }
            ExprKind::Call { f, args } => {
                walk(f, g, out);
                for a in args {
                    walk(a, g, out);
                }
            }
            ExprKind::Bin { l, r, .. } => {
                walk(l, g, out);
                walk(r, g, out);
            }
            ExprKind::Un { e, .. } | ExprKind::As { e, .. } => walk(e, g, out),
            ExprKind::Field { obj, .. } => walk(obj, g, out),
            ExprKind::Index { obj, idx } => {
                walk(obj, g, out);
                walk(idx, g, out);
            }
            ExprKind::Slice { obj, lo, hi } => {
                walk(obj, g, out);
                if let Some(x) = lo {
                    walk(x, g, out);
                }
                if let Some(x) = hi {
                    walk(x, g, out);
                }
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, e) in fields {
                    walk(e, g, out);
                }
            }
            ExprKind::Match { e, arms } => {
                walk(e, g, out);
                for a in arms {
                    match &a.body {
                        MatchBody::Expr(be) => walk(be, g, out),
                        MatchBody::Block(b) => {
                            for s in &b.stmts {
                                if let Stmt::Expr(e) | Stmt::Let { init: e, .. }
                                | Stmt::Assign { val: e, .. } = s
                                {
                                    walk(e, g, out);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(call, g, &mut out);
    out
}

fn inst_name(fname: &str, bind: &[(String, Ty)]) -> String {
    let mut n = fname.to_string();
    for (_, t) in bind {
        n.push_str("__");
        n.push_str(&mangle_ty(t));
    }
    n
}

/// bind generic param type `want` against concrete `got`
fn bind_generic(want: &Ty, got: &Ty, bind: &mut HashMap<String, Ty>) {
    match (want, got) {
        (Ty::Var(v), g) => {
            bind.entry(v.clone()).or_insert_with(|| g.clone());
        }
        (Ty::Own(w), Ty::Own(g)) | (Ty::Ref(w), Ty::Ref(g)) | (Ty::Mut(w), Ty::Mut(g)) => {
            bind_generic(w, g, bind)
        }
        (Ty::Ref(w) | Ty::Mut(w), Ty::Own(g) | Ty::Mut(g) | Ty::Ref(g)) => {
            bind_generic(w, g, bind)
        }
        (Ty::Slice(w), Ty::Slice(g)) => bind_generic(w, g, bind),
        (Ty::Fn(wps, wr), Ty::Fn(gps, gr)) => {
            for (w, g) in wps.iter().zip(gps.iter()) {
                bind_generic(w, g, bind);
            }
            bind_generic(wr, gr, bind);
        }
        _ => {}
    }
}

/// ordered bindings for a call to `name` with arg exprs `args`
fn call_bindings(g: &Gen, name: &str, args: &[Expr]) -> Option<Vec<(String, Ty)>> {
    let tp = g.ty_params.get(name)?;
    let (params, _) = g.fns.get(name)?;
    let mut bind = HashMap::new();
    for (a, want) in args.iter().zip(params.iter()) {
        let got = g.expr_ty(a);
        bind_generic(want, &got, &mut bind);
    }
    Some(tp.iter().map(|(t, _)| (t.clone(), bind.get(t).cloned().unwrap_or(Ty::Int))).collect())
}

impl<'a> Gen<'a> {
    fn ty_of_ref(&self, t: &TyRef) -> Ty {
        match &t.kind {
            TyKind::Named(n) => match n.as_str() {
                "int" => Ty::Int,
                "float" => Ty::Float,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "cstr" => Ty::CStr,
                "err" => Ty::Err,
                "void" => Ty::Void,
                "shared" => Ty::Shared,
                s => {
                    if let Some(b) = self.subst.get(s) {
                        b.clone()
                    } else if self.cur_tyvars.contains(s) {
                        Ty::Var(s.to_string())
                    } else if self.enums.contains_key(s) {
                        Ty::Enum(s.to_string())
                    } else if self.ifaces.contains_key(s) {
                        Ty::Iface(s.to_string())
                    } else {
                        Ty::Struct(s.to_string())
                    }
                }
            },
            TyKind::Own(t) => Ty::Own(Box::new(self.ty_of_ref(t))),
            TyKind::Ref(t) => Ty::Ref(Box::new(self.ty_of_ref(t))),
            TyKind::Mut(t) => Ty::Mut(Box::new(self.ty_of_ref(t))),
            TyKind::Chan => Ty::Chan,
            TyKind::Slice(t) => Ty::Slice(Box::new(self.ty_of_ref(t))),
            TyKind::Fn { params, ret } => Ty::Fn(
                params.iter().map(|p| self.ty_of_ref(p)).collect(),
                Box::new(ret.as_ref().map(|r| self.ty_of_ref(r)).unwrap_or(Ty::Void)),
            ),
        }
    }

    fn cty(&self, t: &Ty) -> String {
        match t {
            Ty::Int | Ty::Err => "int64_t".into(),
            Ty::Float => "double".into(),
            Ty::Bool => "int".into(),
            Ty::Str => "OxlStr".into(),
            Ty::CStr => "const char*".into(),
            Ty::Void => "void".into(),
            Ty::Chan => "RtChan*".into(),
            Ty::Shared => "RtShared*".into(),
            Ty::Struct(n) => format!("struct o_s_{}", cname(n)),
            Ty::Enum(n) => format!("struct o_en_{}", cname(n)),
            Ty::Slice(_) => "RtSlice".into(),
            Ty::Fn(..) => "RtFnv".into(),
            Ty::Iface(_) => "RtIface".into(),
            Ty::Var(v) => self
                .subst
                .get(v)
                .map(|t| self.cty(t))
                .unwrap_or_else(|| "int64_t".into()),
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match &**b {
                Ty::Struct(n) => format!("struct o_s_{}*", cname(n)),
                Ty::Enum(n) => format!("struct o_en_{}*", cname(n)),
                Ty::Slice(_) => "RtSlice*".into(),
                Ty::Var(v) => match self.subst.get(v) {
                    Some(Ty::Struct(n)) => format!("struct o_s_{}*", cname(n)),
                    Some(Ty::Slice(_)) => "RtSlice*".into(),
                    _ => "void*".into(),
                },
                _ => "void*".into(),
            },
            Ty::Unknown => "int64_t".into(),
        }
    }

    fn pty(&self, t: &Ty) -> String {
        self.cty(t)
    }

    fn proto(&self, out: &mut String, f: &Fn, is_extern: bool) {
        let (name, ret_c) = if is_extern {
            (f.name.clone(), self.ret_decl(f, false))
        } else {
            (format!("o_f_{}", cname(&f.name)), self.ret_decl(f, true))
        };
        let params: Vec<String> = all_params(f)
            .iter()
            .map(|p| format!("{} {}", self.pty(&self.ty_of_ref(&p.ty)), p.name))
            .collect();
        let params = if params.is_empty() { "void".into() } else { params.join(", ") };
        let _ = writeln!(out, "{ret_c} {name}({params});");
    }

    fn ret_decl(&self, f: &Fn, is_oxl: bool) -> String {
        match &f.ret {
            RetTy::Void => "void".into(),
            RetTy::One(t) => self.pty(&self.ty_of_ref(t)),
            RetTy::Pair(_) => {
                if is_oxl {
                    format!("struct o_ret_{}", cname(&f.name))
                } else {
                    "int64_t".into()
                }
            }
        }
    }

    fn func(&mut self, f: &Fn) {
        if self.susp.contains(&f.name) {
            self.func_frame(f, &f.name.clone());
            if f.name == "main" && !self.test_mode {
                let _ = writeln!(self.out,
                    "void oxl_main(void) {{ /*GLOBALS*/ struct o_frm_main *_rt = (struct o_frm_main *)rt_task_new(o_poll_main, (int64_t)sizeof(struct o_frm_main));\n    rt_run_root((RtFrame *)_rt); }}\n");
            }
            return;
        }
        let ret_c = self.ret_decl(f, true);
        let params: Vec<String> = all_params(f)
            .iter()
            .map(|p| format!("{} {}", self.pty(&self.ty_of_ref(&p.ty)), p.name))
            .collect();
        let params = if params.is_empty() { "void".into() } else { params.join(", ") };
        let _ = writeln!(self.out, "{ret_c} o_f_{}({params}) {{", cname(&f.name));
        self.cur_fn = f.name.clone();
        self.cur_ret_pair = matches!(f.ret, RetTy::Pair(_));
        self.cur_ret_ty = match &f.ret {
            RetTy::One(t) | RetTy::Pair(t) => self.ty_of_ref(t),
            RetTy::Void => Ty::Void,
        };
        self.defers.clear();
        self.env.push(HashMap::new());
        let _ = writeln!(self.out, "    RtDefer *_dmark = rt_defer_mark();");
        for p in all_params(f) {
            let t = self.ty_of_ref(&p.ty);
            self.env.last_mut().unwrap().insert(p.name.clone(), t);
        }
        if let Some(b) = &f.body {
            for s in &b.stmts {
                self.stmt(s);
            }
        }
        self.emit_defers();
        self.env.pop();
        let _ = writeln!(self.out, "}}\n");
        if f.name == "main" && !self.test_mode {
            let _ = writeln!(self.out, "void oxl_main(void) {{ /*GLOBALS*/ o_f_main(); }}\n");
        }
    }

    /// emit a monomorphized copy of generic fn `f` as `o_f_{mangled}`
    fn func_inst(&mut self, f: &Fn, mangled: &str, bind: &[(String, Ty)]) {
        if self.susp.contains(&f.name) {
            let saved_subst = std::mem::replace(
                &mut self.subst,
                bind.iter().cloned().collect::<HashMap<_, _>>(),
            );
            if let RetTy::Pair(t) = &f.ret {
                let _ = writeln!(
                    self.frame_head,
                    "struct o_ret_{} {{ {} val; int64_t err; }};",
                    cname(&mangled), self.cty(&self.ty_of_ref(t))
                );
            }
            self.func_frame(f, mangled);
            self.subst = saved_subst;
            return;
        }
        let saved_subst = std::mem::replace(
            &mut self.subst,
            bind.iter().cloned().collect::<HashMap<_, _>>(),
        );
        let saved_out = std::mem::take(&mut self.out);
        // ret struct (if [T, err]) + prototype — inst_protos lands
        // ahead of ALL bodies so frame poll fns and other insts can
        // call this instantiation before its definition
        if let RetTy::Pair(t) = &f.ret {
            let _ = writeln!(
                self.inst_protos,
                "struct o_ret_{} {{ {} val; int64_t err; }};",
                cname(&mangled), self.cty(&self.ty_of_ref(t))
            );
        }
        let mut proto = String::new();
        self.proto_named(&mut proto, f, mangled);
        self.inst_protos.push_str(&proto);
        // body — emitted into the swapped-out self.out (stmt() writes there)
        let ret_c = self.ret_decl_named(f, mangled);
        let params: Vec<String> = all_params(f)
            .iter()
            .map(|p| format!("{} {}", self.pty(&self.ty_of_ref(&p.ty)), p.name))
            .collect();
        let params = if params.is_empty() { "void".into() } else { params.join(", ") };
        let _ = writeln!(self.out, "{ret_c} o_f_{}({params}) {{", cname(&mangled));
        self.cur_fn = mangled.to_string();
        self.cur_ret_pair = matches!(f.ret, RetTy::Pair(_));
        self.cur_ret_ty = match &f.ret {
            RetTy::One(t) | RetTy::Pair(t) => self.ty_of_ref(t),
            RetTy::Void => Ty::Void,
        };
        self.env.push(HashMap::new());
        let _ = writeln!(self.out, "    RtDefer *_dmark = rt_defer_mark();");
        for p in all_params(f) {
            let t = self.ty_of_ref(&p.ty);
            self.env.last_mut().unwrap().insert(p.name.clone(), t);
        }
        if let Some(b) = &f.body {
            for s in &b.stmts {
                self.stmt(s);
            }
        }
        self.emit_defers();
        self.env.pop();
        let _ = writeln!(self.out, "}}\n");
        self.inst_out.push_str(&self.out);
        self.out = saved_out;
        self.subst = saved_subst;
    }

    fn proto_named(&self, out: &mut String, f: &Fn, name: &str) {
        let ret_c = self.ret_decl_named(f, name);
        let params: Vec<String> = all_params(f)
            .iter()
            .map(|p| format!("{} {}", self.pty(&self.ty_of_ref(&p.ty)), p.name))
            .collect();
        let params = if params.is_empty() { "void".into() } else { params.join(", ") };
        let _ = writeln!(out, "{ret_c} o_f_{}({params});", cname(&name));
    }

    fn ret_decl_named(&self, f: &Fn, name: &str) -> String {
        match &f.ret {
            RetTy::Void => "void".into(),
            RetTy::One(t) => self.pty(&self.ty_of_ref(t)),
            RetTy::Pair(_) => format!("struct o_ret_{}", cname(&name)),
        }
    }

    fn tmp(&mut self) -> usize {
        self.tmp += 1;
        self.tmp
    }

    fn lookup(&self, n: &str) -> Ty {
        if let Some(t) = self.bind_env.borrow().iter().rev().find_map(|s| s.get(n)) {
            return t.clone();
        }
        self.env.iter().rev().find_map(|s| s.get(n).cloned()).unwrap_or(Ty::Unknown)
    }

    /// resolve a Ty::Var through the active instantiation bindings
    fn dt(&self, t: &Ty) -> Ty {
        match t {
            Ty::Var(v) => self.subst.get(v).map(|t| self.dt(t)).unwrap_or_else(|| t.clone()),
            _ => t.clone(),
        }
    }

    fn expr_ty(&self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::TyLit(_) => Ty::Void,
            ExprKind::Int(_) => Ty::Int,
            ExprKind::Float(_) => Ty::Float,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Nil => Ty::Err,
            ExprKind::Ident(n) => {
                let t = self.lookup(n);
                if t != Ty::Unknown {
                    t
                } else {
                    // named fn as a value — {env=0, wrapper} pair
                    match self.fns.get(n) {
                        Some((ps, ret)) if !self.ty_params.contains_key(n) => Ty::Fn(
                            ps.clone(),
                            Box::new(match ret {
                                crate::check::Ret::One(t) => t.clone(),
                                _ => Ty::Void,
                            }),
                        ),
                        _ => Ty::Unknown,
                    }
                }
            }
            ExprKind::Call { f, args } => {
                // indirect call through a fn value → its ret type
                if let Ty::Fn(_, r) = self.fn_callee_ty(f) {
                    return *r;
                }
                if let ExprKind::Ident(n) = &f.kind {
                    // enum variant ctor → the enum type; `.kind()` → int
                    if let Some((en, _, _)) = self.enum_variant(n) {
                        return Ty::Enum(en.to_string());
                    }
                    if let Some((obj, "kind")) = n.rsplit_once('.') {
                        if matches!(self.expr_ty(&Expr {
                            kind: ExprKind::Ident(obj.to_string()), span: e.span,
                        }), Ty::Enum(_)) {
                            return Ty::Int;
                        }
                    }
                    if self.fns.get(n).is_none() {
                        // method/iface call — declared sig from the plan
                        match self.call_plan(f, args) {
                            CallPlan::Vt { ret, .. } => return match ret {
                                crate::check::Ret::One(t) | crate::check::Ret::Pair(t) => t,
                                crate::check::Ret::Void => Ty::Void,
                            },
                            CallPlan::Fn { q, recv } => {
                                if let Some((_, r)) = self.fns.get(&q) {
                                    let eff: Vec<Expr> = recv.iter().cloned()
                                        .chain(args.iter().cloned()).collect();
                                    return match r {
                                        crate::check::Ret::One(t) | crate::check::Ret::Pair(t) => {
                                            if let Some(bind) = call_bindings(self, &q, &eff) {
                                                let m: HashMap<String, Ty> =
                                                    bind.into_iter().collect();
                                                crate::check::subst_ty(t, &m)
                                            } else {
                                                self.dt(t)
                                            }
                                        }
                                        crate::check::Ret::Void => Ty::Void,
                                    };
                                }
                            }
                            CallPlan::None => {}
                        }
                    }
                    if let Some((_, r)) = self.fns.get(n) {
                        return match r {
                            crate::check::Ret::One(t) => {
                                if let Some(bind) = call_bindings(self, n, args) {
                                    let m: HashMap<String, Ty> = bind.into_iter().collect();
                                    crate::check::subst_ty(t, &m)
                                } else {
                                    self.dt(t)
                                }
                            }
                            crate::check::Ret::Pair(t) => {
                                if let Some(bind) = call_bindings(self, n, args) {
                                    let m: HashMap<String, Ty> = bind.into_iter().collect();
                                    crate::check::subst_ty(t, &m)
                                } else {
                                    self.dt(t)
                                }
                            }
                            _ => Ty::Void,
                        };
                    }
                    match n.as_str() {
                        "mkchan" => return Ty::Chan,
                        "mkslice" => {
                            if let Some(a) = args.first() {
                                match &a.kind {
                                    ExprKind::Ident(tn) => {
                                        return Ty::Slice(Box::new(self.ty_of_ref(&TyRef {
                                            kind: TyKind::Named(tn.clone()),
                                            span: a.span,
                                        })));
                                    }
                                    ExprKind::TyLit(tr) => {
                                        return Ty::Slice(Box::new(self.ty_of_ref(tr)));
                                    }
                                    _ => {}
                                }
                            }
                            return Ty::Slice(Box::new(Ty::Unknown));
                        }
                        "push" => return self.expr_ty(&args[0]),
                        "recv" | "argc" | "len" | "byteat" | "find" | "atoi" | "ftoi"
                        | "htons" | "htonl" | "sqlite_exec" | "writefile" | "sget" | "scas"
                        | "sadd" | "now" | "now_ms" => {
                            return Ty::Int
                        }
                        "itof" | "atof" => return Ty::Float,
                        "argv" | "itoa" | "substr" | "bufstr" | "sqlite_query" | "readfile" | "ftoa" => {
                            return Ty::Str
                        }
                        "str" => return Ty::Str,
                        "cstr" => return Ty::CStr,
                        "mkbuf" | "sqlite_open" => return Ty::Own(Box::new(Ty::Unknown)),
                        "mkshared" => return Ty::Shared,
                        "new" => {
                            if let ExprKind::Call { args, .. } = &e.kind {
                                if let Some(a) = args.first() {
                                    if let Expr { kind: ExprKind::Ident(sn), .. } = a {
                                        if self.structs.contains_key(sn.as_str()) {
                                            return Ty::Own(Box::new(Ty::Struct(sn.clone())));
                                        }
                                    }
                                    return Ty::Own(Box::new(self.expr_ty(a)));
                                }
                            }
                            return Ty::Own(Box::new(Ty::Unknown));
                        }
                        _ => {}
                    }
                }
                Ty::Unknown
            }
            ExprKind::Field { obj, name } => {
                if let Some(on) = field_path(obj) {
                    if let Some((en, _, _)) = self.enum_variant(&format!("{on}.{name}")) {
                        return Ty::Enum(en.to_string());
                    }
                }
                let ot = self.expr_ty(obj);
                let sn = match &ot {
                    Ty::Struct(n) => Some(n.clone()),
                    Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match &**b {
                        Ty::Struct(n) => Some(n.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(sn) = sn {
                    if let Some(fs) = self.structs.get(&sn) {
                        if let Some((_, t)) = fs.iter().find(|(n, _)| n == name) {
                            return t.clone();
                        }
                    }
                }
                Ty::Unknown
            }
            ExprKind::StructLit { name, .. } => {
                if let Some((en, _, _)) = self.enum_variant(name) {
                    Ty::Enum(en.to_string())
                } else {
                    Ty::Struct(name.clone())
                }
            }
            ExprKind::As { ty, .. } => match &ty.kind {
                TyKind::Named(n) if self.enum_variant(n).is_some() => Ty::Struct(n.clone()),
                _ => Ty::Unknown,
            },
            ExprKind::Match { e: scrut, arms } => {
                let st = self.expr_ty(scrut);
                arms.iter()
                    .find_map(|a| match &a.body {
                        MatchBody::Expr(be) => {
                            self.bind_env.borrow_mut()
                                .push(self.pat_binds(&a.pat, &st).into_iter().collect());
                            let t = self.expr_ty(be);
                            self.bind_env.borrow_mut().pop();
                            Some(t)
                        }
                        _ => None,
                    })
                    .unwrap_or(Ty::Void)
            }
            ExprKind::Index { obj, .. } => self.slice_elem(obj),
            ExprKind::Slice { obj, .. } => match self.dt(&self.expr_ty(obj)) {
                t @ Ty::Slice(_) => t,
                Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match *b {
                    t @ Ty::Slice(_) => t,
                    _ => Ty::Unknown,
                },
                _ => Ty::Unknown,
            },
            ExprKind::Closure { params, body } => Ty::Fn(
                params.iter().map(|p| self.ty_of_ref(&p.ty)).collect(),
                Box::new(self.closure_ret_ty(body)),
            ),
            ExprKind::Bin { op, l, r } => match op {
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                | BinOp::And | BinOp::Or => Ty::Bool,
                BinOp::Add => {
                    let (la, ra) = (self.dt(&self.expr_ty(l)), self.dt(&self.expr_ty(r)));
                    if la == Ty::Str && ra == Ty::Str {
                        Ty::Str
                    } else if la == Ty::Float || ra == Ty::Float {
                        Ty::Float
                    } else {
                        Ty::Int
                    }
                }
                _ => {
                    if self.dt(&self.expr_ty(l)) == Ty::Float
                        || self.dt(&self.expr_ty(r)) == Ty::Float
                    {
                        Ty::Float
                    } else {
                        Ty::Int
                    }
                }
            },
            ExprKind::Un { op, e } => match op {
                UnOp::Not => Ty::Bool,
                UnOp::Neg => self.expr_ty(e),
            },
        }
    }

    /// drain the fn's runtime defer stack down to its entry mark —
    /// emitted at every exit path (the task itself drains leftovers on
    /// trap/cancel unwind in run_task)
    fn emit_defers(&mut self) {
        if self.in_frame {
            let _ = writeln!(self.out, "    rt_defer_drain_to(fr->_dmark);");
        } else {
            let _ = writeln!(self.out, "    rt_defer_drain_to(_dmark);");
        }
    }

    /// `defer call` — register a thunk on the task's defer stack NOW
    /// (per execution: loops accumulate, untaken branches register
    /// nothing). The call's local idents are captured by value into a
    /// generated env so the deferred call reads register-time values.
    /// Per spec b5 the *argument expressions* also evaluate at
    /// registration — `defer f(g())` must not run `g()` at fn exit — so
    /// each arg is bound to a temp before the env capture runs.
    fn stmt_defer(&mut self, call: &Expr) {
        let di = self.defers.len();
        self.defers.push(call.clone());
        let fname = cname(&self.cur_fn);
        let denv = format!("o_denv_{fname}_{di}");
        let thunk = format!("o_def_{fname}_{di}");
        // args -> registration temps; the thunk's call reads the temps
        // through the ordinary env capture. Non-call `defer expr` has
        // no arguments — it keeps the old eval-at-drain behavior.
        let rewritten = match &call.kind {
            ExprKind::Call { f, args } if !args.is_empty() => {
                let mut temps = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let t = self.dt(&self.expr_ty(a));
                    let an = format!("_da{di}_{i}");
                    // raw value: call-site conversion (cstr wrap, borrow
                    // regions) stays part of the deferred call at drain
                    let v = self.expr(a);
                    let _ = writeln!(self.out, "    {} {} = {};", self.cty(&t), an, v);
                    self.env.last_mut().unwrap().insert(an.clone(), t);
                    temps.push(Expr { kind: ExprKind::Ident(an), span: a.span });
                }
                Expr {
                    kind: ExprKind::Call { f: f.clone(), args: temps },
                    span: call.span,
                }
            }
            _ => call.clone(),
        };
        let caps = defer_caps(self, &rewritten);
        // struct + thunk proto precede all bodies; body goes last
        let mut proto = format!("struct {denv} {{");
        if caps.is_empty() {
            proto += " char _none;";
        }
        for (n, t) in &caps {
            proto += &format!(" {} {};", self.cty(t), cname(n));
        }
        proto += &format!(" }}; static void {thunk}(void *);\n");
        self.closure_protos.push_str(&proto);
        // thunk: locals of the captured names shadow, so the deferred
        // call compiles exactly as written against register-time values
        let saved_frame = self.in_frame;
        let saved_fslots = std::mem::take(&mut self.fslots);
        self.in_frame = false;
        let mut body = format!("static void {thunk}(void *_env) {{\n    struct {denv} *_e = _env;\n");
        for (n, t) in &caps {
            body += &format!("    {} {} = _e->{};\n", self.cty(t), cname(n), cname(n));
        }
        let v = self.expr(&rewritten);
        body += &format!("    (void)({v});\n}}\n");
        self.in_frame = saved_frame;
        self.fslots = saved_fslots;
        self.closure_bodies.push_str(&body);
        // registration site: evaluate captures, push the node
        let mut reg = format!("    {{ struct {denv} *_de = malloc(sizeof *_de);");
        for (n, _) in &caps {
            let val = self.expr(&Expr { kind: ExprKind::Ident(n.clone()), span: call.span });
            reg += &format!(" _de->{} = {val};", cname(n));
        }
        reg += &format!(" rt_defer_push({thunk}, _de); }}");
        let _ = writeln!(self.out, "{reg}");
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { name, ty, init, .. } => {
                let t = match ty {
                    Some(tr) => self.ty_of_ref(tr),
                    None => self.expr_ty(init),
                };
                let ct = self.cty(&t);
                let v = self.conv(init, &t);
                let _ = writeln!(self.out, "    {ct} {name} = {v};");
                self.env.last_mut().unwrap().insert(name.clone(), t);
            }
            Stmt::Let2 { a, b, call, .. } => {
                if let ExprKind::Call { f, args } = &call.kind {
                    if let ExprKind::Ident(fn_name) = &f.kind {
                        let n = self.tmp();
                        let c = self.expr(call);
                        // ret-struct name + val type by call kind: direct fn
                        // (o_ret_<name>), instantiated generic (o_ret_<mangled>),
                        // method (o_ret_<T.m>), iface vtab (o_vret_<W>_<m>)
                        let (ret_name, vt) = match self.call_plan(f, args) {
                            CallPlan::Vt { w, mname, ret, .. } => {
                                let vt = match ret {
                                    crate::check::Ret::Pair(t) => t,
                                    _ => Ty::Unknown,
                                };
                                (format!("o_vret_{}_{}", cname(&w), cname(&mname)), vt)
                            }
                            _ => {
                                let q = match self.call_plan(f, args) {
                                    CallPlan::Fn { q, .. } => q,
                                    _ => fn_name.clone(),
                                };
                                let eff: Vec<Expr> = match self.call_plan(f, args) {
                                    CallPlan::Fn { recv, .. } => recv.iter().cloned()
                                        .chain(args.iter().cloned()).collect(),
                                    _ => args.to_vec(),
                                };
                                let bind = call_bindings(self, &q, &eff);
                                let ret_name = match &bind {
                                    Some(b) => inst_name(&q, b),
                                    None => q.clone(),
                                };
                                let vt = match self.fns.get(&q).map(|x| x.1.clone()) {
                                    Some(crate::check::Ret::Pair(t)) => match &bind {
                                        Some(bind) => {
                                            let m: HashMap<String, Ty> =
                                                bind.iter().cloned().collect();
                                            crate::check::subst_ty(&t, &m)
                                        }
                                        None => self.dt(&t),
                                    },
                                    _ => Ty::Unknown,
                                };
                                (format!("o_ret_{}", cname(&ret_name)), vt)
                            }
                        };
                        let _ = writeln!(self.out, "    struct {ret_name} _r{n} = {c};");
                        if let Bind::Name(na) = a {
                            let ct = self.cty(&vt);
                            let _ = writeln!(self.out, "    {ct} {na} = _r{n}.val;");
                            self.env.last_mut().unwrap().insert(na.clone(), vt);
                        }
                        if let Bind::Name(nb) = b {
                            let _ = writeln!(self.out, "    int64_t {nb} = _r{n}.err;");
                            self.env.last_mut().unwrap().insert(nb.clone(), Ty::Err);
                        }
                    }
                }
            }
            Stmt::Assign { target, val, .. } => {
                let t = self.expr(target);
                let v = self.conv(val, &self.expr_ty(target).clone());
                let _ = writeln!(self.out, "    {t} = {v};");
            }
            Stmt::Expr(e) => {
                let v = self.expr(e);
                let _ = writeln!(self.out, "    {v};");
            }
            Stmt::Defer { call, .. } => {
                self.stmt_defer(call);
            }
            Stmt::ExitBoundary { .. } => {
                self.emit_defers();
                self.exit_drained = true;
            }
            Stmt::Return { vals, .. } => {
                if !self.exit_drained {
                    self.emit_defers();
                }
                self.exit_drained = false;
                if vals.is_empty() {
                    let _ = writeln!(self.out, "    return;");
                } else if self.cur_ret_pair && vals.len() == 2 {
                    let want = self.cur_ret_ty.clone();
                    let v = self.conv(&vals[0], &want);
                    let e = self.expr(&vals[1]);
                    let cf = self.cur_fn.clone();
                    let _ = writeln!(self.out, "    return (struct o_ret_{}){{ {v}, {e} }};", cname(&cf));
                } else if self.cur_ret_pair && vals.len() == 1 {
                    // `return paircall()` — eval once into _fw, then
                    // convert .val into this fn's o_ret_<f> when the
                    // pair structs differ (iface rewrap / boxing)
                    if let Some((tag, _)) = self.pair_call_info(&vals[0]) {
                        let c = self.expr(&vals[0]);
                        let cf = self.cur_fn.clone();
                        let cur = format!("o_ret_{}", cname(&cf));
                        if tag != cur {
                            let want = self.cur_ret_ty.clone();
                            let cv = self.conv_v(&vals[0], "_fw.val".to_string(), &want);
                            let _ = writeln!(self.out,
                                "    {{ struct {tag} _fw = {c}; return (struct {cur}){{ {cv}, _fw.err }}; }}");
                        } else {
                            let _ = writeln!(self.out, "    return {c};");
                        }
                    } else {
                        let want = self.cur_ret_ty.clone();
                        let v = self.conv(&vals[0], &want);
                        let _ = writeln!(self.out, "    return {v};");
                    }
                } else {
                    let v = self.conv(&vals[0], &self.cur_ret_ty.clone());
                    let _ = writeln!(self.out, "    return {v};");
                }
            }
            Stmt::If { cond, then, els, .. } => {
                let c = self.expr(cond);
                let _ = writeln!(self.out, "    if ({c}) {{");
                self.env.push(HashMap::new());
                for s in &then.stmts {
                    self.stmt(s);
                }
                self.env.pop();
                if let Some(e) = els {
                    let _ = writeln!(self.out, "    }} else {{");
                    self.env.push(HashMap::new());
                    for s in &e.stmts {
                        self.stmt(s);
                    }
                    self.env.pop();
                }
                let _ = writeln!(self.out, "    }}");
            }
            Stmt::For { cond, body, .. } => {
                let c = self.expr(cond);
                let _ = writeln!(self.out, "    while ({c}) {{");
                self.env.push(HashMap::new());
                for s in &body.stmts {
                    self.stmt(s);
                }
                self.env.pop();
                let _ = writeln!(self.out, "    }}");
            }
            Stmt::Break { .. } => {
                let _ = writeln!(self.out, "    break;");
            }
            Stmt::Continue { .. } => {
                let _ = writeln!(self.out, "    continue;");
            }
            Stmt::Scope { body, .. } => {
                let id = self.scope_id;
                self.scope_id += 1;
                let _ = writeln!(self.out, "    {{ RtScope _sc{id}; rt_scope_enter(&_sc{id});");
                self.env.push(HashMap::new());
                for s in &body.stmts {
                    self.stmt(s);
                }
                self.env.pop();
                let _ = writeln!(self.out, "      rt_scope_leave(&_sc{id}); }}");
            }
            Stmt::Spawn { call, blocking, .. } => self.spawn(call, *blocking),
            Stmt::Select { .. } => {
                // select is only reachable inside suspendable fns (checker
                // seeds the susp set) — those take the frame path (fstmt),
                // so a plain-stmt Select means the checker missed it.
                let _ = writeln!(self.out,
                    "    fprintf(stderr, \"select in non-suspendable fn\\n\"); abort();");
            }
            Stmt::Unsafe { body, .. } => {
                self.env.push(HashMap::new());
                for s in &body.stmts {
                    self.stmt(s);
                }
                self.env.pop();
            }
        }
    }

    fn spawn(&mut self, call: &Expr, blocking: bool) {
        if let ExprKind::Call { f, args } = &call.kind {
            // spawn f() where f is a fn value — pack the fat pair as a0
            if let Ty::Fn(ps, r) = self.fn_callee_ty(f) {
                let id = self.tmp();
                let mut tr = format!("struct o_args_{id} {{ RtFnv a0;");
                for (i, p) in ps.iter().enumerate() {
                    tr.push_str(&format!(" {} a{};", self.cty(p), i + 1));
                }
                tr.push_str(" };\n");
                let ret_c = self.cty(&r);
                let mut sig = String::from("void*");
                let mut argl = String::new();
                for (i, p) in ps.iter().enumerate() {
                    sig += &format!(", {}", self.cty(p));
                    argl += &format!(", a->a{}", i + 1);
                }
                tr.push_str(&format!(
                    "static void* l_tr_{id}(void* _p) {{ struct o_args_{id}* a = _p; \
                     (({ret_c} (*)({sig}))a->a0.fp)(a->a0.env{argl}); return 0; }}\n"));
                self.trampolines.push_str(&tr);
                let _ = writeln!(self.out,
                    "    {{ struct o_args_{id}* a = rt_alloc(sizeof(struct o_args_{id}));");
                let fv = self.expr(f);
                let _ = writeln!(self.out, "      a->a0 = {fv};");
                for (i, arg) in args.iter().enumerate() {
                    let v = match ps.get(i).cloned() {
                        Some(pt) => self.conv(arg, &pt),
                        None => self.expr(arg),
                    };
                    let _ = writeln!(self.out, "      a->a{} = {v};", i + 1);
                }
                let spawn_fn = if blocking { "rt_spawnb" } else { "rt_spawn" };
                let _ = writeln!(self.out, "      {spawn_fn}(l_tr_{id}, a); }}");
                return;
            }
            if let ExprKind::Ident(fn_name) = &f.kind {
                let fn_name = &fn_name.clone();
                // suspendable callee → frame task (heap frame + poll driver)
                if self.susp.contains(fn_name) {
                    let (mangled, bind) = match call_bindings(self, fn_name, args) {
                        Some(bind) => {
                            let m = inst_name(fn_name, &bind);
                            if !self.inst_done.contains(&m) {
                                self.insts.push((fn_name.clone(), bind));
                            }
                            (m, true)
                        }
                        None => (fn_name.clone(), false),
                    };
                    let _ = bind;
                    let pnames: Vec<String> = self
                        .ast_fns
                        .get(fn_name)
                        .map(|af| af.params.iter().map(|p| p.name.clone()).collect())
                        .unwrap_or_default();
                    let _ = writeln!(self.out,
                        "    {{ struct o_frm_{} *_t = (struct o_frm_{} *)rt_task_new(o_poll_{}, (int64_t)sizeof(struct o_frm_{}));",
                        cname(&mangled), cname(&mangled), cname(&mangled), cname(&mangled));
                    let ptys = self.decl_param_tys(fn_name, args);
                    for (i, arg) in args.iter().enumerate() {
                        let v = match ptys.as_ref().and_then(|p| p.get(i)).cloned() {
                            Some(pt) => self.conv(arg, &pt),
                            None => self.expr(arg),
                        };
                        if let Some(pn) = pnames.get(i) {
                            let _ = writeln!(self.out, "      _t->p_{pn} = {v};");
                        }
                    }
                    let spawn_fn = if blocking { "rt_spawnb_task" } else { "rt_spawn_task" };
                    let _ = writeln!(self.out,
                        "      {spawn_fn}((RtFrame *)_t); }}");
                    return;
                }
                let id = self.tmp();
                // args struct + trampoline at file scope
                let dtys = self.decl_param_tys(fn_name, args);
                let arg_tys: Vec<Ty> = args.iter().enumerate()
                    .map(|(i, a)| dtys.as_ref().and_then(|p| p.get(i)).cloned()
                        .unwrap_or_else(|| self.expr_ty(a)))
                    .collect();
                let mut tr = format!("struct o_args_{id} {{");
                for (i, t) in arg_tys.iter().enumerate() {
                    tr.push_str(&format!(" {} a{i};", self.cty(t)));
                }
                tr.push_str(" };\n");
                let arg_refs: Vec<String> = (0..args.len()).map(|i| format!("a->a{i}")).collect();
            let body = match fn_name.as_str() {
                // type-dependent builtins can't expand here — trap visibly
                "print" | "println" | "new" => "rt_trap(9);".into(),
                "close" => {
                    if self.expr_ty(&args[0]) == Ty::Chan {
                        "rt_close(a->a0);".into()
                    } else {
                        "close(a->a0);".into()
                    }
                }
                _ => match self.builtin_emit(fn_name, &arg_refs) {
                    Some(b) => format!("{b};"),
                    None => format!("o_f_{}({});", cname(&fn_name), arg_refs.join(", ")),
                },
            };
            tr.push_str(&format!(
                "static void* l_tr_{id}(void* _p) {{ struct o_args_{id}* a = _p; {body} return 0; }}\n"
            ));
                self.trampolines.push_str(&tr);
                // call site
                let _ = writeln!(self.out,
                    "    {{ struct o_args_{id}* a = rt_alloc(sizeof(struct o_args_{id}));");
                for (i, arg) in args.iter().enumerate() {
                    let v = match dtys.as_ref().and_then(|p| p.get(i)).cloned() {
                        Some(pt) => self.conv(arg, &pt),
                        None => self.expr(arg),
                    };
                    let _ = writeln!(self.out, "      a->a{i} = {v};");
                }
                let spawn_fn = if blocking { "rt_spawnb" } else { "rt_spawn" };
                let _ = writeln!(self.out, "      {spawn_fn}(l_tr_{id}, a); }}");
            }
        }
    }

    fn expr(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int(v) => format!("{v}LL"),
            ExprKind::Float(v) => format!("{v:e}"),
            ExprKind::Str(s) => format!("rt_str_lit(\"{}\")", c_escape(s)),
            ExprKind::Bool(b) => format!("{b}"),
            ExprKind::Nil => "0".into(),
            ExprKind::Ident(n) => {
                // frame locals/params read through fr->
                if self.in_frame {
                    if let Some(slot) = self.fslots.iter().rev().find_map(|s| s.get(n)) {
                        return format!("fr->{slot}");
                    }
                }
                // locals shadow globals; a bare global emits o_g_name
                let local = self.env.iter().skip(1).any(|s| s.contains_key(n));
                if !local && self.globals.contains_key(n) {
                    format!("o_g_{}", cname(&n))
                } else if !local
                    && self.fns.contains_key(n)
                    && !self.ty_params.contains_key(n)
                    && !self.susp.contains(n)
                    && !matches!(self.fns[n].1, crate::check::Ret::Pair(_))
                {
                    // named fn as a value — zero-env fat pair
                    self.fnv_wraps.insert(n.clone());
                    format!("(RtFnv){{ 0, (void*)o_fnv_wrap_{} }}", cname(&n))
                } else {
                    n.clone()
                }
            }
            ExprKind::Call { f, args } => self.call(f, args),
            ExprKind::Field { obj, name } => {
                // unit variant value: `ExprKind.Nil` (dotted/module path)
                if let Some(on) = field_path(obj) {
                    if let Some((en, _v, i)) = self.enum_variant(&format!("{on}.{name}")) {
                        return format!("(struct o_en_{}){{ .tag = {} }}", cname(&en), i);
                    }
                }
                let o = self.expr(obj);
                match self.expr_ty(obj) {
                    Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_) => format!("{o}->f_{name}"),
                    _ => format!("{o}.f_{name}"),
                }
            }
            ExprKind::StructLit { name, fields } => {
                // named variant construction: `ExprKind.Bin{op, l, r}`
                if let Some((en, _v, i)) = self.enum_variant(name) {
                    let want = self.enum_fields.get(name).cloned().unwrap_or_default();
                    let init = fields
                        .iter()
                        .map(|(n, fe)| {
                            let wt = want.iter().find(|(wn, _)| wn == n)
                                .map(|(_, t)| t.clone()).unwrap_or(Ty::Unknown);
                            format!(".f_{n} = {}", self.conv(fe, &wt))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    return format!(
                        "(struct o_en_{}){{ .tag = {}, .u.v{} = (struct o_s_{}){{ {} }} }}",
                        cname(&en), i, i, cname(name), init);
                }
                let want = self.structs.get(name).cloned().unwrap_or_default();
                let init = fields
                    .iter()
                    .map(|(n, fe)| {
                        let wt = want.iter().find(|(wn, _)| wn == n)
                            .map(|(_, t)| t.clone()).unwrap_or(Ty::Unknown);
                        format!(".f_{n} = {}", self.conv(fe, &wt))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("(struct o_s_{}){{ {init} }}", cname(&name))
            }
            ExprKind::As { e: inner, ty } => {
                // `e as E.V` — extract the variant payload struct
                let tn = match &ty.kind {
                    TyKind::Named(n) => n.clone(),
                    _ => "?".into(),
                };
                match self.enum_variant(&tn) {
                    Some((en, _v, i)) => {
                        let e = self.expr(inner);
                        let ec = cname(&en);
                        // tag-checked payload extraction — a wrong-variant
                        // `as` traps instead of reading the union raw
                        if matches!(self.expr_ty(inner), Ty::Own(_)) {
                            format!("({{ struct o_en_{ec} *_ev = ({e}); if ((*_ev).tag != {i}) rt_trap(9); (*_ev).u.v{i}; }})")
                        } else {
                            format!("({{ struct o_en_{ec} _ev = ({e}); if (_ev.tag != {i}) rt_trap(9); _ev.u.v{i}; }})")
                        }
                    }
                    None => format!("({})", self.expr(inner)),
                }
            }
            ExprKind::Bin { op, l, r } => {
                let lt = self.expr_ty(l);
                let lv = self.expr(l);
                let rv = self.expr(r);
                let cop = match op {
                    BinOp::Add => "+", BinOp::Sub => "-", BinOp::Mul => "*",
                    BinOp::Div => "/", BinOp::Mod => "%",
                    BinOp::BitAnd => "&", BinOp::BitOr => "|", BinOp::Xor => "^",
                    BinOp::Eq => "==", BinOp::Ne => "!=",
                    BinOp::Lt => "<", BinOp::Le => "<=",
                    BinOp::Gt => ">", BinOp::Ge => ">=",
                    BinOp::And => "&&", BinOp::Or => "||",
                };
                if *op == BinOp::Add && self.dt(&lt) == Ty::Str {
                    format!("rt_strcat({lv}, {rv})")
                } else if matches!(op, BinOp::Eq | BinOp::Ne) && self.dt(&lt) == Ty::Str {
                    let neg = if *op == BinOp::Ne { "!" } else { "" };
                    format!("({neg}rt_streq({lv}, {rv}))")
                } else if matches!(op, BinOp::Eq | BinOp::Ne)
                    && (matches!(self.dt(&lt), Ty::Iface(_))
                        || matches!(self.dt(&self.expr_ty(r)), Ty::Iface(_)))
                {
                    // `w == nil` — compare the data ptr
                    let le = if matches!(self.dt(&lt), Ty::Iface(_)) {
                        format!("{lv}.data")
                    } else { lv.clone() };
                    let re = if matches!(self.dt(&self.expr_ty(r)), Ty::Iface(_)) {
                        format!("{rv}.data")
                    } else { rv.clone() };
                    format!("({le} {cop} (void*){re})")
                } else if *op == BinOp::Mul && self.dt(&lt) == Ty::Int {
                    // signed int64 overflow is UB in C — the language's
                    // int mul wraps, so emit through unsigned where -O2
                    // can't exploit it (stdlib fnv relies on the wrap)
                    format!("((int64_t)((uint64_t)({lv}) * (uint64_t)({rv})))")
                } else {
                    format!("({lv} {cop} {rv})")
                }
            }
            ExprKind::Un { op, e } => {
                let v = self.expr(e);
                match op {
                    UnOp::Neg => format!("(-{v})"),
                    UnOp::Not => format!("(!{v})"),
                }
            }
            ExprKind::Match { e: scrut, arms } => self.match_expr(e, scrut, arms),
            ExprKind::Index { obj, idx } => {
                // lvalue form `*({ ...; ptr + i; })` — valid both sides of `=`
                let el = self.slice_elem(obj);
                let ect = self.cty(&el);
                let sv = self.slice_val(obj);
                let iv = self.expr(idx);
                format!(
                    "(*({{ RtSlice _ms = {sv}; int64_t _mi = {iv}; \
                     if (_mi < 0 || _mi >= _ms.len) rt_trap(10); \
                     ({ect}*)_ms.ptr + _mi; }}))")
            }
            ExprKind::Slice { obj, lo, hi } => {
                let el = self.slice_elem(obj);
                let ect = self.cty(&el);
                let sv = self.slice_val(obj);
                let av = lo.as_ref().map(|x| self.expr(x)).unwrap_or_else(|| "0LL".into());
                let bv = hi.as_ref().map(|x| self.expr(x)).unwrap_or_else(|| "_ms.len".into());
                format!(
                    "({{ RtSlice _ms = {sv}; int64_t _ma = {av}; int64_t _mb = {bv}; \
                     if (_ma < 0 || _mb < _ma || _mb > _ms.len) rt_trap(10); \
                     (RtSlice){{ ({ect}*)_ms.ptr + _ma, _mb - _ma, _ms.cap - _ma, _ms.heap }}; }})")
            }
            ExprKind::Closure { params, body } => self.closure_expr(e, params, body),
            // checker rejects TyLit outside mkslice — unreachable there
            ExprKind::TyLit(_) => "0".into(),
        }
    }

    /// callee `f` resolves to a `fn(...)` value type — var callee when
    /// the name is a local (shadow rule), any other expr when its type
    /// is Fn. Named fns keep the direct-call path.
    fn fn_callee_ty(&self, f: &Expr) -> Ty {
        match &f.kind {
            ExprKind::Ident(n) => match self.lookup(n) {
                t @ Ty::Fn(..) => self.dt(&t),
                _ => Ty::Unknown,
            },
            _ => self.dt(&self.expr_ty(f)),
        }
    }

    /// closure return type: first `return <expr>` in the body, else void
    fn closure_ret_ty(&self, b: &Block) -> Ty {
        for s in &b.stmts {
            match s {
                Stmt::Return { vals, .. } if vals.len() == 1 => {
                    return self.expr_ty(&vals[0]);
                }
                Stmt::If { then, els, .. } => {
                    let t = self.closure_ret_ty(then);
                    if t != Ty::Void {
                        return t;
                    }
                    if let Some(e) = els {
                        let t = self.closure_ret_ty(e);
                        if t != Ty::Void {
                            return t;
                        }
                    }
                }
                Stmt::For { body, .. } | Stmt::Scope { body, .. } | Stmt::Unsafe { body, .. } => {
                    let t = self.closure_ret_ty(body);
                    if t != Ty::Void {
                        return t;
                    }
                }
                Stmt::Select { arms, .. } => {
                    for a in arms {
                        let t = self.closure_ret_ty(&a.body);
                        if t != Ty::Void {
                            return t;
                        }
                    }
                }
                Stmt::Expr(e) | Stmt::Let { init: e, .. } => {
                    if let ExprKind::Match { arms, .. } = &e.kind {
                        for a in arms {
                            if let MatchBody::Block(b) = &a.body {
                                let t = self.closure_ret_ty(b);
                                if t != Ty::Void {
                                    return t;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ty::Void
    }

    /// `{ |params| stmts }` — env struct + wrapper fn + {env, fp} value
    fn closure_expr(&mut self, e: &Expr, params: &[Param], body: &Block) -> String {
        let id = self.closure_n;
        self.closure_n += 1;
        let mut scan = crate::check::BodyScan::default();
        crate::check::scan_block(body, &mut scan);
        let pnames: std::collections::HashSet<&str> =
            params.iter().map(|p| p.name.as_str()).collect();
        // captures come from refs ∪ callees — `base(x)` lands the callee in
        // `callees`, but a named fn callee never resolves through `lookup`
        // so only real locals (incl. fn-valued vars) get captured
        let mut captures: Vec<(String, Ty)> = scan
            .refs
            .union(&scan.callees)
            .filter(|n| !pnames.contains(n.as_str()))
            .filter_map(|n| match self.lookup(n) {
                Ty::Unknown => None,
                t => Some((n.clone(), t)),
            })
            .collect();
        captures.sort_by(|a, b| a.0.cmp(&b.0));
        let ptys: Vec<Ty> = params.iter().map(|p| self.ty_of_ref(&p.ty)).collect();
        let ret = self.closure_ret_ty(body);
        let ret_c = self.cty(&ret);
        let mut psig = String::new();
        for (p, t) in params.iter().zip(ptys.iter()) {
            psig += &format!(", {} {}", self.cty(t), cname(&p.name));
        }
        let mut env_fields = String::new();
        for (n, t) in &captures {
            env_fields += &format!(" {} {};", self.cty(t), cname(n));
        }
        let _ = writeln!(self.closure_protos, "struct o_env_{id} {{{env_fields} }};");
        let _ = writeln!(self.closure_protos,
            "static {ret_c} o_cl_{id}(struct o_env_{id} *_e{psig});");
        // wrapper body — emitted like a plain fn, captures become locals
        let saved_out = std::mem::take(&mut self.out);
        let saved_frame = self.in_frame;
        self.in_frame = false;
        let saved_fslots = std::mem::take(&mut self.fslots);
        let saved_ret_pair = self.cur_ret_pair;
        self.cur_ret_pair = false;
        let saved_defers = std::mem::take(&mut self.defers);
        let saved_fn = std::mem::replace(&mut self.cur_fn, format!("o_cl_{id}"));
        let _ = writeln!(self.out, "static {ret_c} o_cl_{id}(struct o_env_{id} *_e{psig}) {{");
        let _ = writeln!(self.out, "    RtDefer *_dmark = rt_defer_mark();");
        let mut scope = HashMap::new();
        for (n, t) in &captures {
            let _ = writeln!(self.out, "    {} {} = _e->{};", self.cty(t), cname(n), cname(n));
            scope.insert(n.clone(), t.clone());
        }
        for (p, t) in params.iter().zip(ptys.iter()) {
            scope.insert(p.name.clone(), t.clone());
        }
        self.env.push(scope);
        for s in &body.stmts {
            self.stmt(s);
        }
        self.emit_defers();
        self.env.pop();
        let _ = writeln!(self.out, "}}");
        self.closure_bodies.push_str(&self.out);
        self.out = saved_out;
        self.in_frame = saved_frame;
        self.fslots = saved_fslots;
        self.cur_ret_pair = saved_ret_pair;
        self.defers = saved_defers;
        self.cur_fn = saved_fn;
        // the value: escaping-heap env populated from the enclosing scope
        let mut inits = String::new();
        for (n, t) in &captures {
            let v = self.expr(&Expr {
                kind: ExprKind::Ident(n.clone()),
                span: e.span,
            });
            // a str capture copies only the OxlStr header — the backing
            // stays in the creating scope's arena and dangles once it
            // closes. Copy the bytes into own storage so the env really
            // does outlive its creating scope.
            if matches!(t, Ty::Str) {
                inits += &format!(" _e->{} = rt_str_own({v});", cname(n));
            } else {
                inits += &format!(" _e->{} = {v};", cname(n));
            }
        }
        format!(
            "({{ struct o_env_{id} *_e = rt_alloc_own(sizeof(struct o_env_{id}));{inits} \
             (RtFnv){{ _e, (void*)o_cl_{id} }}; }})")
    }

    /// element type of a slice-valued expr (unwraps `own`/`ref`/`mut` headers)
    fn slice_elem(&self, obj: &Expr) -> Ty {
        match self.dt(&self.expr_ty(obj)) {
            Ty::Slice(el) => *el,
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match *b {
                Ty::Slice(el) => *el,
                _ => Ty::Unknown,
            },
            _ => Ty::Unknown,
        }
    }

    /// slice expr as a value — `own`/`ref`/`mut` headers dereference
    fn slice_val(&mut self, obj: &Expr) -> String {
        let ov = self.expr(obj);
        match self.dt(&self.expr_ty(obj)) {
            Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_) => format!("(*{ov})"),
            _ => ov,
        }
    }

    /// `match` → C statement-expression:
    /// `({ T _ms = s; T _mr; if (_ms.tag==i){ binds; _mr = body; } else … _mr; })`
    /// Void (all-block) matches drop `_mr`. Block arms emit via stmt().
    fn match_expr(&mut self, e: &Expr, scrut: &Expr, arms: &[MatchArm]) -> String {
        let st = self.expr_ty(scrut);
        let sct = self.cty(&st);
        let rt = self.expr_ty(e);
        let valued = rt != Ty::Void && rt != Ty::Unknown;
        let acc = match st {
            Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_) => "(*_ms)".to_string(),
            _ => "_ms".to_string(),
        };
        let sv = self.expr(scrut);
        let mut o = format!("({{ {sct} _ms = {sv}; ");
        if valued {
            o.push_str(&format!("{} _mr; ", self.cty(&rt)));
        }
        let mut first = true;
        for arm in arms {
            let (cond, binds) = self.arm_cond(&arm.pat, &acc, &st);
            match &cond {
                Some(c) => {
                    o.push_str(if first { "if " } else { "else if " });
                    o.push_str(&format!("({c}) {{ "));
                }
                None => o.push_str("else { "),
            }
            first = false;
            self.env.push(HashMap::new());
            for (bn, bv, bt) in &binds {
                o.push_str(&format!("{} {bn} = {bv}; ", self.cty(bt)));
                self.env.last_mut().unwrap().insert(bn.clone(), bt.clone());
            }
            match &arm.body {
                MatchBody::Expr(be) => {
                    let bv = self.expr(be);
                    if valued {
                        o.push_str(&format!("_mr = {bv}; "));
                    } else {
                        o.push_str(&format!("({bv}); "));
                    }
                }
                MatchBody::Block(b) => {
                    let saved = std::mem::take(&mut self.out);
                    for s in &b.stmts {
                        self.stmt(s);
                    }
                    let inner = std::mem::take(&mut self.out);
                    self.out = saved;
                    o.push_str(&inner);
                }
            }
            self.env.pop();
            o.push_str("} ");
        }
        if valued {
            o.push_str("_mr; ");
        }
        o.push_str("})");
        o
    }

    /// pattern binds as (name, ty) — pure lookup for `expr_ty`/env-free use
    fn pat_binds(&self, pat: &Pattern, st: &Ty) -> Vec<(String, Ty)> {
        match pat {
            Pattern::Wild | Pattern::Lit(_) | Pattern::VarUnit { .. } => vec![],
            Pattern::Bind(n) => vec![(n.clone(), st.clone())],
            Pattern::VarPos { path, binds } => {
                let fields = self.structs.get(path).cloned().unwrap_or_default();
                binds.iter().enumerate()
                    .filter(|(_, b)| b.as_str() != "_")
                    .map(|(j, b)| {
                        (b.clone(), fields.get(j)
                            .map(|(_, t)| t.clone())
                            .unwrap_or(Ty::Unknown))
                    })
                    .collect()
            }
            Pattern::VarNamed { path, binds } => {
                let fields = self.structs.get(path).cloned().unwrap_or_default();
                binds.iter()
                    .map(|b| {
                        (b.clone(), fields.iter().find(|(n, _)| n == b)
                            .map(|(_, t)| t.clone())
                            .unwrap_or(Ty::Unknown))
                    })
                    .collect()
            }
        }
    }

    /// pattern → (C condition over `acc`, binds as (name, init-expr, ty))
    /// `acc` is the scrutinee access: `_ms` or `(*_ms)` for owned handles
    fn arm_cond(
        &mut self, pat: &Pattern, acc: &str, st: &Ty,
    ) -> (Option<String>, Vec<(String, String, Ty)>) {
        match pat {
            Pattern::Wild => (None, vec![]),
            Pattern::Bind(n) => (None, vec![(n.clone(), "_ms".into(), st.clone())]),
            Pattern::Lit(le) => {
                let lv = self.expr(le);
                let c = if self.dt(st) == Ty::Str {
                    format!("rt_streq({acc}, {lv})")
                } else {
                    format!("({acc} == {lv})")
                };
                (Some(c), vec![])
            }
            Pattern::VarUnit { path } => match self.enum_variant(path) {
                Some((_, _, i)) => (Some(format!("{acc}.tag == {i}")), vec![]),
                None => (Some("0".into()), vec![]),
            },
            Pattern::VarPos { path, binds } => match self.enum_variant(path) {
                Some((_, _, i)) => {
                    let fields = self.structs.get(path).cloned().unwrap_or_default();
                    let bs = binds
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| b.as_str() != "_")
                        .map(|(j, b)| {
                            let (fn_, ft) = fields.get(j)
                                .map(|(n, t)| (n.clone(), t.clone()))
                                .unwrap_or_else(|| (format!("f{j}"), Ty::Unknown));
                            (b.clone(), format!("{acc}.u.v{i}.f_{fn_}"), ft)
                        })
                        .collect();
                    (Some(format!("{acc}.tag == {i}")), bs)
                }
                None => (Some("0".into()), vec![]),
            },
            Pattern::VarNamed { path, binds } => match self.enum_variant(path) {
                Some((_, _, i)) => {
                    let fields = self.structs.get(path).cloned().unwrap_or_default();
                    let bs = binds
                        .iter()
                        .map(|b| {
                            let ft = fields.iter().find(|(n, _)| n == b)
                                .map(|(_, t)| t.clone())
                                .unwrap_or(Ty::Unknown);
                            (b.clone(), format!("{acc}.u.v{i}.f_{b}"), ft)
                        })
                        .collect();
                    (Some(format!("{acc}.tag == {i}")), bs)
                }
                None => (Some("0".into()), vec![]),
            },
        }
    }

    /// Builtins that expand to an rt_* call without needing arg types.
    /// Used for normal calls AND spawn trampolines. None = not this kind
    /// of builtin.
    fn builtin_emit(&self, name: &str, argvs: &[String]) -> Option<String> {
        let j = |v: &[String]| v.join(", ");
        let a0 = argvs.first().cloned().unwrap_or_default();
        Some(match name {
            "assert" => format!("(({}) ? (void)0 : (rt_trap(3)))", a0),
            "mkchan" => format!("rt_mkchan({})", if argvs.is_empty() { "0LL".into() } else { a0 }),
            "send" => format!("rt_send({})", j(argvs)),
            "recv" => format!("rt_recv({})", a0),
            "sleep" => format!("rt_sleep({})", a0),
            "msleep" => format!("rt_msleep({})", a0),
            "itoa" => format!("rt_itoa({})", a0),
            "argc" => "rt_argc()".into(),
            "argv" => format!("rt_argv({})", a0),
            "len" => format!("rt_len({})", a0),
            "byteat" | "substr" | "find" | "bufstr" | "poke8" | "poke16" | "poke32"
            | "sqlite_exec" | "sqlite_query" => format!("rt_{name}({})", j(argvs)),
            "atoi" | "mkbuf" | "htons" | "htonl" | "cstr" | "str" | "sqlite_open" | "sqlite_close"
            | "sqlite_exec_bind" | "set_rcvtimo" | "set_sndtimo"
            | "readfile" | "writefile" | "exit" | "mkshared" | "sget" | "sset" | "scas"
            | "sadd" | "now" | "now_ms" | "usleep" | "atof" | "ftoa" => {
                format!("rt_{name}({})", j(argvs))
            }
            "ftoi" => format!("((int64_t)({a0}))"),
            "itof" => format!("((double)({a0}))"),
            _ => return None,
        })
    }

    /// base struct/enum name behind own/ref/mut — what methods key on
    fn ty_base(&self, t: &Ty) -> Option<String> {
        match t {
            Ty::Struct(n) | Ty::Enum(n) => Some(n.clone()),
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => self.ty_base(b),
            _ => None,
        }
    }

    /// bind a generic impl method's ty params against the iface's
    /// declared param types — vtab slots pin the instantiation
    fn vtab_bind(&self, fname: &str, m: &IMeth) -> Option<Vec<(String, Ty)>> {
        let tp = self.ty_params.get(fname)?;
        let (params, _) = self.fns.get(fname)?;
        let mut bind = HashMap::new();
        // params[0] is the receiver; iface decls carry the rest
        for (want, got) in params.iter().skip(1).zip(m.params.iter()) {
            bind_generic(want, got, &mut bind);
        }
        Some(tp.iter().map(|(t, _)| (t.clone(), bind.get(t).cloned().unwrap_or(Ty::Int))).collect())
    }

    /// push method instantiations the current vtab_uses need; true if
    /// anything new was queued (drives the emit fixpoint)
    fn seed_vtab_insts(&mut self) -> bool {
        let mut progress = false;
        for (w, implkey) in self.vtab_uses.clone() {
            if implkey.starts_with("iface:") {
                continue;
            }
            let ms = self.ifaces.get(&w).cloned().unwrap_or_default();
            for m in &ms {
                let fname = format!("{implkey}.{}", m.name);
                if let Some(bind) = self.vtab_bind(&fname, m) {
                    let mangled = inst_name(&fname, &bind);
                    if !self.inst_done.contains(&mangled) {
                        self.insts.push((fname, bind));
                        progress = true;
                    }
                }
            }
        }
        progress
    }

    /// D3: concrete vtabs + receiver-normalizing thunks for every
    /// (iface, impl) pair a conversion site produced. Each thunk is a
    /// `RET f(void* _d, params...)` shim that adapts the erased data
    /// ptr to the impl method's receiver and forwards.
    fn emit_vtabs(&mut self, out: &mut String) {
        for (w, implkey) in self.vtab_uses.clone() {
            let ms = self.ifaces.get(&w).cloned().unwrap_or_default();
            let mut slots = Vec::new();
            for (i, m) in ms.iter().enumerate() {
                let tname = format!("o_vtt_{}_{}_{i}", cname(&w), vtkey(&implkey));
                let mut psig = String::new();
                for (k, p) in m.params.iter().enumerate() {
                    psig += &format!(", {} a{k}", self.cty(p));
                }
                let ret_c = match &m.ret {
                    crate::check::Ret::One(t) => self.cty(t),
                    crate::check::Ret::Pair(_) =>
                        format!("struct o_vret_{}_{}", cname(&w), cname(&m.name)),
                    crate::check::Ret::Void => "void".into(),
                };
                let argl: Vec<String> =
                    (0..m.params.len()).map(|k| format!("a{k}")).collect();
                let _ = writeln!(out, "static {ret_c} {tname}(void* _d{psig}) {{");
                if let Some(w2) = implkey.strip_prefix("iface:") {
                    // iface -> iface rewrap: forward through the source
                    // pair's vtab slot for the same-named method
                    let sms = self.ifaces.get(w2).cloned().unwrap_or_default();
                    let j = sms.iter().position(|mm| mm.name == m.name).unwrap_or(0);
                    let mut vsig = String::from("void*");
                    for p in &m.params {
                        vsig += &format!(", {}", self.cty(p));
                    }
                    let callargs = if argl.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", argl.join(", "))
                    };
                    match &m.ret {
                        crate::check::Ret::Void => {
                            let _ = writeln!(out,
                                "    RtIface *_iv = (RtIface*)_d;
    ((void (*)({vsig}))_iv->vtab[{j}])(_iv->data{callargs});");
                        }
                        crate::check::Ret::One(t) => {
                            let c = self.cty(t);
                            let _ = writeln!(out,
                                "    RtIface *_iv = (RtIface*)_d;
    return (({c} (*)({vsig}))_iv->vtab[{j}])(_iv->data{callargs});");
                        }
                        crate::check::Ret::Pair(_) => {
                            let src = format!("struct o_vret_{}_{}", cname(w2), cname(&m.name));
                            let _ = writeln!(out,
                                "    RtIface *_iv = (RtIface*)_d;
    {src} _r = (({src} (*)({vsig}))_iv->vtab[{j}])(_iv->data{callargs});
    return ({ret_c}){{ _r.val, _r.err }};");
                        }
                    }
                } else {
                    // concrete impl — data is a ptr to the boxed value
                    // (or the ptr itself for pointer impls)
                    let fname = format!("{implkey}.{}", m.name);
                    let cn = if self.enums.contains_key(&implkey) {
                        format!("struct o_en_{}", cname(&implkey))
                    } else {
                        format!("struct o_s_{}", cname(&implkey))
                    };
                    let (callee, smap) = match self.vtab_bind(&fname, m) {
                        Some(bind) => {
                            let mangled = inst_name(&fname, &bind);
                            let smap: HashMap<String, Ty> =
                                bind.iter().cloned().collect();
                            (format!("o_f_{}", cname(&mangled)), smap)
                        }
                        None => (format!("o_f_{}", cname(&fname)), HashMap::new()),
                    };
                    let (mps, mret) = self.fns.get(&fname)
                        .cloned().unwrap_or((Vec::new(), crate::check::Ret::Void));
                    let rty = mps.first().cloned().unwrap_or(Ty::Unknown);
                    let rty = if smap.is_empty() { self.dt(&rty) }
                        else { crate::check::subst_ty(&rty, &smap) };
                    let recv = match &rty {
                        Ty::Struct(_) | Ty::Enum(_) => format!("(*({cn}*)_d)"),
                        _ => format!("(({cn}*)_d)"),
                    };
                    let callargs = if argl.is_empty() {
                        recv.clone()
                    } else {
                        format!("{recv}, {}", argl.join(", "))
                    };
                    match &mret {
                        crate::check::Ret::Void => {
                            let _ = writeln!(out, "    {callee}({callargs});");
                        }
                        crate::check::Ret::One(_) => {
                            let _ = writeln!(out, "    return {callee}({callargs});");
                        }
                        crate::check::Ret::Pair(_) => {
                            let rname = callee.trim_start_matches("o_f_");
                            let _ = writeln!(out,
                                "    struct o_ret_{rname} _r = {callee}({callargs});
    return ({ret_c}){{ _r.val, _r.err }};");
                        }
                    }
                }
                let _ = writeln!(out, "}}");
                slots.push(format!("(void*){tname}"));
            }
            let _ = writeln!(out,
                "static void* o_vt_{}_{}[] = {{ {} }};
",
                cname(&w), vtkey(&implkey), slots.join(", "));
        }
    }

    /// field type of `name` on struct `t` — field-of-Fn shadows methods
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

    /// resolve a dotted `x.m` callee to the call plan — mirrors the
    /// checker: direct `T.m` fn, iface vtab slot, or not-a-method
    fn call_plan(&self, f: &Expr, args: &[Expr]) -> CallPlan {
        let ExprKind::Ident(name) = &f.kind else { return CallPlan::None };
        if self.fns.contains_key(name.as_str()) {
            return CallPlan::Fn { q: name.clone(), recv: None };
        }
        if let Some((objp, mname)) = name.rsplit_once('.') {
            if let Some(oe) = unflatten(objp, f.span) {
                let ot = self.dt(&self.expr_ty(&oe));
                if let Some(mt) = self.m_target(&ot, mname) {
                    return match mt {
                        MT::Fn(q) => CallPlan::Fn { q, recv: Some(oe) },
                        MT::Vt(midx, ps, r) => CallPlan::Vt {
                            obj: oe, w: self.iface_name(&ot).unwrap_or_default(),
                            midx, mname: mname.to_string(), params: ps, ret: r,
                        },
                    };
                }
            }
        } else if let Some(a0) = args.first() {
            // `m(x, ..)` — bare method call, receiver is args[0] already
            let t0 = self.dt(&self.expr_ty(a0));
            if let Some(MT::Fn(q)) = self.m_target(&t0, name) {
                return CallPlan::Fn { q, recv: None };
            }
        }
        CallPlan::None
    }

    fn iface_name(&self, t: &Ty) -> Option<String> {
        match t {
            Ty::Iface(w) => Some(w.clone()),
            Ty::Var(v) => self.subst.get(v).and_then(|t| self.iface_name(t)),
            _ => None,
        }
    }

    fn m_target(&self, t: &Ty, m: &str) -> Option<MT> {
        match t {
            Ty::Struct(s) | Ty::Enum(s) => {
                let q = format!("{s}.{m}");
                if self.fns.contains_key(&q) { Some(MT::Fn(q)) } else { None }
            }
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => self.m_target(b, m),
            Ty::Iface(w) => self.ifaces.get(w)?
                .iter().enumerate()
                .find(|(_, mm)| mm.name == m)
                .map(|(i, mm)| MT::Vt(i, mm.params.clone(), mm.ret.clone())),
            Ty::Var(v) => self.subst.get(v).and_then(|t| self.m_target(t, m)),
            _ => None,
        }
    }

    /// emit the receiver arg adapted to the method's receiver type:
    /// value wants deref pointers, pointer wants ref values
    fn adapt_recv(&mut self, e: &Expr, want: &Ty) -> String {
        let got = self.dt(&self.expr_ty(e));
        let ev = self.expr(e);
        match (want, &got) {
            (Ty::Struct(_) | Ty::Enum(_), Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_)) => {
                format!("(*{ev})")
            }
            (Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_), Ty::Struct(_) | Ty::Enum(_)) => {
                format!("(&{ev})")
            }
            _ => ev,
        }
    }

    /// (pair-struct name, val ty) for a [v, err] call — o_vret_<W>_<m>
    /// for vtab slots, o_ret_<callee|inst> for fns; None otherwise
    fn pair_call_info(&mut self, e: &Expr) -> Option<(String, Ty)> {
        let ExprKind::Call { f, args } = &e.kind else { return None };
        match self.call_plan(f, args) {
            CallPlan::Vt { w, mname, ret, .. } => match ret {
                crate::check::Ret::Pair(t) => Some((
                    format!("o_vret_{}_{}", cname(&w), cname(&mname)),
                    t,
                )),
                _ => None,
            },
            CallPlan::Fn { q, recv, .. } => {
                let t = match self.fns.get(&q).map(|x| x.1.clone()) {
                    Some(crate::check::Ret::Pair(t)) => t,
                    _ => return None,
                };
                let eff: Vec<Expr> = recv
                    .iter()
                    .cloned()
                    .chain(args.iter().cloned())
                    .collect();
                match call_bindings(self, &q, &eff) {
                    Some(bind) => {
                        let m: HashMap<String, Ty> = bind.iter().cloned().collect();
                        Some((
                            format!("o_ret_{}", cname(&inst_name(&q, &bind))),
                            crate::check::subst_ty(&t, &m),
                        ))
                    }
                    None => Some((format!("o_ret_{}", cname(&q)), self.dt(&t))),
                }
            }
            _ => None,
        }
    }

    /// declared Ret of a call expr — for the pair-forward shape check
    fn call_plan_ret(&self, e: &Expr) -> Option<crate::check::Ret> {
        let ExprKind::Call { f, args } = &e.kind else { return None };
        match self.call_plan(f, args) {
            CallPlan::Vt { ret, .. } => Some(ret),
            CallPlan::Fn { q, .. } => self.fns.get(&q).map(|(_, r)| r.clone()),
            CallPlan::None => None,
        }
    }

    /// emit `e` coerced to `want` — only iface conversions do anything:
    /// wrap the impl into a {data, vtab} pair; nil → the empty pair
    fn conv(&mut self, e: &Expr, want: &Ty) -> String {
        let v = self.expr(e);
        self.conv_v(e, v, want)
    }

    /// conv on an already-emitted `v` — e's ty still decides the wrap
    fn conv_v(&mut self, e: &Expr, v: String, want: &Ty) -> String {
        let got = self.dt(&self.expr_ty(e));
        let Ty::Iface(w) = self.dt(want) else { return v };
        match &got {
            Ty::Err => "(RtIface){ 0, 0 }".into(),
            Ty::Iface(g) if *g == w => v,
            Ty::Iface(g) => {
                let g = g.clone();
                self.iface_wrap_v(e, v, &w, &format!("iface:{g}"))
            }
            _ => match self.ty_base(&got) {
                Some(b) => self.iface_wrap_v(e, v, &w, &b),
                None => v,
            },
        }
    }

    /// `{data, o_vt_W_S}` — pointer impls hand over directly, values get
    /// boxed so the data outlives the stack slot (mut receivers write
    /// through the box)
    fn iface_wrap(&mut self, e: &Expr, w: &str, implkey: &str) -> String {
        let v = self.expr(e);
        self.iface_wrap_v(e, v, w, implkey)
    }

    /// iface_wrap on an already-emitted `v` (e only supplies the ty)
    fn iface_wrap_v(&mut self, e: &Expr, v: String, w: &str, implkey: &str) -> String {
        self.vtab_uses.insert((w.to_string(), implkey.to_string()));
        let got = self.dt(&self.expr_ty(e));
        let vt = format!("o_vt_{}_{}", cname(w), vtkey(implkey));
        match got {
            Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_) => {
                format!("(RtIface){{ (void*)({v}), {vt} }}")
            }
            Ty::Iface(_) => format!(
                "({{ RtIface *_iw = rt_alloc_own(sizeof(RtIface)); *_iw = ({v});                  (RtIface){{ (void*)_iw, {vt} }}; }})"),
            _ => format!(
                "({{ {ct} *_iw = rt_alloc_own(sizeof({ct})); *_iw = ({v});                  (RtIface){{ (void*)_iw, {vt} }}; }})",
                ct = self.cty(&got)),
        }
    }

    /// declared param types for a call to `name` (generic-substituted),
    /// incl. the receiver slot for methods — None for builtins/unknown
    fn decl_param_tys(&self, name: &str, args: &[Expr]) -> Option<Vec<Ty>> {
        let (params, _) = self.fns.get(name)?;
        let smap: HashMap<String, Ty> = call_bindings(self, name, args)
            .map(|b| b.into_iter().collect())
            .unwrap_or_default();
        Some(params.iter().map(|p| if smap.is_empty() {
            self.dt(p)
        } else {
            crate::check::subst_ty(p, &smap)
        }).collect())
    }

    /// `E.V` — enum name + variant index (tag = declaration order)
    fn enum_variant(&self, path: &str) -> Option<(String, String, usize)> {
        let (e, v) = path.rsplit_once('.')?;
        let i = self.enums.get(e)?.iter().position(|n| n == v)?;
        Some((e.to_string(), v.to_string(), i))
    }

    fn call(&mut self, f: &Expr, args: &[Expr]) -> String {
        let argvs: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
        // indirect call through a fn value: bind once, jump through fp
        if let Ty::Fn(ps, r) = self.fn_callee_ty(f) {
            let fv = self.expr(f);
            let ret_c = self.cty(&r);
            let mut sig = String::from("void*");
            for p in &ps {
                sig += &format!(", {}", self.cty(p));
            }
            let argl = if argvs.is_empty() {
                String::new()
            } else {
                format!(", {}", argvs.join(", "))
            };
            return format!(
                "({{ RtFnv _fv = {fv}; (({ret_c} (*)({sig}))_fv.fp)(_fv.env{argl}); }})");
        }
        if let ExprKind::Ident(name) = &f.kind {
            // enum variant construction: `ExprKind.Int(7)`
            if let Some((en, _v, i)) = self.enum_variant(name) {
                let key = cname(name);
                let want = self.enum_fields.get(name).cloned().unwrap_or_default();
                let cargs: Vec<String> = args.iter().enumerate().map(|(i2, a)| {
                    let wt = want.get(i2).map(|(_, t)| t.clone()).unwrap_or(Ty::Unknown);
                    self.conv(a, &wt)
                }).collect();
                return format!(
                    "(struct o_en_{}){{ .tag = {}, .u.v{} = (struct o_s_{}){{ {} }} }}",
                    cname(&en), i, i, key,
                    cargs.join(", "));
            }
            // `e.kind()` — enum tag accessor (deref `own` handles)
            if let Some((obj, "kind")) = name.rsplit_once('.') {
                if args.is_empty() {
                    let oe = Expr {
                        kind: ExprKind::Ident(obj.to_string()), span: f.span,
                    };
                    let ot = self.expr_ty(&oe);
                    if matches!(ot, Ty::Enum(_)) {
                        return format!("({}).tag", self.expr(&oe));
                    }
                    if matches!(&ot, Ty::Own(b) if matches!(**b, Ty::Enum(_))) {
                        return format!("(*{}).tag", self.expr(&oe));
                    }
                }
            }
            // method dispatch (D3): `x.m(..)` -> `T.m(x)`, `w.m(..)` ->
            // vtab call through the iface pair, bare `m(x,..)` -> `T.m`.
            let mut recv_e: Option<Expr> = None;
            let mut qname = name.clone();
            if !self.fns.contains_key(name.as_str()) {
                match self.call_plan(f, args) {
                    CallPlan::Fn { q, recv } => {
                        recv_e = recv;
                        qname = q;
                    }
                    CallPlan::Vt { obj, w, midx, mname, params, ret } => {
                        let ov = self.expr(&obj);
                        let ret_c = match &ret {
                            crate::check::Ret::One(t) => self.cty(t),
                            crate::check::Ret::Pair(_) =>
                                format!("struct o_vret_{}_{}", cname(&w), cname(&mname)),
                            crate::check::Ret::Void => "void".into(),
                        };
                        let mut sig = String::from("void*");
                        for p in &params {
                            sig += &format!(", {}", self.cty(p));
                        }
                        let argl = if argvs.is_empty() {
                            String::new()
                        } else {
                            format!(", {}", argvs.join(", "))
                        };
                        return format!(
                            "({{ RtIface _iv = {ov}; (({ret_c} (*)({sig}))_iv.vtab[{midx}])(_iv.data{argl}); }})");
                    }
                    CallPlan::None => {
                        // `x.m()` may be a call through a fn-valued field —
                        // expr the unflattened field path if it types Fn
                        if let Some((objp, mname)) = name.rsplit_once('.') {
                            if let Some(oe) = unflatten(objp, f.span) {
                                let ot = self.dt(&self.expr_ty(&oe));
                                if let Some(Ty::Fn(..)) = self.field_ty(&ot, mname) {
                                    let fe = Expr {
                                        kind: ExprKind::Field {
                                            obj: Box::new(oe),
                                            name: mname.to_string(),
                                        },
                                        span: f.span,
                                    };
                                    return self.call(&fe, args);
                                }
                            }
                        }
                    }
                }
            }
            let name = qname.as_str();
            match name {
                "print" | "println" => {
                    let t = self.expr_ty(&args[0]);
                    let (sfx, a) = match t {
                        Ty::Str => ("str", argvs[0].clone()),
                        Ty::CStr => ("str", format!("rt_str({})", argvs[0])),
                        Ty::Float => ("f64", argvs[0].clone()),
                        _ => ("i64", argvs[0].clone()),
                    };
                    return format!("rt_{name}_{sfx}({a})");
                }
                "close" => {
                    return if self.expr_ty(&args[0]) == Ty::Chan {
                        format!("rt_close({})", argvs[0])
                    } else {
                        format!("close({})", argvs[0])
                    }
                }
                "new" => {
                    if let ExprKind::Ident(sn) = &args[0].kind {
                        if self.structs.contains_key(sn.as_str()) {
                            // zero-init: field presence is tested with `== nil`
                            return format!("({{ struct o_s_{0} *_nb = rt_alloc_own(sizeof(struct o_s_{0})); __builtin_memset(_nb, 0, sizeof(struct o_s_{0})); _nb; }})", cname(&sn));
                        }
                    }
                    // new(slice) — deep-copy header+backing to heap so the
                    // own slice can outlive the scope arena (slices.md)
                    if let Ty::Slice(el) = self.expr_ty(&args[0]) {
                        let ect = self.cty(&el);
                        let v = self.expr(&args[0]);
                        return format!("rt_slice_own({v}, sizeof({ect}))");
                    }
                    // new(value) — heap-box the value: `own <ty>`
                    let ct = self.cty(&self.expr_ty(&args[0]));
                    let v = self.expr(&args[0]);
                    return format!(
                        "({{ {ct} *_nb = rt_alloc_own(sizeof({ct})); *_nb = ({v}); _nb; }})");
                }
                "mkslice" => {
                    // mkslice(T, n) — element C type from the type-name arg
                    let et = args.first().and_then(|a| match &a.kind {
                        ExprKind::Ident(n) => Some(self.ty_of_ref(&TyRef {
                            kind: TyKind::Named(n.clone()),
                            span: a.span,
                        })),
                        ExprKind::TyLit(tr) => Some(self.ty_of_ref(tr)),
                        _ => None,
                    }).unwrap_or(Ty::Unknown);
                    let n = if args.len() > 1 { argvs[1].clone() } else { "0LL".into() };
                    return format!("rt_mkslice({n}, sizeof({}))", self.cty(&et));
                }
                "push" => {
                    // push(s, v) → stmt-expr: bind v once, grow+append header
                    let st = self.dt(&self.expr_ty(&args[0]));
                    let (el, owned) = match &st {
                        Ty::Slice(el) => ((**el).clone(), false),
                        Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match &**b {
                            Ty::Slice(el) => ((**el).clone(), true),
                            _ => (Ty::Unknown, false),
                        },
                        _ => (Ty::Unknown, false),
                    };
                    let ect = self.cty(&el);
                    let sv = if owned { format!("(*{})", argvs[0]) } else { argvs[0].clone() };
                    let v = match args.get(1) {
                        Some(a) => self.conv(a, &el),
                        None => String::new(),
                    };
                    return format!(
                        "({{ {ect} _pv = {v}; rt_slice_push({sv}, sizeof({ect}), &_pv); }})");
                }
                "len" => {
                    // slice header → .len; strings keep rt_len
                    return match self.dt(&self.expr_ty(&args[0])) {
                        Ty::Slice(_) => format!("({}).len", argvs[0]),
                        Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b)
                            if matches!(*b, Ty::Slice(_)) =>
                        {
                            format!("(*{}).len", argvs[0])
                        }
                        _ => format!("rt_len({})", argvs[0]),
                    };
                }
                _ => {
                    // `cstr("lit")` — a literal already is a C string; an
                    // rt_cstr copy would only sit in the scope arena
                    if name == "cstr" {
                        if let Some(Expr { kind: ExprKind::Str(s), .. }) = args.first() {
                            return format!("\"{}\"", c_escape(s));
                        }
                    }
                    if let Some(b) = self.builtin_emit(name, &argvs) {
                        return b;
                    }
                    // the method receiver flows through the same
                    // machinery as args[0]
                    let eff_args: Vec<Expr> = recv_e.iter().cloned()
                        .chain(args.iter().cloned()).collect();
                    let oxl = self.fns.contains_key(name) && !self.externs.contains(name);
                    let callee = if oxl {
                        if let Some(bind) = call_bindings(self, name, &eff_args) {
                            let m = inst_name(name, &bind);
                            if !self.inst_done.contains(&m) {
                                self.insts.push((name.to_string(), bind));
                            }
                            format!("o_f_{}", cname(&m))
                        } else {
                            format!("o_f_{}", cname(name))
                        }
                    } else {
                        name.to_string()
                    };
                    // str -> cstr coercion + borrow checks for ref/mut params
                    if let Some((params, ret)) = self.fns.get(name).cloned() {
                        let smap: HashMap<String, Ty> = call_bindings(self, name, &eff_args)
                            .map(|b| b.into_iter().collect())
                            .unwrap_or_default();
                        let mut pre = String::new();
                        let mut post = String::new();
                        let mut coerced: Vec<String> = Vec::new();
                        for (i, (a, pt)) in eff_args.iter().zip(params.iter()).enumerate() {
                            let pt = if smap.is_empty() {
                                self.dt(pt)
                            } else {
                                crate::check::subst_ty(pt, &smap)
                            };
                            let mut v = if i == 0 && recv_e.is_some() {
                                self.adapt_recv(a, &pt)
                            } else if matches!(pt, Ty::Iface(_)) {
                                self.conv(a, &pt)
                            } else {
                                self.expr(a)
                            };
                            let raw_at = self.expr_ty(a);
                            if pt == Ty::CStr && self.dt(&raw_at) == Ty::Str {
                                // a literal already is a C string — an
                                // rt_cstr copy would only sit in the
                                // caller's scope arena
                                v = match &a.kind {
                                    ExprKind::Str(s) => format!("\"{}\"", c_escape(s)),
                                    _ => format!("rt_cstr({v})"),
                                };
                            }
                            // only `own` args start a fresh borrow — mut/ref args
                            // are reborrows inside the caller's existing region
                            if matches!(raw_at, Ty::Own(_)) {
                                match &pt {
                                    Ty::Mut(_) => {
                                        pre.push_str(&format!("rt_borrow_mut({v}); "));
                                        post.push_str(&format!("rt_unborrow({v}); "));
                                    }
                                    Ty::Ref(_) => {
                                        pre.push_str(&format!("rt_borrow_read({v}); "));
                                        post.push_str(&format!("rt_unborrow({v}); "));
                                    }
                                    _ => {}
                                }
                            }
                            coerced.push(v);
                        }
                        let inner = format!("{callee}({})", coerced.join(", "));
                        if !pre.is_empty() {
                            // borrows are lexical to the call — release on the way out
                            match &ret {
                                Ret::Void => {
                                    return format!("({{ {pre}{inner}; {post}}})");
                                }
                                Ret::One(rt) => {
                                    let rty = if smap.is_empty() { rt.clone() } else { subst_ty(rt, &smap) };
                                    return format!(
                                        "({{ {} __rc; {pre}__rc = {inner}; {post}__rc; }})",
                                        self.cty(&rty)
                                    );
                                }
                                Ret::Pair(_) => {
                                    return format!(
                                        "({{ struct o_ret_{} __rc; {pre}__rc = {inner}; {post}__rc; }})",
                                        cname(callee.trim_start_matches("o_f_")).as_str()
                                    );
                                }
                            }
                        }
                        return inner;
                    }
                    return format!("{callee}({})", argvs.join(", "));
                }
            }
        }
        "0 /*badcall*/".into()
    }

    // ---------------- state-machine transform (susp fns) ----------------

    fn new_label(&mut self) -> String {
        self.label_ctr += 1;
        format!("L{}", self.label_ctr)
    }

    fn fresh(&mut self, pfx: &str) -> String {
        self.hoist_ctr += 1;
        format!("{pfx}{}", self.hoist_ctr)
    }

    /// is `n` a suspend point callee (susp fn or blocking builtin)?
    fn susp_name(&self, n: &str) -> bool {
        self.susp.contains(n) || matches!(n, "recv" | "send" | "sleep" | "msleep")
    }

    /// is this expr a call to a susp callee?
    fn call_is_susp(&self, e: &Expr) -> bool {
        if let ExprKind::Call { f, .. } = &e.kind {
            if let ExprKind::Ident(n) = &f.kind {
                return self.susp_name(n);
            }
        }
        false
    }

    fn expr_has_susp(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Call { f, args } => {
                matches!(&f.kind, ExprKind::Ident(n) if self.susp_name(n))
                    || args.iter().any(|a| self.expr_has_susp(a))
                    || self.expr_has_susp(f)
            }
            ExprKind::Bin { l, r, .. } => self.expr_has_susp(l) || self.expr_has_susp(r),
            ExprKind::Un { e, .. } => self.expr_has_susp(e),
            ExprKind::Field { obj, .. } => self.expr_has_susp(obj),
            ExprKind::As { e, .. } => self.expr_has_susp(e),
            ExprKind::StructLit { fields, .. } => {
                fields.iter().any(|(_, fe)| self.expr_has_susp(fe))
            }
            ExprKind::Index { obj, idx } => {
                self.expr_has_susp(obj) || self.expr_has_susp(idx)
            }
            ExprKind::Slice { obj, lo, hi } => {
                self.expr_has_susp(obj)
                    || lo.as_ref().is_some_and(|x| self.expr_has_susp(x))
                    || hi.as_ref().is_some_and(|x| self.expr_has_susp(x))
            }
            _ => false,
        }
    }

    fn eint(v: i64) -> Expr {
        Expr { kind: ExprKind::Int(v), span: crate::diag::Span { lo: 0, hi: 0 } }
    }

    fn eident(n: &str, sp: crate::diag::Span) -> Expr {
        Expr { kind: ExprKind::Ident(n.to_string()), span: sp }
    }

    /// lift susp calls out of expressions into fresh `let` statements so
    /// every suspend point sits at statement level. `a && susp()` /
    /// `a || susp()` desugar to `b := a; if (b) { b = susp() }` etc. —
    /// eager hoisting would evaluate the call unconditionally.
    fn hoist_expr(&mut self, e: &Expr, pre: &mut Vec<Stmt>) -> Expr {
        match &e.kind {
            ExprKind::Call { f, args } => {
                let f2 = self.hoist_expr(f, pre);
                let args2: Vec<Expr> =
                    args.iter().map(|a| self.hoist_expr(a, pre)).collect();
                let call = Expr {
                    kind: ExprKind::Call { f: Box::new(f2), args: args2 },
                    span: e.span,
                };
                if self.call_is_susp(&call) {
                    let tmp = self.fresh("_h");
                    pre.push(Stmt::Let {
                        name: tmp.clone(),
                        ty: None,
                        init: call,
                        span: e.span,
                    });
                    Self::eident(&tmp, e.span)
                } else {
                    call
                }
            }
            ExprKind::Bin { op, l, r } => {
                if matches!(op, BinOp::And | BinOp::Or) && self.expr_has_susp(r) {
                    let l2 = self.hoist_expr(l, pre);
                    let b = self.fresh("_b");
                    pre.push(Stmt::Let {
                        name: b.clone(),
                        ty: None,
                        init: l2,
                        span: e.span,
                    });
                    let mut inner = Vec::new();
                    let r2 = self.hoist_expr(r, &mut inner);
                    inner.push(Stmt::Assign {
                        target: Self::eident(&b, e.span),
                        val: r2,
                        span: e.span,
                    });
                    let cond = if *op == BinOp::And {
                        Self::eident(&b, e.span)
                    } else {
                        Expr {
                            kind: ExprKind::Un {
                                op: UnOp::Not,
                                e: Box::new(Self::eident(&b, e.span)),
                            },
                            span: e.span,
                        }
                    };
                    pre.push(Stmt::If {
                        cond,
                        then: Block { stmts: inner, span: e.span },
                        els: None,
                        span: e.span,
                    });
                    Self::eident(&b, e.span)
                } else {
                    let l2 = self.hoist_expr(l, pre);
                    let r2 = self.hoist_expr(r, pre);
                    Expr {
                        kind: ExprKind::Bin {
                            op: *op,
                            l: Box::new(l2),
                            r: Box::new(r2),
                        },
                        span: e.span,
                    }
                }
            }
            ExprKind::Un { op, e: i } => Expr {
                kind: ExprKind::Un { op: *op, e: Box::new(self.hoist_expr(i, pre)) },
                span: e.span,
            },
            ExprKind::Field { obj, name } => Expr {
                kind: ExprKind::Field {
                    obj: Box::new(self.hoist_expr(obj, pre)),
                    name: name.clone(),
                },
                span: e.span,
            },
            ExprKind::As { e: i, ty } => Expr {
                kind: ExprKind::As {
                    e: Box::new(self.hoist_expr(i, pre)),
                    ty: ty.clone(),
                },
                span: e.span,
            },
            ExprKind::StructLit { name, fields } => Expr {
                kind: ExprKind::StructLit {
                    name: name.clone(),
                    fields: fields
                        .iter()
                        .map(|(n, fe)| (n.clone(), self.hoist_expr(fe, pre)))
                        .collect(),
                },
                span: e.span,
            },
            ExprKind::Index { obj, idx } => Expr {
                kind: ExprKind::Index {
                    obj: Box::new(self.hoist_expr(obj, pre)),
                    idx: Box::new(self.hoist_expr(idx, pre)),
                },
                span: e.span,
            },
            ExprKind::Slice { obj, lo, hi } => Expr {
                kind: ExprKind::Slice {
                    obj: Box::new(self.hoist_expr(obj, pre)),
                    lo: lo.as_ref().map(|x| Box::new(self.hoist_expr(x, pre))),
                    hi: hi.as_ref().map(|x| Box::new(self.hoist_expr(x, pre))),
                },
                span: e.span,
            },
            ExprKind::Match { e: scrut, arms } => {
                // arm bodies are conditional — their susp calls must NOT
                // hoist to `pre` (checker restricts them to stmt-level
                // matches); block arms hoist within their own block
                let scrut2 = self.hoist_expr(scrut, pre);
                let arms2 = arms
                    .iter()
                    .map(|a| MatchArm {
                        pat: a.pat.clone(),
                        body: match &a.body {
                            MatchBody::Expr(be) => MatchBody::Expr(be.clone()),
                            MatchBody::Block(b) => MatchBody::Block(self.hoist_block(b)),
                        },
                        span: a.span,
                    })
                    .collect();
                Expr {
                    kind: ExprKind::Match { e: Box::new(scrut2), arms: arms2 },
                    span: e.span,
                }
            }
            _ => e.clone(),
        }
    }

    /// hoist a call expr's ARGS only — the call itself stays in place
    fn hoist_call_args(&mut self, call: &Expr, pre: &mut Vec<Stmt>) -> Expr {
        if let ExprKind::Call { f, args } = &call.kind {
            let args2: Vec<Expr> =
                args.iter().map(|a| self.hoist_expr(a, pre)).collect();
            Expr {
                kind: ExprKind::Call { f: f.clone(), args: args2 },
                span: call.span,
            }
        } else {
            self.hoist_expr(call, pre)
        }
    }

    fn hoist_block(&mut self, b: &Block) -> Block {
        let mut stmts = Vec::new();
        for s in &b.stmts {
            self.hoist_stmt(s, &mut stmts);
        }
        Block { stmts, span: b.span }
    }

    fn hoist_stmt(&mut self, s: &Stmt, out: &mut Vec<Stmt>) {
        match s {
            Stmt::Let { name, ty, init, span } => {
                let mut pre = Vec::new();
                let i2 = self.hoist_expr(init, &mut pre);
                out.append(&mut pre);
                out.push(Stmt::Let {
                    name: name.clone(),
                    ty: ty.clone(),
                    init: i2,
                    span: *span,
                });
            }
            Stmt::Let2 { a, b, call, span } => {
                let mut pre = Vec::new();
                let c2 = self.hoist_call_args(call, &mut pre);
                out.append(&mut pre);
                out.push(Stmt::Let2 {
                    a: clone_bind(a),
                    b: clone_bind(b),
                    call: c2,
                    span: *span,
                });
            }
            Stmt::Assign { target, val, span } => {
                let mut pre = Vec::new();
                let t2 = self.hoist_expr(target, &mut pre);
                let v2 = self.hoist_expr(val, &mut pre);
                out.append(&mut pre);
                out.push(Stmt::Assign { target: t2, val: v2, span: *span });
            }
            Stmt::Expr(e) => {
                let mut pre = Vec::new();
                // a stmt-level susp call IS the suspend point — hoist its
                // args, not the call itself
                let e2 = self.hoist_call_args(e, &mut pre);
                out.append(&mut pre);
                out.push(Stmt::Expr(e2));
            }
            Stmt::Return { vals, span } => {
                // susp operands evaluate AFTER the scope/defer drain
                // (B5 timing) — the boundary marks where that drain runs
                if vals.iter().any(|v| self.expr_has_susp(v)) {
                    out.push(Stmt::ExitBoundary { span: *span });
                }
                // `return suspPair()` forwards as [v, err] — desugar to let2
                if vals.len() == 1 && self.call_is_susp(&vals[0]) {
                    if let ExprKind::Call { f, .. } = &vals[0].kind {
                        if let ExprKind::Ident(n) = &f.kind {
                            if let Some((_, Ret::Pair(_))) = self.fns.get(n) {
                                let mut pre = Vec::new();
                                let c2 = self.hoist_call_args(&vals[0], &mut pre);
                                out.append(&mut pre);
                                let (pa, pe) = (self.fresh("_pa"), self.fresh("_pe"));
                                out.push(Stmt::Let2 {
                                    a: Bind::Name(pa.clone()),
                                    b: Bind::Name(pe.clone()),
                                    call: c2,
                                    span: *span,
                                });
                                out.push(Stmt::Return {
                                    vals: vec![
                                        Self::eident(&pa, *span),
                                        Self::eident(&pe, *span),
                                    ],
                                    span: *span,
                                });
                                return;
                            }
                        }
                    }
                }
                let mut pre = Vec::new();
                let vals2: Vec<Expr> =
                    vals.iter().map(|v| self.hoist_expr(v, &mut pre)).collect();
                out.append(&mut pre);
                out.push(Stmt::Return { vals: vals2, span: *span });
            }
            Stmt::If { cond, then, els, span } => {
                let mut pre = Vec::new();
                let c2 = self.hoist_expr(cond, &mut pre);
                out.append(&mut pre);
                out.push(Stmt::If {
                    cond: c2,
                    then: self.hoist_block(then),
                    els: els.as_ref().map(|e| self.hoist_block(e)),
                    span: *span,
                });
            }
            Stmt::For { cond, body, span } => {
                let mut pre = Vec::new();
                let c2 = self.hoist_expr(cond, &mut pre);
                out.append(&mut pre);
                out.push(Stmt::For {
                    cond: c2,
                    body: self.hoist_block(body),
                    span: *span,
                });
            }
            Stmt::Scope { body, span } => out.push(Stmt::Scope {
                body: self.hoist_block(body),
                span: *span,
            }),
            Stmt::Unsafe { body, span } => out.push(Stmt::Unsafe {
                body: self.hoist_block(body),
                span: *span,
            }),
            Stmt::Spawn { call, pinned, blocking, span } => {
                let mut pre = Vec::new();
                let c2 = self.hoist_call_args(call, &mut pre);
                out.append(&mut pre);
                out.push(Stmt::Spawn {
                    call: c2,
                    pinned: *pinned,
                    blocking: *blocking,
                    span: *span,
                });
            }
            Stmt::Defer { call, span } => {
                let mut pre = Vec::new();
                let c2 = self.hoist_call_args(call, &mut pre);
                out.append(&mut pre);
                out.push(Stmt::Defer { call: c2, span: *span });
            }
            Stmt::Select { arms, span } => {
                let mut arms2 = Vec::new();
                for arm in arms {
                    let kind = match &arm.kind {
                        SelKind::Recv { chan, bind, span: asp } => {
                            let mut pre = Vec::new();
                            let c2 = self.hoist_expr(chan, &mut pre);
                            out.append(&mut pre);
                            SelKind::Recv { chan: c2, bind: bind.clone(), span: *asp }
                        }
                        SelKind::Timeout { ms, span: asp } => {
                            let mut pre = Vec::new();
                            let m2 = self.hoist_expr(ms, &mut pre);
                            out.append(&mut pre);
                            SelKind::Timeout { ms: m2, span: *asp }
                        }
                        SelKind::Else { span: asp } => SelKind::Else { span: *asp },
                    };
                    let body = self.hoist_block(&arm.body);
                    arms2.push(SelectArm { kind, body });
                }
                out.push(Stmt::Select { arms: arms2, span: *span });
            }
            Stmt::Break { span } => out.push(Stmt::Break { span: *span }),
            Stmt::Continue { span } => out.push(Stmt::Continue { span: *span }),
            Stmt::ExitBoundary { span } => {
                out.push(Stmt::ExitBoundary { span: *span })
            }
        }
    }

    fn hoist_fn(&mut self, f: &Fn) -> Fn {
        Fn {
            name: f.name.clone(),
            recv: f.recv.clone(),
            ty_params: f.ty_params.clone(),
            params: f.params.iter().map(|p| Param {
                name: p.name.clone(),
                ty: p.ty.clone(),
                span: p.span,
            }).collect(),
            ret: f.ret.clone(),
            body: f.body.as_ref().map(|b| self.hoist_block(b)),
            span: f.span,
        }
    }

    /// the callee behind a susp call: a leaf builtin or an oxl fn
    /// (generics resolve to their instantiation's frame type)
    fn susp_frame_ty(&mut self, name: &str, args: &[Expr]) -> (String, String) {
        let mangled = match call_bindings(self, name, args) {
            Some(bind) => {
                let m = inst_name(name, &bind);
                if !self.inst_done.contains(&m) {
                    self.insts.push((name.to_string(), bind));
                }
                m
            }
            None => name.to_string(),
        };
        (format!("o_frm_{}", cname(&mangled)), format!("o_poll_{}", cname(&mangled)))
    }

    /// `fr->cur_scope = tls_scope; tls_scope = saved; return <code>;`
    fn susp_ret(&mut self, code: &str) {
        let _ = writeln!(
            self.out,
            "    fr->cur_scope = tls_scope; tls_scope = saved; return {code};"
        );
    }

    /// join+finish frame scope `sc{i}` — same suspend point a scope's own
    /// exit emits: park until children drain, then tear the scope down
    fn fscope_close(&mut self, i: usize) {
        let k = self.state_ctr;
        self.state_ctr += 1;
        self.state_labels.push(k);
        let _ = writeln!(self.out,
            "    fr->state = {k};\n    if (rt_scope_join_poll(&fr->sc{i}, task) == RT_PENDING) {{ fr->cur_scope = tls_scope; tls_scope = saved; return RT_PENDING; }}\n    s{k}: ;\n    {{ int64_t _e = rt_scope_finish(&fr->sc{i});\n      if (_e) rt_scope_propagate(tls_scope, _e); }}");
    }

    /// close every frame scope open at stack depth `from`, innermost
    /// first. break/continue/return exits route through this — a bare
    /// goto/return orphans the scope's live children and leaks the
    /// frame-resident RtScope
    fn fscope_drain(&mut self, from: usize) {
        let open: Vec<usize> = self.fscope_stack[from..].to_vec();
        for i in open.into_iter().rev() {
            self.fscope_close(i);
        }
    }

    /// emit a suspend point for an oxl-fn call: allocate the child frame,
    /// poll it, propagate PENDING/CANCEL, bind the result on READY
    fn fsusp_call(&mut self, name: &str, args: &[Expr], dest: FDest) {
        let (frty, poll) = self.susp_frame_ty(name, args);
        let frname = frty.trim_start_matches("o_frm_").to_string();
        let pnames: Vec<String> = self
            .ast_fns
            .get(name)
            .map(|af| af.params.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        let ptys = self.decl_param_tys(name, args);
        let argvs: Vec<String> = args.iter().enumerate()
            .map(|(i, a)| match ptys.as_ref().and_then(|p| p.get(i)).cloned() {
                Some(pt) => self.conv(a, &pt),
                None => self.expr(a),
            })
            .collect();
        let k = self.state_ctr;
        self.state_ctr += 1;
        self.state_labels.push(k);
        let _ = writeln!(self.out,
            "    {{ struct {frty} *_c = (struct {frty} *)malloc(sizeof(struct {frty}));\n      ((RtFrame *)_c)->cur_scope = tls_scope; ((RtFrame *)_c)->state = 0;");
        for (i, v) in argvs.iter().enumerate() {
            if let Some(pn) = pnames.get(i) {
                let _ = writeln!(self.out, "      _c->p_{pn} = {v};");
            }
        }
        let _ = writeln!(self.out,
            "      fr->child = (RtFrame *)_c; fr->state = {k};\n    s{k}: ;\n      _p = {poll}(fr->child, task);\n      if (_p != RT_READY) {{ fr->cur_scope = tls_scope; tls_scope = saved; return _p; }}");
        match dest {
            FDest::Void => {}
            FDest::Field(fd) => {
                let _ = writeln!(self.out,
                    "      fr->{fd} = ((struct o_frm_{frname} *)fr->child)->result;");
            }
            FDest::Assign(tc) => {
                let _ = writeln!(self.out,
                    "      {tc} = ((struct o_frm_{frname} *)fr->child)->result;");
            }
            FDest::Pair(fa, fb) => {
                let _ = writeln!(self.out,
                    "      {{ struct o_ret_{frname} _r = ((struct o_frm_{frname} *)fr->child)->result;");
                if let Some(fa) = fa {
                    let _ = writeln!(self.out, "        fr->{fa} = _r.val;");
                }
                if let Some(fb) = fb {
                    let _ = writeln!(self.out, "        fr->{fb} = _r.err;");
                }
                let _ = writeln!(self.out, "      }}");
            }
        }
        let _ = writeln!(self.out, "      free(fr->child);\n    }}");
    }

    /// leaf suspends: recv/send/sleep — park on the chan/timer wait queue
    fn fsusp_leaf(&mut self, name: &str, args: &[Expr], dest: FDest) {
        let argvs: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
        let k = self.state_ctr;
        self.state_ctr += 1;
        self.state_labels.push(k);
        let _ = writeln!(self.out,
            "    if (rt_cancelled()) {{ fr->cur_scope = tls_scope; tls_scope = saved; return RT_CANCEL; }}");
        let _ = writeln!(self.out, "    fr->state = {k};");
        let poll = match name {
            "recv" => format!("rt_recv_poll({}, task)", argvs[0]),
            "send" => format!("rt_send_poll({}, {}, task)", argvs[0], argvs[1]),
            "msleep" => format!("rt_msleep_poll({}, task)", argvs[0]),
            _ => format!("rt_sleep_poll({}, task)", argvs[0]),
        };
        let _ = writeln!(self.out,
            "    if ({poll} == RT_PENDING) {{ fr->cur_scope = tls_scope; tls_scope = saved; return RT_PENDING; }}");
        let _ = writeln!(self.out, "    s{k}: ;");
        // a cancel-driven wake delivers no value — die at the resume point
        let _ = writeln!(self.out,
            "    if (rt_cancelled()) {{ fr->cur_scope = tls_scope; tls_scope = saved; return RT_CANCEL; }}");
        match dest {
            FDest::Field(fd) => {
                let _ = writeln!(self.out, "    fr->{fd} = rt_task_data(task);");
            }
            FDest::Assign(tc) => {
                let _ = writeln!(self.out, "    {tc} = rt_task_data(task);");
            }
            _ => {}
        }
    }

    /// is the stmt's payload call a suspend point? returns the callee name
    fn stmt_susp_call<'x>(&self, e: &'x Expr) -> Option<&'x str> {
        if let ExprKind::Call { f, .. } = &e.kind {
            if let ExprKind::Ident(n) = &f.kind {
                if self.susp_name(n) {
                    return Some(n);
                }
            }
        }
        None
    }

    /// register a frame field for an oxl local, return its slot name
    fn fslot(&mut self, name: &str, cty: &str) -> String {
        let slot = format!("v{}", self.frame_decls.len());
        self.frame_decls.push(format!("{cty} {slot};"));
        self.fslots
            .last_mut()
            .unwrap()
            .insert(name.to_string(), slot.clone());
        slot
    }

    /// flat label/goto emission of a suspendable fn body into self.out
    /// (func_frame swaps buffers so this lands in frame_out)
    fn fstmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.fstmt(s);
        }
    }

    /// statement-level `match` in a suspendable fn — real if-chain so
    /// block arms can suspend; payload binds live in frame slots
    fn fstmt_match(&mut self, scrut: &Expr, arms: &[MatchArm]) {
        let st = self.expr_ty(scrut);
        let sct = self.cty(&st);
        let acc = match st {
            Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_) => "(*_ms)".to_string(),
            _ => "_ms".to_string(),
        };
        let sv = self.expr(scrut);
        let _ = writeln!(self.out, "    {{ {sct} _ms = {sv};");
        let mut first = true;
        for arm in arms {
            let (cond, binds) = self.arm_cond(&arm.pat, &acc, &st);
            match &cond {
                Some(c) if first => {
                    let _ = writeln!(self.out, "    if ({c}) {{");
                }
                Some(c) => {
                    let _ = writeln!(self.out, "    else if ({c}) {{");
                }
                None => {
                    let _ = writeln!(self.out, "    else {{");
                }
            }
            first = false;
            self.env.push(HashMap::new());
            self.fslots.push(HashMap::new());
            for (bn, bv, bt) in &binds {
                let fd = self.fslot(bn, &self.cty(bt));
                self.env.last_mut().unwrap().insert(bn.clone(), bt.clone());
                let _ = writeln!(self.out, "      fr->{fd} = {bv};");
            }
            match &arm.body {
                MatchBody::Expr(be) => {
                    self.fstmt(&Stmt::Expr(be.clone()));
                }
                MatchBody::Block(b) => self.fstmts(&b.stmts),
            }
            self.fslots.pop();
            self.env.pop();
            let _ = writeln!(self.out, "    }}");
        }
        let _ = writeln!(self.out, "    }}");
    }

    fn fstmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { name, ty, init, .. } => {
                if self.call_is_susp(init) {
                    let t = match ty {
                        Some(tr) => self.ty_of_ref(tr),
                        None => self.expr_ty(init),
                    };
                    let ct = self.cty(&t);
                    let fd = self.fslot(name, &ct);
                    self.env.last_mut().unwrap().insert(name.clone(), t);
                    if let ExprKind::Call { f, args } = &init.kind {
                        if let ExprKind::Ident(n) = &f.kind {
                            let n = n.clone();
                            if matches!(n.as_str(), "recv" | "send" | "sleep" | "msleep") {
                                self.fsusp_leaf(&n, args, FDest::Field(fd));
                            } else {
                                self.fsusp_call(&n, args, FDest::Field(fd));
                            }
                        }
                    }
                } else {
                    let t = match ty {
                        Some(tr) => self.ty_of_ref(tr),
                        None => self.expr_ty(init),
                    };
                    let ct = self.cty(&t);
                    let v = self.expr(init);
                    let fd = self.fslot(name, &ct);
                    self.env.last_mut().unwrap().insert(name.clone(), t);
                    let _ = writeln!(self.out, "    fr->{fd} = {v};");
                }
            }
            Stmt::Let2 { a, b, call, .. } => {
                if let ExprKind::Call { f, args } = &call.kind {
                    if let ExprKind::Ident(fn_name) = &f.kind {
                        let fn_name = fn_name.clone();
                        if self.susp.contains(&fn_name) {
                            let fa = match a {
                                Bind::Name(na) => {
                                    let vt = match self.fns.get(&fn_name).map(|x| x.1.clone()) {
                                        Some(Ret::Pair(t)) => {
                                            match call_bindings(self, &fn_name, args) {
                                                Some(bind) => {
                                                    let m: HashMap<String, Ty> =
                                                        bind.into_iter().collect();
                                                    subst_ty(&t, &m)
                                                }
                                                None => self.dt(&t),
                                            }
                                        }
                                        _ => Ty::Unknown,
                                    };
                                    let ct = self.cty(&vt);
                                    let fd = self.fslot(na, &ct);
                                    self.env.last_mut().unwrap().insert(na.clone(), vt);
                                    Some(fd)
                                }
                                Bind::Blank => None,
                            };
                            let fb = match b {
                                Bind::Name(nb) => {
                                    let fd = self.fslot(nb, "int64_t");
                                    self.env
                                        .last_mut()
                                        .unwrap()
                                        .insert(nb.clone(), Ty::Err);
                                    Some(fd)
                                }
                                Bind::Blank => None,
                            };
                            self.fsusp_call(&fn_name, args, FDest::Pair(fa, fb));
                            return;
                        }
                    }
                }
                // non-susp path: same shape as stmt() but targets are fields
                if let ExprKind::Call { f, args } = &call.kind {
                    if let ExprKind::Ident(fn_name) = &f.kind {
                        let n = self.tmp();
                        let c = self.expr(call);
                        // pair-struct name + val ty by call kind — direct
                        // fn (o_ret_<name|inst>), iface vtab (o_vret_<W>_<m>)
                        let (tag, vt) = match self.pair_call_info(call) {
                            Some(pair) => pair,
                            None => {
                                let ret_name = match call_bindings(self, fn_name, args) {
                                    Some(bind) => inst_name(fn_name, &bind),
                                    None => fn_name.clone(),
                                };
                                (format!("o_ret_{}", cname(&ret_name)), Ty::Unknown)
                            }
                        };
                        let _ = writeln!(self.out, "    {{ struct {tag} _r{n} = {c};");
                        if let Bind::Name(na) = a {
                            let ct = self.cty(&vt);
                            let fd = self.fslot(na, &ct);
                            self.env.last_mut().unwrap().insert(na.clone(), vt);
                            let _ = writeln!(self.out, "      fr->{fd} = _r{n}.val;");
                        }
                        if let Bind::Name(nb) = b {
                            let fd = self.fslot(nb, "int64_t");
                            self.env
                                .last_mut()
                                .unwrap()
                                .insert(nb.clone(), Ty::Err);
                            let _ = writeln!(self.out, "      fr->{fd} = _r{n}.err;");
                        }
                        let _ = writeln!(self.out, "    }}");
                    }
                }
            }
            Stmt::Assign { target, val, .. } => {
                if self.call_is_susp(val) {
                    let tc = self.expr(target);
                    if let ExprKind::Call { f, args } = &val.kind {
                        if let ExprKind::Ident(n) = &f.kind {
                            let n = n.clone();
                            if matches!(n.as_str(), "recv" | "send" | "sleep" | "msleep") {
                                self.fsusp_leaf(&n, args, FDest::Assign(tc));
                            } else {
                                self.fsusp_call(&n, args, FDest::Assign(tc));
                            }
                        }
                    }
                } else {
                    let t = self.expr(target);
                    let v = self.expr(val);
                    let _ = writeln!(self.out, "    {t} = {v};");
                }
            }
            Stmt::Expr(e) => {
                // stmt-level match emits as a real if-chain so block arms
                // can suspend through the frame machinery
                if let ExprKind::Match { e: scrut, arms } = &e.kind {
                    self.fstmt_match(scrut, arms);
                    return;
                }
                if self.call_is_susp(e) {
                    if let ExprKind::Call { f, args } = &e.kind {
                        if let ExprKind::Ident(n) = &f.kind {
                            let n = n.clone();
                            if matches!(n.as_str(), "recv" | "send" | "sleep" | "msleep") {
                                self.fsusp_leaf(&n, args, FDest::Void);
                            } else {
                                self.fsusp_call(&n, args, FDest::Void);
                            }
                            return;
                        }
                    }
                }
                let v = self.expr(e);
                let _ = writeln!(self.out, "    {v};");
            }
            Stmt::Defer { call, .. } => {
                self.stmt_defer(call);
            }
            Stmt::ExitBoundary { .. } => {
                self.fscope_drain(0);
                self.emit_defers();
                self.exit_drained = true;
            }
            Stmt::Return { vals, .. } => {
                if !self.exit_drained {
                    self.fscope_drain(0);
                    self.emit_defers();
                }
                self.exit_drained = false;
                if !vals.is_empty() {
                    if self.cur_ret_pair && vals.len() == 1 {
                        if let Some(cn) = self.stmt_susp_call(&vals[0]).map(str::to_string) {
                            // `return suspPair()` — run the child through
                            // the frame machinery, convert its .val
                            if let ExprKind::Call { args, .. } = &vals[0].kind {
                                if let Some((_, vt)) = self.pair_call_info(&vals[0]) {
                                    let n = self.tmp();
                                    let ct = self.cty(&vt);
                                    let pa = self.fslot(&format!("_pa{n}"), &ct);
                                    let pe = self.fslot(&format!("_pe{n}"), "int64_t");
                                    self.fsusp_call(&cn, args,
                                        FDest::Pair(Some(pa.clone()), Some(pe.clone())));
                                    let want = self.cur_ret_ty.clone();
                                    let cv = self.conv_v(&vals[0], format!("fr->{pa}"), &want);
                                    let cf = self.cur_fn.clone();
                                    let _ = writeln!(self.out,
                                        "    fr->result = (struct o_ret_{}){{ {cv}, fr->{pe} }};", cname(&cf));
                                    self.susp_ret("RT_READY");
                                    return;
                                }
                                // retk==1 susp call — child's scalar
                                // result lands in a slot, then converts
                                let n = self.tmp();
                                let rt = match self.call_plan_ret(&vals[0]) {
                                    Some(crate::check::Ret::One(t)) => self.cty(&t),
                                    _ => "int64_t".to_string(),
                                };
                                let fd = self.fslot(&format!("_rv{n}"), &rt);
                                if matches!(cn.as_str(), "recv" | "send" | "sleep" | "msleep") {
                                    self.fsusp_leaf(&cn, args, FDest::Field(fd.clone()));
                                } else {
                                    self.fsusp_call(&cn, args, FDest::Field(fd.clone()));
                                }
                                let want = self.cur_ret_ty.clone();
                                let cv = self.conv_v(&vals[0], format!("fr->{fd}"), &want);
                                let _ = writeln!(self.out, "    fr->result = {cv};");
                                self.susp_ret("RT_READY");
                                return;
                            }
                        }
                    }
                    if self.cur_ret_pair && vals.len() == 2 {
                        let want = self.cur_ret_ty.clone();
                        let v = self.conv(&vals[0], &want);
                        let e = self.expr(&vals[1]);
                        let cf = self.cur_fn.clone();
                        let _ = writeln!(self.out,
                            "    fr->result = (struct o_ret_{}){{ {v}, {e} }};", cname(&cf));
                    } else if self.cur_ret_pair {
                        // non-susp `return paircall()` inside a susp fn —
                        // same _fw rewrap as the stack path
                        if let Some((tag, _)) = self.pair_call_info(&vals[0]) {
                            let c = self.expr(&vals[0]);
                            let cf = self.cur_fn.clone();
                            let cur = format!("o_ret_{}", cname(&cf));
                            if tag != cur {
                                let want = self.cur_ret_ty.clone();
                                let cv = self.conv_v(&vals[0], "_fw.val".to_string(), &want);
                                let _ = writeln!(self.out,
                                    "    {{ struct {tag} _fw = {c}; fr->result = (struct {cur}){{ {cv}, _fw.err }}; }}");
                            } else {
                                let _ = writeln!(self.out, "    fr->result = {c};");
                            }
                        } else {
                            let want = self.cur_ret_ty.clone();
                            let v = self.conv(&vals[0], &want);
                            let _ = writeln!(self.out, "    fr->result = {v};");
                        }
                    } else {
                        let want = self.cur_ret_ty.clone();
                        let v = self.conv(&vals[0], &want);
                        let _ = writeln!(self.out, "    fr->result = {v};");
                    }
                }
                self.susp_ret("RT_READY");
            }
            Stmt::If { cond, then, els, .. } => {
                let c = self.expr(cond);
                let lelse = self.new_label();
                let lend = self.new_label();
                let _ = writeln!(self.out, "    if (!({c})) goto {lelse};");
                self.env.push(HashMap::new());
                self.fslots.push(HashMap::new());
                self.fstmts(&then.stmts);
                self.fslots.pop();
                self.env.pop();
                let _ = writeln!(self.out, "    goto {lend};\n    {lelse}: ;");
                if let Some(e) = els {
                    self.env.push(HashMap::new());
                    self.fslots.push(HashMap::new());
                    self.fstmts(&e.stmts);
                    self.fslots.pop();
                    self.env.pop();
                }
                let _ = writeln!(self.out, "    {lend}: ;");
            }
            Stmt::For { cond, body, .. } => {
                let lhead = self.new_label();
                let lend = self.new_label();
                let c = self.expr(cond);
                let _ = writeln!(self.out, "    {lhead}: ;\n    if (!({c})) goto {lend};");
                self.loop_labels.push((
                    lhead.clone(),
                    lend.clone(),
                    self.fscope_stack.len(),
                ));
                self.env.push(HashMap::new());
                self.fslots.push(HashMap::new());
                self.fstmts(&body.stmts);
                self.fslots.pop();
                self.env.pop();
                self.loop_labels.pop();
                let _ = writeln!(self.out, "    goto {lhead};\n    {lend}: ;");
            }
            Stmt::Break { .. } => {
                if let Some((_, lend, depth)) = self.loop_labels.last() {
                    let (lend, depth) = (lend.clone(), *depth);
                    self.fscope_drain(depth);
                    let _ = writeln!(self.out, "    goto {lend};");
                }
            }
            Stmt::Continue { .. } => {
                if let Some((lhead, _, depth)) = self.loop_labels.last() {
                    let (lhead, depth) = (lhead.clone(), *depth);
                    self.fscope_drain(depth);
                    let _ = writeln!(self.out, "    goto {lhead};");
                }
            }
            Stmt::Scope { body, .. } => {
                let i = self.scope_decls.len();
                self.scope_decls.push(format!("RtScope sc{i};"));
                let _ = writeln!(self.out, "    rt_scope_enter(&fr->sc{i});");
                self.fscope_stack.push(i);
                self.env.push(HashMap::new());
                self.fslots.push(HashMap::new());
                self.fstmts(&body.stmts);
                self.fslots.pop();
                self.env.pop();
                self.fscope_stack.pop();
                self.fscope_close(i);
            }
            Stmt::Spawn { call, blocking, .. } => self.spawn(call, *blocking),
            Stmt::Select { arms, .. } => self.fselect(arms),
            Stmt::Unsafe { body, .. } => {
                self.env.push(HashMap::new());
                self.fslots.push(HashMap::new());
                self.fstmts(&body.stmts);
                self.fslots.pop();
                self.env.pop();
            }
        }
    }

    /// select { v := recv(ch) => .. timeout(ms) => .. else => .. }
    /// — parks on all arm chans; the first fire wins (uniform random
    /// among ready arms); resume dispatches to the winning arm's body.
    fn fselect(&mut self, arms: &[SelectArm]) {
        // evaluate arm chans / timeout ms once, before the suspend
        let mut chs: Vec<usize> = Vec::new(); // arm index per chs[] slot
        let mut cvs: Vec<String> = Vec::new();
        let mut tmo = "-1".to_string();
        let mut has_else = false;
        for (i, arm) in arms.iter().enumerate() {
            match &arm.kind {
                SelKind::Recv { chan, .. } => {
                    cvs.push(self.expr(chan));
                    chs.push(i);
                }
                SelKind::Timeout { ms, .. } => tmo = self.expr(ms),
                SelKind::Else { .. } => has_else = true,
            }
        }
        let n = cvs.len();
        let k = self.state_ctr;
        self.state_ctr += 1;
        self.state_labels.push(k);
        let _ = writeln!(self.out,
            "    if (rt_cancelled()) {{ fr->cur_scope = tls_scope; tls_scope = saved; return RT_CANCEL; }}");
        let _ = writeln!(self.out, "    fr->state = {k};");
        let _ = writeln!(self.out, "    {{ RtChan *_sc[{n}];");
        for (j, c) in cvs.iter().enumerate() {
            let _ = writeln!(self.out, "      _sc[{j}] = {c};");
        }
        let _ = writeln!(self.out,
            "      if (rt_select_poll(_sc, {n}, {tmo}, {}, task) == RT_PENDING) {{ fr->cur_scope = tls_scope; tls_scope = saved; return RT_PENDING; }} }}",
            if has_else { 1 } else { 0 });
        let _ = writeln!(self.out, "    s{k}: ;");
        // resume: observe cancel before consuming the delivered value
        let _ = writeln!(self.out,
            "    if (rt_cancelled()) {{ fr->cur_scope = tls_scope; tls_scope = saved; return RT_CANCEL; }}");
        // dispatch: recv slot j → arm chs[j]; RT_SEL_TMO → timeout arm;
        // RT_SEL_ELSE → else arm
        let lend = self.new_label();
        let mut arm_labels: Vec<String> = Vec::new();
        let mut cases: Vec<(i64, usize)> = Vec::new();
        let mut tmo_arm = None;
        let mut else_arm = None;
        for (i, arm) in arms.iter().enumerate() {
            let l = self.new_label();
            arm_labels.push(l.clone());
            match &arm.kind {
                SelKind::Recv { .. } => {
                    let slot = chs.iter().position(|&x| x == i).unwrap();
                    cases.push((slot as i64, i));
                }
                SelKind::Timeout { .. } => tmo_arm = Some(i),
                SelKind::Else { .. } => else_arm = Some(i),
            }
        }
        let _ = writeln!(self.out, "    switch ((int)rt_select_arm(task)) {{");
        for (slot, i) in &cases {
            let _ = writeln!(self.out, "      case {slot}: goto {};", arm_labels[*i]);
        }
        if let Some(i) = tmo_arm {
            let _ = writeln!(self.out, "      case RT_SEL_TMO: goto {};", arm_labels[i]);
        }
        if let Some(i) = else_arm {
            let _ = writeln!(self.out, "      case RT_SEL_ELSE: goto {};", arm_labels[i]);
        }
        let _ = writeln!(self.out, "      default: goto {lend};\n    }}");
        for (i, arm) in arms.iter().enumerate() {
            let _ = writeln!(self.out, "    {}: ;", arm_labels[i]);
            if let SelKind::Recv { bind: Some(name), .. } = &arm.kind {
                let fd = self.fslot(name, "int64_t");
                self.env.last_mut().unwrap().insert(name.clone(), Ty::Int);
                let _ = writeln!(self.out, "    fr->{fd} = rt_task_data(task);");
            }
            self.env.push(HashMap::new());
            self.fslots.push(HashMap::new());
            self.fstmts(&arm.body.stmts);
            self.fslots.pop();
            self.env.pop();
            let _ = writeln!(self.out, "    goto {lend};");
        }
        let _ = writeln!(self.out, "    {lend}: ;");
    }

    /// emit `struct o_frm_<e>` + `int o_poll_<e>` driver for a susp fn
    /// (or its monomorphized instantiation when `ename` is mangled)
    fn func_frame(&mut self, f: &Fn, ename: &str) {
        let hoisted = self.hoist_fn(f);
        self.in_frame = true;
        self.fslots = vec![HashMap::new()];
        self.frame_decls = Vec::new();
        self.scope_decls = Vec::new();
        self.fscope_stack = Vec::new();
        self.state_labels = Vec::new();
        self.state_ctr = 1;
        self.loop_labels = Vec::new();
        self.cur_fn = ename.to_string();
        self.cur_ret_pair = matches!(f.ret, RetTy::Pair(_));
        self.defers.clear();
        self.env.push(HashMap::new());
        let mut pfields = String::new();
        for p in &f.params {
            let t = self.ty_of_ref(&p.ty);
            let _ = write!(pfields, " {} p_{};", self.cty(&t), p.name);
            self.env.last_mut().unwrap().insert(p.name.clone(), t);
            self.fslots[0].insert(p.name.clone(), format!("p_{}", p.name));
        }
        let res_field = match &f.ret {
            RetTy::Void => String::new(),
            RetTy::One(t) => format!(" {} result;", self.cty(&self.ty_of_ref(t))),
            RetTy::Pair(_) => format!(" struct o_ret_{} result;", cname(ename)),
        };
        let saved_out = std::mem::take(&mut self.out);
        // defer mark must live on the frame — a suspend between push and
        // exit would drop a C-local mark
        let _ = writeln!(self.out, "    fr->_dmark = rt_defer_mark();");
        self.fslots.push(HashMap::new());
        if let Some(b) = &hoisted.body {
            self.fstmts(&b.stmts);
        }
        self.fslots.pop();
        self.emit_defers();
        let body = std::mem::replace(&mut self.out, saved_out);
        self.env.pop();
        let mut cases = String::from("case 0: goto s0;");
        for k in &self.state_labels {
            let _ = write!(cases, " case {k}: goto s{k};");
        }
        let ce = cname(ename);
        let _ = writeln!(self.frame_head,
            "struct o_frm_{ce} {{ RtScope *cur_scope; int state; RtFrame *child; RtDefer *_dmark;{res_field}{pfields} {}}};\nint o_poll_{ce}(RtFrame *_f, RtTask *task);\n",
            self.frame_decls.join(" " ) + " " + &self.scope_decls.join(" "));
        let _ = writeln!(self.frame_out,
            "int o_poll_{ce}(RtFrame *_f, RtTask *task) {{\n    struct o_frm_{ce} *fr = (struct o_frm_{ce} *)_f;\n    RtScope *saved = tls_scope;\n    tls_scope = fr->cur_scope;\n    int _p;\n    switch (fr->state) {{ {cases} default: tls_scope = saved; return RT_READY; }}\n    s0: ;\n{body}    fr->cur_scope = tls_scope; tls_scope = saved; return RT_READY;\n}}\n");
        self.in_frame = false;
    }
}

/// where a suspend point's result lands
enum FDest {
    Void,
    Field(String),
    Assign(String),
    Pair(Option<String>, Option<String>),
}

fn clone_bind(b: &Bind) -> Bind {
    match b {
        Bind::Name(n) => Bind::Name(n.clone()),
        Bind::Blank => Bind::Blank,
    }
}
