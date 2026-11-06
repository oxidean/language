mod ast;
mod check;
mod codegen;
mod diag;
mod fmt;
mod lexer;
mod parser;

use crate::diag::Span;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|a| a.as_str()) == Some("lsp") {
        cmd_lsp();
        return;
    }
    if args.len() < 2 {
        eprintln!("oxl — one toolchain binary\n\n  oxl build [<pkg dir>|<file.oxl>...] [-o out]\n  oxl run [<pkg dir>|<file.oxl>...]\n  oxl test [<pkg dir>|<file.oxl>...]\n  oxl fmt <file.oxl>...\n  oxl doc <file.oxl>\n  oxl lsp");
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

/// language version this toolchain implements — the `oxl` directive's
/// floor compares against it; tied to the crate version so a release
/// bump moves the floor ceiling too
const OXL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// the toolchain version as `major.minor`, for diagnostics
fn tool_ver() -> String {
    let mut it = OXL_VERSION.split('.');
    format!("{}.{}", it.next().unwrap_or("0"), it.next().unwrap_or("0"))
}

/// `X.Y[.Z]` -> (maj, min, patch); leading `v` tolerated
fn ver_tuple(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut it = v.split('.');
    let maj = it.next()?.parse().ok()?;
    let min = it.next()?.parse().ok()?;
    let patch = it.next().map_or(Ok(0), |s| s.parse()).ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((maj, min, patch))
}

/// `vX.Y.Z` or a pseudo-version (`vX.Y.Z-<anything>`) — the require/
/// replace version shape; full semver grammar lands with V1
fn mod_ver_ok(v: &str) -> bool {
    let core = v.split('-').next().unwrap_or(v);
    let mut it = core.strip_prefix('v').unwrap_or("").split('.');
    let dig = |s: Option<&str>| matches!(s, Some(x) if !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()));
    dig(it.next()) && dig(it.next()) && dig(it.next()) && it.next().is_none()
}

/// first path segment carrying a `.` reads as a remote module path —
/// drives the `oxl get` hint on `cannot find package`
fn remoteish(imp: &str) -> bool {
    imp.split('/').next().map_or(false, |s| s.contains('.'))
}

/// the main module's view of the build graph — v1 resolves EVERY
/// import through it (dependency manifests never re-enter resolution;
/// MVS flattens the build list in V1 anyway)
struct ModCtx {
    path: String,
    root: PathBuf,
    requires: Vec<(String, String)>,
    /// replace LHS path -> local dir (v1 accepts dir RHS only)
    replaces: Vec<(String, PathBuf)>,
    modcache: PathBuf,
}

/// nearest ancestor of `dir` holding `oxl.mod`, if any — nearest wins,
/// so a nested module is its own root
fn find_root(dir: &Path) -> Option<PathBuf> {
    // canonicalize the start: a lexical walk stalls at `.`/`""` (a bare
    // filename's parent) and never reaches the cwd's real ancestors
    let mut d = match std::fs::canonicalize(dir) {
        Ok(p) => Some(p),
        Err(_) => std::env::current_dir().ok(),
    };
    while let Some(x) = d {
        if x.join("oxl.mod").is_file() {
            return Some(x);
        }
        d = x.parent().map(Path::to_path_buf);
    }
    None
}

/// module cache root: OXLMODCACHE wins, else ~/.oxl/mod
fn modcache_dir() -> PathBuf {
    if let Ok(v) = std::env::var("OXLMODCACHE") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    for k in ["HOME", "USERPROFILE"] {
        if let Ok(h) = std::env::var(k) {
            if !h.is_empty() {
                return PathBuf::from(h).join(".oxl").join("mod");
            }
        }
    }
    PathBuf::from(".oxl").join("mod")
}

/// parse oxl.mod text -> (module path, `oxl` floor as written,
/// requires, replaces). Errors come back ready to print:
/// `oxl.mod:<line>: <msg>` — display stays hermetic (the manifest is
/// always AT the module root, so the bare name locates it).
fn parse_manifest(
    src: &str,
) -> Result<(String, Option<String>, Vec<(String, String)>, Vec<(String, PathBuf)>), String> {
    let mut module = String::new();
    let mut floor: Option<String> = None;
    let mut requires: Vec<(String, String)> = Vec::new();
    let mut replaces: Vec<(String, PathBuf)> = Vec::new();
    let mut lines = src.lines().enumerate().peekable();
    while let Some((ln, raw)) = lines.next() {
        let line_no = ln + 1;
        let text = raw.split("//").next().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let toks: Vec<&str> = text.split_whitespace().collect();
        let err = |m: String| format!("oxl.mod:{line_no}: {m}");
        match toks[0] {
            "module" => {
                if !module.is_empty() {
                    return Err(err("duplicate directive \"module\"".into()));
                }
                if toks.len() != 2 {
                    return Err(err("expected module path".into()));
                }
                module = toks[1].to_string();
            }
            "oxl" => {
                if floor.is_some() {
                    return Err(err("duplicate directive \"oxl\"".into()));
                }
                if toks.len() != 2 || ver_tuple(toks[1]).is_none() {
                    return Err(err("expected version".into()));
                }
                floor = Some(toks[1].to_string());
            }
            "require" | "replace" => {
                let kw = toks[0];
                // entries: inline tokens or a parenthesized block
                let mut body: Vec<(usize, Vec<String>)> = Vec::new();
                if toks.len() == 2 && toks[1] == "(" {
                    let mut closed = false;
                    for (ln2, raw2) in lines.by_ref() {
                        let t2 = raw2.split("//").next().unwrap_or("").trim();
                        if t2.is_empty() {
                            continue;
                        }
                        if t2 == ")" {
                            closed = true;
                            break;
                        }
                        body.push((ln2 + 1, t2.split_whitespace().map(String::from).collect()));
                    }
                    if !closed {
                        return Err(err(format!("unterminated {kw} block")));
                    }
                } else {
                    body.push((line_no, toks[1..].iter().map(|s| s.to_string()).collect()));
                }
                for (bln, bt) in body {
                    let berr = |m: String| format!("oxl.mod:{bln}: {m}");
                    if kw == "require" {
                        if bt.len() != 2 {
                            return Err(berr("expected `path version`".into()));
                        }
                        if !mod_ver_ok(&bt[1]) {
                            return Err(berr(format!("bad version \"{}\"", bt[1])));
                        }
                        if requires.iter().any(|(p, _)| p == &bt[0]) {
                            return Err(berr(format!("duplicate require \"{}\"", bt[0])));
                        }
                        requires.push((bt[0].clone(), bt[1].clone()));
                    } else {
                        // replace <path> [ver] => <dir>
                        let arrow = bt
                            .iter()
                            .position(|t| t == "=>")
                            .ok_or_else(|| berr("expected `=>`".into()))?;
                        if arrow == 0 || arrow > 2 || arrow + 1 != bt.len() - 1 {
                            return Err(berr("expected `path [version] => dir`".into()));
                        }
                        if arrow == 2 && !mod_ver_ok(&bt[1]) {
                            return Err(berr(format!("bad version \"{}\"", bt[1])));
                        }
                        replaces.push((bt[0].clone(), PathBuf::from(&bt[arrow + 1])));
                    }
                }
            }
            _ => return Err(err(format!("unknown directive \"{}\"", toks[0]))),
        }
    }
    if module.is_empty() {
        return Err("oxl.mod: missing module directive".into());
    }
    Ok((module, floor, requires, replaces))
}

