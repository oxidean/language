mod ast;
mod check;
mod codegen;
mod diag;
mod fmt;
mod lexer;
mod parser;

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|a| a.as_str()) == Some("lsp") {
        cmd_lsp();
        return;
    }
    if args.len() < 3 {
        eprintln!("oxl — one toolchain binary\n\n  oxl build <file.oxl> [-o out]\n  oxl run <file.oxl>\n  oxl test <file.oxl>\n  oxl fmt <file.oxl>\n  oxl doc <file.oxl>\n  oxl lsp");
        exit(2);
    }
    match args[1].as_str() {
        "build" => cmd_build(&args),
        "run" => cmd_run(&args),
        "test" => cmd_test(&args),
        "fmt" => cmd_fmt(&args),
        "doc" => cmd_doc(&args),
        "lsp" => cmd_lsp(),
        other => {
            eprintln!("unknown command `{other}`");
            exit(2);
        }
    }
}

/// resolve an import path relative to the importing file, then `stdlib/`
/// under any ancestor of the importing file (for `std/...` paths)
fn resolve_import(from_file: &Path, imp: &str) -> Option<PathBuf> {
    let mut cand = imp.to_string();
    if !cand.ends_with(".oxl") {
        cand.push_str(".oxl");
    }
    let rel = from_file.parent().unwrap_or(Path::new(".")).join(&cand);
    if rel.exists() {
        return Some(rel);
    }
    // std/x -> <ancestor>/stdlib/x.oxl walking up from the importing file
    let stripped = cand.strip_prefix("std/").unwrap_or(&cand);
    let mut dir = from_file.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        let p = d.join("stdlib").join(stripped);
        if p.exists() {
            return Some(p);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

/// stem of a resolved path = its canonical module name
fn mod_stem(path: &Path) -> String {
    path.file_stem().unwrap_or_default().to_string_lossy().to_string()
}

/// rename an oxl identifier through this module's maps:
/// `a.b` -> alias-mapped `stem.b`; bare `n` -> own-decl `mod.n` / selective `s.n`
fn map_name(
    n: &str,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
) -> String {
    if let Some(r) = renames.get(n) {
        return r.clone();
    }
    if let Some((head, rest)) = n.split_once('.') {
        if let Some(stem) = aliases.get(head) {
            return format!("{stem}.{rest}");
        }
        if let Some(stem) = renames.get(head) {
            return format!("{stem}.{rest}");
        }
        n.to_string()
    } else {
        n.to_string()
    }
}

type Locals = Vec<std::collections::HashSet<String>>;

fn local(locals: &Locals, n: &str) -> bool {
    locals.iter().rev().any(|s| s.contains(n))
}

/// canonicalize `a.b(...)` calls to `Ident("a.b")` then map through the
/// module's alias/rename tables; also maps type names and bare decl idents.
/// `locals` tracks in-scope binders — a local that shadows a same-module
/// decl name must stay bare (its decl-qualified form would miss in env and
/// fall through to the fn value).
fn qualify_expr(
    e: &mut ast::Expr,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
    locals: &mut Locals,
) {
    match &mut e.kind {
        ast::ExprKind::Call { f, args } => {
            if let Some(n) = ast::callee_name(f) {
                let head = n.split('.').next().unwrap_or("");
                // always flatten to `Ident("a.b")` — the checker keys on the
                // dotted form (e.g. `e.kind()`) — but skip the rename when
                // the head is a local
                let mapped = if local(locals, head) {
                    n
                } else {
                    map_name(&n, renames, aliases)
                };
                f.kind = ast::ExprKind::Ident(mapped);
            } else {
                qualify_expr(f, renames, aliases, locals);
            }
            for a in args {
                qualify_expr(a, renames, aliases, locals);
            }
        }
        ast::ExprKind::Ident(n) => {
            if !local(locals, n) {
                *n = map_name(n, renames, aliases)
            }
        }
        ast::ExprKind::StructLit { name, fields } => {
            *name = map_name(name, renames, aliases);
            for (_, v) in fields {
                qualify_expr(v, renames, aliases, locals);
            }
        }
        ast::ExprKind::Field { obj, .. } => qualify_expr(obj, renames, aliases, locals),
        ast::ExprKind::Index { obj, idx } => {
            qualify_expr(obj, renames, aliases, locals);
            qualify_expr(idx, renames, aliases, locals);
        }
        ast::ExprKind::Slice { obj, lo, hi } => {
            qualify_expr(obj, renames, aliases, locals);
            if let Some(x) = lo {
                qualify_expr(x, renames, aliases, locals);
            }
            if let Some(x) = hi {
                qualify_expr(x, renames, aliases, locals);
            }
        }
        ast::ExprKind::Closure { params, body } => {
            let names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
            for p in params.iter_mut() {
                qualify_ty(&mut p.ty, renames, aliases);
            }
            locals.push(names.into_iter().collect());
            qualify_block(body, renames, aliases, locals);
            locals.pop();
        }
        ast::ExprKind::Bin { l, r, .. } => {
            qualify_expr(l, renames, aliases, locals);
            qualify_expr(r, renames, aliases, locals);
        }
        ast::ExprKind::Un { e, .. } => qualify_expr(e, renames, aliases, locals),
        ast::ExprKind::As { e, ty } => {
            qualify_expr(e, renames, aliases, locals);
            qualify_ty(ty, renames, aliases);
        }
        ast::ExprKind::Match { e, arms } => {
            qualify_expr(e, renames, aliases, locals);
            for a in arms {
                locals.push(std::collections::HashSet::new());
                match &mut a.pat {
                    ast::Pattern::VarUnit { path }
                    | ast::Pattern::VarPos { path, .. }
                    | ast::Pattern::VarNamed { path, .. } => {
                        *path = map_name(path, renames, aliases);
                    }
                    ast::Pattern::Lit(le) => qualify_expr(le, renames, aliases, locals),
                    _ => {}
                }
                let binds: Vec<String> = match &a.pat {
                    ast::Pattern::VarPos { binds, .. }
                    | ast::Pattern::VarNamed { binds, .. } => binds.clone(),
                    ast::Pattern::Bind(b) => vec![b.clone()],
                    _ => Vec::new(),
                };
                for b in binds {
                    if b != "_" {
                        locals.last_mut().unwrap().insert(b);
                    }
                }
                match &mut a.body {
                    ast::MatchBody::Expr(be) => qualify_expr(be, renames, aliases, locals),
                    ast::MatchBody::Block(b) => qualify_block(b, renames, aliases, locals),
                }
                locals.pop();
            }
        }
        _ => {}
    }
}

fn qualify_ty(
    t: &mut ast::TyRef,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
) {
    match &mut t.kind {
        ast::TyKind::Named(n) => *n = map_name(n, renames, aliases),
        ast::TyKind::Own(i) | ast::TyKind::Ref(i) | ast::TyKind::Mut(i) => {
            qualify_ty(i, renames, aliases)
        }
        ast::TyKind::Chan => {}
        ast::TyKind::Slice(i) => qualify_ty(i, renames, aliases),
        ast::TyKind::Fn { params, ret } => {
            for p in params {
                qualify_ty(p, renames, aliases);
            }
            if let Some(r) = ret {
                qualify_ty(r, renames, aliases);
            }
        }
    }
}

fn qualify_block(
    b: &mut ast::Block,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
    locals: &mut Locals,
) {
    locals.push(std::collections::HashSet::new());
    for s in &mut b.stmts {
        match s {
            ast::Stmt::Let { name, ty, init, .. } => {
                if let Some(t) = ty {
                    qualify_ty(t, renames, aliases);
                }
                qualify_expr(init, renames, aliases, locals);
                locals.last_mut().unwrap().insert(name.clone());
            }
            ast::Stmt::Let2 { a, b, call, .. } => {
                qualify_expr(call, renames, aliases, locals);
                for bn in [a, b] {
                    if let ast::Bind::Name(n) = bn {
                        locals.last_mut().unwrap().insert(n.clone());
                    }
                }
            }
            ast::Stmt::Assign { target, val, .. } => {
                qualify_expr(target, renames, aliases, locals);
                qualify_expr(val, renames, aliases, locals);
            }
            ast::Stmt::Expr(x) => qualify_expr(x, renames, aliases, locals),
            ast::Stmt::Return { vals, .. } => {
                for v in vals {
                    qualify_expr(v, renames, aliases, locals);
                }
            }
            ast::Stmt::If { cond, then, els, .. } => {
                qualify_expr(cond, renames, aliases, locals);
                qualify_block(then, renames, aliases, locals);
                if let Some(e) = els {
                    qualify_block(e, renames, aliases, locals);
                }
            }
            ast::Stmt::For { cond, body, .. } => {
                qualify_expr(cond, renames, aliases, locals);
                qualify_block(body, renames, aliases, locals);
            }
            ast::Stmt::Scope { body, .. } | ast::Stmt::Unsafe { body, .. } => {
                qualify_block(body, renames, aliases, locals)
            }
            ast::Stmt::Spawn { call, .. } => qualify_expr(call, renames, aliases, locals),
            ast::Stmt::Defer { call, .. } => qualify_expr(call, renames, aliases, locals),
            ast::Stmt::Select { arms, .. } => {
                for a in arms {
                    locals.push(std::collections::HashSet::new());
                    match &mut a.kind {
                        ast::SelKind::Recv { chan, bind, .. } => {
                            qualify_expr(chan, renames, aliases, locals);
                            if let Some(bn) = bind {
                                locals.last_mut().unwrap().insert(bn.clone());
                            }
                        }
                        ast::SelKind::Timeout { ms, .. } => {
                            qualify_expr(ms, renames, aliases, locals)
                        }
                        ast::SelKind::Else { .. } => {}
                    }
                    qualify_block(&mut a.body, renames, aliases, locals);
                    locals.pop();
                }
            }
            ast::Stmt::Break { .. } | ast::Stmt::Continue { .. } => {}
        }
    }
    locals.pop();
}

/// named base of a receiver type: `own T`/`ref T`/`mut T`/`T` -> "T"
fn recv_base(t: &ast::TyRef) -> Option<String> {
    match &t.kind {
        ast::TyKind::Named(n) => Some(n.clone()),
        ast::TyKind::Own(i) | ast::TyKind::Ref(i) | ast::TyKind::Mut(i) => recv_base(i),
        _ => None,
    }
}

fn load_crate(
    path: &Path,
    seen: &mut std::collections::HashSet<PathBuf>,
) -> Option<(ast::Crate, Vec<PathBuf>)> {
    load_mod(path, "", seen, true)
}

/// parse a file, recursively qualify its imports under their module stems,
/// then qualify this file's own decls as `mod.name` (root: modname "").
/// The returned Vec<PathBuf> is item-parallel: the file each item's spans
/// index into (imported items keep their own file's coordinates).
fn load_mod(
    path: &Path,
    modname: &str,
    seen: &mut std::collections::HashSet<PathBuf>,
    loud: bool,
) -> Option<(ast::Crate, Vec<PathBuf>)> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !seen.insert(canon.clone()) {
        return Some((ast::Crate { items: Vec::new() }, Vec::new()));
    }
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            if loud {
                eprintln!("{}: {e}", path.display());
            }
            return None;
        }
    };
    let mut diags = diag::Diags::new(path.to_string_lossy().to_string(), src.clone());
    let toks = lexer::lex(&src, &mut diags);
    let krate = parser::parse(&toks, &mut diags);
    if loud {
        diags.report();
    }
    // own decl names (externs keep C names) + per-import maps
    let mut renames: std::collections::HashMap<String, String> = Default::default();
    let mut aliases: std::collections::HashMap<String, String> = Default::default();
    if !modname.is_empty() {
        for it in &krate.items {
            // methods key on the receiver type (`T.m`), not the bare name —
            // two types may share a method name, so `m` can't be a decl key
            if let ast::Item::Fn(f) = it {
                if f.recv.is_some() {
                    continue;
                }
            }
            let n = match it {
                ast::Item::Fn(f) => Some(&f.name),
                ast::Item::Struct(s) => Some(&s.name),
                ast::Item::Enum(e) => Some(&e.name),
                ast::Item::Interface(i) => Some(&i.name),
                ast::Item::Global(g) => Some(&g.name),
                _ => None,
            };
            if let Some(n) = n {
                renames.insert(n.clone(), format!("{modname}.{n}"));
            }
        }
    }
    let mut items = Vec::new();
    let mut item_files: Vec<PathBuf> = Vec::new();
    // pass 1: resolve every import first so its alias/selective bindings are
    // visible to all decls regardless of source order
    for it in &krate.items {
        if let ast::Item::Import(imp) = it {
            match resolve_import(path, &imp.path) {
                Some(p) => {
                    let stem = mod_stem(&p);
                    aliases.insert(imp.alias.clone().unwrap_or_else(|| stem.clone()), stem.clone());
                    if imp.alias.is_none() {
                        aliases.insert(stem.clone(), stem.clone());
                    }
                    for s in &imp.selective {
                        renames.insert(s.clone(), format!("{stem}.{s}"));
                    }
                    if let Some((sub, sub_files)) = load_mod(&p, &stem, seen, loud) {
                        items.extend(sub.items);
                        item_files.extend(sub_files);
                    }
                }
                None if loud => {
                    eprintln!("{}: cannot resolve import `{}`", path.display(), imp.path)
                }
                None => {}
            }
        }
    }
    // pass 2: qualify own decls against the complete maps
    let own = krate.items.len();
    for it in krate.items {
        match it {
            ast::Item::Import(_) => {}
            mut it => {
                qualify_item(&mut it, &renames, &aliases);
                items.push(it);
            }
        }
    }
    item_files.extend(std::iter::repeat(path.to_path_buf()).take(items.len() - item_files.len()));
    debug_assert_eq!(items.len(), item_files.len(), "{own} own items");
    Some((ast::Crate { items }, item_files))
}

