//! V2: module cache layout, content hashing, `oxl.sum` integrity and
//! safe archive extraction. Mirrors the established module-fetch +
//! dirhash design on a
//! smaller grammar — no crates, everything self-contained so the
//! self-hosted checker can port the same procedures.

use std::path::{Path, PathBuf};

/// Path escaping: every uppercase letter becomes `!` +
/// lowercase, so case-insensitive filesystems can't collide slots.
/// Applied to module paths and versions alike.
pub fn escape_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_uppercase() {
            out.push('!');
            out.push(b.to_ascii_lowercase() as char);
        } else {
            out.push(b as char);
        }
    }
    out
}

/// module cache root: OXLMODCACHE wins, else the platform default —
/// %LOCALAPPDATA%\oxl\mod on Windows, ~/.oxl/mod elsewhere.
pub fn cache_dir() -> PathBuf {
    if let Ok(v) = std::env::var("OXLMODCACHE") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    #[cfg(windows)]
    if let Ok(v) = std::env::var("LOCALAPPDATA") {
        if !v.is_empty() {
            return PathBuf::from(v).join("oxl").join("mod");
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

/// the cache slot a (module, version) occupies
pub fn slot(cache: &Path, path: &str, ver: &str) -> PathBuf {
    cache.join(format!("{}@{}", escape_path(path), escape_path(ver)))
}

// ---------- sha256 (FIPS 180-4; no deps so check.oxl can port it) --

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bitlen = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e;
            e = d.wrapping_add(t1);
            d = c; c = b; b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn b64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = c.iter().fold(0u32, |a, &x| (a << 8) | x as u32) << (8 * (3 - c.len()));
        for i in 0..4 {
            let shift = 18 - 6 * i;
            out.push(if i <= c.len() { T[((n >> shift) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

// ---------- h1 dirhash (the dirhash.Hash1 shape) ------------------

/// hash1 over (relpath, content-hex) entries: sort by name, hash the
/// concatenated "<hex>  <name>\n" lines, base64 the digest.
fn hash1(entries: &mut Vec<(String, String)>) -> String {
    entries.sort();
    let mut buf = Vec::new();
    for (name, h) in entries.iter() {
        buf.extend_from_slice(h.as_bytes());
        buf.extend_from_slice(b"  ");
        buf.extend_from_slice(name.as_bytes());
        buf.push(b'\n');
    }
    format!("h1:{}", b64(&sha256(&buf)))
}

/// hash of a directory tree: every regular file, relpaths joined with
/// '/' (never string-replaced — a POSIX name may contain '\')
pub fn dir_h1(dir: &Path) -> Result<String, String> {
    let mut entries = Vec::new();
    walk(dir, dir, &mut entries)?;
    Ok(hash1(&mut entries))
}

fn walk(base: &Path, dir: &Path, entries: &mut Vec<(String, String)>) -> Result<(), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in rd {
        let e = e.map_err(|x| x.to_string())?;
        let p = e.path();
        let ft = e.file_type().map_err(|x| x.to_string())?;
        if ft.is_dir() {
            walk(base, &p, entries)?;
        } else if ft.is_file() {
            let rel = p
                .strip_prefix(base)
                .map_err(|_| "relpath".to_string())?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let bytes = std::fs::read(&p).map_err(|x| format!("{}: {x}", p.display()))?;
            entries.push((rel, hex(&sha256(&bytes))));
        }
    }
    Ok(())
}

/// hash of a fetched manifest, recorded as `<esc-path>@<ver>.mod` —
/// HashMod shape (single synthetic entry)
pub fn mod_h1(path: &str, ver: &str, bytes: &[u8]) -> String {
    let name = format!("{}@{}.mod", escape_path(path), escape_path(ver));
    hash1(&mut vec![(name, hex(&sha256(bytes)))])
}

// ---------- oxl.sum ------------------------------------------------

#[derive(Debug, Clone)]
pub struct SumLine {
    pub path: String,
    pub ver: String,
    pub is_mod: bool,
    pub h1: String,
}

/// parse oxl.sum text — `<path> <ver> h1:<b64>` and
/// `<path> <ver>/mod h1:<b64>` rows; malformed lines are an error,
/// never silently skipped (a corrupt sum file must not weaken to
/// "no verification")
pub fn parse_sums(text: &str) -> Result<Vec<SumLine>, String> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 3 || !f[2].starts_with("h1:") {
            return Err(format!("oxl.sum:{}: malformed line", i + 1));
        }
        let (ver, is_mod) = match f[1].strip_suffix("/mod") {
            Some(v) => (v.to_string(), true),
            None => (f[1].to_string(), false),
        };
        out.push(SumLine { path: f[0].to_string(), ver, is_mod, h1: f[2].to_string() });
    }
    Ok(out)
}

pub fn sum_for<'a>(sums: &'a [SumLine], path: &str, ver: &str, is_mod: bool) -> Option<&'a str> {
    sums.iter()
        .find(|s| s.path == path && s.ver == ver && s.is_mod == is_mod)
        .map(|s| s.h1.as_str())
}

/// serialize sorted + deduped (path, ver, mod-kind ordering)
pub fn sums_text(sums: &[SumLine]) -> String {
    let mut v: Vec<&SumLine> = sums.iter().collect();
    v.sort_by(|a, b| (&a.path, &a.ver, a.is_mod).cmp(&(&b.path, &b.ver, b.is_mod)));
    let mut out = String::new();
    for s in v {
        out.push_str(&format!("{} {}{} {}\n", s.path, s.ver, if s.is_mod { "/mod" } else { "" }, s.h1));
    }
    out
}

/// verify a cached slot against the sums: both the tree line and the
/// /mod line must exist and match — the two-entry rule.
pub fn verify_slot(dir: &Path, path: &str, ver: &str, sums: &[SumLine]) -> Result<(), String> {
    let tree = sum_for(sums, path, ver, false)
        .ok_or_else(|| format!("missing oxl.sum entry for module {path}"))?;
    let modsum = sum_for(sums, path, ver, true)
        .ok_or_else(|| format!("missing oxl.sum entry for oxl.mod file for module {path}"))?;
    if dir_h1(dir)? != tree {
        return Err(format!("verifying {path}@{ver}: checksum mismatch"));
    }
    let mpath = dir.join("oxl.mod");
    let mbytes = std::fs::read(&mpath).map_err(|e| format!("{}: {e}", mpath.display()))?;
    if mod_h1(path, ver, &mbytes) != modsum {
        return Err(format!("verifying {path}@{ver}: checksum mismatch"));
    }
    Ok(())
}

// ---------- filename safety (all platforms — the archive either
// extracts identically everywhere or fails everywhere) --------------

const DEVICES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// one path component must be safe on every filesystem oxl supports:
/// no separators/backslash, no Windows-forbidden chars or trailing
/// dot/space, no reserved device names (extension-stripped, any case)
pub fn check_component(c: &str) -> Result<(), String> {
    if c.is_empty() || c == "." || c == ".." {
        return Err(format!("unsafe path component {c:?}"));
    }
    if c.contains('\\') || c.contains('/') {
        return Err(format!("unsafe path component {c:?}"));
    }
    if c.ends_with('.') || c.ends_with(' ') {
        return Err(format!("unsafe path component {c:?} (trailing dot/space)"));
    }
    for ch in c.chars() {
        if ch < ' ' || "<>:\"|?*".contains(ch) {
            return Err(format!("unsafe path component {c:?} (illegal character)"));
        }
    }
    let stem = c.split('.').next().unwrap_or("").to_ascii_lowercase();
    if DEVICES.contains(&stem.as_str()) {
        return Err(format!("unsafe path component {c:?} (reserved device name)"));
    }
    Ok(())
}

// ---------- tar extraction -----------------------------------------

fn octal(f: &[u8]) -> u64 {
    let mut n = 0u64;
    for &b in f {
        match b {
            b'0'..=b'7' => n = (n << 3) | (b - b'0') as u64,
            _ => break,
        }
    }
    n
}

fn field(f: &[u8]) -> String {
    let end = f.iter().position(|&b| b == 0).unwrap_or(f.len());
    String::from_utf8_lossy(&f[..end]).into_owned()
}

/// extract a `git archive` tar into `dest`: ustar + pax `path=`
/// overrides, regular files and dirs only, every name validated and
/// case-collision checked, no absolute/parent escapes. Returns the
/// number of files written.
pub fn untar(bytes: &[u8], dest: &Path) -> Result<u64, String> {
    let mut nfiles = 0u64;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut pos = 0usize;
    let mut pax_path: Option<String> = None;
    while pos + 512 <= bytes.len() {
        let h = &bytes[pos..pos + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let size = octal(&h[124..136]) as usize;
        let typeflag = h[156];
        let mut name = field(&h[0..100]);
        let prefix = field(&h[345..500]);
        if !prefix.is_empty() {
            name = format!("{prefix}/{name}");
        }
        let mode = octal(&h[100..108]);
        pos += 512;
        let body_end = pos + size;
        if body_end > bytes.len() {
            return Err("truncated tar archive".into());
        }
        let body = &bytes[pos..body_end];
        pos = body_end.div_ceil(512) * 512;
        match typeflag {
            b'x' | b'g' => {
                // pax extended header — honor `path=` only
                let txt = String::from_utf8_lossy(body);
                for rec in txt.split('\n') {
                    if let Some((_, kv)) = rec.split_once(' ') {
                        if let Some(v) = kv.strip_prefix("path=") {
                            pax_path = Some(v.trim_end_matches('\0').to_string());
                        }
                    }
                }
                continue;
            }
            b'0' | 0 | b'5' => {}
            _ => continue, // links, devices, fifos — never materialize
        }
        if let Some(p) = pax_path.take() {
            name = p;
        }
        // normalize: strip leading ./ components; `git archive` adds
        // no prefix unless asked, and we ask for none
        let comps: Vec<&str> = name.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
        if comps.is_empty() {
            continue;
        }
        let mut rel = PathBuf::new();
        let mut folded = String::new();
        for (i, c) in comps.iter().enumerate() {
            check_component(c)?;
            if i > 0 {
                folded.push('/');
            }
            folded.push_str(&c.to_ascii_lowercase());
            rel.push(c);
        }
        if !seen.insert(folded) {
            return Err(format!("archive has case-colliding paths ({name})"));
        }
        let out = dest.join(&rel);
        if typeflag == b'5' || name.ends_with('/') {
            std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            continue;
        }
        if let Some(par) = out.parent() {
            std::fs::create_dir_all(par).map_err(|e| format!("{}: {e}", par.display()))?;
        }
        let mut f = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        use std::io::Write;
        f.write_all(body).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode as u32 & 0o777));
        }
        nfiles += 1;
    }
    Ok(nfiles)
}

/// strip write permission everywhere below `dir` — a cache slot is
/// immutable once sealed (corruption-by-edit is a real failure mode)
pub fn seal(dir: &Path) -> Result<(), String> {
    fn ro(p: &Path) -> Result<(), String> {
        let mut perm = std::fs::metadata(p).map_err(|e| e.to_string())?.permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(p, perm).map_err(|e| e.to_string())
    }
    fn rec(d: &Path) -> Result<(), String> {
        for e in std::fs::read_dir(d).map_err(|e| e.to_string())? {
            let p = e.map_err(|x| x.to_string())?.path();
            if p.is_dir() {
                rec(&p)?;
            }
            ro(&p)?;
        }
        Ok(())
    }
    rec(dir)?;
    ro(dir)
}

/// best-effort removal of a possibly-sealed tree: re-add write
/// permission top-down, then remove — a plain remove_dir_all fails on
/// read-only dirs
pub fn remove_tree(dir: &Path) {
    fn unro(p: &Path) {
        if let Ok(m) = std::fs::metadata(p) {
            let mut perm = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(p, perm);
        }
    }
    fn rec(d: &Path) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    rec(&p);
                }
                unro(&p);
            }
        }
    }
    if dir.is_dir() {
        rec(dir);
    }
    unro(dir);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_vectors() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let big = vec![b'a'; 1_000_000];
        assert_eq!(hex(&sha256(&big)), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }
    #[test]
    fn b64_works() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
    }
    #[test]
    fn escape() {
        assert_eq!(escape_path("GitHub.com/Ox/Dep"), "!git!hub.com/!ox/!dep");
        assert_eq!(escape_path("std/strings"), "std/strings");
    }
    #[test]
    fn names() {
        assert!(check_component("aux.oxl").is_err());
        assert!(check_component("CON").is_err());
        assert!(check_component("a\\b").is_err());
        assert!(check_component("x.").is_err());
        assert!(check_component("ok name.oxl").is_ok());
        assert!(check_component("fine").is_ok());
    }
}