/// the module context for a compile entry: nearest oxl.mod above
/// `start`, parsed and floor-checked. Ok(None) = module-off mode —
/// only `std/` imports resolve. Err = manifest found but malformed or
/// above the toolchain floor (hard error when `loud`).
fn mod_ctx(start: &Path, loud: bool) -> Result<Option<ModCtx>, ()> {
    let Some(root) = find_root(start) else {
        return Ok(None);
    };
    let mpath = root.join("oxl.mod");
    let text = match std::fs::read_to_string(&mpath) {
        Ok(t) => t,
        Err(e) => {
            if loud {
                eprintln!("error [check] {}: {e}", mpath.display());
            }
            return Err(());
        }
    };
    let (path, floor, requires, replaces) = match parse_manifest(&text) {
        Ok(m) => m,
        Err(e) => {
            if loud {
                eprintln!("error [check] {e}");
            }
            return Err(());
        }
    };
    if let Some(fv) = floor {
        let cur = ver_tuple(OXL_VERSION).unwrap_or((0, 0, 0));
        if ver_tuple(&fv).map_or(false, |f| f > cur) {
            if loud {
                eprintln!("error [check] oxl.mod: requires oxl {fv}, toolchain is {}", tool_ver());
            }
            return Err(());
        }
    }
    // replace dirs resolve against the module root
    let replaces = replaces
        .into_iter()
        .map(|(p, d)| {
            let d = if d.is_absolute() { d } else { root.join(&d) };
            (p, d)
        })
        .collect();
    Ok(Some(ModCtx {
        path,
        root,
        requires,
        replaces,
        modcache: modcache_dir(),
    }))
}

enum Resolve {
    Dir(PathBuf),
    /// declared require with neither replace nor a cache copy —
    /// carries the `path@ver` the diagnostic names
    Missing(String),
    NotFound,
}

/// resolve an import path to a package directory under the module
/// rules: `std/` from beside the toolchain executable, then the main
/// module's own path, then the longest require-prefix match through
/// replace or the module cache. NotFound = cannot find.
fn resolve_pkg(ctx: Option<&ModCtx>, entry: &Path, imp: &str) -> Resolve {
    if let Some(rest) = imp.strip_prefix("std/") {
        // toolchain std ships beside the binary; a self-hosted checker
        // compiled into a scratch dir still finds the SOURCE tree's
        // std via the entry package's ancestors (only ever the root —
        // dep files can't shadow std through it)
        for base in [
            std::env::current_exe().ok(),
            Some(entry.to_path_buf()),
        ]
        .into_iter()
        .flatten()
        {
            let mut d = Some(base);
            while let Some(x) = d {
                let p = x.join("std").join(rest);
                if p.is_dir() {
                    return Resolve::Dir(p);
                }
                d = x.parent().map(Path::to_path_buf);
            }
        }
        return Resolve::NotFound;
    }
    let Some(ctx) = ctx else {
        return Resolve::NotFound;
    };
    if imp == ctx.path || imp.starts_with(&format!("{}/", ctx.path)) {
        let rest = imp[ctx.path.len()..].trim_start_matches('/');
        let p = ctx.root.join(rest);
        return if p.is_dir() {
            Resolve::Dir(p)
        } else {
            Resolve::NotFound
        };
    }
    // requires — longest path prefix wins (module-path boundaries only)
    let mut best: Option<&(String, String)> = None;
    for r in &ctx.requires {
        if (imp == r.0 || imp.starts_with(&format!("{}/", r.0)))
            && best.map_or(true, |b| r.0.len() > b.0.len())
        {
            best = Some(r);
        }
    }
    if let Some((mp, v)) = best {
        let rest = imp[mp.len()..].trim_start_matches('/');
        if let Some((_, rhs)) = ctx.replaces.iter().find(|(rp, _)| rp == mp) {
            let d = rhs.join(rest);
            return if d.is_dir() {
                Resolve::Dir(d)
            } else {
                Resolve::NotFound
            };
        }
        let base = ctx.modcache.join(format!("{mp}@{v}"));
        let d = base.join(rest);
        return if base.is_dir() && d.is_dir() {
            Resolve::Dir(d)
        } else if base.is_dir() {
            Resolve::NotFound // fetched, but the package isn't in it
        } else {
            Resolve::Missing(format!("{mp}@{v}"))
        };
    }
    Resolve::NotFound
}

