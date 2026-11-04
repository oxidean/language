use crate::ast::*;
use crate::check::{subst_ty, Ty, Ret};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

const RT_H: &str = include_str!("../../runtime/rt.h");
const RT_C: &str = include_str!("../../runtime/rt.c");

pub fn runtime_files() -> (&'static str, &'static str) {
    (RT_H, RT_C)
}

/// oxl `mod.name` -> C identifier `mod_name`
fn cname(n: &str) -> String {
    n.replace('.', "_")
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

struct Gen<'a> {
    out: String,
    trampolines: String,
    inst_out: String,
    tmp: usize,
    scope_id: usize,
    env: Vec<HashMap<String, Ty>>,
    structs: HashMap<String, Vec<(String, Ty)>>,
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
    /// collected local field decls for the frame struct
    frame_decls: Vec<String>,
    /// RtScope fields (one per `scope {}` site in this fn)
    scope_decls: Vec<String>,
    /// resume states used (for the switch prologue)
    state_labels: Vec<usize>,
    state_ctr: usize,
    label_ctr: usize,
    /// (continue-label, break-label) per enclosing loop
    loop_labels: Vec<(String, String)>,
    hoist_ctr: usize,
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
        tmp: 0,
        scope_id: 0,
        env: vec![HashMap::new()],
        structs: HashMap::new(),
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
        cur_fn: String::new(),
        defers: Vec::new(),
        test_mode,
        susp: susp.clone(),
        in_frame: false,
        fslots: Vec::new(),
        frame_decls: Vec::new(),
        scope_decls: Vec::new(),
        state_labels: Vec::new(),
        state_ctr: 1,
        label_ctr: 0,
        loop_labels: Vec::new(),
        hoist_ctr: 0,
        frame_out: String::new(),
    };
    for it in &c.items {
        match it {
            Item::Struct(s) => {
                g.structs.insert(
                    s.name.clone(),
                    s.fields.iter().map(|f| (f.name.clone(), g.ty_of_ref(&f.ty))).collect(),
                );
            }
            Item::Fn(f) | Item::Extern(f) => {
                g.cur_tyvars = f.ty_params.iter().map(|(t, _)| t.clone()).collect();
                let params = f.params.iter().map(|p| g.ty_of_ref(&p.ty)).collect();
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
    // monomorphized instantiations (drain until stable — instantiations
    // can themselves call generics)
    while let Some((fname, bind)) = g.insts.pop() {
        let mangled = inst_name(&fname, &bind);
        if !g.inst_done.insert(mangled.clone()) {
            continue;
        }
        if let Some(f) = g.ast_fns.get(&fname).cloned() {
            g.func_inst(f, &mangled, &bind);
        }
    }
    // global initializers run first inside oxl_main (base scope exists)
    let mut global_inits = String::new();
    for it in &c.items {
        if let Item::Global(gl) = it {
            let v = g.expr(&gl.init);
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
    let mut final_out = g.out[..body_start].to_string();
    final_out.push_str(&g.trampolines);
    final_out.push_str(&g.frame_out);
    final_out.push_str(&g.inst_out);
    final_out.push_str(&g.out[body_start..]);
    final_out
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
                s => {
                    if let Some(b) = self.subst.get(s) {
                        b.clone()
                    } else if self.cur_tyvars.contains(s) {
                        Ty::Var(s.to_string())
                    } else {
                        Ty::Struct(s.to_string())
                    }
                }
            },
            TyKind::Own(t) => Ty::Own(Box::new(self.ty_of_ref(t))),
            TyKind::Ref(t) => Ty::Ref(Box::new(self.ty_of_ref(t))),
            TyKind::Mut(t) => Ty::Mut(Box::new(self.ty_of_ref(t))),
            TyKind::Chan => Ty::Chan,
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
            Ty::Struct(n) => format!("struct o_s_{}", cname(n)),
            Ty::Var(v) => self
                .subst
                .get(v)
                .map(|t| self.cty(t))
                .unwrap_or_else(|| "int64_t".into()),
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match &**b {
                Ty::Struct(n) => format!("struct o_s_{}*", cname(n)),
                Ty::Var(v) => match self.subst.get(v) {
                    Some(Ty::Struct(n)) => format!("struct o_s_{}*", cname(n)),
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
        let params: Vec<String> = f
            .params
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
        let params: Vec<String> = f
            .params
            .iter()
            .map(|p| format!("{} {}", self.pty(&self.ty_of_ref(&p.ty)), p.name))
            .collect();
        let params = if params.is_empty() { "void".into() } else { params.join(", ") };
        let _ = writeln!(self.out, "{ret_c} o_f_{}({params}) {{", cname(&f.name));
        self.cur_fn = f.name.clone();
        self.cur_ret_pair = matches!(f.ret, RetTy::Pair(_));
        self.defers.clear();
        self.env.push(HashMap::new());
        for p in &f.params {
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
                    self.frame_out,
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
        // ret struct (if [T, err]) + prototype into inst_out
        if let RetTy::Pair(t) = &f.ret {
            let _ = writeln!(
                self.inst_out,
                "struct o_ret_{} {{ {} val; int64_t err; }};",
                cname(&mangled), self.cty(&self.ty_of_ref(t))
            );
        }
        let mut proto = String::new();
        self.proto_named(&mut proto, f, mangled);
        self.inst_out.push_str(&proto);
        // body — emitted into the swapped-out self.out (stmt() writes there)
        let ret_c = self.ret_decl_named(f, mangled);
        let params: Vec<String> = f
            .params
            .iter()
            .map(|p| format!("{} {}", self.pty(&self.ty_of_ref(&p.ty)), p.name))
            .collect();
        let params = if params.is_empty() { "void".into() } else { params.join(", ") };
        let _ = writeln!(self.out, "{ret_c} o_f_{}({params}) {{", cname(&mangled));
        self.cur_fn = mangled.to_string();
        self.cur_ret_pair = matches!(f.ret, RetTy::Pair(_));
        self.env.push(HashMap::new());
        for p in &f.params {
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
        let params: Vec<String> = f
            .params
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
            ExprKind::Int(_) => Ty::Int,
            ExprKind::Float(_) => Ty::Float,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Nil => Ty::Err,
            ExprKind::Ident(n) => self.lookup(n),
            ExprKind::Call { f, args } => {
                if let ExprKind::Ident(n) = &f.kind {
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
                        "recv" | "argc" | "len" | "byteat" | "find" | "atoi" | "ftoi"
                        | "htons" | "htonl" | "sqlite_exec" | "writefile" | "sget" | "scas"
                        | "sadd" | "now" | "now_ms" => {
                            return Ty::Int
                        }
                        "itof" | "atof" => return Ty::Float,
                        "argv" | "itoa" | "substr" | "bufstr" | "sqlite_query" | "readfile" | "ftoa" => {
                            return Ty::Str
                        }
                        "cstr" => return Ty::CStr,
                        "mkbuf" | "sqlite_open" | "mkshared" => return Ty::Own(Box::new(Ty::Unknown)),
                        "new" => {
                            if let ExprKind::Call { args, .. } = &e.kind {
                                if let Some(Expr { kind: ExprKind::Ident(sn), .. }) = args.first() {
                                    return Ty::Own(Box::new(Ty::Struct(sn.clone())));
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
            ExprKind::StructLit { name, .. } => Ty::Struct(name.clone()),
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

    /// emit registered defers in LIFO order (fn exit paths)
    fn emit_defers(&mut self) {
        let defs = std::mem::take(&mut self.defers);
        for d in defs.iter().rev() {
            let v = self.expr(d);
            let _ = writeln!(self.out, "    {v};");
        }
        self.defers = defs;
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { name, init, .. } => {
                let t = self.expr_ty(init);
                let ct = self.cty(&t);
                let v = self.expr(init);
                let _ = writeln!(self.out, "    {ct} {name} = {v};");
                self.env.last_mut().unwrap().insert(name.clone(), t);
            }
            Stmt::Let2 { a, b, call, .. } => {
                if let ExprKind::Call { f, args } = &call.kind {
                    if let ExprKind::Ident(fn_name) = &f.kind {
                        let n = self.tmp();
                        let c = self.expr(call);
                        let (ret_name, bind) = match call_bindings(self, fn_name, args) {
                            Some(bind) => (inst_name(fn_name, &bind), Some(bind)),
                            None => (fn_name.clone(), None),
                        };
                        let _ = writeln!(self.out, "    struct o_ret_{} _r{n} = {c};", cname(&ret_name));
                        let vt = match self.fns.get(fn_name).map(|x| x.1.clone()) {
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
                let v = self.expr(val);
                let _ = writeln!(self.out, "    {t} = {v};");
            }
            Stmt::Expr(e) => {
                let v = self.expr(e);
                let _ = writeln!(self.out, "    {v};");
            }
            Stmt::Defer { call, .. } => {
                // register: call runs at fn exit, LIFO (see emit_defers)
                self.defers.push(call.clone());
            }
            Stmt::Return { vals, .. } => {
                self.emit_defers();
                if vals.is_empty() {
                    let _ = writeln!(self.out, "    return;");
                } else if self.cur_ret_pair && vals.len() == 2 {
                    let v = self.expr(&vals[0]);
                    let e = self.expr(&vals[1]);
                    let cf = self.cur_fn.clone();
                    let _ = writeln!(self.out, "    return (struct o_ret_{}){{ {v}, {e} }};", cname(&cf));
                } else if self.cur_ret_pair && vals.len() == 1 {
                    let c = self.expr(&vals[0]);
                    let _ = writeln!(self.out, "    return {c};");
                } else {
                    let v = self.expr(&vals[0]);
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
                    for (i, arg) in args.iter().enumerate() {
                        let v = self.expr(arg);
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
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.expr_ty(a)).collect();
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
                    let v = self.expr(arg);
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
                } else {
                    n.clone()
                }
            }
            ExprKind::Call { f, args } => self.call(f, args),
            ExprKind::Field { obj, name } => {
                let o = self.expr(obj);
                match self.expr_ty(obj) {
                    Ty::Own(_) | Ty::Ref(_) | Ty::Mut(_) => format!("{o}->f_{name}"),
                    _ => format!("{o}.f_{name}"),
                }
            }
            ExprKind::StructLit { name, fields } => {
                let init = fields
                    .iter()
                    .map(|(n, fe)| format!(".f_{n} = {}", self.expr(fe)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("(struct o_s_{}){{ {init} }}", cname(&name))
            }
            ExprKind::Bin { op, l, r } => {
                let lt = self.expr_ty(l);
                let lv = self.expr(l);
                let rv = self.expr(r);
                let cop = match op {
                    BinOp::Add => "+", BinOp::Sub => "-", BinOp::Mul => "*",
                    BinOp::Div => "/", BinOp::Mod => "%",
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
            "mkchan" => "rt_mkchan()".into(),
            "send" => format!("rt_send({})", j(argvs)),
            "recv" => format!("rt_recv({})", a0),
            "sleep" => format!("rt_sleep({})", a0),
            "itoa" => format!("rt_itoa({})", a0),
            "argc" => "rt_argc()".into(),
            "argv" => format!("rt_argv({})", a0),
            "len" => format!("rt_len({})", a0),
            "byteat" | "substr" | "find" | "bufstr" | "poke8" | "poke16" | "poke32"
            | "sqlite_exec" | "sqlite_query" => format!("rt_{name}({})", j(argvs)),
            "atoi" | "mkbuf" | "htons" | "htonl" | "cstr" | "sqlite_open" | "sqlite_close"
            | "readfile" | "writefile" | "exit" | "mkshared" | "sget" | "sset" | "scas"
            | "sadd" | "now" | "now_ms" | "usleep" | "atof" | "ftoa" => {
                format!("rt_{name}({})", j(argvs))
            }
            "ftoi" => format!("((int64_t)({a0}))"),
            "itof" => format!("((double)({a0}))"),
            _ => return None,
        })
    }

    fn call(&mut self, f: &Expr, args: &[Expr]) -> String {
        let argvs: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
        if let ExprKind::Ident(name) = &f.kind {
            match name.as_str() {
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
                        return format!("(struct o_s_{0}*)rt_alloc(sizeof(struct o_s_{0}))", cname(&sn));
                    }
                }
                _ => {
                    if let Some(b) = self.builtin_emit(name, &argvs) {
                        return b;
                    }
                    let oxl = self.fns.contains_key(name) && !self.externs.contains(name);
                    let callee = if oxl {
                        if let Some(bind) = call_bindings(self, name, args) {
                            let m = inst_name(name, &bind);
                            if !self.inst_done.contains(&m) {
                                self.insts.push((name.clone(), bind));
                            }
                            format!("o_f_{}", cname(&m))
                        } else {
                            format!("o_f_{}", cname(&name))
                        }
                    } else {
                        name.clone()
                    };
                    // str -> cstr coercion + borrow checks for ref/mut params
                    if let Some((params, ret)) = self.fns.get(name).cloned() {
                        let smap: HashMap<String, Ty> = call_bindings(self, name, args)
                            .map(|b| b.into_iter().collect())
                            .unwrap_or_default();
                        let mut pre = String::new();
                        let mut post = String::new();
                        let mut coerced: Vec<String> = Vec::new();
                        for (a, pt) in args.iter().zip(params.iter()) {
                            let pt = if smap.is_empty() {
                                self.dt(pt)
                            } else {
                                crate::check::subst_ty(pt, &smap)
                            };
                            let mut v = self.expr(a);
                            let raw_at = self.expr_ty(a);
                            if pt == Ty::CStr && self.dt(&raw_at) == Ty::Str {
                                v = format!("rt_cstr({v})");
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
        self.susp.contains(n) || matches!(n, "recv" | "send" | "sleep")
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
            ExprKind::StructLit { fields, .. } => {
                fields.iter().any(|(_, fe)| self.expr_has_susp(fe))
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
        }
    }

    fn hoist_fn(&mut self, f: &Fn) -> Fn {
        Fn {
            name: f.name.clone(),
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
        let argvs: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
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
                            if matches!(n.as_str(), "recv" | "send" | "sleep") {
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
                        let (ret_name, bind) = match call_bindings(self, fn_name, args) {
                            Some(bind) => (inst_name(fn_name, &bind), Some(bind)),
                            None => (fn_name.clone(), None),
                        };
                        let _ = writeln!(self.out, "    {{ struct o_ret_{} _r{n} = {c};", cname(&ret_name));
                        let vt = match self.fns.get(fn_name).map(|x| x.1.clone()) {
                            Some(Ret::Pair(t)) => match &bind {
                                Some(bind) => {
                                    let m: HashMap<String, Ty> =
                                        bind.iter().cloned().collect();
                                    subst_ty(&t, &m)
                                }
                                None => self.dt(&t),
                            },
                            _ => Ty::Unknown,
                        };
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
                            if matches!(n.as_str(), "recv" | "send" | "sleep") {
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
                if self.call_is_susp(e) {
                    if let ExprKind::Call { f, args } = &e.kind {
                        if let ExprKind::Ident(n) = &f.kind {
                            let n = n.clone();
                            if matches!(n.as_str(), "recv" | "send" | "sleep") {
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
                self.defers.push(call.clone());
            }
            Stmt::Return { vals, .. } => {
                self.emit_defers();
                if !vals.is_empty() {
                    if self.cur_ret_pair && vals.len() == 2 {
                        let v = self.expr(&vals[0]);
                        let e = self.expr(&vals[1]);
                        let cf = self.cur_fn.clone();
                        let _ = writeln!(self.out,
                            "    fr->result = (struct o_ret_{}){{ {v}, {e} }};", cname(&cf));
                    } else {
                        let v = self.expr(&vals[0]);
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
                self.loop_labels.push((lhead.clone(), lend.clone()));
                self.env.push(HashMap::new());
                self.fslots.push(HashMap::new());
                self.fstmts(&body.stmts);
                self.fslots.pop();
                self.env.pop();
                self.loop_labels.pop();
                let _ = writeln!(self.out, "    goto {lhead};\n    {lend}: ;");
            }
            Stmt::Break { .. } => {
                if let Some((_, lend)) = self.loop_labels.last() {
                    let _ = writeln!(self.out, "    goto {lend};");
                }
            }
            Stmt::Continue { .. } => {
                if let Some((lhead, _)) = self.loop_labels.last() {
                    let _ = writeln!(self.out, "    goto {lhead};");
                }
            }
            Stmt::Scope { body, .. } => {
                let i = self.scope_decls.len();
                self.scope_decls.push(format!("RtScope sc{i};"));
                let _ = writeln!(self.out, "    rt_scope_enter(&fr->sc{i});");
                self.env.push(HashMap::new());
                self.fslots.push(HashMap::new());
                self.fstmts(&body.stmts);
                self.fslots.pop();
                self.env.pop();
                let k = self.state_ctr;
                self.state_ctr += 1;
                self.state_labels.push(k);
                let _ = writeln!(self.out,
                    "    fr->state = {k};\n    if (rt_scope_join_poll(&fr->sc{i}, task) == RT_PENDING) {{ fr->cur_scope = tls_scope; tls_scope = saved; return RT_PENDING; }}\n    s{k}: ;\n    {{ int64_t _e = rt_scope_finish(&fr->sc{i});\n      if (_e) {{ if (fr->sc{i}.parent) {{ fr->sc{i}.parent->trapped = 1; fr->sc{i}.parent->err = _e; }} else {{ fprintf(stderr, \"trap %lld escaped top scope\\n\", (long long)_e); abort(); }} }} }}");
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
        let _ = writeln!(self.frame_out,
            "struct o_frm_{ce} {{ RtScope *cur_scope; int state; RtFrame *child;{res_field}{pfields} {}}};\nint o_poll_{ce}(RtFrame *_f, RtTask *task);\n",
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
