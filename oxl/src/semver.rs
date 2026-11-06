//! Go-flavored semantic versions: `vMAJOR.MINOR.PATCH[-pre][+build]`.
//! Pseudo-versions ride the same grammar (`v1.2.4-0.yyyymmddhhmmss-<sha>`
//! sorts after v1.2.3, before v1.2.4 — Go's rule falls out of ordinary
//! prerelease precedence, no special casing needed).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ver {
    pub maj: u64,
    pub min: u64,
    pub pat: u64,
    /// dot-separated prerelease idents, empty = release
    pub pre: Vec<String>,
    /// build metadata — kept for display, ignored in ordering
    pub build: String,
}

fn ident_ok(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn num_field(s: &str) -> Option<u64> {
    // numeric fields: digits only, no leading zeros (semver spec; Go
    // rejects `v01.2.3` too)
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None;
    }
    s.parse().ok()
}

fn pre_idents(s: &str) -> Option<Vec<String>> {
    let v: Vec<String> = s.split('.').map(String::from).collect();
    if v.iter().all(|i| ident_ok(i) && !(i.bytes().all(|b| b.is_ascii_digit()) && i.len() > 1 && i.starts_with('0'))) {
        Some(v)
    } else {
        None
    }
}

/// parse `vX.Y.Z[-pre][+build]`; leading `v` required, full triplet
/// required — the require/replace version shape (Go's go.mod rule)
pub fn parse(s: &str) -> Option<Ver> {
    let s = s.strip_prefix('v')?;
    // an empty post-separator piece is malformed, not absent:
    // `v1.2.3-` / `v1.2.3+` must not parse
    let (core, build) = match s.split_once('+') {
        Some((c, b)) => (c, Some(b)),
        None => (s, None),
    };
    let (core, pre) = match core.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (core, None),
    };
    let mut it = core.split('.');
    let maj = num_field(it.next()?)?;
    let min = num_field(it.next()?)?;
    let pat = num_field(it.next()?)?;
    if it.next().is_some() {
        return None;
    }
    let pre = match pre {
        None => Vec::new(),
        Some(p) => pre_idents(p)?,
    };
    let build = match build {
        None => String::new(),
        Some(b) if b.split('.').all(ident_ok) => b.to_string(),
        Some(_) => return None,
    };
    Some(Ver { maj, min, pat, pre, build })
}