/// apply the module's rename/alias maps to one non-import item in place
fn qualify_item(
    it: &mut ast::Item,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
) {
    match it {
        ast::Item::Fn(f) => {
            if let Some(r) = &mut f.recv {
                qualify_ty(&mut r.ty, renames, aliases);
                // the decl name is `{receiver type}.{method}` — receiver
                // base unwraps own/ref/mut to its Named path
                if let Some(base) = recv_base(&r.ty) {
                    f.name = format!("{base}.{}", f.name);
                }
            } else if let Some(n) = renames.get(&f.name) {
                f.name = n.clone();
            }
            for p in &mut f.params {
                qualify_ty(&mut p.ty, renames, aliases);
            }
            match &mut f.ret {
                ast::RetTy::One(t) | ast::RetTy::Pair(t) => qualify_ty(t, renames, aliases),
                ast::RetTy::Void => {}
            }
            if let Some(b) = &mut f.body {
                let mut scope: std::collections::HashSet<String> =
                    f.params.iter().map(|p| p.name.clone()).collect();
                if let Some(r) = &f.recv {
                    scope.insert(r.name.clone());
                }
                let mut locals: Locals = vec![scope];
                qualify_block(b, renames, aliases, &mut locals);
            }
        }
        ast::Item::Struct(s) => {
            if let Some(n) = renames.get(&s.name) {
                s.name = n.clone();
            }
            for p in &mut s.fields {
                qualify_ty(&mut p.ty, renames, aliases);
            }
        }
        ast::Item::Enum(e) => {
            if let Some(n) = renames.get(&e.name) {
                e.name = n.clone();
            }
            for v in &mut e.variants {
                match &mut v.payload {
                    ast::VariantPayload::Pos(tys) => {
                        for t in tys {
                            qualify_ty(t, renames, aliases);
                        }
                    }
                    ast::VariantPayload::Named(fs) => {
                        for (_, t) in fs {
                            qualify_ty(t, renames, aliases);
                        }
                    }
                    ast::VariantPayload::Unit => {}
                }
            }
        }
        ast::Item::Interface(i) => {
            if let Some(n) = renames.get(&i.name) {
                i.name = n.clone();
            }
            for m in &mut i.methods {
                for p in &mut m.params {
                    qualify_ty(&mut p.ty, renames, aliases);
                }
                match &mut m.ret {
                    ast::RetTy::One(t) | ast::RetTy::Pair(t) => {
                        qualify_ty(t, renames, aliases)
                    }
                    ast::RetTy::Void => {}
                }
            }
        }
        ast::Item::Global(g) => {
            if let Some(n) = renames.get(&g.name) {
                g.name = n.clone();
            }
            if let Some(t) = &mut g.ty {
                qualify_ty(t, renames, aliases);
            }
            let mut locals: Locals = vec![std::collections::HashSet::new()];
            qualify_expr(&mut g.init, renames, aliases, &mut locals);
        }
        ast::Item::Extern(f) => {
            for p in &mut f.params {
                qualify_ty(&mut p.ty, renames, aliases);
            }
            match &mut f.ret {
                ast::RetTy::One(t) | ast::RetTy::Pair(t) => qualify_ty(t, renames, aliases),
                ast::RetTy::Void => {}
            }
        }
        ast::Item::Import(_) => {}
    }
}

