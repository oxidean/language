//! V2: the `oxl get` fetch path — a thin git veneer (clone --bare into
//! a temp dir, rev-parse/describe/archive in it), semver query
//! resolution, and pseudo-version derivation. `git` is a documented
//! toolchain dependency for `get` only; private repos work through
//! git's own auth (ssh agent, credential helpers).

use crate::semver;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("TZ", "UTC")
        .env("GIT_TERMINAL_PROMPT", "0") // never block on a tty — auth failures must error
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("git {}: {err}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// module path -> repo URL: an explicit scheme or scp-style `git@`
/// passes through; a bare host/path gets https://
pub fn repo_url(path: &str) -> String {
    if path.contains("://") || path.starts_with("git@") {
        path.to_string()
    } else {
        format!("https://{path}")
    }
}

pub struct Fetched {
    pub ver: String,
    pub tar: Vec<u8>,
    pub manifest: Vec<u8>,
}

/// resolve `query` against `url` and return the selected version plus
/// the canonical `git archive` of its tree. One bare clone serves both.
pub fn fetch(url: &str, path: &str, query: &str) -> Result<Fetched, String> {
    let tmp = std::env::temp_dir().join(format!("oxl-get-{}-{}", std::process::id(), sanitize(path)));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let res = (|| {
        Command::new("git")
            .args(["clone", "--bare", "--quiet", url])
            .arg(&tmp)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| format!("git clone {url}: {e}"))
            .and_then(|o| {
                if o.status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "git clone {url}: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    ))
                }
            })?;
        let (sha, ver) = resolve(&tmp, query)?;
        let out = Command::new("git")
            .args(["archive", "--format=tar", &sha])
            .current_dir(&tmp)
            .output()
            .map_err(|e| format!("git archive {sha}: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git archive {sha}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        // the manifest bytes come from the archived tree, not a
        // checkout — same bytes the sum will verify later
        let manifest = Command::new("git")
            .args(["show", &format!("{sha}:oxl.mod")])
            .current_dir(&tmp)
            .output()
            .map_err(|e| e.to_string())?;
        let manifest = if manifest.status.success() {
            manifest.stdout
        } else {
            return Err(format!("{path}@{ver}: no oxl.mod at repo root"));
        };
        Ok(Fetched { ver, tar: out.stdout, manifest })
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// every tag pointing at commits, as semver — `vX.Y.Z` names only
fn tags(repo: &Path) -> Result<Vec<(semver::Ver, String)>, String> {
    let out = git(repo, &["tag", "--list", "v*"])?;
    Ok(out
        .lines()
        .filter_map(|l| semver::parse(l.trim()).map(|v| (v, l.trim().to_string())))
        .collect())
}

fn rev_parse(repo: &Path, r: &str) -> Result<String, String> {
    git(repo, &["rev-parse", "--verify", &format!("{r}^{{commit}}")]).map(|s| s.trim().to_string())
}

/// Pseudo-version shape: the commit's nearest semver tag ancestor bumps
/// its patch and gains `-0.<yyyymmddhhmmss>-<sha12>`; no ancestor tag
/// at all starts from v0.0.0
fn pseudo(repo: &Path, sha: &str) -> Result<String, String> {
    let base = git(repo, &["describe", "--tags", "--abbrev=0", "--match", "v*", sha]).ok();
    let ts = git(repo, &["show", "-s", "--format=%cd", "--date=format:%Y%m%d%H%M%S", sha])?;
    let ts = ts.trim();
    let short = &sha[..12.min(sha.len())];
    let v = base
        .and_then(|b| semver::parse(b.trim()))
        .map(|mut v| {
            v.pat += 1;
            v
        })
        .unwrap_or(semver::Ver { maj: 0, min: 0, pat: 0, pre: vec![], build: String::new() });
    Ok(format!("v{}.{}.{}-0.{ts}-{short}", v.maj, v.min, v.pat))
}

fn resolve(repo: &Path, query: &str) -> Result<(String, String), String> {
    match query {
        "latest" | "" => {
            let ts = tags(repo)?;
            match ts.iter().max_by(|a, b| semver::compare(&a.0, &b.0)) {
                Some((v, tag)) => Ok((rev_parse(repo, tag)?, fmt_ver(v))),
                None => {
                    let sha = rev_parse(repo, "HEAD")?;
                    Ok((sha.clone(), pseudo(repo, &sha)?))
                }
            }
        }
        q if q.starts_with('v') && semver::parse(q).is_some() => {
            let sha = rev_parse(repo, &format!("refs/tags/{q}"))
                .map_err(|_| format!("unknown revision {q}"))?;
            Ok((sha, q.to_string()))
        }
        q => {
            // branch name first, then a raw commit-ish
            if let Ok(sha) = rev_parse(repo, &format!("refs/heads/{q}")) {
                let v = pseudo(repo, &sha)?;
                return Ok((sha, v));
            }
            let sha = rev_parse(repo, q).map_err(|_| format!("unknown revision {q}"))?;
            let v = pseudo(repo, &sha)?;
            Ok((sha, v))
        }
    }
}

fn fmt_ver(v: &semver::Ver) -> String {
    let mut s = format!("v{}.{}.{}", v.maj, v.min, v.pat);
    if !v.pre.is_empty() {
        s.push('-');
        s.push_str(&v.pre.join("."));
    }
    if !v.build.is_empty() {
        s.push('+');
        s.push_str(&v.build);
    }
    s
}
