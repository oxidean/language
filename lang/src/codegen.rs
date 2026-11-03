use crate::ast::*;
use crate::check::Ty;
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

struct Gen {
    out: String,
    trampolines: String,
    tmp: usize,
    scope_id: usize,
    env: Vec<HashMap<String, Ty>>,
    structs: HashMap<String, Vec<(String, Ty)>>,
    fns: HashMap<String, (Vec<Ty>, crate::check::Ret)>,
    externs: std::collections::HashSet<String>,
    cur_ret_pair: bool,
    cur_fn: String,
    test_mode: bool,
}

pub fn generate(c: &Crate) -> String {
    generate_mode(c, false)
}

pub fn generate_mode(c: &Crate, test_mode: bool) -> String {
    let mut g = Gen {
        out: String::new(),
        trampolines: String::new(),
        tmp: 0,
        scope_id: 0,
        env: vec![HashMap::new()],
        structs: HashMap::new(),
        fns: HashMap::new(),
        externs: std::collections::HashSet::new(),
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
                let params = f.params.iter().map(|p| g.ty_of_ref(&p.ty)).collect();
                let ret = match &f.ret {
                    RetTy::Void => crate::check::Ret::Void,
                    RetTy::One(t) => crate::check::Ret::One(g.ty_of_ref(t)),
                    RetTy::Pair(t) => crate::check::Ret::Pair(g.ty_of_ref(t)),
                };
                if matches!(it, Item::Extern(_)) {
                    g.externs.insert(f.name.clone());
                }
                g.fns.insert(f.name.clone(), (params, ret));
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
            if let RetTy::Pair(t) = &f.ret {
                let _ = writeln!(
                    out, "struct l_ret_{} {{ {} val; int64_t err; }};",
                    f.name, g.cty(&g.ty_of_ref(t))
                );
            }
        }
    }

    // fn prototypes + extern passthroughs
    for it in &c.items {
        match it {
            Item::Fn(f) => g.proto(&mut out, f, false),
            Item::Extern(f) => g.proto(&mut out, f, true),
            _ => {}
        }
    }

    g.out = out;
    let body_start = g.out.len();
    for it in &c.items {
        if let Item::Fn(f) = it {
            g.func(f);
        }
    }
    if test_mode {
        let _ = writeln!(g.out, "void lang_main(void) {{");
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
    let mut final_out = g.out[..body_start].to_string();
    final_out.push_str(&g.trampolines);
    final_out.push_str(&g.out[body_start..]);
    final_out
}

impl Gen {
    fn ty_of_ref(&self, t: &TyRef) -> Ty {
        match &t.kind {
            TyKind::Named(n) => match n.as_str() {
                "int" => Ty::Int,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "cstr" => Ty::CStr,
                "err" => Ty::Err,
                "void" => Ty::Void,
                s => Ty::Struct(s.to_string()),
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
            Ty::Own(b) | Ty::Ref(b) | Ty::Mut(b) => match &**b {
                Ty::Struct(n) => format!("struct l_s_{n}*"),
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
            let _ = writeln!(self.out, "void lang_main(void) {{ l_f_main(); }}\n");
        }
    }

    fn tmp(&mut self) -> usize {
        self.tmp += 1;
        self.tmp
    }

    fn lookup(&self, n: &str) -> Ty {
        self.env.iter().rev().find_map(|s| s.get(n).cloned()).unwrap_or(Ty::Unknown)
    }

    fn expr_ty(&self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Int(_) => Ty::Int,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Nil => Ty::Err,
            ExprKind::Ident(n) => self.lookup(n),
            ExprKind::Call { f, .. } => {
                if let ExprKind::Ident(n) = &f.kind {
                    if let Some((_, r)) = self.fns.get(n) {
                        return match r {
                            crate::check::Ret::One(t) => t.clone(),
                            _ => Ty::Void,
                        };
                    }
                    match n.as_str() {
                        "mkchan" => return Ty::Chan,
                        "recv" | "argc" | "len" | "byteat" | "find" | "atoi"
                        | "htons" | "htonl" | "sqlite_exec" => return Ty::Int,
                        "argv" | "itoa" | "substr" | "bufstr" | "sqlite_query" => {
                            return Ty::Str
                        }
                        "cstr" => return Ty::CStr,
                        "mkbuf" | "sqlite_open" => return Ty::Own(Box::new(Ty::Unknown)),
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
                if let Ty::Struct(sn) = ot {
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
                    if self.expr_ty(l) == Ty::Str && self.expr_ty(r) == Ty::Str {
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
                if let ExprKind::Call { f, .. } = &call.kind {
                    if let ExprKind::Ident(fn_name) = &f.kind {
                        let n = self.tmp();
                        let c = self.expr(call);
                        let _ = writeln!(self.out, "    struct l_ret_{fn_name} _r{n} = {c};");
                        let vt = match self.fns.get(fn_name).map(|x| x.1.clone()) {
                            Some(crate::check::Ret::Pair(t)) => t,
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
            Stmt::Spawn { call, .. } => self.spawn(call),
            Stmt::Unsafe { body, .. } => {
                self.env.push(HashMap::new());
                for s in &body.stmts {
                    self.stmt(s);
                }
                self.env.pop();
            }
        }
    }

    fn spawn(&mut self, call: &Expr) {
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
                let _ = writeln!(self.out, "      rt_spawn(l_tr_{id}, a); }}");
            }
        }
    }

    fn expr(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int(v) => format!("{v}LL"),
            ExprKind::Str(s) => format!("rt_str_lit(\"{}\")", c_escape(s)),
            ExprKind::Bool(b) => format!("{b}"),
            ExprKind::Nil => "0".into(),
            ExprKind::Ident(n) => n.clone(),
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
                if *op == BinOp::Add && lt == Ty::Str {
                    format!("rt_strcat({lv}, {rv})")
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
            "atoi" | "mkbuf" | "htons" | "htonl" | "cstr" | "sqlite_open" | "sqlite_close" => {
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
                    let callee = if lang { format!("l_f_{name}") } else { name.clone() };
                    // str -> cstr coercion + borrow checks for ref/mut params
                    if let Some((params, _)) = self.fns.get(name).cloned() {
                        let mut pre = String::new();
                        let mut coerced: Vec<String> = Vec::new();
                        for (a, pt) in args.iter().zip(params.iter()) {
                            let mut v = self.expr(a);
                            if *pt == Ty::CStr && self.expr_ty(a) == Ty::Str {
                                v = format!("rt_cstr({v})");
                            }
                            match pt {
                                Ty::Mut(_) => pre.push_str(&format!("rt_borrow_mut({v}); ")),
                                Ty::Ref(_) => pre.push_str(&format!("rt_borrow_read({v}); ")),
                                _ => {}
                            }
                            coerced.push(v);
                        }
                        let inner = format!("{callee}({})", coerced.join(", "));
                        if !pre.is_empty() {
                            return format!("({{ {pre}{inner}; }})");
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
