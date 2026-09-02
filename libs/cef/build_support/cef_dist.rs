//! Where the CEF distribution comes from when the build has none: the
//! Spotify CDN's index of official builds, read and unpacked by the build
//! script itself — in Rust, on every host, with no shell, python or tar
//! around it. A clean clone builds the browser on Windows as on the Mac.
//!
//! The pieces, each a plain function so the picker is testable without a
//! network:
//!
//! - [`pick`]: the index (`{platform: {versions: [{cef_version, channel,
//!   files: [{type, name, sha1, size}]}]}}`, newest first) → the newest
//!   `stable` `standard` archive for a platform, or the one a pin names.
//! - [`Pointer`]: which extracted directory `current-<platform>` means. The
//!   shell script used a symlink; a text file beside it works on every
//!   filesystem without privileges, so the build reads the file first and
//!   the symlink only as the older form.
//! - [`ensure_dist`]: the whole thing — pointer, else download, verify,
//!   extract, write the pointer. A platform that already has a dist is
//!   never bumped: the CDN moves on, our pin does not.
//!
//! The index is 10 MB of JSON; this reads exactly the fields it needs with
//! a small strict scanner rather than pulling a JSON crate into a build
//! script. Anything shaped differently is an error, never a guess.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const INDEX_URL: &str = "https://cef-builds.spotifycdn.com/index.json";
pub const BASE_URL: &str = "https://cef-builds.spotifycdn.com";

/// The platforms the CDN builds and we link. `linux32` and friends exist
/// there too, but nothing of ours runs on them.
pub const PLATFORMS: &[&str] = &[
    "linux64",
    "linuxarm64",
    "macosarm64",
    "macosx64",
    "windows64",
    "windowsarm64",
];

/// One downloadable archive, as the index describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archive {
    pub cef_version: String,
    pub channel: String,
    pub name: String,
    pub sha1: String,
    pub size: u64,
}

impl Archive {
    pub fn url(&self) -> String {
        format!("{BASE_URL}/{}", self.name)
    }

    /// The directory the archive unpacks to: its own name without the
    /// `.tar.bz2` — the CDN's archives carry exactly that top-level dir.
    pub fn extract_dir_name(&self) -> String {
        self.name
            .strip_suffix(".tar.bz2")
            .unwrap_or(&self.name)
            .to_string()
    }
}

// ------------------------------------------------------------- the index