fn compile_mode(src_file: &str, test_mode: bool) -> Result<String, ()> {
    let mut seen = std::collections::HashSet::new();
    let (krate, item_files) = match load_crate(Path::new(src_file), &mut seen) {
        Some(k) => k,
        None => return Err(()),
    };
    let src = match std::fs::read_to_string(src_file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{src_file}: {e}");
            return Err(());
        }
    };
    // note: semantic diagnostics in imported files map spans against the
    // entry file's source (v0 limitation — spans are per-crate)
    let mut diags = diag::Diags::new(src_file, src);
    let files: Vec<String> = item_files.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let checked = check::check(&krate, &files, &mut diags);
    diags.report();
    if diags.has_errors() {
        return Err(());
    }
    Ok(codegen::generate_mode(&krate, test_mode, &checked.suspend))
}

fn compile(src_file: &str) -> Result<String, ()> {
    compile_mode(src_file, false)
}

fn emit_and_cc(c_src: &str, out: &Path) -> bool {
    let dir = out.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).ok();
    let c_file = dir.join("out.c");
    std::fs::write(&c_file, c_src).unwrap();
    let (rt_h, rt_c) = codegen::runtime_files();
    std::fs::write(dir.join("rt.h"), rt_h).unwrap();
    std::fs::write(dir.join("rt.c"), rt_c).unwrap();
    let cc = std::env::var("LANG_CC").unwrap_or_else(|_| "cc".into());
    let status = Command::new(&cc)
        .args(["-O2", "-pthread", "-lsqlite3", "-o"])
        .arg(out)
        .arg(&c_file)
        .arg(dir.join("rt.c"))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    match status {
        Ok(s) if s.success() => true,
        _ => {
            eprintln!("cc failed (emitted C kept at {})", c_file.display());
            false
        }
    }
}