/// internal rule: a resolved path with an `internal` segment may only
/// be imported from inside the tree rooted at that segment's parent —
/// checked for EVERY internal segment, canonicalized so `./`-relative
/// module roots and absolute cache paths compare honestly
fn internal_ok(resolved: &Path, importer_dir: &Path) -> bool {
    let rd = resolved
        .canonicalize()
        .unwrap_or_else(|_| resolved.to_path_buf());
    let id = importer_dir
        .canonicalize()
        .unwrap_or_else(|_| importer_dir.to_path_buf());
    let comps: Vec<_> = rd.components().collect();
    for (i, c) in comps.iter().enumerate() {
        if c.as_os_str() == "internal" {
            let parent: PathBuf = comps[..i].iter().collect();
            if !id.starts_with(&parent) {
                return false;
            }
        }
    }
    true
}

/// `*.oxl` files of a package directory, sorted for deterministic
/// loads. Scan failures propagate — an unreadable dir must not degrade
/// to "no .oxl source files" and a dropped entry must not degrade to
/// "undefined name" for the decls it held
fn pkg_dir_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut fs = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().map_or(false, |x| x == "oxl") {
            fs.push(p);
        }
    }
    fs.sort();
    Ok(fs)
}

/// a package's sources: a directory scan or an explicit file list
/// (command-line file mode — Go's `go run a.go b.go`)
enum PkgSrc {
    Dir(PathBuf),
    Files(Vec<PathBuf>),
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
            ast::Stmt::ExitBoundary { .. } => unreachable!("codegen-internal marker"),
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

/// parse one package file — `over` buffers substitute their paths so the
/// LSP analyzes unsaved text. Returns (clause, items); on a parse failure
/// with loud set the diags have already been reported.
fn parse_pkg_file(
    path: &Path,
    over: &[(&Path, &str)],
    loud: bool,
) -> Option<(String, Vec<ast::Item>)> {
    let src = match over.iter().find(|(p, _)| *p == path) {
        Some((_, s)) => s.to_string(),
        _ => match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                if loud {
                    eprintln!("{}: {e}", path.display());
                }
                return None;
            }
        },
    };
    let mut diags = diag::Diags::new(path.to_string_lossy().to_string(), src.clone());
    let toks = lexer::lex(&src, &mut diags);
    let krate = parser::parse(&toks, &mut diags);
    if loud {
        diags.report();
        if diags.has_errors() {
            return None;
        }
    }
    Some((krate.pkg, krate.items))
}

/// real last path component for the dir-name/package-name check —
/// canonicalize first so `.` (the cwd package) and Windows-style args
/// resolve to the actual directory name; matches the `seen` keying
fn path_base(p: &str) -> String {
    let p = Path::new(p);
    let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    p.file_name().unwrap_or_default().to_string_lossy().to_string()
}

fn fname(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().to_string()
}

/// load a package — every `.oxl` of `src` shares one clause and one decl
/// namespace (file-scope imports like Go). Decls qualify as `pkg.name`
/// for imported packages, bare for the root (`is_root`). The returned
/// Vec<PathBuf> is item-parallel: the file each item's spans index into.
fn load_pkg(
    src: &PkgSrc,
    imp: &str,
    as_import: bool,
    is_root: bool,
    seen: &mut std::collections::HashSet<PathBuf>,
    pkg_names: &mut std::collections::HashMap<PathBuf, String>,
    loading: &mut Vec<(PathBuf, String)>,
    ctx: Option<&ModCtx>,
    entry: &Path,
    over: &[(&Path, &str)],
    loud: bool,
) -> Option<(ast::Crate, Vec<PathBuf>)> {
    // dir packages: canonical key drives both finished-load dedup
    // (`seen`/`pkg_names`) and in-flight cycle detection (`loading`).
    // A finished package dedups to its cached clause; an in-flight one
    // is an import cycle — Go-forbidden, reported as a resolved-path
    // chain. Quiet mode keeps the old bind-what's-there behavior so the
    // LSP survives cycles in incomplete code.
    if let PkgSrc::Dir(d) = src {
        let key = d.canonicalize().unwrap_or_else(|_| d.clone());
        if let Some(pos) = loading.iter().position(|(k, _)| *k == key) {
            if loud {
                let mut cyc: Vec<String> =
                    loading[pos..].iter().map(|(_, i)| i.clone()).collect();
                cyc.push(imp.to_string());
                eprintln!("error [check] import cycle not allowed: {}", cyc.join(" \u{2192} "));
                return None;
            }
        }
        if !seen.insert(key.clone()) {
            // already loaded: items dedup away but the importer still
            // binds the cached clause name
            let pkg = pkg_names.get(&key).cloned().unwrap_or_default();
            return Some((ast::Crate { pkg, pkg_span: Span::new(0, 0), items: Vec::new() }, Vec::new()));
        }
        loading.push((key, imp.to_string()));
        let r = load_pkg_inner(
            src, imp, as_import, is_root, seen, pkg_names, loading, ctx, entry, over, loud,
        );
        loading.pop();
        return r;
    }
    load_pkg_inner(
        src, imp, as_import, is_root, seen, pkg_names, loading, ctx, entry, over, loud,
    )
}