/// A strict, allocation-light JSON reader over the index text: it walks
/// the structure it expects and refuses anything else. The index is a
/// map of platform → object, each with `"versions"`: an array of objects
/// with string `cef_version`/`channel` and `"files"`: an array of objects
/// with string `type`/`name`/`sha1` and a number `size`. Other keys are
/// skipped, whatever their type.
struct Scanner<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Scanner<'a> {
    fn new(s: &'a str) -> Self {
        Scanner { s: s.as_bytes(), i: 0 }
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("cef index: {what} at byte {}", self.i))
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            self.err(&format!("expected `{}`", c as char))
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let Some(&c) = self.s.get(self.i) else { return self.err("unterminated string") };
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else { return self.err("bad escape") };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' | b'f' => {}
                        b'u' => {
                            // The index carries no non-ASCII in the fields we
                            // read; keep the four hex digits as text rather
                            // than decode them.
                            let end = (self.i + 4).min(self.s.len());
                            out.push_str(&String::from_utf8_lossy(&self.s[self.i..end]));
                            self.i = end;
                        }
                        _ => return self.err("bad escape"),
                    }
                }
                _ => out.push(c as char),
            }
        }
    }

    fn number(&mut self) -> Result<u64, String> {
        self.ws();
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || matches!(self.s[self.i], b'.' | b'-' | b'+' | b'e' | b'E')) {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| "bad number".to_string())?;
        text.parse::<u64>().or_else(|_| text.parse::<f64>().map(|f| f as u64)).map_err(|_| format!("cef index: bad number `{text}`"))
    }

    /// Skip any value.
    fn skip(&mut self) -> Result<(), String> {
        match self.peek() {
            Some(b'"') => self.string().map(|_| ()),
            Some(b'{') => {
                self.i += 1;
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(());
                }
                loop {
                    self.string()?;
                    self.expect(b':')?;
                    self.skip()?;
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(());
                        }
                        _ => return self.err("expected `,` or `}`"),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(());
                }
                loop {
                    self.skip()?;
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(());
                        }
                        _ => return self.err("expected `,` or `]`"),
                    }
                }
            }
            Some(b't') | Some(b'f') | Some(b'n') | Some(b'-') | Some(b'0'..=b'9') => {
                while self.i < self.s.len() && !matches!(self.s[self.i], b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t') {
                    self.i += 1;
                }
                Ok(())
            }
            _ => self.err("unexpected value"),
        }
    }

    /// `{ "k": v, ... }` — calls `f(key)` for every key with the scanner
    /// positioned at the value; `f` must consume the value.
    fn object(&mut self, mut f: impl FnMut(&mut Self, &str) -> Result<(), String>) -> Result<(), String> {
        self.expect(b'{')?;
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(());
        }
        loop {
            let key = self.string()?;
            self.expect(b':')?;
            f(self, &key)?;
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(());
                }
                _ => return self.err("expected `,` or `}`"),
            }
        }
    }

    /// `[ v, ... ]` — calls `f` for every element with the scanner at it.
    fn array(&mut self, mut f: impl FnMut(&mut Self) -> Result<(), String>) -> Result<(), String> {
        self.expect(b'[')?;
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(());
        }
        loop {
            f(self)?;
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(());
                }
                _ => return self.err("expected `,` or `]`"),
            }
        }
    }
}

/// Every archive the index lists for `platform`, in the index's order
/// (newest first), any channel, any file type.
pub fn archives_for(index_json: &str, platform: &str) -> Result<Vec<Archive>, String> {
    if !PLATFORMS.contains(&platform) {
        return Err(format!("cef: `{platform}` is not a platform we link (one of {})", PLATFORMS.join(", ")));
    }
    let mut sc = Scanner::new(index_json);
    let mut found_platform = false;
    let mut out: Vec<Archive> = Vec::new();
    sc.object(|sc, key| {
        if key != platform {
            return sc.skip();
        }
        found_platform = true;
        sc.object(|sc, key| {
            if key != "versions" {
                return sc.skip();
            }
            sc.array(|sc| {
                let mut cef_version = String::new();
                let mut channel = String::from("stable");
                let mut files: Vec<(String, String, String, u64)> = Vec::new();
                sc.object(|sc, key| match key {
                    "cef_version" => {
                        cef_version = sc.string()?;
                        Ok(())
                    }
                    "channel" => {
                        channel = sc.string()?;
                        Ok(())
                    }
                    "files" => sc.array(|sc| {
                        let (mut ty, mut name, mut sha1, mut size) = (String::new(), String::new(), String::new(), 0u64);
                        sc.object(|sc, key| match key {
                            "type" => {
                                ty = sc.string()?;
                                Ok(())
                            }
                            "name" => {
                                name = sc.string()?;
                                Ok(())
                            }
                            "sha1" => {
                                sha1 = sc.string()?;
                                Ok(())
                            }
                            "size" => {
                                size = sc.number()?;
                                Ok(())
                            }
                            _ => sc.skip(),
                        })?;
                        files.push((ty, name, sha1, size));
                        Ok(())
                    }),
                    _ => sc.skip(),
                })?;
                if cef_version.is_empty() {
                    return Err("cef index: a version entry without cef_version".into());
                }
                for (ty, name, sha1, size) in files {
                    if ty != "standard" {
                        continue;
                    }
                    if name.is_empty() {
                        return Err(format!("cef index: {cef_version} has a standard file without a name"));
                    }
                    out.push(Archive { cef_version: cef_version.clone(), channel: channel.clone(), name, sha1, size });
                }
                Ok(())
            })
        })
    })?;
    if !found_platform {
        return Err(format!("cef index: no `{platform}` section"));
    }
    Ok(out)
}

