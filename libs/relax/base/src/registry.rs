//! crates.io packages for programs that are not vendored: the sparse index (index.crates.io),
//! the .crate archives (static.crates.io) checked against the index's SHA-256, unpacked with
//! Relax's own gzip and tar readers into a content-addressed cache in the workspace's relax dir
//! (`<target>/relax/registry`, dirs.rs), offline-first: a cached index and a cached package are used as they are,
//! the network is asked only for what is missing (MAKEPAD_RUN_OFFLINE=1: never). The network itself is
//! the embedder's ([`set_fetch`]: the makepad binary and the loader install an HTTPS GET), so this
//! module does no I/O beyond files.
//!
//! Never used for a workspace whose cargo config replaces crates-io with a vendored directory
//! (resolve.rs reads only that directory then). Packages that compile C (a `cc` or `cmake`
//! build-dependency) are refused with a clear error: Relax runs no C compiler.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// An HTTPS GET: the body of a 200 response, else an error naming the status.
pub type Fetch = fn(url: &str) -> Result<Vec<u8>, String>;

static FETCH: OnceLock<Fetch> = OnceLock::new();

/// Installs the network access registry packages are fetched through (once per process).
pub fn set_fetch(f: Fetch) {
    let _ = FETCH.set(f);
}

const INDEX: &str = "https://index.crates.io";
const DOWNLOAD: &str = "https://static.crates.io/crates";

fn offline() -> bool {
    std::env::var_os("MAKEPAD_RUN_OFFLINE").is_some_and(|v| v != "0")
}

fn get(url: &str, what: &str) -> Result<Vec<u8>, String> {
    if offline() {
        return Err(format!("{} is not cached and MAKEPAD_RUN_OFFLINE is set", what));
    }
    let f = FETCH.get().ok_or_else(|| format!("{} is not cached and this process has no network access for registry packages", what))?;
    f(url).map_err(|e| format!("{}: {}: {}", what, url, e))
}

/// `<relax dir>/registry`: the workspace's own copy, removed with its build output (dirs.rs).
fn root() -> Result<PathBuf, String> {
    Ok(crate::dirs::relax_dir().join("registry"))
}

/// The sparse index's path for a package name (cargo's layout: 1/, 2/, 3/<c>/, <ab>/<cd>/).
fn index_path(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    match n.len() {
        1 => format!("1/{}", n),
        2 => format!("2/{}", n),
        3 => format!("3/{}/{}", &n[..1], n),
        _ => format!("{}/{}/{}", &n[..2], &n[2..4], n),
    }
}

/// One published version of a package, as the index lists it.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub cksum: String,
    pub yanked: bool,
}

/// The index lines of `name` (cached under `<root>/index/`; fetched when not cached).
fn releases(name: &str) -> Result<Vec<Release>, String> {
    let file = root()?.join("index").join(index_path(name));
    let text = match std::fs::read(&file) {
        Ok(t) => t,
        Err(_) => {
            let body = get(&format!("{}/{}", INDEX, index_path(name)), &format!("the index entry of `{}`", name))?;
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
            }
            write_atomic(&file, &body)?;
            body
        }
    };
    let text = String::from_utf8_lossy(&text);
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let j = crate::json::parse(line.as_bytes()).map_err(|e| format!("index entry of `{}`: {}", name, e))?;
        let (Some(v), Some(c)) = (j.get("vers").map(|x| x.str()), j.get("cksum").map(|x| x.str())) else { continue };
        let yanked = matches!(j.get("yanked"), Some(crate::json::Json::Bool(true)));
        out.push(Release { version: v.to_string(), cksum: c.to_string(), yanked });
    }
    Ok(out)
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("part{}", std::process::id()));
    std::fs::write(&tmp, data).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            // another run wrote it first and has it open (Windows refuses the replace): the
            // same index text, so theirs is kept
            if path.is_file() { Ok(()) } else { Err(format!("{}: {}", path.display(), e)) }
        }
    }
}