fn cmd_build(args: &[String]) {
    let file = &args[2];
    let out = out_arg(args).unwrap_or_else(|| PathBuf::from("a.out"));
    match compile(file) {
        Ok(c) => {
            if !emit_and_cc(&c, &out) {
                exit(1);
            }
        }
        Err(()) => exit(1),
    }
}

fn cmd_run(args: &[String]) {
    let file = &args[2];
    match compile(file) {
        Ok(c) => {
            let dir = std::env::temp_dir().join(format!("oxl-{}", std::process::id()));
            let out = dir.join("a.out");
            if !emit_and_cc(&c, &out) {
                exit(1);
            }
            let child_args: &[String] = if args.len() > 3 && args[3] == "--" {
                &args[4..]
            } else {
                &args[3..]
            };
            let st = Command::new(&out).args(child_args).status().unwrap();
            exit(st.code().unwrap_or(1));
        }
        Err(()) => exit(1),
    }
}

fn cmd_test(args: &[String]) {
    let file = &args[2];
    match compile_mode(file, true) {
        Ok(c) => {
            let dir = std::env::temp_dir().join(format!("oxl-test-{}", std::process::id()));
            let out = dir.join("a.out");
            if !emit_and_cc(&c, &out) {
                exit(1);
            }
            let st = Command::new(&out).status().unwrap();
            exit(st.code().unwrap_or(1));
        }
        Err(()) => exit(1),
    }
}

