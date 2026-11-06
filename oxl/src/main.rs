mod ast;
mod check;
mod codegen;
mod diag;
mod fetch;
mod fmt;
mod modcache;
mod lexer;
mod parser;
mod semver;

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
        eprintln!("oxl — one toolchain binary\n\n  oxl build [<pkg dir>|<file.oxl>...] [-o out]\n  oxl run [<pkg dir>|<file.oxl>...]\n  oxl test [<pkg dir>|<file.oxl>...]\n  oxl fmt <file.oxl>...\n  oxl doc <file.oxl>\n  oxl get <path>@<query>\n  oxl mod <init|tidy|verify|list> ...\n  oxl list -m all\n  oxl lsp");
        exit(2);
    }
    match args[1].as_str() {
        "build" => cmd_build(&args),
        "run" => cmd_run(&args),
        "test" => cmd_test(&args),
        "fmt" => cmd_fmt(&args),
        "doc" => cmd_doc(&args),
        "get" => cmd_get(&args),
        "mod" => cmd_mod(&args),
        "list" => cmd_list(&args),
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
    /// replace LHS (path, optional version) -> local dir
    replaces: Vec<(String, Option<String>, PathBuf)>,
    modcache: PathBuf,
    /// oxl.sum beside oxl.mod — every cache-resolved module verifies
    /// against it on load (replaces bypass by design). Empty when the
    /// file is absent; a missing line then fails like Go's
    /// "missing go.sum entry".
    sums: Vec<modcache::SumLine>,
    /// MVS build list: each required module path at its selected
    /// (max) version — resolution reads dirs at THESE versions, not
    /// the raw require lines
    selected: Vec<(String, String)>,
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

/// module cache root: OXLMODCACHE wins, else the platform default —
/// %LOCALAPPDATA%\oxl\mod on Windows, ~/.oxl/mod elsewhere.
fn modcache_dir() -> PathBuf {
    modcache::cache_dir()
}

/// parse oxl.mod text -> (module path, `oxl` floor as written,
/// requires, replaces). Errors come back ready to print:
/// `oxl.mod:<line>: <msg>` — display stays hermetic (the manifest is
/// manifest comment rule: `//` opens a comment only at line start or
/// after whitespace — a `//` glued inside a token (`file://`, `https://`)
/// is part of the path, same constraint Go's modfile lexer applies
/// implicitly. Returns the line up to the first real comment.
fn strip_comment(raw: &str) -> &str {
    let b = raw.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'/' && b[i + 1] == b'/' && (i == 0 || b[i - 1].is_ascii_whitespace()) {
            return &raw[..i];
        }
        i += 1;
    }
    raw
}