/// prerelease precedence: no pre > any pre; per-ident numeric<alpha,
/// numeric numerically, alpha lexically; prefix sorts shorter
fn cmp_pre(a: &[String], b: &[String]) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Equal,
        (true, false) => return Greater,
        (false, true) => return Less,
        _ => {}
    }
    for (x, y) in a.iter().zip(b.iter()) {
        let xn = x.bytes().all(|b| b.is_ascii_digit());
        let yn = y.bytes().all(|b| b.is_ascii_digit());
        let o = match (xn, yn) {
            (true, true) => {
                let (xv, yv) = (x.parse::<u64>().unwrap_or(u64::MAX), y.parse::<u64>().unwrap_or(u64::MAX));
                xv.cmp(&yv).then(x.len().cmp(&y.len()))
            }
            (true, false) => Less,
            (false, true) => Greater,
            (false, false) => x.cmp(y),
        };
        if o != Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

pub fn compare(a: &Ver, b: &Ver) -> std::cmp::Ordering {
    (a.maj, a.min, a.pat)
        .cmp(&(b.maj, b.min, b.pat))
        .then_with(|| cmp_pre(&a.pre, &b.pre))
}

/// trailing `/vN` module-path suffix, N >= 2 (semantic import versioning)
pub fn path_major(path: &str) -> Option<u64> {
    let last = path.rsplit('/').next()?;
    let n = last.strip_prefix('v')?;
    let v = num_field(n)?;
    (v >= 2).then_some(v)
}

/// Go's require rule: the version's major must match the path's /vN
/// suffix (v0/v1 paths carry no suffix). Err echoes the raw version
/// token like Go's own diagnostics.
pub fn require_ok(path: &str, raw: &str, ver: &Ver) -> Result<(), String> {
    match (path_major(path), ver.maj) {
        (Some(n), m) if n == m => Ok(()),
        (Some(n), m) => Err(format!(
            "version \"{raw}\" invalid: should be v{n}, not v{m}"
        )),
        (None, m) if m <= 1 => Ok(()),
        (None, m) => Err(format!(
            "version \"{raw}\" invalid: should be v0 or v1, not v{m}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    fn v(s: &str) -> Ver {
        parse(s).unwrap_or_else(|| panic!("bad ver {s}"))
    }

    #[test]
    fn parse_grammar() {
        for ok in [
            "v0.0.0", "v1.2.3", "v1.2.3-alpha", "v1.2.3-alpha.1",
            "v1.2.3-0.20240101000000-deadbeef1234", "v2.0.0-rc.1+build.5",
            "v10.20.30",
        ] {
            assert!(parse(ok).is_some(), "{ok}");
        }
        for bad in [
            "1.2.3", "v1.2", "v1.2.3.4", "v01.2.3", "v1.2.3-", "v1.2.3+",
            "v1.2.3-a..b", "v1.2.3-01", "", "vx.y.z",
        ] {
            assert!(parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn ordering() {
        let cases: &[(&str, &str, std::cmp::Ordering)] = &[
            ("v1.2.3", "v1.2.3", Equal),
            ("v1.2.3", "v1.2.4", Less),
            ("v1.10.0", "v1.9.9", Greater),
            ("v1.2.3-alpha", "v1.2.3", Less),
            ("v1.2.3-alpha", "v1.2.3-beta", Less),
            ("v1.2.3-alpha.1", "v1.2.3-alpha", Greater),
            ("v1.2.3-1", "v1.2.3-a", Less),
        ];
        for (a, b, want) in cases {
            assert_eq!(compare(&v(a), &v(b)), *want, "{a} vs {b}");
        }
    }

    #[test]
    fn pseudo_version_ordering() {
        // after base release, before next patch — Go's pseudo rule
        let base = v("v1.2.3");
        let pseudo = v("v1.2.4-0.20240101000000-deadbeef1234");
        let next = v("v1.2.4");
        assert_eq!(compare(&base, &pseudo), Less);
        assert_eq!(compare(&pseudo, &next), Less);
        // no-base form
        assert_eq!(compare(&v("v0.0.0-20240101000000-deadbeef1234"), &v("v0.0.1")), Less);
        // prerelease-base form: sorts within the prerelease
        let pre = v("v1.2.3-rc.1");
        let pseu_pre = v("v1.2.3-rc.1.0.20240101000000-deadbeef1234");
        assert_eq!(compare(&pre, &pseu_pre), Less);
        assert_eq!(compare(&pseu_pre, &v("v1.2.3")), Less);
    }

    #[test]
    fn major_suffix_rule() {
        assert_eq!(path_major("github.com/x/y/v2"), Some(2));
        assert_eq!(path_major("github.com/x/v10"), Some(10));
        assert_eq!(path_major("github.com/x/y"), None);
        assert_eq!(path_major("github.com/x/v1"), None); // v1 carries no suffix
        assert_eq!(path_major("github.com/x/v02"), None); // no leading zeros
        assert!(require_ok("github.com/x/y", "v1.9.9", &v("v1.9.9")).is_ok());
        assert!(require_ok("github.com/x/y/v2", "v2.1.0", &v("v2.1.0")).is_ok());
        assert!(require_ok("github.com/x/y/v2", "v2.0.0-0.2024-x", &v("v2.0.0-0.2024-x")).is_ok());
        assert!(require_ok("github.com/x/y", "v2.0.0", &v("v2.0.0")).is_err());
        assert!(require_ok("github.com/x/y/v2", "v3.0.0", &v("v3.0.0")).is_err());
        assert!(require_ok("github.com/x/y/v2", "v1.0.0", &v("v1.0.0")).is_err());
    }
}