/// The archive to fetch: the newest stable standard build for `platform`,
/// or, with a pin, the stable build whose cef_version starts with it
/// (`138.0.59` matches `138.0.59+g21d63d5+chromium-138.0.7204.306`). Beta
/// builds are never picked.
pub fn pick(index_json: &str, platform: &str, pin: Option<&str>) -> Result<Archive, String> {
    let archives = archives_for(index_json, platform)?;
    let stable = archives.into_iter().filter(|a| a.channel == "stable");
    match pin {
        Some(pin) => {
            let pin = pin.trim();
            stable
                .filter(|a| a.cef_version == pin || a.cef_version.starts_with(&format!("{pin}+")))
                .next()
                .ok_or_else(|| format!("cef index: no stable standard build {pin} for {platform}"))
        }
        None => stable
            .max_by(|a, b| version_key(&a.cef_version).cmp(&version_key(&b.cef_version)))
            .ok_or_else(|| format!("cef index: no stable standard build for {platform}")),
    }
}

/// `138.0.59+g21d63d5+chromium-138.0.7204.306` → (138, 0, 59): the CDN's
/// order is newest first, but the pick must not depend on it.
fn version_key(v: &str) -> (u64, u64, u64) {
    let head = v.split('+').next().unwrap_or(v);
    let mut it = head.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

// ------------------------------------------------------------ the pointer

/// `current-<platform>` as a text file: the extracted directory's name
/// (relative to the prebuilt dir) on the first line. Works on every
/// filesystem without privileges; the older symlink form is still read.
pub struct Pointer;

impl Pointer {
    pub fn file(prebuilt_dir: &Path, platform: &str) -> PathBuf {
        prebuilt_dir.join(format!("current-{platform}.txt"))
    }

    pub fn link(prebuilt_dir: &Path, platform: &str) -> PathBuf {
        prebuilt_dir.join(format!("current-{platform}"))
    }

    /// The dist dir the pointer names, when it exists: the text file
    /// first, then the symlink (or a real directory of that name).
    pub fn read(prebuilt_dir: &Path, platform: &str) -> Option<PathBuf> {
        let file = Self::file(prebuilt_dir, platform);
        if let Ok(text) = fs::read_to_string(&file) {
            let name = text.lines().next().unwrap_or("").trim();
            if !name.is_empty() {
                let dir = prebuilt_dir.join(name);
                if dir.join("include").is_dir() {
                    return Some(dir);
                }
            }
        }
        let link = Self::link(prebuilt_dir, platform);
        if link.join("include").is_dir() {
            return Some(link);
        }
        None
    }

    pub fn write(prebuilt_dir: &Path, platform: &str, dir_name: &str) -> Result<(), String> {
        let file = Self::file(prebuilt_dir, platform);
        fs::write(&file, format!("{dir_name}\n")).map_err(|e| format!("cef: cannot write {}: {e}", file.display()))
    }
}

// ------------------------------------------------------------- the fetch

/// Progress goes to cargo as warnings: a build that sits for minutes on a
/// 300 MB download must say so.
fn say(msg: &str) {
    println!("cargo:warning=cef: {msg}");
}

fn http_get(url: &str) -> Result<ureq::Body, String> {
    let response = ureq::get(url).call().map_err(|e| format!("cef: GET {url}: {e}"))?;
    Ok(response.into_body())
}

pub fn fetch_index() -> Result<String, String> {
    let mut body = http_get(INDEX_URL)?;
    let text = body.read_to_string().map_err(|e| format!("cef: reading {INDEX_URL}: {e}"))?;
    Ok(text)
}

fn sha1_hex(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("cef: open {}: {e}", path.display()))?;
    let mut hasher = sha1_smol::Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("cef: read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.digest().to_string())
}

/// Download `archive` into `prebuilt_dir` (to a `.part` file, renamed when
/// complete), unless the archive is already there and matches its sha1.
fn download(archive: &Archive, prebuilt_dir: &Path) -> Result<PathBuf, String> {
    let path = prebuilt_dir.join(&archive.name);
    if path.is_file() {
        if archive.sha1.is_empty() || sha1_hex(&path)? == archive.sha1 {
            say(&format!("using the archive already at {}", path.display()));
            return Ok(path);
        }
        say(&format!("{} does not match the index's sha1; downloading again", path.display()));
        let _ = fs::remove_file(&path);
    }
    let part = prebuilt_dir.join(format!("{}.part", archive.name));
    let _ = fs::remove_file(&part);
    say(&format!(
        "downloading {} ({:.0} MB) — this takes a few minutes",
        archive.url(),
        archive.size as f64 / 1e6
    ));
    let url = archive.url();
    let mut body = http_get(&url)?;
    let mut reader = body.as_reader();
    let mut out = fs::File::create(&part).map_err(|e| format!("cef: create {}: {e}", part.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done: u64 = 0;
    let mut next_report: u64 = 50_000_000;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("cef: download {url}: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| format!("cef: write {}: {e}", part.display()))?;
        done += n as u64;
        if done >= next_report {
            say(&format!("{:.0} of {:.0} MB", done as f64 / 1e6, archive.size as f64 / 1e6));
            next_report += 50_000_000;
        }
    }
    drop(out);
    if archive.size > 0 && done != archive.size {
        return Err(format!("cef: {} is {done} bytes; the index says {}", archive.name, archive.size));
    }
    if !archive.sha1.is_empty() {
        let actual = sha1_hex(&part)?;
        if actual != archive.sha1 {
            let _ = fs::remove_file(&part);
            return Err(format!("cef: sha1 of {} is {actual}; the index says {}", archive.name, archive.sha1));
        }
        say("sha1 verified");
    }
    fs::rename(&part, &path).map_err(|e| format!("cef: rename {}: {e}", part.display()))?;
    Ok(path)
}

/// Unpack the archive beside itself; the top-level directory inside the
/// tarball is the dist dir. A half-extracted directory from an earlier
/// failure is removed first.
fn extract(archive_path: &Path, prebuilt_dir: &Path, dir_name: &str) -> Result<PathBuf, String> {
    let dir = prebuilt_dir.join(dir_name);
    if dir.exists() {
        say(&format!("removing the incomplete {}", dir.display()));
        fs::remove_dir_all(&dir).map_err(|e| format!("cef: remove {}: {e}", dir.display()))?;
    }
    say(&format!("extracting {} into {}", archive_path.display(), prebuilt_dir.display()));
    let file = fs::File::open(archive_path).map_err(|e| format!("cef: open {}: {e}", archive_path.display()))?;
    let decoder = bzip2::read::MultiBzDecoder::new(std::io::BufReader::new(file));
    let mut tar = tar::Archive::new(decoder);
    tar.set_preserve_permissions(true);
    tar.unpack(prebuilt_dir).map_err(|e| format!("cef: extract {}: {e}", archive_path.display()))?;
    if !dir.join("include").is_dir() {
        return Err(format!("cef: {} unpacked but has no include/ — not a CEF distribution", dir.display()));
    }
    Ok(dir)
}

/// What the build wants, in one shape for both the real run and the dry run.
#[derive(Clone, Debug)]
pub struct Plan {
    pub platform: String,
    pub archive: Archive,
    pub extract_dir: PathBuf,
    pub archive_path: PathBuf,
}

impl Plan {
    pub fn describe(&self) -> String {
        format!(
            "platform {}: {} ({}), {} bytes, sha1 {}\n  from {}\n  archive {}\n  extract to {}",
            self.platform,
            self.archive.cef_version,
            self.archive.channel,
            self.archive.size,
            self.archive.sha1,
            self.archive.url(),
            self.archive_path.display(),
            self.extract_dir.display()
        )
    }
}

/// Resolve the index into a plan for `platform` — the dry run's whole job.
pub fn plan(prebuilt_dir: &Path, platform: &str, pin: Option<&str>) -> Result<Plan, String> {
    let index = fetch_index()?;
    let archive = pick(&index, platform, pin)?;
    let dir_name = archive.extract_dir_name();
    Ok(Plan {
        platform: platform.to_string(),
        extract_dir: prebuilt_dir.join(&dir_name),
        archive_path: prebuilt_dir.join(&archive.name),
        archive,
    })
}

/// The dist dir for `platform` under `prebuilt_dir`: the one the pointer
/// names, else downloaded, verified, extracted and pointed at. `offline`
/// turns a miss into an error naming what to put where.
pub fn ensure_dist(prebuilt_dir: &Path, platform: &str, pin: Option<&str>, offline: bool) -> Result<PathBuf, String> {
    if let Some(dir) = Pointer::read(prebuilt_dir, platform) {
        return Ok(dir);
    }
    if offline {
        return Err(format!(
            "cef: no distribution for {platform} under {} and MAKEPAD_CEF_OFFLINE is set — put a cef_binary_*_{platform} directory there and name it in {}",
            prebuilt_dir.display(),
            Pointer::file(prebuilt_dir, platform).display()
        ));
    }
    fs::create_dir_all(prebuilt_dir).map_err(|e| format!("cef: create {}: {e}", prebuilt_dir.display()))?;
    let plan = plan(prebuilt_dir, platform, pin)?;
    say(&format!("no distribution for {platform}; {}", plan.describe().replace('\n', " ")));
    let dir_name = plan.archive.extract_dir_name();
    let dir = if plan.extract_dir.join("include").is_dir() {
        say(&format!("using the extracted {}", plan.extract_dir.display()));
        plan.extract_dir.clone()
    } else {
        let archive_path = download(&plan.archive, prebuilt_dir)?;
        extract(&archive_path, prebuilt_dir, &dir_name)?
    };
    Pointer::write(prebuilt_dir, platform, &dir_name)?;
    say(&format!("{platform} → {} ({})", dir_name, plan.archive.cef_version));
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"{
      "linux64": {"versions": [
        {"cef_version": "144.0.34+g8fc21c8+chromium-144.0.7559.261", "chromium_version": "144.0.7559.261", "channel": "stable",
         "files": [{"type": "standard", "name": "cef_binary_144.0.34+g8fc21c8+chromium-144.0.7559.261_linux64.tar.bz2", "sha1": "23e2", "size": 872549074, "last_modified": "x"},
                   {"type": "minimal", "name": "cef_binary_144.0.34+g8fc21c8+chromium-144.0.7559.261_linux64_minimal.tar.bz2", "sha1": "aa", "size": 1}]},
        {"cef_version": "145.0.1+gbeta+chromium-145.0.1.1", "channel": "beta",
         "files": [{"type": "standard", "name": "cef_binary_145.0.1+gbeta+chromium-145.0.1.1_linux64.tar.bz2", "sha1": "bb", "size": 2}]},
        {"cef_version": "144.0.32+g5ce7d26+chromium-144.0.7559.258", "channel": "stable",
         "files": [{"type": "standard", "name": "cef_binary_144.0.32+g5ce7d26+chromium-144.0.7559.258_linux64.tar.bz2", "sha1": "0827", "size": 872549258}]},
        {"cef_version": "138.0.59+g21d63d5+chromium-138.0.7204.306", "channel": "stable",
         "files": [{"type": "standard", "name": "cef_binary_138.0.59+g21d63d5+chromium-138.0.7204.306_linux64.tar.bz2", "sha1": "913e", "size": 810258845}]}
      ]},
      "linux32": {"versions": []},
      "windows64": {"versions": [
        {"cef_version": "144.0.34+g8fc21c8+chromium-144.0.7559.261", "channel": "stable",
         "files": [{"type": "standard", "name": "cef_binary_144.0.34+g8fc21c8+chromium-144.0.7559.261_windows64.tar.bz2", "sha1": "81e9", "size": 320482619}]}
      ]}
    }"#;

    #[test]
    fn the_newest_stable_standard_build_wins_and_beta_is_never_picked() {
        let a = pick(INDEX, "linux64", None).unwrap();
        assert_eq!(a.cef_version, "144.0.34+g8fc21c8+chromium-144.0.7559.261");
        assert_eq!(a.channel, "stable");
        assert_eq!(a.sha1, "23e2");
        assert_eq!(a.size, 872549074);
        assert_eq!(a.url(), "https://cef-builds.spotifycdn.com/cef_binary_144.0.34+g8fc21c8+chromium-144.0.7559.261_linux64.tar.bz2");
        assert_eq!(a.extract_dir_name(), "cef_binary_144.0.34+g8fc21c8+chromium-144.0.7559.261_linux64");
        // Minimal archives are never listed; the beta 145 is skipped even
        // though it is newer.
        let all = archives_for(INDEX, "linux64").unwrap();
        assert_eq!(all.len(), 4);
        assert!(all.iter().all(|a| !a.name.contains("minimal")));
    }

    #[test]
    fn a_pin_is_honoured_by_prefix_or_whole() {
        assert_eq!(pick(INDEX, "linux64", Some("138.0.59")).unwrap().sha1, "913e");
        assert_eq!(pick(INDEX, "linux64", Some("144.0.32+g5ce7d26+chromium-144.0.7559.258")).unwrap().sha1, "0827");
        // A pin on the beta is refused: only stable builds are ever picked.
        assert!(pick(INDEX, "linux64", Some("145.0.1")).unwrap_err().contains("no stable standard build 145.0.1"));
        assert!(pick(INDEX, "linux64", Some("999")).unwrap_err().contains("999"));
    }

    #[test]
    fn a_missing_or_unsupported_platform_is_a_clear_error() {
        assert!(pick(INDEX, "macosarm64", None).unwrap_err().contains("no `macosarm64` section"));
        assert!(pick(INDEX, "linux32", None).unwrap_err().contains("not a platform we link"));
        assert!(pick("{\"linux64\": {\"versions\": [{\"channel\": \"stable\", \"files\": []}]}}", "linux64", None)
            .unwrap_err()
            .contains("without cef_version"));
        assert!(pick("{\"linux64\": ", "linux64", None).is_err());
    }

    #[test]
    fn the_pointer_file_names_the_dist_and_survives_a_missing_dir() {
        let root = std::env::temp_dir().join(format!("cef-pointer-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("cef_binary_1_linux64").join("include")).unwrap();
        assert!(Pointer::read(&root, "linux64").is_none());
        Pointer::write(&root, "linux64", "cef_binary_1_linux64").unwrap();
        assert_eq!(Pointer::read(&root, "linux64").unwrap(), root.join("cef_binary_1_linux64"));
        // A pointer at a directory that is not a dist is ignored.
        Pointer::write(&root, "linux64", "nowhere").unwrap();
        assert!(Pointer::read(&root, "linux64").is_none());
        // The older symlink form still counts.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("cef_binary_1_linux64"), Pointer::link(&root, "linux64")).unwrap();
            assert_eq!(Pointer::read(&root, "linux64").unwrap(), Pointer::link(&root, "linux64"));
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn version_keys_order_numerically() {
        assert!(version_key("144.0.34+g8fc21c8+chromium-144.0.7559.261") > version_key("144.0.32+g5ce7d26+chromium-144.0.7559.258"));
        assert!(version_key("144.0.32+x") > version_key("138.0.59+x"));
        assert!(version_key("10.0.0") > version_key("9.9.9"));
    }
}