/// always AT the module root, so the bare name locates it).
fn parse_manifest(
    src: &str,
) -> Result<(String, Option<String>, Vec<(String, String)>, Vec<(String, Option<String>, PathBuf)>), String> {
    let mut module = String::new();
    let mut floor: Option<String> = None;
    let mut requires: Vec<(String, String)> = Vec::new();
    let mut replaces: Vec<(String, Option<String>, PathBuf)> = Vec::new();
    let mut lines = src.lines().enumerate().peekable();
    while let Some((ln, raw)) = lines.next() {
        let line_no = ln + 1;
        let text = strip_comment(raw).trim();
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
                        let t2 = strip_comment(raw2).trim();
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
                        let Some(rv) = semver::parse(&bt[1]) else {
                            return Err(berr(format!("bad version \"{}\"", bt[1])));
                        };
                        if let Err(e) = semver::require_ok(&bt[0], &bt[1], &rv) {
                            return Err(berr(e));
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
                        if arrow == 2 && semver::parse(&bt[1]).is_none() {
                            return Err(berr(format!("bad version \"{}\"", bt[1])));
                        }
                        let lver = (arrow == 2).then(|| bt[1].clone());
                        replaces.push((bt[0].clone(), lver, PathBuf::from(&bt[arrow + 1])));
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
        .map(|(p, v, d)| {
            let d = if d.is_absolute() { d } else { root.join(&d) };
            (p, v, d)
        })
        .collect();
    // oxl.sum sits beside oxl.mod — absent = no cached module can be
    // trusted (every lookup reports its missing entry); malformed is
    // a hard manifest error, never silently degraded
    let sums = match std::fs::read_to_string(root.join("oxl.sum")) {
        Ok(t) => match modcache::parse_sums(&t) {
            Ok(s) => s,
            Err(e) => {
                if loud {
                    eprintln!("error [check] {e}");
                }
                return Err(());
            }
        },
        Err(_) => Vec::new(),
    };
    let mut ctx = ModCtx {
        path,
        root,
        requires,
        replaces,
        modcache: modcache_dir(),
        sums,
        selected: Vec::new(),
    };
    ctx.selected = build_list(&ctx);
    Ok(Some(ctx))
}

enum Resolve {
    Dir(PathBuf),
    /// declared require with neither replace nor a cache copy —
    /// carries the `path@ver` the diagnostic names
    Missing(String),
    /// cache copy failed oxl.sum verification — carries the
    /// Go-worded mismatch text for the diagnostic
    Tampered(String),
    NotFound,
}

/// the directory a required module version occupies: the matching
/// replace RHS (version-specific replaces beat blanket ones) or the
/// module-cache `path@ver` slot. None = not fetched. A cache hit is
/// verified against oxl.sum before use — Err carries the Go-worded
/// mismatch text for the caller's diagnostic.
fn module_dir(ctx: &ModCtx, mp: &str, ver: &str) -> Result<Option<PathBuf>, String> {
    let mut blanket: Option<&PathBuf> = None;
    for (rp, rv, rd) in &ctx.replaces {
        if rp != mp {
            continue;
        }
        match rv {
            Some(v) if v == ver => return Ok(Some(rd.clone())),
            None => blanket = Some(rd),
            _ => {}
        }
    }
    if let Some(rd) = blanket {
        return Ok(Some(rd.clone()));
    }
    let d = modcache::slot(&ctx.modcache, mp, ver);
    if !d.is_dir() {
        return Ok(None);
    }
    modcache::verify_slot(&d, mp, ver, &ctx.sums)?;
    Ok(Some(d))
}

/// minimal version selection: BFS the requirement graph from the main
/// module's requires; every (module, version) node's own manifest
/// contributes edges; per path keep the MAX version seen — Go's MVS,
/// deterministic with no backtracking. Unfetched nodes still count
/// toward selection; their edges just can't be read.
fn build_list(ctx: &ModCtx) -> Vec<(String, String)> {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    let mut sel: BTreeMap<String, semver::Ver> = BTreeMap::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut queue: VecDeque<(String, String)> = VecDeque::new();
    let offer = |sel: &mut BTreeMap<String, semver::Ver>,
                     queue: &mut VecDeque<(String, String)>,
                     mp: &str,
                     mv: &str| {
        if let Some(v) = semver::parse(mv) {
            let better = sel.get(mp).map_or(true, |cur| semver::compare(&v, cur) == std::cmp::Ordering::Greater);
            if better {
                sel.insert(mp.to_string(), v);
            }
            queue.push_back((mp.to_string(), mv.to_string()));
        }
    };
    for (mp, mv) in &ctx.requires {
        offer(&mut sel, &mut queue, mp, mv);
    }
    while let Some((mp, mv)) = queue.pop_front() {
        if !seen.insert((mp.clone(), mv.clone())) {
            continue;
        }
        let Ok(Some(dir)) = module_dir(ctx, &mp, &mv) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(dir.join("oxl.mod")) else {
            continue;
        };
        let Ok((_, _, reqs, _)) = parse_manifest(&text) else {
            continue;
        };
        for (rp, rv) in reqs {
            offer(&mut sel, &mut queue, &rp, &rv);
        }
    }
    sel.into_iter()
        .map(|(p, v)| {
            let mut vs = format!("v{}.{}.{}", v.maj, v.min, v.pat);
            if !v.pre.is_empty() {
                vs.push('-');
                vs.push_str(&v.pre.join("."));
            }
            if !v.build.is_empty() {
                vs.push('+');
                vs.push_str(&v.build);
            }
            (p, vs)
        })
        .collect()
}

/// the version selection assigns to a required module path — the
/// build list's entry when present, else the main require's floor
fn selected_ver<'a>(ctx: &'a ModCtx, mp: &str) -> Option<&'a str> {
    ctx.selected
        .iter()
        .find(|(p, _)| p == mp)
        .map(|(_, v)| v.as_str())
        .or_else(|| ctx.requires.iter().find(|(p, _)| p == mp).map(|(_, v)| v.as_str()))
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
    // requires — longest path prefix wins (module-path boundaries only);
    // the build list's selection decides which version dir resolves
    let mut best: Option<&String> = None;
    for (mp, _) in &ctx.requires {
        if (imp == *mp || imp.starts_with(&format!("{mp}/")))
            && best.map_or(true, |b| mp.len() > b.len())
        {
            best = Some(mp);
        }
    }
    if let Some(mp) = best {
        let rest = imp[mp.len()..].trim_start_matches('/');
        let v = selected_ver(ctx, mp).unwrap_or("");
        match module_dir(ctx, mp, v) {
            Ok(Some(base)) => {
                let d = base.join(rest);
                return if d.is_dir() {
                    Resolve::Dir(d)
                } else {
                    Resolve::NotFound
                };
            }
            Ok(None) => return Resolve::Missing(format!("{mp}@{v}")),
            Err(e) => return Resolve::Tampered(e),
        }
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

/// per-file import-use marks, recorded by `map_name` as it rewrites:
/// `quals` = local qualifier names that resolved through an import
/// alias (incl. the identity binding); `sels` = resolved `bound.name`
/// targets, so same-named selectives from different imports can't
/// mark each other used
#[derive(Default)]
struct Used {
    quals: std::collections::HashSet<String>,
    sels: std::collections::HashSet<String>,
}

/// rename an oxl identifier through this module's maps:
/// `a.b` -> alias-mapped `stem.b`; bare `n` -> own-decl `mod.n` / selective `s.n`
fn map_name(
    n: &str,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
    used: &mut Used,
) -> String {
    if let Some(r) = renames.get(n) {
        used.sels.insert(r.clone());
        return r.clone();
    }
    if let Some((head, rest)) = n.split_once('.') {
        if let Some(stem) = aliases.get(head) {
            used.quals.insert(head.to_string());
            return format!("{stem}.{rest}");
        }
        if let Some(stem) = renames.get(head) {
            used.sels.insert(stem.clone());
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
    used: &mut Used,
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
                    map_name(&n, renames, aliases, used)
                };
                f.kind = ast::ExprKind::Ident(mapped);
            } else {
                qualify_expr(f, renames, aliases, locals, used);
            }
            for a in args {
                qualify_expr(a, renames, aliases, locals, used);
            }
        }
        ast::ExprKind::Ident(n) => {
            if !local(locals, n) {
                *n = map_name(n, renames, aliases, used)
            }
        }
        ast::ExprKind::StructLit { name, fields } => {
            *name = map_name(name, renames, aliases, used);
            for (_, v) in fields {
                qualify_expr(v, renames, aliases, locals, used);
            }
        }
        ast::ExprKind::Field { obj, .. } => qualify_expr(obj, renames, aliases, locals, used),
        ast::ExprKind::Index { obj, idx } => {
            qualify_expr(obj, renames, aliases, locals, used);
            qualify_expr(idx, renames, aliases, locals, used);
        }
        ast::ExprKind::Slice { obj, lo, hi } => {
            qualify_expr(obj, renames, aliases, locals, used);
            if let Some(x) = lo {
                qualify_expr(x, renames, aliases, locals, used);
            }
            if let Some(x) = hi {
                qualify_expr(x, renames, aliases, locals, used);
            }
        }
        ast::ExprKind::Closure { params, body } => {
            let names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
            for p in params.iter_mut() {
                qualify_ty(&mut p.ty, renames, aliases, used);
            }
            locals.push(names.into_iter().collect());
            qualify_block(body, renames, aliases, locals, used);
            locals.pop();
        }
        ast::ExprKind::Bin { l, r, .. } => {
            qualify_expr(l, renames, aliases, locals, used);
            qualify_expr(r, renames, aliases, locals, used);
        }
        ast::ExprKind::Un { e, .. } => qualify_expr(e, renames, aliases, locals, used),
        ast::ExprKind::As { e, ty } => {
            qualify_expr(e, renames, aliases, locals, used);
            qualify_ty(ty, renames, aliases, used);
        }
        ast::ExprKind::Match { e, arms } => {
            qualify_expr(e, renames, aliases, locals, used);
            for a in arms {
                locals.push(std::collections::HashSet::new());
                match &mut a.pat {
                    ast::Pattern::VarUnit { path }
                    | ast::Pattern::VarPos { path, .. }
                    | ast::Pattern::VarNamed { path, .. } => {
                        *path = map_name(path, renames, aliases, used);
                    }
                    ast::Pattern::Lit(le) => qualify_expr(le, renames, aliases, locals, used),
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
                    ast::MatchBody::Expr(be) => qualify_expr(be, renames, aliases, locals, used),
                    ast::MatchBody::Block(b) => qualify_block(b, renames, aliases, locals, used),
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
    used: &mut Used,
) {
    match &mut t.kind {
        ast::TyKind::Named(n) => *n = map_name(n, renames, aliases, used),
        ast::TyKind::Own(i) | ast::TyKind::Ref(i) | ast::TyKind::Mut(i) => {
            qualify_ty(i, renames, aliases, used)
        }
        ast::TyKind::Chan => {}
        ast::TyKind::Slice(i) => qualify_ty(i, renames, aliases, used),
        ast::TyKind::Fn { params, ret } => {
            for p in params {
                qualify_ty(p, renames, aliases, used);
            }
            if let Some(r) = ret {
                qualify_ty(r, renames, aliases, used);
            }
        }
    }
}

fn qualify_block(
    b: &mut ast::Block,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
    locals: &mut Locals,
    used: &mut Used,
) {
    locals.push(std::collections::HashSet::new());
    for s in &mut b.stmts {
        match s {
            ast::Stmt::ExitBoundary { .. } => unreachable!("codegen-internal marker"),
            ast::Stmt::Let { name, ty, init, .. } => {
                if let Some(t) = ty {
                    qualify_ty(t, renames, aliases, used);
                }
                qualify_expr(init, renames, aliases, locals, used);
                locals.last_mut().unwrap().insert(name.clone());
            }
            ast::Stmt::Let2 { a, b, call, .. } => {
                qualify_expr(call, renames, aliases, locals, used);
                for bn in [a, b] {
                    if let ast::Bind::Name(n) = bn {
                        locals.last_mut().unwrap().insert(n.clone());
                    }
                }
            }
            ast::Stmt::Assign { target, val, .. } => {
                qualify_expr(target, renames, aliases, locals, used);
                qualify_expr(val, renames, aliases, locals, used);
            }
            ast::Stmt::Expr(x) => qualify_expr(x, renames, aliases, locals, used),
            ast::Stmt::Return { vals, .. } => {
                for v in vals {
                    qualify_expr(v, renames, aliases, locals, used);
                }
            }
            ast::Stmt::If { cond, then, els, .. } => {
                qualify_expr(cond, renames, aliases, locals, used);
                qualify_block(then, renames, aliases, locals, used);
                if let Some(e) = els {
                    qualify_block(e, renames, aliases, locals, used);
                }
            }
            ast::Stmt::For { cond, body, .. } => {
                qualify_expr(cond, renames, aliases, locals, used);
                qualify_block(body, renames, aliases, locals, used);
            }
            ast::Stmt::Scope { body, .. } | ast::Stmt::Unsafe { body, .. } => {
                qualify_block(body, renames, aliases, locals, used)
            }
            ast::Stmt::Spawn { call, .. } => qualify_expr(call, renames, aliases, locals, used),
            ast::Stmt::Defer { call, .. } => qualify_expr(call, renames, aliases, locals, used),
            ast::Stmt::Select { arms, .. } => {
                for a in arms {
                    locals.push(std::collections::HashSet::new());
                    match &mut a.kind {
                        ast::SelKind::Recv { chan, bind, .. } => {
                            qualify_expr(chan, renames, aliases, locals, used);
                            if let Some(bn) = bind {
                                locals.last_mut().unwrap().insert(bn.clone());
                            }
                        }
                        ast::SelKind::Timeout { ms, .. } => {
                            qualify_expr(ms, renames, aliases, locals, used)
                        }
                        ast::SelKind::Else { .. } => {}
                    }
                    qualify_block(&mut a.body, renames, aliases, locals, used);
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
    let mut dead = false;
    for (fpath, _, fitems) in pfiles {
        // per-file maps: package-shared decls + this file's own imports
        let mut renames = shared.clone();
        let mut aliases: std::collections::HashMap<String, String> = Default::default();
        // M3: import-use tracking — `used` fills during pass 2,
        // `binds` = (written path, local qualifier, expected sel
        // targets) in source order for the report
        let mut used = Used::default();
        let mut binds: Vec<(String, String, Vec<String>)> = Vec::new();
        // every local name an import introduces — a second binding of
        // the same name is an error (Go: redeclared in this block)
        let mut bound_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
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
                                // `as _` — explicit side-effect import: the
                                // package loads, nothing binds, exempt (M3)
                                if !bound.is_empty() && i.alias.as_deref() != Some("_") {
                                    let qual = i.alias.clone().unwrap_or_else(|| bound.clone());
                                    if !bound_names.insert(qual.clone()) {
                                        if loud {
                                            eprintln!(
                                                "error [check] `{qual}` redeclared as imported package name"
                                            );
                                            return None;
                                        }
                                    } else {
                                        aliases.insert(qual.clone(), bound.clone());
                                        let sels: Vec<String> = i
                                            .selective
                                            .iter()
                                            .map(|s| format!("{bound}.{s}"))
                                            .collect();
                                        for s in &i.selective {
                                            if !bound_names.insert(s.clone()) {
                                                if loud {
                                                    eprintln!(
                                                        "error [check] `{s}` redeclared as imported name"
                                                    );
                                                    return None;
                                                }
                                                continue;
                                            }
                                            renames.insert(s.clone(), format!("{bound}.{s}"));
                                        }
                                        binds.push((i.path.clone(), qual, sels));
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
                    Resolve::Tampered(e) => {
                        if loud {
                            eprintln!("error [check] {e}");
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
                    qualify_item(&mut it, &renames, &aliases, &mut used);
                    items.push(it);
                }
            }
        }
        // M3: each import must have been touched by the qualify walk —
        // one diag per dead binding, source order, then abort
        for (path, qual, sels) in &binds {
            if !used.quals.contains(qual) {
                if loud {
                    eprintln!("error [check] imported and not used: \"{path}\"");
                }
                dead = true;
            }
            for t in sels {
                let sname = t.rsplit('.').next().unwrap_or(t);
                if !used.sels.contains(t) {
                    if loud {
                        eprintln!("error [check] imported and not used: \"{path}\".{{{sname}}}");
                    }
                    dead = true;
                }
            }
        }
        item_files.extend(
            std::iter::repeat(fpath.clone()).take(items.len() - item_files.len()),
        );
    }
    if dead && loud {
        return None;
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
    used: &mut Used,
) {
    match it {
        ast::Item::Fn(f) => {
            if let Some(r) = &mut f.recv {
                qualify_ty(&mut r.ty, renames, aliases, used);
                // the decl name is `{receiver type}.{method}` — receiver
                // base unwraps own/ref/mut to its Named path
                if let Some(base) = recv_base(&r.ty) {
                    f.name = format!("{base}.{}", f.name);
                }
            } else if let Some(n) = renames.get(&f.name) {
                f.name = n.clone();
            }
            for p in &mut f.params {
                qualify_ty(&mut p.ty, renames, aliases, used);
            }
            match &mut f.ret {
                ast::RetTy::One(t) | ast::RetTy::Pair(t) => qualify_ty(t, renames, aliases, used),
                ast::RetTy::Void => {}
            }
            if let Some(b) = &mut f.body {
                let mut scope: std::collections::HashSet<String> =
                    f.params.iter().map(|p| p.name.clone()).collect();
                if let Some(r) = &f.recv {
                    scope.insert(r.name.clone());
                }
                let mut locals: Locals = vec![scope];
                qualify_block(b, renames, aliases, &mut locals, used);
            }
        }
        ast::Item::Struct(s) => {
            if let Some(n) = renames.get(&s.name) {
                s.name = n.clone();
            }
            for p in &mut s.fields {
                qualify_ty(&mut p.ty, renames, aliases, used);
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
                            qualify_ty(t, renames, aliases, used);
                        }
                    }
                    ast::VariantPayload::Named(fs) => {
                        for (_, t) in fs {
                            qualify_ty(t, renames, aliases, used);
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
                    qualify_ty(&mut p.ty, renames, aliases, used);
                }
                match &mut m.ret {
                    ast::RetTy::One(t) | ast::RetTy::Pair(t) => {
                        qualify_ty(t, renames, aliases, used)
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
                qualify_ty(t, renames, aliases, used);
            }
            let mut locals: Locals = vec![std::collections::HashSet::new()];
            qualify_expr(&mut g.init, renames, aliases, &mut locals, used);
        }
        ast::Item::Extern(f) => {
            for p in &mut f.params {
                qualify_ty(&mut p.ty, renames, aliases, used);
            }
            match &mut f.ret {
                ast::RetTy::One(t) | ast::RetTy::Pair(t) => qualify_ty(t, renames, aliases, used),
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
    let needs_sqlite = c_src.contains("rt_sqlite_");
    // OXL_NET wraps the socket family (sock_*, bind_any, set_*addr/
    // timo, htons/htonl wrappers) — emitted C references these names
    // only when the program declares the externs/builtins that emit
    // them, so scanning it is an exact "uses sockets" test.
    let needs_net = [
        "rt_sock_",
        "rt_bind_any",
        "rt_set_reuseaddr",
        "rt_set_rcvtimo",
        "rt_set_sndtimo",
        "rt_htons",
        "rt_htonl",
    ]
    .iter()
    .any(|p| c_src.contains(p));
    let mut cmd = Command::new(&cc);
    cmd.arg("-O2");
    #[cfg(not(windows))]
    cmd.arg("-pthread");
    // G2: every emitted fn gets its own section so the linker can GC
    // unreferenced runtime machinery — spelling is ELF/mingw
    // --gc-sections, Mach-O -dead_strip (MSVC cl never reaches here:
    // pick_cc only tries cc/gcc/clang driver spellings)
    cmd.arg("-ffunction-sections").arg("-fdata-sections");
    if cfg!(target_os = "macos") {
        cmd.arg("-Wl,-dead_strip");
    } else {
        cmd.arg("-Wl,--gc-sections");
    }
    cmd.arg("-o").arg(out).arg(&c_file).arg(dir.join("rt.c"));
    // ws2_32 only when the program uses sockets — under OXL_NET gating
    // rt.c's WinSock code isn't even compiled otherwise
    #[cfg(windows)]
    if needs_net {
        cmd.arg("-lws2_32");
    }
    if needs_net {
        cmd.arg("-DOXL_NET");
    }
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

/// `oxl list -m all` — the resolved build list: main module then every
/// selected requirement at its winning version (Go's `go list -m all`).
fn cmd_list(args: &[String]) {
    let mflag = args.iter().any(|a| a == "-m" || a.starts_with("-m="));
    let all = args.iter().any(|a| a == "all");
    if !mflag || !all || args.len() != 4 {
        eprintln!("usage: oxl list -m all");
        exit(2);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(ctx) = (match mod_ctx(&cwd, true) {
        Ok(c) => c,
        Err(()) => exit(1),
    }) else {
        eprintln!("error [check] not a module — no oxl.mod found above {}", cwd.display());
        exit(1);
    };
    print_build_list(&ctx);
}

/// main module path then the MVS-selected build list — `oxl list -m
/// all` and `oxl mod list` share it
fn print_build_list(ctx: &ModCtx) {
    println!("{}", ctx.path);
    for (mp, mv) in &ctx.selected {
        // `=> dir` only when a replace actually matched this version
        let rep = ctx
            .replaces
            .iter()
            .find(|(rp, rv, _)| rp == mp && rv.as_deref() == Some(mv.as_str()))
            .or_else(|| ctx.replaces.iter().find(|(rp, rv, _)| rp == mp && rv.is_none()));
        let suffix = rep.map_or(String::new(), |(_, _, d)| format!(" => {}", d.display()));
        println!("{mp} {mv}{suffix}");
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


/// `oxl get <path>@<query>` — fetch a module into the cache and pin it:
/// the require line is upserted in oxl.mod, both h1 lines in oxl.sum.
/// `git` does the transport (bare clone -> `git archive` of the resolved
/// rev); extraction is canonical tar bytes, never a checkout. A present
/// slot is reused — re-get only re-pins the sums, matching Go's
/// idempotent `go get` on a cached module.
fn cmd_get(args: &[String]) {
    if args.len() != 3 {
        eprintln!("usage: oxl get <path>@<query>");
        exit(2);
    }
    let spec = &args[2];
    if spec.starts_with('-') {
        eprintln!("usage: oxl get <path>@<query>");
        exit(2);
    }
    let (mpath, query) = match spec.rsplit_once('@') {
        Some((p, q)) if !p.is_empty() => (p.to_string(), if q.is_empty() { "latest" } else { q }),
        _ => (spec.clone(), "latest"),
    };
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("error: {e}");
        exit(1);
    });
    let Some(root) = find_root(&cwd) else {
        eprintln!("error: no oxl.mod in current directory or any parent");
        exit(1);
    };
    let f = match fetch_mod(&mpath, query) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    };
    let cache = modcache_dir();
    let slot = match install(&cache, &mpath, &f) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    };
    // oxl.mod: replace the existing require's version or append one
    let mpath_file = root.join("oxl.mod");
    let mut mtext = std::fs::read_to_string(&mpath_file).unwrap_or_else(|e| {
        eprintln!("error: {}: {e}", mpath_file.display());
        exit(1);
    });
    // oxl.sum: drop stale lines for the module, add the fresh pair
    let spath = root.join("oxl.sum");
    let stext = std::fs::read_to_string(&spath).unwrap_or_default();
    let mut sums = match modcache::parse_sums(&stext) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    };
    let old_ver = match pin_require(&mut mtext, &mut sums, &mpath, &f, &slot) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    };
    if let Err(e) = std::fs::write(&mpath_file, &mtext) {
        eprintln!("error: {}: {e}", mpath_file.display());
        exit(1);
    }
    if let Err(e) = std::fs::write(&spath, modcache::sums_text(&sums)) {
        eprintln!("error: {}: {e}", spath.display());
        exit(1);
    }
    match old_ver {
        Some(o) if o == f.ver => println!("unchanged {mpath}@{}", f.ver),
        Some(o) => println!("upgraded {mpath}@{o} => {}", f.ver),
        None => println!("added {mpath}@{}", f.ver),
    }
}

/// fetch + the self-declared-path check (Go refuses modules whose
/// oxl.mod names a different path than the one they were fetched as —
/// mis-pinned forks have to be replaced explicitly)
fn fetch_mod(mp: &str, query: &str) -> Result<fetch::Fetched, String> {
    let f = fetch::fetch(&fetch::repo_url(mp), mp, query)?;
    let declared = std::str::from_utf8(&f.manifest)
        .map_err(|e| e.to_string())
        .and_then(|t| parse_manifest(t).map(|(p, _, _, _)| p))
        .map_err(|e| format!("{mp}@{}: parsing oxl.mod: {e}", f.ver))?;
    if declared != mp {
        return Err(format!(
            "{mp}@{}: parsing oxl.mod: module declares its path as: {declared}, but was required as: {mp}",
            f.ver
        ));
    }
    Ok(f)
}

/// does the cached slot's tree hash equal the fetched archive's? The
/// check re-extracts fresh — a hand-edited slot must not be re-pinned
/// as trustworthy
fn slot_matches(slot: &Path, f: &fetch::Fetched) -> Result<bool, String> {
    let tmp = slot
        .parent()
        .unwrap_or(slot)
        .join(format!(".verify-{}-{}", std::process::id(), sanitize_tag(&f.ver)));
    let _ = std::fs::remove_dir_all(&tmp);
    let r = std::fs::create_dir_all(&tmp)
        .and_then(|_| modcache::untar(&f.tar, &tmp).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)))
        .map_err(|e| e.to_string())
        .and_then(|_| modcache::dir_h1(&tmp))
        .and_then(|fresh| modcache::dir_h1(slot).map(|cached| fresh == cached));
    modcache::remove_tree(&tmp);
    r
}

/// extract a fetched module into a tmp sibling of its slot — caller
/// inspects the tree, then commit_tmp()s it into place or drops it
/// (modcache::remove_tree)
fn stage_tmp(cache: &Path, ver: &str, f: &fetch::Fetched) -> Result<PathBuf, String> {
    let tmp = cache.join(format!(".tmp-{}-{}", std::process::id(), sanitize_tag(ver)));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)
        .and_then(|_| modcache::untar(&f.tar, &tmp).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)))
        .map_err(|e| {
            modcache::remove_tree(&tmp);
            e.to_string()
        })?;
    Ok(tmp)
}

/// move a staged tree into its slot and seal it. The rename happens
/// while the tree is still writable (a dir's `..` is rewritten on
/// move, so a read-only dir can't be renamed at all), then it seals
/// in place. Consumers can't see the module before its oxl.sum line
/// exists anyway — the sum, not the permission bit, is the integrity
/// mechanism.
fn commit_tmp(tmp: &Path, slot: &Path) -> Result<(), String> {
    if let Some(par) = slot.parent() {
        std::fs::create_dir_all(par).map_err(|e| {
            modcache::remove_tree(tmp);
            e.to_string()
        })?;
    }
    if let Err(e) = std::fs::rename(tmp, slot) {
        // a concurrent get may have won the race — the slot is
        // content-addressed so theirs is identical; reuse it
        if !slot.is_dir() {
            modcache::remove_tree(tmp);
            return Err(format!("{}: {e}", slot.display()));
        }
        modcache::remove_tree(tmp);
    }
    modcache::seal(slot)
}

/// make a fetched module available in the cache: reuse an identical
/// slot, or stage + commit a fresh extraction
fn install(cache: &Path, mp: &str, f: &fetch::Fetched) -> Result<PathBuf, String> {
    let slot = modcache::slot(cache, mp, &f.ver);
    if slot.is_dir() {
        return match slot_matches(&slot, f)? {
            true => Ok(slot),
            false => Err(format!(
                "cached {mp}@{} does not match the fetched archive — remove the slot and get again",
                f.ver
            )),
        };
    }
    let tmp = stage_tmp(cache, &f.ver, f)?;
    commit_tmp(&tmp, &slot)?;
    Ok(slot)
}

/// in-memory require + oxl.sum pin: upserts `require <mp> <ver>` in
/// the manifest text and refreshes the module's two h1 lines in the
/// sums vec. Returns the previous required version, if any.
fn pin_require(
    mtext: &mut String,
    sums: &mut Vec<modcache::SumLine>,
    mp: &str,
    f: &fetch::Fetched,
    slot: &Path,
) -> Result<Option<String>, String> {
    let old = upsert_require(mtext, mp, &f.ver)?;
    sums.retain(|l| !(l.path == mp && l.ver == f.ver));
    sums.push(modcache::SumLine {
        path: mp.to_string(),
        ver: f.ver.clone(),
        is_mod: false,
        h1: modcache::dir_h1(slot)?,
    });
    sums.push(modcache::SumLine {
        path: mp.to_string(),
        ver: f.ver.clone(),
        is_mod: true,
        h1: modcache::mod_h1(mp, &f.ver, &f.manifest),
    });
    Ok(old)
}

/// filename-safe fragment for the tmp extraction dir
fn sanitize_tag(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}

/// upsert `require <path> <ver>` in manifest text: an existing require
/// (single-line or inside a `require (` block) has its version swapped
/// in place; absent gets an appended single-line form. Returns the
/// previous version when one was already pinned.
fn upsert_require(mtext: &mut String, path: &str, ver: &str) -> Result<Option<String>, String> {
    let mut out = String::with_capacity(mtext.len() + 64);
    let mut in_block = false;
    let mut found: Option<String> = None;
    for line in mtext.lines() {
        let t = line.trim();
        if in_block {
            if t.starts_with(')') {
                in_block = false;
            } else if found.is_none() {
                let mut it = t.split_whitespace();
                if it.next() == Some(path) {
                    if let Some(old) = it.next() {
                        found = Some(old.to_string());
                        out.push_str(&line.replacen(old, ver, 1));
                        out.push('\n');
                        continue;
                    }
                }
            }
        } else if t.starts_with("require") {
            let rest = t["require".len()..].trim();
            if rest.starts_with('(') {
                in_block = true;
            } else {
                let mut it = rest.split_whitespace();
                if it.next() == Some(path) {
                    if let Some(old) = it.next() {
                        found = Some(old.to_string());
                        out.push_str(&line.replacen(old, ver, 1));
                        out.push('\n');
                        continue;
                    }
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if found.is_none() {
        out.push_str(&format!("require {path} {ver}\n"));
    }
    *mtext = out;
    Ok(found)
}

// --- mod: manifest maintenance (init / tidy / verify / list) ---

fn cmd_mod(args: &[String]) {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("error: {e}");
        exit(1);
    });
    match args.get(2).map(String::as_str) {
        Some("init") => mod_init(&args[3..], &cwd),
        Some("tidy") => mod_tidy(&args[3..], &cwd),
        Some("verify") => mod_verify(&args[3..], &cwd),
        Some("list") => mod_list(&args[3..], &cwd),
        _ => {
            eprintln!("usage: oxl mod <init <path>|tidy|verify|list>");
            exit(2);
        }
    }
}

/// `oxl mod init <path>` — skeleton manifest in cwd; refuses to
/// overwrite (Go's `go mod init` semantics)
fn mod_init(args: &[String], cwd: &Path) {
    if args.len() != 1 || args[0].starts_with('-') {
        eprintln!("usage: oxl mod init <module path>");
        exit(2);
    }
    let path = &args[0];
    if path.is_empty()
        || path.chars().any(char::is_whitespace)
        || path.contains('@')
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
    {
        eprintln!("error: malformed module path \"{path}\"");
        exit(1);
    }
    let f = cwd.join("oxl.mod");
    if f.exists() {
        eprintln!("error: oxl.mod already exists");
        exit(1);
    }
    let text = format!("module {path}\n\noxl {}\n", tool_ver());
    if let Err(e) = std::fs::write(&f, text) {
        eprintln!("error: {}: {e}", f.display());
        exit(1);
    }
    println!("created oxl.mod: module {path}");
}

/// ModCtx built from in-memory manifest state (tidy rewrites before
/// it writes) — same normalization mod_ctx applies to disk state
fn mk_ctx(root: &Path, mtext: &str, sums: Vec<modcache::SumLine>) -> Result<ModCtx, String> {
    let (path, floor, requires, replaces) = parse_manifest(mtext)?;
    if let Some(fv) = floor {
        let cur = ver_tuple(OXL_VERSION).unwrap_or((0, 0, 0));
        if ver_tuple(&fv).map_or(false, |f| f > cur) {
            return Err(format!("oxl.mod: requires oxl {fv}, toolchain is {}", tool_ver()));
        }
    }
    let replaces = replaces
        .into_iter()
        .map(|(p, v, d)| {
            let d = if d.is_absolute() { d } else { root.join(&d) };
            (p, v, d)
        })
        .collect();
    let mut ctx = ModCtx {
        path,
        root: root.to_path_buf(),
        requires,
        replaces,
        modcache: modcache_dir(),
        sums,
        selected: Vec::new(),
    };
    ctx.selected = build_list(&ctx);
    Ok(ctx)
}

/// is this selected (module, version) covered by a replace? Replaced
/// modules bypass cache + sums entirely
fn replace_covers(ctx: &ModCtx, mp: &str, mv: &str) -> bool {
    ctx.replaces
        .iter()
        .any(|(rp, rv, _)| rp == mp && (rv.is_none() || rv.as_deref() == Some(mv)))
}

/// `oxl mod verify` — every required module's selected cache slot
/// re-hashes against oxl.sum (Go's `go mod verify`)
fn mod_verify(args: &[String], cwd: &Path) {
    if !args.is_empty() {
        eprintln!("usage: oxl mod verify");
        exit(2);
    }
    let Some(root) = find_root(cwd) else {
        eprintln!("error: no oxl.mod in current directory or any parent");
        exit(1);
    };
    let Some(ctx) = (match mod_ctx(&root, true) {
        Ok(c) => c,
        Err(()) => exit(1),
    }) else {
        eprintln!("error: no oxl.mod in current directory or any parent");
        exit(1);
    };
    let mut bad = false;
    for (mp, _) in &ctx.requires {
        let Some(mv) = selected_ver(&ctx, mp).map(str::to_string) else {
            continue;
        };
        if replace_covers(&ctx, mp, &mv) {
            continue;
        }
        let d = modcache::slot(&ctx.modcache, mp, &mv);
        if !d.is_dir() {
            eprintln!("error: {mp}@{mv}: not in module cache");
            bad = true;
            continue;
        }
        if let Err(e) = modcache::verify_slot(&d, mp, &mv, &ctx.sums) {
            eprintln!("error: {e}");
            bad = true;
        }
    }
    if bad {
        exit(1);
    }
    println!("all modules verified");
}

/// `oxl mod list` — the build list (V1's debug surface as the
/// user-facing command; same output as `oxl list -m all`)
fn mod_list(args: &[String], cwd: &Path) {
    if !args.is_empty() {
        eprintln!("usage: oxl mod list");
        exit(2);
    }
    let Some(ctx) = (match mod_ctx(cwd, true) {
        Ok(c) => c,
        Err(()) => exit(1),
    }) else {
        eprintln!("error: no oxl.mod in current directory or any parent");
        exit(1);
    };
    print_build_list(&ctx);
}

// -- tidy --

/// recursive `.oxl` walk of the main module — skips nested module
/// roots (dirs holding their own oxl.mod), dot/underscore dirs, and
/// `testdata`, matching Go's package-enumeration rules
fn main_mod_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.')
                    || name.starts_with('_')
                    || name == "testdata"
                    || p.join("oxl.mod").is_file()
                {
                    continue;
                }
                stack.push(p);
            } else if p.extension().map_or(false, |x| x == "oxl") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// the module a source file belongs to: its nearest oxl.mod dir.
/// None = the main module itself (find_root walks to ctx.root)
fn dep_root_of(f: &Path, ctx: &ModCtx) -> Option<PathBuf> {
    let start = f.parent().unwrap_or(f);
    match find_root(start) {
        Some(r) if r == ctx.root => None,
        other => other,
    }
}

/// longest require-prefix covering `imp` whose selected module dir
/// actually contains the package — the provider of record, same
/// longest-match rule resolve_pkg applies
fn provider(ctx: &ModCtx, imp: &str) -> Option<(String, PathBuf)> {
    let mut best: Option<(String, PathBuf)> = None;
    for (mp, _) in &ctx.requires {
        if !(imp == *mp || imp.starts_with(&format!("{mp}/"))) {
            continue;
        }
        if best.as_ref().map_or(false, |(b, _)| mp.len() <= b.len()) {
            continue;
        }
        let Some(v) = selected_ver(ctx, mp) else {
            continue;
        };
        let Ok(Some(base)) = module_dir(ctx, mp, v) else {
            continue;
        };
        let d = base.join(imp[mp.len()..].trim_start_matches('/'));
        if d.is_dir() {
            best = Some((mp.clone(), d));
        }
    }
    best
}

/// BFS the package graph from the main module's dirs, resolving every
/// import through `ctx`: (providers -> direct/indirect marking,
/// imports no require provides -> dep root for manifest lookup).
/// Importer class comes from dep_root_of — a dep importing a main-
/// module package walks those files as main sources (their imports
/// are direct), matching the compiler's own graph walk.
fn tidy_walk(
    ctx: &ModCtx,
    main_files: &[PathBuf],
) -> (
    std::collections::BTreeMap<String, bool>,
    Vec<(String, Option<PathBuf>)>,
) {
    use std::collections::{BTreeMap, HashSet, VecDeque};
    let mut needed: BTreeMap<String, bool> = BTreeMap::new(); // mp -> is_direct
    let mut unresolved: Vec<(String, Option<PathBuf>)> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    for f in main_files {
        if let Some(d) = f.parent() {
            queue.push_back(d.to_path_buf());
        }
    }
    while let Some(dir) = queue.pop_front() {
        let canon = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if !seen.insert(canon) {
            continue;
        }
        let Ok(files) = pkg_dir_files(&dir) else {
            continue;
        };
        for f in files {
            let main = dep_root_of(&f, ctx).is_none();
            let Some((_, items)) = parse_pkg_file(&f, &[], false) else {
                continue;
            };
            for it in items {
                let ast::Item::Import(i) = it else {
                    continue;
                };
                let imp = i.path.as_str();
                if imp.starts_with("std/") {
                    continue;
                }
                if imp == ctx.path || imp.starts_with(&format!("{}/", ctx.path)) {
                    let d = ctx.root.join(imp[ctx.path.len()..].trim_start_matches('/'));
                    if d.is_dir() {
                        queue.push_back(d);
                    }
                    continue;
                }
                match provider(ctx, imp) {
                    Some((mp, d)) => {
                        let e = needed.entry(mp).or_insert(false);
                        if main {
                            *e = true;
                        }
                        queue.push_back(d);
                    }
                    None => {
                        unresolved.push((imp.to_string(), dep_root_of(&f, ctx)));
                    }
                }
            }
        }
    }
    (needed, unresolved)
}

/// satisfy one unresolved import — mutates the manifest/sums in
/// memory and installs cache slots. Returns whether state changed
/// (the fixpoint driver: no change + still unresolved = hard error).
fn tidy_satisfy(
    imp: &str,
    dep_root: Option<&Path>,
    ctx: &ModCtx,
    mtext: &mut String,
    sums: &mut Vec<modcache::SumLine>,
    probed: &mut std::collections::HashMap<String, Option<std::rc::Rc<fetch::Fetched>>>,
    report: &mut Vec<String>,
) -> Result<bool, String> {
    let cache = ctx.modcache.clone();
    // dep manifest coverage: a dep package's import resolves through
    // ITS module's require — add it to ours at the asked version
    // (MVS picks the max if several deps ask)
    if let Some(dr) = dep_root {
        if let Ok(text) = std::fs::read_to_string(dr.join("oxl.mod")) {
            if let Ok((_, _, dreq, _)) = parse_manifest(&text) {
                let mut best: Option<&(String, String)> = None;
                for r in &dreq {
                    if (imp == r.0 || imp.starts_with(&format!("{}/", r.0)))
                        && best.map_or(true, |(b, _)| r.0.len() > b.len())
                    {
                        best = Some(r);
                    }
                }
                if let Some((mp, mv)) = best {
                    if !ctx.requires.iter().any(|(p, _)| p == mp) {
                        // fetch the dep's asked tag — MVS never selects
                        // below it
                        let f = fetch_mod(mp, mv)?;
                        let slot = install(&cache, mp, &f)?;
                        pin_require(mtext, sums, mp, &f, &slot)?;
                        report.push(format!("added {mp}@{} // indirect", f.ver));
                        return Ok(true);
                    }
                    // already required but doesn't provide the
                    // package — fall to the generic probe (upgrade
                    // path or a more specific nested module)
                }
            }
        }
    }
    // standalone replace coverage: replace LHS covers imp and its dir
    // holds the package — the import works once a require exists (Go's
    // local-replace placeholder version)
    let mut best_rep: Option<&(String, Option<String>, PathBuf)> = None;
    for r in &ctx.replaces {
        if (imp == r.0 || imp.starts_with(&format!("{}/", r.0)))
            && best_rep.map_or(true, |(b, _, _)| r.0.len() > b.len())
        {
            best_rep = Some(r);
        }
    }
    if let Some((rp, _, rd)) = best_rep {
        if rd.join(imp[rp.len()..].trim_start_matches('/')).is_dir()
            && !ctx.requires.iter().any(|(p, _)| p == rp)
        {
            upsert_require(mtext, rp, "v0.0.0-00010101000000-000000000000")?;
            report.push(format!("added {rp} (replace)"));
            return Ok(true);
        }
    }
    // generic probe: try each prefix of the import path as a module,
    // longest first — a required module that lacks the package gets
    // re-probed at @latest (Go's upgrade path); a deeper path may be
    // its own nested module
    let segs: Vec<&str> = imp.split('/').collect();
    for n in (1..=segs.len()).rev() {
        let m = segs[..n].join("/");
        let f = match probed.get(&m) {
            Some(Some(f)) => Some(f.clone()),
            Some(None) => continue,
            None => {
                let r = fetch_mod(&m, "latest").ok().map(std::rc::Rc::new);
                probed.insert(m.clone(), r.clone());
                match r {
                    Some(f) => Some(f),
                    None => continue,
                }
            }
        };
        let f = f.unwrap();
        let rest = imp[m.len()..].trim_start_matches('/');
        // check the package dir against an installed slot or a
        // fresh staging of the fetched archive
        let slot = modcache::slot(&cache, &m, &f.ver);
        let (base, tmp) = if slot.is_dir() {
            (slot.clone(), None)
        } else {
            let t = stage_tmp(&cache, &f.ver, &f)?;
            (t.clone(), Some(t))
        };
        let contains = rest.is_empty() || base.join(rest).is_dir();
        if !contains {
            if let Some(t) = tmp {
                modcache::remove_tree(&t);
            }
            probed.insert(m.clone(), None);
            continue;
        }
        if let Some(t) = tmp {
            commit_tmp(&t, &slot)?;
        } else if !slot_matches(&slot, &f)? {
            return Err(format!(
                "cached {m}@{} does not match the fetched archive — remove the slot and get again",
                f.ver
            ));
        }
        let old = pin_require(mtext, sums, &m, &f, &slot)?;
        match old {
            Some(o) if o == f.ver => {}
            Some(o) => report.push(format!("upgraded {m}@{o} => {}", f.ver)),
            None => report.push(format!("added {m}@{}", f.ver)),
        }
        return Ok(true);
    }
    Err(format!(
        "no required module provides package \"{imp}\"; to add it: oxl get {imp}"
    ))
}

/// render one require entry: `<indent>[require ]path ver` plus the
/// managed `// indirect` marker and any preserved user comment
fn render_require(
    indent: &str,
    kw: bool,
    path: &str,
    ver: &str,
    indirect: bool,
    comment: &str,
) -> String {
    let mut l = format!("{}{}{path} {ver}", indent, if kw { "require " } else { "" });
    if indirect {
        l.push_str(" // indirect");
    }
    if !comment.is_empty() {
        if !indirect {
            l.push_str(" //");
        }
        l.push(' ');
        l.push_str(comment);
    }
    l
}

/// the trailing comment of a manifest line, excluding a managed
/// `// indirect` marker — user text is preserved verbatim
fn user_comment(raw: &str) -> String {
    let code = strip_comment(raw);
    let c = raw[code.len()..].trim();
    let c = c.strip_prefix("// indirect").unwrap_or(c);
    c.trim_start_matches('/').trim().to_string()
}

/// surgical require rewrite: the full require set is re-emitted
/// sorted at the first require directive (canonical block when >1
/// entry or a block already existed, single line otherwise), dead
/// entries drop, user comments travel with their entries, and every
/// non-require line survives byte-for-byte
fn rewrite_requires(mtext: &str, want: &std::collections::BTreeMap<String, (String, bool)>) -> String {
    let lines: Vec<&str> = mtext.lines().collect();
    let mut consumed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut first_req: Option<usize> = None;
    let mut had_block = false;
    let mut comments: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // standalone comment lines inside require regions — re-emitted
    // above the rewritten set so no user text is lost
    let mut hoisted: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = strip_comment(lines[i]).trim().to_string();
        let toks: Vec<&str> = t.split_whitespace().collect();
        if toks.len() == 2 && toks[0] == "require" && toks[1] == "(" {
            had_block = true;
            if first_req.is_none() {
                first_req = Some(i);
            }
            consumed.insert(i);
            i += 1;
            while i < lines.len() {
                consumed.insert(i);
                let t2 = strip_comment(lines[i]).trim().to_string();
                if t2 == ")" {
                    i += 1;
                    break;
                }
                let b: Vec<&str> = t2.split_whitespace().collect();
                if b.len() >= 2 && semver::parse(b[1]).is_some() {
                    comments.insert(b[0].to_string(), user_comment(lines[i]));
                } else if !lines[i].trim().is_empty() {
                    // non-entry content inside the block (a standalone
                    // comment) — re-emitted above the rewritten set
                    hoisted.push(lines[i].to_string());
                }
                i += 1;
            }
        } else if toks.len() >= 2 && toks[0] == "require" {
            if first_req.is_none() {
                first_req = Some(i);
            }
            consumed.insert(i);
            if toks.len() >= 3 {
                comments.insert(toks[1].to_string(), user_comment(lines[i]));
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    let old_comment = |path: &str| comments.get(path).cloned().unwrap_or_default();
    let all: Vec<(&String, &(String, bool))> = want.iter().collect();
    let mut out = String::with_capacity(mtext.len() + 64);
    let mut emitted = false;
    for (idx, line) in lines.iter().enumerate() {
        if Some(idx) == first_req && !emitted {
            emitted = true;
            for c in &hoisted {
                out.push_str(c);
                out.push('\n');
            }
            if all.len() == 1 && !had_block {
                let (p, (v, ind)) = all[0];
                out.push_str(&render_require("", true, p, v, *ind, &old_comment(p)));
                out.push('\n');
            } else if !all.is_empty() {
                out.push_str("require (\n");
                for (p, (v, ind)) in &all {
                    out.push_str(&render_require("\t", false, p, v, *ind, &old_comment(p)));
                    out.push('\n');
                }
                out.push_str(")\n");
            }
            continue;
        }
        if consumed.contains(&idx) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !emitted && !all.is_empty() {
        if all.len() == 1 {
            let (p, (v, ind)) = all[0];
            out.push_str(&render_require("", true, p, v, *ind, &old_comment(p)));
            out.push('\n');
        } else {
            out.push_str("require (\n");
            for (p, (v, ind)) in &all {
                out.push_str(&render_require("\t", false, p, v, *ind, &old_comment(p)));
                out.push('\n');
            }
            out.push_str(")\n");
        }
    }
    out
}

/// `oxl mod tidy` — sync oxl.mod/oxl.sum with what the source imports:
/// walk the import graph, fetch+require providers for missing
/// imports, drop requires nothing imports, rewrite both files
/// surgically (comments and non-require lines survive)
fn mod_tidy(args: &[String], cwd: &Path) {
    if !args.is_empty() {
        eprintln!("usage: oxl mod tidy");
        exit(2);
    }
    let Some(root) = find_root(cwd) else {
        eprintln!("error: no oxl.mod in current directory or any parent");
        exit(1);
    };
    let mpath = root.join("oxl.mod");
    let orig_mtext = std::fs::read_to_string(&mpath).unwrap_or_else(|e| {
        eprintln!("error: {}: {e}", mpath.display());
        exit(1);
    });
    let mut mtext = orig_mtext.clone();
    let spath = root.join("oxl.sum");
    // the write comparisons go against on-disk state — tidy mutates
    // mtext/sums in memory, so they can't be the baseline
    let orig_stext = std::fs::read_to_string(&spath).unwrap_or_default();
    let mut sums = match modcache::parse_sums(&orig_stext) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    };
    let main_files = main_mod_files(&root);
    let mut probed: std::collections::HashMap<String, Option<std::rc::Rc<fetch::Fetched>>> =
        std::collections::HashMap::new();
    let mut report: Vec<String> = Vec::new();
    // fixpoint: fetch/require changes alter resolution, so re-walk
    // until a pass leaves nothing unresolved
    let (ctx, needed) = loop {
        let ctx = match mk_ctx(&root, &mtext, sums.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error [check] {e}");
                exit(1);
            }
        };
        let (needed, unresolved) = tidy_walk(&ctx, &main_files);
        if unresolved.is_empty() {
            break (ctx, needed);
        }
        let mut changed = false;
        for (imp, dep_root) in &unresolved {
            match tidy_satisfy(
                imp,
                dep_root.as_deref(),
                &ctx,
                &mut mtext,
                &mut sums,
                &mut probed,
                &mut report,
            ) {
                Ok(c) => changed |= c,
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(1);
                }
            }
        }
        if !changed {
            let (imp, _) = &unresolved[0];
            eprintln!("error: no required module provides package \"{imp}\"; to add it: oxl get {imp}");
            exit(1);
        }
    };
    // final require set: direct providers keep their floor, indirect
    // ones record the MVS-selected version with the marker
    let mut want: std::collections::BTreeMap<String, (String, bool)> =
        std::collections::BTreeMap::new();
    for (mp, direct) in &needed {
        let ver = if *direct {
            ctx.requires
                .iter()
                .find(|(p, _)| p == mp)
                .map(|(_, v)| v.clone())
                .or_else(|| selected_ver(&ctx, mp).map(str::to_string))
                .unwrap_or_else(|| "v0.0.0-00010101000000-000000000000".into())
        } else {
            selected_ver(&ctx, mp)
                .map(str::to_string)
                .unwrap_or_else(|| "v0.0.0-00010101000000-000000000000".into())
        };
        want.insert(mp.clone(), (ver, !direct));
    }
    for p in ctx.requires.iter().map(|(p, _)| p).filter(|p| !needed.contains_key(*p)) {
        report.push(format!("dropped {p}"));
    }
    let new_text = rewrite_requires(&mtext, &want);
    // the sums the new require set needs: every non-replaced require's
    // selected version gets its h1 pair — kept lines carry over,
    // missing ones hash from the cache slot
    let final_ctx = match mk_ctx(&root, &new_text, sums.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error [check] {e}");
            exit(1);
        }
    };
    let mut new_sums: Vec<modcache::SumLine> = Vec::new();
    for (mp, _) in &final_ctx.requires {
        let Some(mv) = selected_ver(&final_ctx, mp).map(str::to_string) else {
            continue;
        };
        if replace_covers(&final_ctx, mp, &mv) {
            continue;
        }
        for is_mod in [false, true] {
            if let Some(h) = modcache::sum_for(&sums, mp, &mv, is_mod) {
                new_sums.push(modcache::SumLine {
                    path: mp.clone(),
                    ver: mv.clone(),
                    is_mod,
                    h1: h.to_string(),
                });
                continue;
            }
            let slot = modcache::slot(&ctx.modcache, mp, &mv);
            let h = if is_mod {
                match std::fs::read(slot.join("oxl.mod")) {
                    Ok(b) => modcache::mod_h1(mp, &mv, &b),
                    Err(e) => {
                        eprintln!("error: {}: {e}", slot.join("oxl.mod").display());
                        exit(1);
                    }
                }
            } else {
                match modcache::dir_h1(&slot) {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!("error: {e}");
                        exit(1);
                    }
                }
            };
            new_sums.push(modcache::SumLine {
                path: mp.clone(),
                ver: mv.clone(),
                is_mod,
                h1: h,
            });
        }
    }
    if new_text != orig_mtext {
        if let Err(e) = std::fs::write(&mpath, &new_text) {
            eprintln!("error: {}: {e}", mpath.display());
            exit(1);
        }
    }
    let new_sum_text = modcache::sums_text(&new_sums);
    if new_sum_text != orig_stext {
        if let Err(e) = std::fs::write(&spath, &new_sum_text) {
            eprintln!("error: {}: {e}", spath.display());
            exit(1);
        }
    }
    for r in &report {
        println!("{r}");
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