/// The newest non-yanked release of `name` that `req` (a cargo version requirement) accepts,
/// and that every requirement in `also` accepts too.
pub fn pick(name: &str, req: &str, also: &[String]) -> Result<Release, String> {
    let rs = releases(name)?;
    let mut best: Option<(Version, Release)> = None;
    for r in rs {
        let Some(v) = Version::parse(&r.version) else { continue };
        if r.yanked || !matches(req, &v) || !also.iter().all(|a| matches(a, &v)) {
            continue;
        }
        if best.as_ref().map_or(true, |(b, _)| v > *b) {
            best = Some((v, r));
        }
    }
    best.map(|b| b.1).ok_or_else(|| {
        let mut all = vec![req.to_string()];
        all.extend(also.iter().cloned());
        format!("registry package `{}`: no published version matches {}", name, all.join(" and "))
    })
}

/// The release of `name` at exactly `version` (a Cargo.lock entry).
pub fn exact(name: &str, version: &str) -> Result<Release, String> {
    releases(name)?.into_iter().find(|r| r.version == version).ok_or_else(|| format!("registry package `{}` {} is not in the crates.io index", name, version))
}

/// The unpacked sources of a release (fetched, checked and unpacked when not cached): a folder
/// named by the package, its version and its checksum, holding `Cargo.toml`.
pub fn sources(name: &str, r: &Release) -> Result<String, String> {
    let dir = root()?.join("src").join(format!("{}-{}-{}", name, r.version, &r.cksum[..16.min(r.cksum.len())]));
    let done = dir.join(".makepad-run-unpacked");
    if !done.is_file() {
        let what = format!("`{}` {}", name, r.version);
        let body = get(&format!("{}/{}/{}-{}.crate", DOWNLOAD, name, name, r.version), &what)?;
        let got = crate::sha256::sha256_hex(&body);
        if !got.eq_ignore_ascii_case(&r.cksum) {
            return Err(format!("{}: the download's SHA-256 {} is not the index's {}", what, got, r.cksum));
        }
        let tar = crate::inflate::inflate::decompress_to_vec_gzip(&body).map_err(|e| format!("{}: not a gzip archive: {:?}", what, e))?;
        let staging = dir.with_extension(format!("unpack{}", std::process::id()));
        crate::tar::unpack(&tar[..], &staging).map_err(|e| format!("{}: unpacking: {}", what, e))?;
        // the archive holds one folder, `<name>-<version>/`
        let top = staging.join(format!("{}-{}", name, r.version));
        if !top.join("Cargo.toml").is_file() {
            return Err(format!("{}: the archive has no {}-{}/Cargo.toml", what, name, r.version));
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        }
        std::fs::rename(&top, &dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::write(&done, &r.cksum).map_err(|e| format!("{}: {}", done.display(), e))?;
    }
    Ok(dir.to_string_lossy().replace('\\', "/"))
}

/// A build-dependency that means the package compiles C or C++ (Relax runs no C compiler).
pub fn c_build_dep(build_deps: &[String]) -> Option<&String> {
    build_deps.iter().find(|d| matches!(d.as_str(), "cc" | "cmake" | "cxx-build" | "bindgen"))
}

// ---- versions and requirements (cargo's: https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(|x| x.to_string()).collect()),
            None => (s, Vec::new()),
        };
        let mut it = core.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch, pre })
    }
}

fn pre_cmp(a: &[String], b: &[String]) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Equal,
        (true, false) => return Greater,
        (false, true) => return Less,
        _ => {}
    }
    for (x, y) in a.iter().zip(b) {
        let o = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(p), Ok(q)) => p.cmp(&q),
            (Ok(_), Err(_)) => Less,
            (Err(_), Ok(_)) => Greater,
            _ => x.cmp(y),
        };
        if o != Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch).cmp(&(o.major, o.minor, o.patch)).then_with(|| pre_cmp(&self.pre, &o.pre))
    }
}

