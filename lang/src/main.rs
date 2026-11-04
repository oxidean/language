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
        eprintln!("lang — one toolchain binary\n\n  lang build <file.lang> [-o out]\n  lang run <file.lang>\n  lang test <file.lang>\n  lang fmt <file.lang>\n  lang doc <file.lang>\n  lang lsp");
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
    if !cand.ends_with(".lang") {
        cand.push_str(".lang");
    }
    let rel = from_file.parent().unwrap_or(Path::new(".")).join(&cand);
    if rel.exists() {
        return Some(rel);
    }
    // std/x -> <ancestor>/stdlib/x.lang walking up from the importing file
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

fn load_crate(path: &Path, seen: &mut std::collections::HashSet<PathBuf>) -> Option<ast::Crate> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !seen.insert(canon) {
        return Some(ast::Crate { items: Vec::new() });
    }
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return None;
        }
    };
    let mut diags = diag::Diags::new(path.to_string_lossy().to_string(), src.clone());
    let toks = lexer::lex(&src, &mut diags);
    let krate = parser::parse(&toks, &mut diags);
    diags.report();
    let mut items = Vec::new();
    for it in krate.items {
        if let ast::Item::Import(imp, _) = it {
            match resolve_import(path, &imp) {
                Some(p) => {
                    if let Some(sub) = load_crate(&p, seen) {
                        items.extend(sub.items);
                    }
                }
                None => eprintln!("{}: cannot resolve import `{imp}`", path.display()),
            }
        } else {
            items.push(it);
        }
    }
    Some(ast::Crate { items })
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
    check::check(&krate, &mut diags);
    diags.report();
    if diags.has_errors() {
        return Err(());
    }
    Ok(codegen::generate_mode(&krate, test_mode))
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
            let dir = std::env::temp_dir().join(format!("lang-{}", std::process::id()));
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
            let dir = std::env::temp_dir().join(format!("lang-test-{}", std::process::id()));
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
    seen.insert(path.to_path_buf());
    let mut items = Vec::new();
    for it in krate.items {
        match it {
            ast::Item::Import(imp, _) => {
                if let Some(p) = resolve_import(path, &imp) {
                    if let Some(sub) = load_crate_quiet(&p, &mut seen) {
                        items.extend(sub.items);
                    }
                }
            }
            other => items.push(other),
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

/// load_crate variant that collects parse diags silently (LSP only wants
/// diagnostics for the open buffer, not imported files)
fn load_crate_quiet(path: &Path, seen: &mut std::collections::HashSet<PathBuf>) -> Option<ast::Crate> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !seen.insert(canon) {
        return Some(ast::Crate { items: Vec::new() });
    }
    let src = std::fs::read_to_string(path).ok()?;
    let mut diags = diag::Diags::new(path.to_string_lossy(), src.clone());
    let toks = lexer::lex(&src, &mut diags);
    let krate = parser::parse(&toks, &mut diags);
    let mut items = Vec::new();
    for it in krate.items {
        if let ast::Item::Import(imp, _) = it {
            if let Some(p) = resolve_import(path, &imp) {
                if let Some(sub) = load_crate_quiet(&p, seen) {
                    items.extend(sub.items);
                }
            }
        } else {
            items.push(it);
        }
    }
    Some(ast::Crate { items })
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