fn out_arg(args: &[String]) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == "-o")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

// --- fmt: indentation + whitespace normalizer (comments preserved) ---
/// `oxl fmt [-w|--check] <file.oxl>...` — AST pretty-printer (E2):
/// canonical layout from the parsed crate; comments re-attach as
/// trivia. --check exits nonzero on any diff (CI); -w rewrites.
fn cmd_fmt(args: &[String]) {
    let write = args.iter().any(|a| a == "-w");
    let check = args.iter().any(|a| a == "--check");
    let files: Vec<&String> = args[2..].iter().filter(|a| !a.starts_with('-')).collect();
    if files.is_empty() {
        eprintln!("usage: oxl fmt [-w|--check] <file.oxl>...");
        exit(1);
    }
    let mut bad = false;
    for file in files {
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{file}: {e}");
                bad = true;
                continue;
            }
        };
        match fmt::format_file(file, &src) {
            Ok(out) => {
                if check {
                    if out != src {
                        eprintln!("{file}: not formatted");
                        bad = true;
                    }
                } else if write {
                    if out != src {
                        std::fs::write(file, out).unwrap();
                    }
                } else {
                    print!("{out}");
                }
            }
            Err(()) => bad = true,
        }
    }
    if bad {
        exit(1);
    }
}

// --- doc: /// comments attached to fn/struct/extern decls -> markdown ---
fn cmd_doc(args: &[String]) {
    let file = &args[2];
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{file}: {e}");
            exit(1);
        }
    };
    let mut doc = String::from("# API\n");
    let mut pending: Vec<String> = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if let Some(c) = t.strip_prefix("///") {
            pending.push(c.trim().to_string());
            continue;
        }
        for kw in ["fn ", "struct ", "extern fn ", "interface "] {
            if t.starts_with(kw) {
                let sig = t.split('{').next().unwrap_or(t).trim();
                if !pending.is_empty() {
                    doc.push_str(&format!("\n## `{sig}`\n\n{}\n", pending.join("\n")));
                } else {
                    doc.push_str(&format!("\n## `{sig}`\n"));
                }
                break;
            }
        }
        if t != "//" && !t.starts_with("///") {
            pending.clear();
        }
    }
    print!("{doc}");
}