fn load_pkg_inner(
    src: &PkgSrc,
    imp: &str,
    as_import: bool,
    is_root: bool,
    seen: &mut std::collections::HashSet<PathBuf>,
    pkg_names: &mut std::collections::HashMap<PathBuf, String>,
    loading: &mut Vec<(PathBuf, String)>,
    ctx: Option<&ModCtx>,
    entry: &Path,
    over: &[(&Path, &str)],
    loud: bool,
) -> Option<(ast::Crate, Vec<PathBuf>)> {
    let (dir_disp, files): (String, Vec<PathBuf>) = match src {
        PkgSrc::Dir(d) => {
            let files = match pkg_dir_files(d) {
                Ok(fs) => fs,
                Err(e) => {
                    if loud {
                        eprintln!("error [check] {}: {e}", d.display());
                        return None;
                    }
                    return Some((ast::Crate { pkg: String::new(), pkg_span: Span::new(0, 0), items: Vec::new() }, Vec::new()));
                }
            };
            (d.to_string_lossy().to_string(), files)
        }
        PkgSrc::Files(fs) => (
            fs[0]
                .parent()
                .unwrap_or(Path::new("."))
                .to_string_lossy()
                .to_string(),
            fs.clone(),
        ),
    };
    if let PkgSrc::Dir(_) = src {
        if files.is_empty() {
            if loud {
                eprintln!("error [check] no .oxl source files in `{dir_disp}`");
                return None;
            }
            return Some((ast::Crate { pkg: String::new(), pkg_span: Span::new(0, 0), items: Vec::new() }, Vec::new()));
        }
    }
    // parse all files first — clause validation precedes decl collection
    let mut pfiles: Vec<(PathBuf, String, Vec<ast::Item>)> = Vec::new();
    for f in &files {
        match parse_pkg_file(f, over, loud) {
            Some((clause, items)) => pfiles.push((f.clone(), clause, items)),
            None if loud => return None,
            None => return Some((ast::Crate { pkg: String::new(), pkg_span: Span::new(0, 0), items: Vec::new() }, Vec::new())),
        }
    }
    let clause0 = pfiles[0].1.clone();
    for pf in &pfiles[1..] {
        if pf.1 != clause0 {
            if loud {
                eprintln!(
                    "error [check] found packages `{clause0}` (`{}`) and `{}` (`{}`) in `{dir_disp}`",
                    fname(&pfiles[0].0), pf.1, fname(&pf.0)
                );
                return None;
            }
            break;
        }
    }
    if as_import && clause0 == "main" {
        if loud {
            eprintln!("error [check] `{imp}` is a program, not an importable package");
            return None;
        }
    }
    if matches!(src, PkgSrc::Dir(_)) && clause0 != "main" && clause0 != path_base(&dir_disp) {
        if loud {
            eprintln!(
                "error [check] package name `{clause0}` does not match directory `{}`",
                path_base(&dir_disp)
            );
            return None;
        }
    }
    // Files-mode file lists must share one directory (Go's file-list rule)
    if let PkgSrc::Files(fs) = src {
        let d0 = fs[0].parent().map(Path::to_path_buf);
        for f in &fs[1..] {
            if f.parent().map(Path::to_path_buf) != d0 {
                if loud {
                    eprintln!(
                        "error [check] named files must all be in one directory; have `{}` and `{}`",
                        d0.as_deref().unwrap_or(Path::new(".")).display(),
                        f.parent().unwrap_or(Path::new(".")).display()
                    );
                    return None;
                }
                break;
            }
        }
    }
    if let PkgSrc::Dir(d) = src {
        pkg_names.insert(d.canonicalize().unwrap_or_else(|_| d.clone()), clause0.clone());
    }
    // shared decl renames across every file of the package (root stays bare)
    let mut shared: std::collections::HashMap<String, String> = Default::default();
    if !is_root {
        for (_, _, items) in &pfiles {
            for it in items {
                // methods key on the receiver type (`T.m`), not the bare
                // name — two types may share a method name
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
                    shared.insert(n.clone(), format!("{clause0}.{n}"));
                }
            }
        }
    }
    let mut items = Vec::new();
    let mut item_files: Vec<PathBuf> = Vec::new();
    for (fpath, _, fitems) in pfiles {
        // per-file maps: package-shared decls + this file's own imports
        let mut renames = shared.clone();
        let mut aliases: std::collections::HashMap<String, String> = Default::default();
        // pass 1: resolve this file's imports first so its decls see the
        // bindings regardless of source order
        for it in &fitems {
            if let ast::Item::Import(i) = it {
                match resolve_pkg(ctx, entry, &i.path) {
                    Resolve::Dir(d) => {
                        let idir = fpath
                            .parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or(Path::new("."));
                        if !internal_ok(&d, idir) {
                            if loud {
                                eprintln!(
                                    "error [check] use of internal package `{}` not allowed",
                                    i.path
                                );
                                return None;
                            }
                            continue;
                        }
                        match load_pkg(
                            &PkgSrc::Dir(d), &i.path, true, false, seen, pkg_names, loading, ctx,
                            entry, over, loud,
                        ) {
                            Some((sub, sub_files)) => {
                                let bound = sub.pkg.clone();
                                if !bound.is_empty() {
                                    aliases.insert(
                                        i.alias.clone().unwrap_or_else(|| bound.clone()),
                                        bound.clone(),
                                    );
                                    if i.alias.is_none() {
                                        aliases.insert(bound.clone(), bound.clone());
                                    }
                                    for s in &i.selective {
                                        renames.insert(s.clone(), format!("{bound}.{s}"));
                                    }
                                }
                                items.extend(sub.items);
                                item_files.extend(sub_files);
                            }
                            None if loud => return None,
                            None => {}
                        }
                    }
                    Resolve::Missing(mv) => {
                        if loud {
                            eprintln!(
                                "error [check] missing module `{mv}` — oxl get it or replace it"
                            );
                            return None;
                        }
                    }
                    Resolve::NotFound => {
                        if loud {
                            let hint = if remoteish(&i.path) {
                                " (missing module — oxl get it or add a require)"
                            } else {
                                ""
                            };
                            eprintln!("error [check] cannot find package `{}`{hint}", i.path);
                            return None;
                        }
                    }
                }
            }
        }
        // pass 2: qualify this file's decls against the complete maps
        for it in fitems {
            match it {
                ast::Item::Import(_) => {}
                mut it => {
                    qualify_item(&mut it, &renames, &aliases);
                    items.push(it);
                }
            }
        }
        item_files.extend(
            std::iter::repeat(fpath.clone()).take(items.len() - item_files.len()),
        );
    }
    debug_assert_eq!(items.len(), item_files.len());
    Some((ast::Crate { pkg: clause0, pkg_span: Span::new(0, 0), items }, item_files))
}

