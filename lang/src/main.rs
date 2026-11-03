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
        "fmt" => {
            eprintln!("lang fmt: not yet implemented (phase 2)");
            exit(0);
        }
        "doc" => {
            eprintln!("lang doc: not yet implemented (phase 2)");
            exit(0);
        }
        "lsp" => cmd_lsp(),
        other => {
            eprintln!("unknown command `{other}`");
            exit(2);
        }
    }
}

fn compile_mode(src_file: &str, test_mode: bool) -> Result<String, ()> {
    let src = match std::fs::read_to_string(src_file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{src_file}: {e}");
            return Err(());
        }
    };
    let mut diags = diag::Diags::new(src_file, src.clone());
    let toks = lexer::lex(&src, &mut diags);
    let krate = parser::parse(&toks, &mut diags);
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

// --- minimal stdio LSP scaffold (phase 2 builds the real server) ---
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