/// analyze one in-memory buffer (imports resolved from disk); returns
/// (line, col, message) diagnostics for the buffer itself
struct Analysis {
    diags: Vec<(usize, usize, String)>,
    index: check::Index,
}

fn analyze(path: &Path, src: &str) -> Analysis {
    let mut diags = diag::Diags::new(path.to_string_lossy(), src);
    let toks = lexer::lex(src, &mut diags);
    let krate = parser::parse(&toks, &mut diags);
    let mut seen = std::collections::HashSet::new();
    seen.insert(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
    let mut renames: std::collections::HashMap<String, String> = Default::default();
    let mut aliases: std::collections::HashMap<String, String> = Default::default();
    let mut items = Vec::new();
    let mut item_files: Vec<PathBuf> = Vec::new();
    for it in &krate.items {
        if let ast::Item::Import(imp) = it {
            if let Some(p) = resolve_import(path, &imp.path) {
                let stem = mod_stem(&p);
                let alias = imp.alias.clone().unwrap_or_else(|| stem.clone());
                aliases.insert(alias.clone(), stem.clone());
                if imp.alias.is_none() {
                    aliases.insert(stem.clone(), stem.clone());
                }
                for s in &imp.selective {
                    renames.insert(s.clone(), format!("{stem}.{s}"));
                }
                if let Some((sub, sub_files)) = load_mod(&p, &stem, &mut seen, false) {
                    items.extend(sub.items);
                    item_files.extend(sub_files);
                }
            }
        }
    }
    let mut own = 0;
    for mut it in krate.items {
        if !matches!(it, ast::Item::Import(_)) {
            qualify_item(&mut it, &renames, &aliases);
            items.push(it);
            own += 1;
        }
    }
    item_files.extend(std::iter::repeat(path.to_path_buf()).take(own));
    let files: Vec<String> = item_files.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let checked = check::check(&ast::Crate { items }, &files, &mut diags);
    if std::env::var("OXL_IDX_DEBUG").is_ok() {
        for u in &checked.index.uses {
            eprintln!("USE {} {:?} -> {} {:?} `{}`", u.file, u.span, u.def_file, u.def_span, u.sig);
        }
    }
    Analysis {
        diags: diags
            .list
            .iter()
            .map(|d| {
                let (l, c) = diags.line_col(d.span.lo);
                (l, c, format!("[{}] {}", d.code, d.msg))
            })
            .collect(),
        index: checked.index,
    }
}

/// naive JSON string-field extraction (v0 — no serde dep); tolerates
/// `"k": "v"` and `"k":"v"` spacing
fn json_str<'a>(body: &'a str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":");
    let at = body.find(&pat)? + pat.len();
    let i = at + body[at..].find('"')? + 1;
    let mut out = String::new();
    let mut esc = false;
    for c in body[i..].chars() {
        if esc {
            out.push(match c {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'u' => continue, // \uXXXX passthrough is lossy in v0
                c => c,
            });
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    None
}

fn publish_diags(stdout: &mut impl Write, uri: &str, list: &[(usize, usize, String)]) {
    let mut ds = String::new();
    for (i, (l, c, msg)) in list.iter().enumerate() {
        if i > 0 {
            ds.push(',');
        }
        let (l0, c0) = (l.saturating_sub(1), c.saturating_sub(1));
        ds.push_str(&format!(
            "{{\"range\":{{\"start\":{{\"line\":{l0},\"character\":{c0}}},\"end\":{{\"line\":{l0},\"character\":{}}}}},\"severity\":1,\"message\":\"{}\"}}",
            c0 + 1,
            msg.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }
    let notif = format!(
        "{{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{{\"uri\":\"{uri}\",\"diagnostics\":[{ds}]}}}}"
    );
    write!(stdout, "Content-Length: {}\r\n\r\n{}", notif.len(), notif).unwrap();
    stdout.flush().unwrap();
}

/// naive JSON number-field extraction (matching json_str's first-hit)
fn json_num(body: &str, key: &str) -> Option<usize> {
    let pat = format!("\"{key}\":");
    let i = body.find(&pat)? + pat.len();
    let rest = &body[i..];
    let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

/// LSP position (0-based line, 0-based char col) → byte offset.
/// Cols are chars, not UTF-16 units (v0 — non-BMP text misaligns).
fn lsp_offset(src: &str, line: usize, col: usize) -> u32 {
    let mut l = 0usize;
    for (i, c) in src.char_indices() {
        if l == line {
            // scan `col` chars forward on this line
            let mut off = i;
            let mut k = 0;
            for (j, c2) in src[i..].char_indices() {
                if k == col || c2 == '\n' {
                    return off as u32;
                }
                k += 1;
                off = i + j + c2.len_utf8();
            }
            return off as u32;
        }
        if c == '\n' {
            l += 1;
        }
    }
    src.len() as u32
}

/// byte offset → LSP (0-based line, 0-based char col)
fn lsp_line_col(src: &str, off: u32) -> (usize, usize) {
    let (mut l, mut c) = (0usize, 0usize);
    for (i, ch) in src.char_indices() {
        if i as u32 >= off {
            break;
        }
        if ch == '\n' {
            l += 1;
            c = 0;
        } else {
            c += 1;
        }
    }
    (l, c)
}

/// the index entry with the smallest span containing `off`
fn index_at<'a>(a: &'a Analysis, file: &str, off: u32) -> Option<&'a check::UseRec> {
    a.index
        .uses
        .iter()
        .filter(|u| u.file == file && u.span.lo <= off && off < u.span.hi)
        .min_by_key(|u| u.span.hi - u.span.lo)
}

// --- stdio LSP: diagnostics + hover/definition over the position index ---
fn cmd_lsp() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut r = std::io::BufReader::new(stdin.lock());
    // per-doc state: the buffer text and the analysis built from it
    let mut docs: std::collections::HashMap<String, (String, Analysis)> =
        std::collections::HashMap::new();
    loop {
        // read headers
        let mut len = 0usize;
        let mut line = String::new();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let t = line.trim();
            if t.is_empty() {
                break;
            }
            if let Some(v) = t.strip_prefix("Content-Length:") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        if len == 0 {
            continue;
        }
        let mut body = vec![0u8; len];
        if r.read_exact(&mut body).is_err() {
            return;
        }
        let body = String::from_utf8_lossy(&body);
        if body.contains("\"exit\"") {
            return;
        }
        if body.contains("didOpen") || body.contains("didChange") {
            if let (Some(uri), Some(text)) = (json_str(&body, "uri"), json_str(&body, "text")) {
                let path = Path::new(uri.strip_prefix("file://").unwrap_or(&uri));
                let a = analyze(path, &text);
                publish_diags(&mut stdout, &uri, &a.diags);
                docs.insert(uri, (text, a));
            }
            continue;
        }
        if body.contains("didClose") {
            if let Some(uri) = json_str(&body, "uri") {
                docs.remove(&uri);
            }
            continue;
        }
        let resp = if body.contains("\"initialize\"") {
            let id = extract_id(&body);
            format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"capabilities\":{{\"textDocumentSync\":1,\"hoverProvider\":true,\"definitionProvider\":true}}}}}}")
        } else if body.contains("\"shutdown\"") {
            let id = extract_id(&body);
            format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":null}}")
        } else if body.contains("textDocument/hover") || body.contains("textDocument/definition") {
            let id = extract_id(&body);
            let uri = json_str(&body, "uri").unwrap_or_default();
            let (line, col) = (
                json_num(&body, "line").unwrap_or(0),
                json_num(&body, "character").unwrap_or(0),
            );
            let def = body.contains("textDocument/definition");
            let file = uri.strip_prefix("file://").unwrap_or(&uri).to_string();
            let hit = docs.get(&uri).and_then(|(src, a)| {
                index_at(a, &file, lsp_offset(src, line, col))
            });
            match (hit, def) {
                (Some(u), false) => format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"contents\":{{\"kind\":\"plaintext\",\"value\":\"{}\"}}}}}}",
                    u.sig.replace('\\', "\\\\").replace('"', "\\\"")
                ),
                (Some(u), true) => {
                    // def spans index into their own file's source — the
                    // open doc uses its buffer, imports read from disk
                    let src = docs
                        .get(&uri)
                        .map(|(s, _)| s.clone())
                        .filter(|_| u.def_file == file)
                        .or_else(|| std::fs::read_to_string(&u.def_file).ok())
                        .unwrap_or_default();
                    let (l0, c0) = lsp_line_col(&src, u.def_span.lo);
                    let (l1, c1) = lsp_line_col(&src, u.def_span.hi);
                    let uri2 = if u.def_file.starts_with('/') {
                        format!("file://{}", u.def_file)
                    } else {
                        // relative decl paths resolve against the doc dir
                        let base = Path::new(&file).parent().unwrap_or(Path::new("."));
                        let p = base.join(&u.def_file);
                        let p = p.canonicalize().unwrap_or(p);
                        format!("file://{}", p.display())
                    };
                    format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"uri\":\"{uri2}\",\"range\":{{\"start\":{{\"line\":{l0},\"character\":{c0}}},\"end\":{{\"line\":{l1},\"character\":{c1}}}}}}}}}"
                    )
                }
                (None, _) => format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":null}}"),
            }
        } else {
            continue;
        };
        write!(stdout, "Content-Length: {}\r\n\r\n{}", resp.len(), resp).unwrap();
        stdout.flush().unwrap();
    }
}

fn extract_id(body: &str) -> String {
    if let Some(i) = body.find("\"id\":") {
        let rest = &body[i + 5..];
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        return rest[..end].trim().to_string();
    }
    "0".into()
}