/// root package: the file/dir args name the sources; decls stay bare
fn load_crate(src: &PkgSrc) -> Option<(ast::Crate, Vec<PathBuf>)> {
    let entry = match src {
        PkgSrc::Dir(d) => d.clone(),
        PkgSrc::Files(fs) => fs[0].parent().unwrap_or(Path::new(".")).to_path_buf(),
    };
    let ctx = match mod_ctx(&entry, true) {
        Ok(c) => c,
        Err(()) => return None,
    };
    let mut seen = std::collections::HashSet::new();
    let mut pkg_names = std::collections::HashMap::new();
    let mut loading = Vec::new();
    let label = match src {
        PkgSrc::Dir(d) => d.to_string_lossy().to_string(),
        PkgSrc::Files(fs) => fs[0].to_string_lossy().to_string(),
    };
    load_pkg(
        src,
        &label,
        false,
        true,
        &mut seen,
        &mut pkg_names,
        &mut loading,
        ctx.as_ref(),
        &entry,
        &[],
        true,
    )
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

/// positional args naming sources: a leading directory, or consecutive
/// `.oxl` files (Go's `go run a.go b.go` file-list rule). Returns the
/// source args and the index where program args begin (after `--`).
fn pkg_args(args: &[String], start: usize) -> (Vec<String>, usize) {
    let mut fs = Vec::new();
    let mut i = start;
    while i < args.len() && args[i] != "--" {
        let a = &args[i];
        if fs.is_empty() && Path::new(a).is_dir() {
            fs.push(a.clone());
            i += 1;
            break;
        }
        if !a.ends_with(".oxl") {
            break;
        }
        fs.push(a.clone());
        i += 1;
    }
    (fs, i)
}

/// source args -> package source: one dir = dir package, files = the
/// file-list package, none = the cwd package
fn pkg_src(fs: &[String]) -> PkgSrc {
    if fs.len() == 1 && Path::new(&fs[0]).is_dir() {
        PkgSrc::Dir(PathBuf::from(&fs[0]))
    } else if fs.is_empty() {
        PkgSrc::Dir(PathBuf::from("."))
    } else {
        PkgSrc::Files(fs.iter().map(PathBuf::from).collect())
    }
}

/// the file whose source backs diag rendering — the first package file
/// (spans in other files mis-map: the pre-existing per-crate limitation)
fn entry_file(src: &PkgSrc) -> Option<PathBuf> {
    match src {
        PkgSrc::Dir(d) => pkg_dir_files(d).unwrap_or_default().into_iter().next(),
        PkgSrc::Files(fs) => fs.first().cloned(),
    }
}

fn compile_mode(src: &PkgSrc, test_mode: bool) -> Result<String, ()> {
    let (krate, item_files) = match load_crate(src) {
        Some(k) => k,
        None => return Err(()),
    };
    // `oxl test` emits its own driver main, so library packages test fine;
    // build/run produce a binary and need `package main` + fn main
    if !test_mode && krate.pkg != "main" {
        eprintln!("error [check] package `{}` is not a main package", krate.pkg);
        return Err(());
    }
    let Some(entry) = entry_file(src) else {
        return Err(());
    };
    let src_text = match std::fs::read_to_string(&entry) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", entry.display());
            return Err(());
        }
    };
    // note: semantic diagnostics in imported files map spans against the
    // entry file's source (v0 limitation — spans are per-crate)
    let mut diags = diag::Diags::new(entry.to_string_lossy(), src_text);
    let files: Vec<String> = item_files.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let checked = check::check(&krate, &files, &mut diags);
    diags.report();
    if diags.has_errors() {
        return Err(());
    }
    Ok(codegen::generate_mode(&krate, test_mode, &checked.suspend))
}

fn compile(src: &PkgSrc) -> Result<String, ()> {
    compile_mode(src, false)
}

/// private scratch dir under $TMPDIR: created exclusively (never a
/// pre-existing dir someone else owns), 0700 on unix, retried a few
/// times on a name collision before giving up.
fn mk_scratch_dir(prefix: &str) -> Option<PathBuf> {
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    for attempt in 0..16u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{nanos}-{attempt}",
            std::process::id()
        ));
        match b.create(&dir) {
            Ok(()) => return Some(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                eprintln!("cannot create build dir {}: {e}", dir.display());
                return None;
            }
        }
    }
    eprintln!(
        "cannot create a fresh {prefix} dir under {}",
        std::env::temp_dir().display()
    );
    None
}

// LANG_CC wins unconditionally; else the first compiler that spawns.
// POSIX finds cc; Windows finds mingw gcc or LLVM clang (MSVC's cl can't
// take this C — _Thread_local, __sync builtins).
fn pick_cc() -> String {
    if let Ok(cc) = std::env::var("LANG_CC") {
        return cc;
    }
    for c in ["cc", "gcc", "clang"] {
        if Command::new(c)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return c.into();
        }
    }
    "cc".into() // let the spawn error name a binary that isn't there
}