/// Does requirement `req` ("1.2", "^1", "~0.3.1", "=1.0.0", ">=1, <2", "1.*", "*") accept `v`?
/// A pre-release matches only a comparator that names a pre-release of the same x.y.z (cargo).
pub fn matches(req: &str, v: &Version) -> bool {
    let req = req.trim();
    if req.is_empty() || req == "*" {
        return v.pre.is_empty();
    }
    let mut pre_ok = v.pre.is_empty();
    for c in req.split(',') {
        let c = c.trim();
        let (op, rest) = ["<=", ">=", "=", "<", ">", "^", "~"].iter().find_map(|op| c.strip_prefix(op).map(|r| (*op, r.trim()))).unwrap_or(("^", c));
        // a wildcard (`1.*`, `1.2.*`) is an exact match on the components given
        let op = if rest.contains('*') { "=" } else { op };
        // partial versions and wildcards: 1, 1.2, 1.*, 1.2.*
        let parts: Vec<&str> = rest.split('-').next().unwrap_or("").split('.').collect();
        let num = |i: usize| parts.get(i).and_then(|p| p.parse::<u64>().ok());
        let (maj, min, pat) = (num(0), num(1), num(2));
        let Some(maj) = maj else { return false };
        let pre: Vec<String> = rest.split_once('-').map_or(Vec::new(), |(_, p)| p.split('.').map(|x| x.to_string()).collect());
        if !v.pre.is_empty() && !pre.is_empty() && (v.major, v.minor, v.patch) == (maj, min.unwrap_or(0), pat.unwrap_or(0)) {
            pre_ok = true;
        }
        let lo = Version { major: maj, minor: min.unwrap_or(0), patch: pat.unwrap_or(0), pre: pre.clone() };
        let ok = match op {
            "=" => match (min, pat) {
                (Some(_), Some(_)) => *v == lo,
                (Some(mi), None) => v.major == maj && v.minor == mi,
                _ => v.major == maj,
            },
            ">" => match (min, pat) {
                (Some(_), Some(_)) => *v > lo,
                (Some(mi), None) => (v.major, v.minor) > (maj, mi),
                _ => v.major > maj,
            },
            ">=" => *v >= lo,
            "<" => *v < lo,
            "<=" => match (min, pat) {
                (Some(_), Some(_)) => *v <= lo,
                (Some(mi), None) => (v.major, v.minor) <= (maj, mi),
                _ => v.major <= maj,
            },
            "~" => {
                *v >= lo
                    && match min {
                        Some(mi) => (v.major, v.minor) == (maj, mi),
                        None => v.major == maj,
                    }
            }
            _ => {
                // caret (and bare): the left-most non-zero component stays the same
                *v >= lo
                    && if maj > 0 || min.is_none() {
                        v.major == maj
                    } else if min.unwrap_or(0) > 0 || pat.is_none() {
                        v.major == 0 && v.minor == min.unwrap_or(0)
                    } else {
                        (v.major, v.minor, v.patch) == (0, 0, pat.unwrap_or(0))
                    }
            }
        };
        if !ok {
            return false;
        }
    }
    pre_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(r: &str, v: &str) -> bool {
        matches(r, &Version::parse(v).unwrap())
    }
    #[test]
    fn requirements() {
        assert!(m("1.2.3", "1.9.0") && !m("1.2.3", "2.0.0") && !m("1.2.3", "1.2.2"));
        assert!(m("0.2.3", "0.2.9") && !m("0.2.3", "0.3.0"));
        assert!(m("0.0.3", "0.0.3") && !m("0.0.3", "0.0.4"));
        assert!(m("0", "0.9.0") && m("1", "1.5.0") && !m("1", "2.0.0"));
        assert!(m("~1.2.3", "1.2.9") && !m("~1.2.3", "1.3.0") && m("~1", "1.9.0"));
        assert!(m("=1.2.3", "1.2.3") && !m("=1.2.3", "1.2.4"));
        assert!(m(">=1.0, <1.5", "1.4.9") && !m(">=1.0, <1.5", "1.5.0"));
        assert!(m("1.*", "1.7.0") && m("*", "3.0.0") && !m("1.2.*", "1.3.0"));
        assert!(!m("1.0", "1.1.0-alpha") && m("1.1.0-alpha", "1.1.0-beta") && !m("1.1.0-beta", "1.1.0-alpha"));
        assert!(Version::parse("1.0.0-alpha.1").unwrap() < Version::parse("1.0.0").unwrap());
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("Serde"), "se/rd/serde");
    }
}
