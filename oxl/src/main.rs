mod ast;
mod check;
mod codegen;
mod diag;
mod lexer;
mod parser;

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

fn main() {
    let args: Vec<String> = std::env::args().collect();
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

/// canonicalize `a.b(...)` calls to `Ident("a.b")` then map through the
/// module's alias/rename tables; also maps type names and bare decl idents.
fn qualify_expr(
    e: &mut ast::Expr,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
) {
    match &mut e.kind {
        ast::ExprKind::Call { f, args } => {
            if let Some(n) = ast::callee_name(f) {
                let mapped = map_name(&n, renames, aliases);
                f.kind = ast::ExprKind::Ident(mapped);
            } else {
                qualify_expr(f, renames, aliases);
            }
            for a in args {
                qualify_expr(a, renames, aliases);
            }
        }
        ast::ExprKind::Ident(n) => *n = map_name(n, renames, aliases),
        ast::ExprKind::StructLit { name, fields } => {
            *name = map_name(name, renames, aliases);
            for (_, v) in fields {
                qualify_expr(v, renames, aliases);
            }
        }
        ast::ExprKind::Field { obj, .. } => qualify_expr(obj, renames, aliases),
        ast::ExprKind::Bin { l, r, .. } => {
            qualify_expr(l, renames, aliases);
            qualify_expr(r, renames, aliases);
        }
        ast::ExprKind::Un { e, .. } => qualify_expr(e, renames, aliases),
        ast::ExprKind::As { e, ty } => {
            qualify_expr(e, renames, aliases);
            qualify_ty(ty, renames, aliases);
        }
        ast::ExprKind::Match { e, arms } => {
            qualify_expr(e, renames, aliases);
            for a in arms {
                match &mut a.pat {
                    ast::Pattern::VarUnit { path }
                    | ast::Pattern::VarPos { path, .. }
                    | ast::Pattern::VarNamed { path, .. } => {
                        *path = map_name(path, renames, aliases);
                    }
                    ast::Pattern::Lit(le) => qualify_expr(le, renames, aliases),
                    _ => {}
                }
                match &mut a.body {
                    ast::MatchBody::Expr(be) => qualify_expr(be, renames, aliases),
                    ast::MatchBody::Block(b) => qualify_block(b, renames, aliases),
                }
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
    }
}

fn qualify_block(
    b: &mut ast::Block,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
) {
    for s in &mut b.stmts {
        match s {
            ast::Stmt::Let { ty, init, .. } => {
                if let Some(t) = ty {
                    qualify_ty(t, renames, aliases);
                }
                qualify_expr(init, renames, aliases);
            }
            ast::Stmt::Let2 { call, .. } => qualify_expr(call, renames, aliases),
            ast::Stmt::Assign { target, val, .. } => {
                qualify_expr(target, renames, aliases);
                qualify_expr(val, renames, aliases);
            }
            ast::Stmt::Expr(x) => qualify_expr(x, renames, aliases),
            ast::Stmt::Return { vals, .. } => {
                for v in vals {
                    qualify_expr(v, renames, aliases);
                }
            }
            ast::Stmt::If { cond, then, els, .. } => {
                qualify_expr(cond, renames, aliases);
                qualify_block(then, renames, aliases);
                if let Some(e) = els {
                    qualify_block(e, renames, aliases);
                }
            }
            ast::Stmt::For { cond, body, .. } => {
                qualify_expr(cond, renames, aliases);
                qualify_block(body, renames, aliases);
            }
            ast::Stmt::Scope { body, .. } | ast::Stmt::Unsafe { body, .. } => {
                qualify_block(body, renames, aliases)
            }
            ast::Stmt::Spawn { call, .. } => qualify_expr(call, renames, aliases),
            ast::Stmt::Defer { call, .. } => qualify_expr(call, renames, aliases),
            ast::Stmt::Select { arms, .. } => {
                for a in arms {
                    match &mut a.kind {
                        ast::SelKind::Recv { chan, .. } => {
                            qualify_expr(chan, renames, aliases)
                        }
                        ast::SelKind::Timeout { ms, .. } => {
                            qualify_expr(ms, renames, aliases)
                        }
                        ast::SelKind::Else { .. } => {}
                    }
                    qualify_block(&mut a.body, renames, aliases);
                }
            }
            ast::Stmt::Break { .. } | ast::Stmt::Continue { .. } => {}
        }
    }
}

fn load_crate(path: &Path, seen: &mut std::collections::HashSet<PathBuf>) -> Option<ast::Crate> {
    load_mod(path, "", seen, true)
}

/// parse a file, recursively qualify its imports under their module stems,
/// then qualify this file's own decls as `mod.name` (root: modname "").
fn load_mod(
    path: &Path,
    modname: &str,
    seen: &mut std::collections::HashSet<PathBuf>,
    loud: bool,
) -> Option<ast::Crate> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !seen.insert(canon.clone()) {
        return Some(ast::Crate { items: Vec::new() });
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
                    if let Some(sub) = load_mod(&p, &stem, seen, loud) {
                        items.extend(sub.items);
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
    for it in krate.items {
        match it {
            ast::Item::Import(_) => {}
            mut it => {
                qualify_item(&mut it, &renames, &aliases);
                items.push(it);
            }
        }
    }
    Some(ast::Crate { items })
}

/// apply the module's rename/alias maps to one non-import item in place
fn qualify_item(
    it: &mut ast::Item,
    renames: &std::collections::HashMap<String, String>,
    aliases: &std::collections::HashMap<String, String>,
) {
    match it {
        ast::Item::Fn(f) => {
            if let Some(n) = renames.get(&f.name) {
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
                qualify_block(b, renames, aliases);
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
        }
        ast::Item::Global(g) => {
            if let Some(n) = renames.get(&g.name) {
                g.name = n.clone();
            }
            if let Some(t) = &mut g.ty {
                qualify_ty(t, renames, aliases);
            }
            qualify_expr(&mut g.init, renames, aliases);
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
    let krate = match load_crate(Path::new(src_file), &mut seen) {
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
    let checked = check::check(&krate, &mut diags);
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
    let status = Command::new("cc")
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
fn fmt_source(src: &str) -> String {
    let mut out = String::new();
    let mut depth: i32 = 0;
    let mut blank = 0;
    for raw in src.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.is_empty() {
            blank += 1;
            if blank <= 1 && !out.is_empty() {
                out.push('\n');
            }
            continue;
        }
        blank = 0;
        // closing-brace-led lines dedent first
        let mut d = depth;
        for c in t.chars() {
            match c {
                '}' | ')' | ']' => d -= 1,
                '{' | '(' | '[' => break,
                _ => break,
            }
            if c != '}' && c != ')' && c != ']' {
                break;
            }
        }
        let d = d.max(0);
        for _ in 0..d {
            out.push_str("    ");
        }
        out.push_str(t);
        out.push('\n');
        // count braces outside string literals
        let mut in_str = false;
        let mut esc = false;
        for c in t.chars() {
            if in_str {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if depth < 0 {
            depth = 0;
        }
    }
    out
}

fn cmd_fmt(args: &[String]) {
    let file = &args[2];
    let write = args.iter().any(|a| a == "-w");
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{file}: {e}");
            exit(1);
        }
    };
    let out = fmt_source(&src);
    if write {
        std::fs::write(file, out).unwrap();
    } else {
        print!("{out}");
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
fn analyze_source(path: &Path, src: &str) -> Vec<(usize, usize, String)> {
    let mut diags = diag::Diags::new(path.to_string_lossy(), src);
    let toks = lexer::lex(src, &mut diags);
    let krate = parser::parse(&toks, &mut diags);
    let mut seen = std::collections::HashSet::new();
    seen.insert(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
    let mut renames: std::collections::HashMap<String, String> = Default::default();
    let mut aliases: std::collections::HashMap<String, String> = Default::default();
    let mut items = Vec::new();
    for it in &krate.items {
        if let ast::Item::Import(imp) = it {
            if let Some(p) = resolve_import(path, &imp.path) {
                let stem = mod_stem(&p);
                aliases.insert(imp.alias.clone().unwrap_or_else(|| stem.clone()), stem.clone());
                if imp.alias.is_none() {
                    aliases.insert(stem.clone(), stem.clone());
                }
                for s in &imp.selective {
                    renames.insert(s.clone(), format!("{stem}.{s}"));
                }
                if let Some(sub) = load_mod(&p, &stem, &mut seen, false) {
                    items.extend(sub.items);
                }
            }
        }
    }
    for mut it in krate.items {
        if !matches!(it, ast::Item::Import(_)) {
            qualify_item(&mut it, &renames, &aliases);
            items.push(it);
        }
    }
    check::check(&ast::Crate { items }, &mut diags);
    diags
        .list
        .iter()
        .map(|d| {
            let (l, c) = diags.line_col(d.span.lo);
            (l, c, format!("[{}] {}", d.code, d.msg))
        })
        .collect()
}

/// naive JSON string-field extraction (v0 — no serde dep)
fn json_str<'a>(body: &'a str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let i = body.find(&pat)? + pat.len();
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

fn publish_diags(stdout: &mut impl Write, uri: &str, src: &str) {
    let path = Path::new(uri.strip_prefix("file://").unwrap_or(uri));
    let list = analyze_source(path, src);
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

// --- stdio LSP: diagnostics on open/change/save ---
fn cmd_lsp() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut r = std::io::BufReader::new(stdin.lock());
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
                publish_diags(&mut stdout, &uri, &text);
            }
            continue;
        }
        let resp = if body.contains("\"initialize\"") {
            let id = extract_id(&body);
            format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"capabilities\":{{\"textDocumentSync\":1}}}}}}")
        } else if body.contains("\"shutdown\"") {
            let id = extract_id(&body);
            format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":null}}")
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