// cc names its product after -o; on Windows the runnable artifact needs
// .exe spelled out or CreateProcess can't resolve it. Append rather than
// with_extension — `-o app.debug` means app.debug.exe, not app.exe
fn exe_out_name(out: PathBuf) -> PathBuf {
    // extension compare is case-insensitive — `-o app.EXE` must not
    // grow a second suffix; Windows resolves either spelling
    let has_exe = out
        .extension()
        .and_then(|e| e.to_str())
        .map_or(false, |e| e.eq_ignore_ascii_case("exe"));
    if cfg!(windows) && !has_exe {
        let mut s = out.into_os_string();
        s.push(".exe");
        s.into()
    } else {
        out
    }
}

fn emit_and_cc(c_src: &str, out: &Path) -> bool {
    // intermediates go in a private scratch dir — never beside the
    // user's -o where they could clobber existing out.c/rt.c/rt.h
    let Some(dir) = mk_scratch_dir("oxl-build") else {
        return false;
    };
    if let Some(od) = out.parent() {
        if let Err(e) = std::fs::create_dir_all(od) {
            eprintln!("cannot create output dir {}: {e}", od.display());
            return false;
        }
    }
    let c_file = dir.join("out.c");
    if let Err(e) = std::fs::write(&c_file, c_src) {
        eprintln!("cannot write {}: {e}", c_file.display());
        return false;
    }
    let (rt_h, rt_c) = codegen::runtime_files();
    for (name, text) in [("rt.h", rt_h), ("rt.c", rt_c)] {
        if let Err(e) = std::fs::write(dir.join(name), text) {
            eprintln!("cannot write {}: {e}", dir.join(name).display());
            return false;
        }
    }
    let cc = pick_cc();
    // Link (and compile) SQLite only when the emitted program uses it:
    // -lsqlite3 fails outright where libsqlite3-dev isn't installed,
    // which would break every build — including `oxl run hello.oxl` —
    // on a stock system. rt.c compiles its sqlite section only under
    // -DOXL_SQLITE, so the flag and the library arrive together.
    let needs_sqlite = std::fs::read_to_string(&c_file)
        .map(|s| s.contains("rt_sqlite_"))
        .unwrap_or(false);
    let mut cmd = Command::new(&cc);
    cmd.arg("-O2");
    #[cfg(not(windows))]
    cmd.arg("-pthread");
    cmd.arg("-o").arg(out).arg(&c_file).arg(dir.join("rt.c"));
    // rt.c's socket shims reference WinSock unconditionally on Windows —
    // ws2_32 must reach the link even when nothing else pulls it
    #[cfg(windows)]
    cmd.arg("-lws2_32");
    if needs_sqlite {
        // -D before sources, -l after: --as-needed linkers (Ubuntu gcc
        // default) drop libs seen before the undefined refs they resolve
        cmd.arg("-DOXL_SQLITE");
        // a stock Windows box has no sqlite dev package — LANG_SQLITE_SRC
        // points at an amalgamation sqlite3.c to compile in its place
        if let Ok(src) = std::env::var("LANG_SQLITE_SRC") {
            let sp = Path::new(&src);
            if let Some(d) = sp.parent() {
                cmd.arg(format!("-I{}", d.display()));
            }
            cmd.arg(sp);
        } else {
            cmd.arg("-lsqlite3");
        }
    }
    // extra link inputs (FFI shim objects, libs) — e.g. the gui pilot's
    // LANG_LDFLAGS="gui_shim.o $(pkg-config --libs gtk4)"
    if let Ok(flags) = std::env::var("LANG_LDFLAGS") {
        cmd.args(flags.split_whitespace());
    }
    let status = cmd
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    match status {
        Ok(s) if s.success() => {
            let _ = std::fs::remove_dir_all(&dir); // scratch dir — best effort
            true
        }
        _ => {
            eprintln!("cc failed (emitted C kept at {})", c_file.display());
            false
        }
    }
}

fn cmd_build(args: &[String]) {
    let (fs, _) = pkg_args(args, 2);
    let src = pkg_src(&fs);
    let out = exe_out_name(out_arg(args).unwrap_or_else(|| PathBuf::from("a.out")));
    match compile(&src) {
        Ok(c) => {
            if !emit_and_cc(&c, &out) {
                exit(1);
            }
        }
        Err(()) => exit(1),
    }
}

fn cmd_run(args: &[String]) {
    let (fs, argi) = pkg_args(args, 2);
    let src = pkg_src(&fs);
    match compile(&src) {
        Ok(c) => {
            let Some(dir) = mk_scratch_dir("oxl-run") else {
                exit(1);
            };
            let out = exe_out_name(dir.join("a.out"));
            if !emit_and_cc(&c, &out) {
                exit(1);
            }
            let child_args: &[String] = if args.get(argi).map(|a| a.as_str()) == Some("--") {
                &args[argi + 1..]
            } else {
                &args[argi..]
            };
            let st = Command::new(&out).args(child_args).status().unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            exit(st.code().unwrap_or(1));
        }
        Err(()) => exit(1),
    }
}

