use crate::ast::*;
use crate::check::{subst_ty, Ty, Ret};
use std::collections::HashMap;
use std::fmt::Write as _;

const RT_H: &str = include_str!("../../runtime/rt.h");
const RT_C: &str = include_str!("../../runtime/rt.c");

pub fn runtime_files() -> (&'static str, &'static str) {
    (RT_H, RT_C)
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
    test_mode: bool,
}

pub fn generate(c: &Crate) -> String {
    generate_mode(c, false)
}

fn mangle_ty(t: &Ty) -> String {
    t.name().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect()
}

pub fn generate_mode(c: &Crate, test_mode: bool) -> String {
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
        test_mode,
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
            let _ = writeln!(out, "struct l_s_{} {{", s.name);
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
                        out, "struct l_ret_{} {{ {} val; int64_t err; }};",
                        f.name, g.cty(&g.ty_of_ref(t))
                    );
                }
            }
        }
    }

    // file-scope globals (initialized in lang_main prologue)
    for it in &c.items {
        if let Item::Global(gl) = it {
            let t = g.globals.get(&gl.name).cloned().unwrap_or(Ty::Int);
            let _ = writeln!(out, "static {} l_g_{};", g.cty(&t), gl.name);
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
    // global initializers run first inside lang_main (base scope exists)
    let mut global_inits = String::new();
    for it in &c.items {
        if let Item::Global(gl) = it {
            let v = g.expr(&gl.init);
            let _ = writeln!(global_inits, "    l_g_{} = {v};", gl.name);
        }
    }
    if test_mode {
        let _ = writeln!(g.out, "void lang_main(void) {{");
        g.out.push_str(&global_inits);
        let mut id = 0usize;
        for it in &c.items {
            if let Item::Fn(f) = it {
                if f.name.starts_with("test_") {
                    let _ = writeln!(g.out,
                        "    {{ RtScope _t{id}; rt_scope_enter(&_t{id}); l_f_{n}(); int _e{id} = rt_scope_try(&_t{id});\n      rt_print_str(rt_str(\"  {n} ... \")); rt_println_str(rt_str(_e{id} ? \"FAIL\" : \"ok\")); }}",
                        n = f.name, id = id);
                    id += 1;
                }
            }
        }
        let _ = writeln!(g.out, "}}\n");
    }

    // trampolines must precede the fn bodies that reference them
    g.out = g.out.replace("/*GLOBALS*/", &global_inits);
    let mut final_out = g.out[..body_start].to_string();
    final_out.push_str(&g.trampolines);
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
            Ty::Bool => "int".into(),
            Ty::Str => "LangStr".into(),
            Ty::CStr => "const char*".into(),
            Ty::Void => "void".into(),
            Ty::Chan => "RtChan*".into(),
            Ty::Struct(n) => format!("struct l_s_{n}"),
            Ty::Var(v) => self
                .subst
                .get(v)
                .map(|t| self.cty(t))
                .unwrap_or_else(|| "int64_t".into()),
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match &**b {
                Ty::Struct(n) => format!("struct l_s_{n}*"),
                Ty::Var(v) => match self.subst.get(v) {
                    Some(Ty::Struct(n)) => format!("struct l_s_{n}*"),
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
            (format!("l_f_{}", f.name), self.ret_decl(f, true))
        };
        let params: Vec<String> = f
            .params
            .iter()
            .map(|p| format!("{} {}", self.pty(&self.ty_of_ref(&p.ty)), p.name))
            .collect();
        let params = if params.is_empty() { "void".into() } else { params.join(", ") };
        let _ = writeln!(out, "{ret_c} {name}({params});");
    }

    fn ret_decl(&self, f: &Fn, is_lang: bool) -> String {
        match &f.ret {
            RetTy::Void => "void".into(),
            RetTy::One(t) => self.pty(&self.ty_of_ref(t)),
            RetTy::Pair(_) => {
                if is_lang {
                    format!("struct l_ret_{}", f.name)
                } else {
                    "int64_t".into()
                }
            }
        }
    }

    fn func(&mut self, f: &Fn) {
        let ret_c = self.ret_decl(f, true);
        let params: Vec<String> = f
            .params
            .iter()
            .map(|p| format!("{} {}", self.pty(&self.ty_of_ref(&p.ty)), p.name))
            .collect();
        let params = if params.is_empty() { "void".into() } else { params.join(", ") };
        let _ = writeln!(self.out, "{ret_c} l_f_{}({params}) {{", f.name);
        self.cur_fn = f.name.clone();
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
        self.env.pop();
        let _ = writeln!(self.out, "}}\n");
        if f.name == "main" && !self.test_mode {
            let _ = writeln!(self.out, "void lang_main(void) {{ /*GLOBALS*/ l_f_main(); }}\n");
        }
    }

    /// emit a monomorphized copy of generic fn `f` as `l_f_{mangled}`
    fn func_inst(&mut self, f: &Fn, mangled: &str, bind: &[(String, Ty)]) {
        let saved_subst = std::mem::replace(
            &mut self.subst,
            bind.iter().cloned().collect::<HashMap<_, _>>(),
        );
        let saved_out = std::mem::take(&mut self.out);
        // ret struct (if [T, err]) + prototype into inst_out
        if let RetTy::Pair(t) = &f.ret {
            let _ = writeln!(
                self.inst_out,
                "struct l_ret_{mangled} {{ {} val; int64_t err; }};",
                self.cty(&self.ty_of_ref(t))
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
        let _ = writeln!(self.out, "{ret_c} l_f_{mangled}({params}) {{");
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
        let _ = writeln!(out, "{ret_c} l_f_{name}({params});");
    }

    fn ret_decl_named(&self, f: &Fn, name: &str) -> String {
        match &f.ret {
            RetTy::Void => "void".into(),
            RetTy::One(t) => self.pty(&self.ty_of_ref(t)),
            RetTy::Pair(_) => format!("struct l_ret_{name}"),
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
                        "recv" | "argc" | "len" | "byteat" | "find" | "atoi"
                        | "htons" | "htonl" | "sqlite_exec" | "writefile" | "sget" | "scas"
                        | "sadd" | "now" | "now_ms" => {
                            return Ty::Int
                        }
                        "argv" | "itoa" | "substr" | "bufstr" | "sqlite_query" | "readfile" => {
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
                    if self.dt(&self.expr_ty(l)) == Ty::Str
                        && self.dt(&self.expr_ty(r)) == Ty::Str
                    {
                        Ty::Str
                    } else {
                        Ty::Int
                    }
                }
                _ => Ty::Int,
            },
            ExprKind::Un { op, .. } => match op {
                UnOp::Not => Ty::Bool,
                UnOp::Neg => Ty::Int,
            },
        }
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
                        let _ = writeln!(self.out, "    struct l_ret_{ret_name} _r{n} = {c};");
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
            Stmt::Return { vals, .. } => {
                if vals.is_empty() {
                    let _ = writeln!(self.out, "    return;");
                } else if self.cur_ret_pair && vals.len() == 2 {
                    let v = self.expr(&vals[0]);
                    let e = self.expr(&vals[1]);
                    let cf = self.cur_fn.clone();
                    let _ = writeln!(self.out, "    return (struct l_ret_{cf}){{ {v}, {e} }};");
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
                let id = self.tmp();
                // args struct + trampoline at file scope
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.expr_ty(a)).collect();
                let mut tr = format!("struct l_args_{id} {{");
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
                    None => format!("l_f_{fn_name}({});", arg_refs.join(", ")),
                },
            };
            tr.push_str(&format!(
                "static void* l_tr_{id}(void* _p) {{ struct l_args_{id}* a = _p; {body} return 0; }}\n"
            ));
                self.trampolines.push_str(&tr);
                // call site
                let _ = writeln!(self.out,
                    "    {{ struct l_args_{id}* a = rt_alloc(sizeof(struct l_args_{id}));");
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
            ExprKind::Str(s) => format!("rt_str_lit(\"{}\")", c_escape(s)),
            ExprKind::Bool(b) => format!("{b}"),
            ExprKind::Nil => "0".into(),
            ExprKind::Ident(n) => {
                // locals shadow globals; a bare global emits l_g_name
                let local = self.env.iter().skip(1).any(|s| s.contains_key(n));
                if !local && self.globals.contains_key(n) {
                    format!("l_g_{n}")
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
                format!("(struct l_s_{name}){{ {init} }}")
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
            | "sadd" | "now" | "now_ms" | "usleep" => {
                format!("rt_{name}({})", j(argvs))
            }
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
                        return format!("(struct l_s_{sn}*)rt_alloc(sizeof(struct l_s_{sn}))");
                    }
                }
                _ => {
                    if let Some(b) = self.builtin_emit(name, &argvs) {
                        return b;
                    }
                    let lang = self.fns.contains_key(name) && !self.externs.contains(name);
                    let callee = if lang {
                        if let Some(bind) = call_bindings(self, name, args) {
                            let m = inst_name(name, &bind);
                            if !self.inst_done.contains(&m) {
                                self.insts.push((name.clone(), bind));
                            }
                            format!("l_f_{m}")
                        } else {
                            format!("l_f_{name}")
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
                                        "({{ struct l_ret_{} __rc; {pre}__rc = {inner}; {post}__rc; }})",
                                        callee.trim_start_matches("l_f_")
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
}