fn cmd_test(args: &[String]) {
    let (fs, _) = pkg_args(args, 2);
    let src = pkg_src(&fs);
    match compile_mode(&src, true) {
        Ok(c) => {
            let Some(dir) = mk_scratch_dir("oxl-test") else {
                exit(1);
            };
            let out = exe_out_name(dir.join("a.out"));
            if !emit_and_cc(&c, &out) {
                exit(1);
            }
            let st = Command::new(&out).status().unwrap();
            let _ = std::fs::remove_dir_all(&dir);
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
    diags: Vec<(String, usize, usize, String)>,
    index: check::Index,
}

fn analyze(
    path: &Path,
    src: &str,
    open: &std::collections::HashMap<String, String>,
) -> Analysis {
    // diagnostic pass: lex+parse the buffer so its parse errors surface
    // (load_pkg re-parses silently for the merged crate)
    let mut diags = diag::Diags::new(path.to_string_lossy(), src);
    let toks = lexer::lex(src, &mut diags);
    let _krate = parser::parse(&toks, &mut diags);
    let mut seen = std::collections::HashSet::new();
    let mut pkg_names = std::collections::HashMap::new();
    let mut loading = Vec::new();
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let ctx = mod_ctx(dir, false).ok().flatten();
    // the whole package loads so decls in sibling files resolve; `over`
    // keeps the open buffer's unsaved text authoritative for `path`
    let src_pkg = if dir.is_dir() {
        PkgSrc::Dir(dir.to_path_buf())
    } else {
        PkgSrc::Files(vec![path.to_path_buf()])
    };
    // every open buffer is authoritative over its path — an unsaved
    // sibling's decls must resolve, not just the analyzed doc's
    let over: Vec<(&Path, &str)> = open
        .iter()
        .map(|(p, t)| (Path::new(p.as_str()), t.as_str()))
        .collect();
    let loaded = load_pkg(
        &src_pkg,
        &dir.to_string_lossy(),
        false,
        true,
        &mut seen,
        &mut pkg_names,
        &mut loading,
        ctx.as_ref(),
        dir,
        &over,
        false,
    );
    let (krate, item_files) = loaded.unwrap_or_else(|| {
        (ast::Crate { pkg: String::new(), pkg_span: Span::new(0, 0), items: Vec::new() }, Vec::new())
    });
    // Path joins emit `\` on Windows while uri_to_path keeps `/` —
    // every downstream identity is the string form, so normalize here
    let files: Vec<String> = item_files
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    let checked = check::check(&krate, &files, &mut diags);
    if std::env::var("OXL_IDX_DEBUG").is_ok() {
        for u in &checked.index.uses {
            eprintln!("USE {} {:?} -> {} {:?} `{}`", u.file, u.span, u.def_file, u.def_span, u.sig);
        }
    }
    // every diag maps against ITS file's text: the open buffers where
    // they exist, disk contents for closed files
    let doc = path.to_string_lossy().to_string();
    let mut srcs: std::collections::HashMap<String, String> = open.clone();
    Analysis {
        diags: diags
            .list
            .iter()
            .map(|d| {
                // LSP columns are UTF-16 units; publish_diags wants 1-based
                let f = if d.file.is_empty() { doc.clone() } else { d.file.replace('\\', "/") };
                let text = srcs
                    .entry(f.clone())
                    .or_insert_with(|| std::fs::read_to_string(&f).unwrap_or_default());
                let (l, c) = lsp_line_col(text, d.span.lo);
                (f, l + 1, c + 1, format!("[{}] {}", d.code, d.msg))
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
    let mut it = body[i..].chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(out),
            '\\' => match it.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let mut cp = hex4(&mut it)?;
                    if (0xD800..0xDC00).contains(&cp) {
                        // high surrogate: pair with a following \uDC00..\uDFFF
                        let mut peek = it.clone();
                        if peek.next() == Some('\\') && peek.next() == Some('u') {
                            let lo = hex4(&mut peek)?;
                            if (0xDC00..0xE000).contains(&lo) {
                                it = peek;
                                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                            }
                        }
                    }
                    out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// reads the four hex digits of a `\uXXXX` escape
/// filesystem path -> `file://` URI (inverse of uri_to_path):
/// percent-encode every byte that is not unreserved and not `/`
fn path_to_uri(p: &str) -> String {
    let mut out = String::from("file://");
    // Windows paths need unix separators and the authority-form slash:
    // D:\a\b -> file:///D:/a/b (POSIX /a/b already has one)
    let mut p = p;
    if p.len() >= 2 && p.as_bytes()[0].is_ascii_alphabetic() && p.as_bytes()[1] == b':' {
        out.push('/');
        out.push(p.as_bytes()[0] as char);
        out.push(':');
        p = &p[2..];
    }
    for &b in p.as_bytes() {
        let b = if b == b'\\' { b'/' } else { b };
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `file://` URI -> filesystem path (LSP clients percent-encode)
fn uri_to_path(uri: &str) -> String {
    let s = uri.strip_prefix("file://").unwrap_or(uri);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            // decode on bytes — a str slice at i+3 can land mid-char
            // after a multibyte byte and panic
            let nib = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
            if let (Some(h), Some(l)) = (nib(b[i + 1]), nib(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    let mut p = String::from_utf8_lossy(&out).into_owned();
    // a Windows file URI carries the drive behind the authority slash
    // (file:///D:/a/b) — POSIX paths actually start with that slash
    if p.len() >= 3
        && p.as_bytes()[0] == b'/'
        && p.as_bytes()[1].is_ascii_alphabetic()
        && p.as_bytes()[2] == b':'
    {
        p.remove(0);
    }
    p
}

fn hex4(it: &mut std::str::Chars) -> Option<u32> {
    let s: String = it.by_ref().take(4).collect();
    u32::from_str_radix(&s, 16).ok()
}

/// escape a string for embedding in a JSON string literal — every
/// control char, not just backslash and quote, or diagnostics and hover
/// text containing newlines/tabs produce invalid JSON on the wire
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
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
            json_escape(msg)
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
/// `initialize` doesn't negotiate a position encoding, so cols follow
/// the LSP default: UTF-16 code units, not chars.
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
                k += c2.len_utf16();
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

/// byte offset → LSP (0-based line, 0-based UTF-16-unit col)
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
            c += ch.len_utf16();
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
    // per-doc state is just the buffer text; each analysis run covers
    // the whole package, so one latest index per package dir is the
    // freshest source for every file in it
    let mut docs: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut analyses: std::collections::HashMap<String, Analysis> =
        std::collections::HashMap::new();
    // URIs that hold published diagnostics, per analysis scope — a
    // file whose list clears still needs an explicit empty publish or
    // the client shows stale, and scoping keeps one package's analysis
    // from wiping another's
    let mut published: std::collections::HashMap<
        String,
        std::collections::HashSet<String>,
    > = std::collections::HashMap::new();
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
        let method = json_str(&body, "method").unwrap_or_default();
        if method == "exit" {
            return;
        }
        if method == "textDocument/didOpen" || method == "textDocument/didChange" {
            if let (Some(uri), Some(text)) = (json_str(&body, "uri"), json_str(&body, "text")) {
                let p = uri_to_path(&uri);
                // every open buffer overrides its disk copy — a sibling's
                // unsaved edits must resolve in this analysis too
                let mut open: std::collections::HashMap<String, String> = docs
                    .iter()
                    .map(|(u, t)| (uri_to_path(u), t.clone()))
                    .collect();
                open.insert(p.clone(), text.clone());
                let a = analyze(Path::new(&p), &text, &open);
                let dir_key = Path::new(&p)
                    .parent()
                    .map(|d| d.to_string_lossy().to_string())
                    .unwrap_or_else(|| ".".to_string());
                let mut order: Vec<String> = Vec::new();
                let mut by_file: std::collections::HashMap<String, Vec<(usize, usize, String)>> =
                    Default::default();
                for (f, l, c, m) in &a.diags {
                    if !by_file.contains_key(f) {
                        order.push(f.clone());
                    }
                    by_file.entry(f.clone()).or_default().push((*l, *c, m.clone()));
                }
                // the open doc always publishes first — even an empty
                // list is meaningful (it clears stale diagnostics), and
                // a client reading one response per didOpen must get it
                let mut ordered = vec![p.clone()];
                ordered.extend(order.iter().filter(|f| **f != p).cloned());
                let mut current: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut sent: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                for f in ordered {
                    // the open doc reuses the client's URI verbatim; a
                    // sibling publishes under its own
                    let u = if f == p { uri.clone() } else { path_to_uri(&f) };
                    let list = by_file.get(&f).map(Vec::as_slice).unwrap_or(&[]);
                    publish_diags(&mut stdout, &u, list);
                    sent.insert(u.clone());
                    if !list.is_empty() {
                        current.insert(u);
                    }
                }
                // a file whose last diagnostic cleared still needs an
                // explicit empty publish — the client keeps the old one
                // otherwise. only URIs this scope published before are
                // cleared: diagnostics other packages own stay valid.
                // skip URIs already published in this round
                let prev = published.get(&dir_key).cloned().unwrap_or_default();
                for u in prev
                    .difference(&current)
                    .filter(|u| !sent.contains(*u))
                    .cloned()
                    .collect::<Vec<_>>()
                {
                    publish_diags(&mut stdout, &u, &[]);
                }
                published.insert(dir_key.clone(), current);
                docs.insert(uri, text);
                analyses.insert(dir_key, a);
            }
            continue;
        }
        if method == "textDocument/didClose" {
            if let Some(uri) = json_str(&body, "uri") {
                docs.remove(&uri);
                for set in published.values_mut() {
                    set.remove(&uri);
                }
            }
            continue;
        }
        let resp = if method == "initialize" {
            let id = extract_id(&body);
            format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"capabilities\":{{\"textDocumentSync\":1,\"hoverProvider\":true,\"definitionProvider\":true}}}}}}")
        } else if method == "shutdown" {
            let id = extract_id(&body);
            format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":null}}")
        } else if method == "textDocument/hover" || method == "textDocument/definition" {
            let id = extract_id(&body);
            let uri = json_str(&body, "uri").unwrap_or_default();
            let (line, col) = (
                json_num(&body, "line").unwrap_or(0),
                json_num(&body, "character").unwrap_or(0),
            );
            let def = method == "textDocument/definition";
            let file = uri_to_path(&uri);
            let dir_key = Path::new(&file)
                .parent()
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or_else(|| ".".to_string());
            let hit = docs.get(&uri).and_then(|src| {
                analyses
                    .get(&dir_key)
                    .and_then(|a| index_at(a, &file, lsp_offset(src, line, col)))
            });
            match (hit, def) {
                (Some(u), false) => format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"contents\":{{\"kind\":\"plaintext\",\"value\":\"{}\"}}}}}}",
json_escape(&u.sig)
                ),
                (Some(u), true) => {
                    // def spans index into their own file's source — the
                    // open doc uses its buffer, imports read from disk
                    let src = docs
                        .iter()
                        .find_map(|(du, t)| {
                            (uri_to_path(du) == u.def_file).then(|| t.clone())
                        })
                        .or_else(|| std::fs::read_to_string(&u.def_file).ok())
                        .unwrap_or_default();
                    let (l0, c0) = lsp_line_col(&src, u.def_span.lo);
                    let (l1, c1) = lsp_line_col(&src, u.def_span.hi);
                    let uri2 = if u.def_file.starts_with('/') {
                        path_to_uri(&u.def_file)
                    } else {
                        // relative decl paths resolve against the doc dir
                        let base = Path::new(&file).parent().unwrap_or(Path::new("."));
                        let p = base.join(&u.def_file);
                        let p = p.canonicalize().unwrap_or(p);
                        path_to_uri(&p.to_string_lossy())
                    };
                    let uri2 = json_escape(&uri2);
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
