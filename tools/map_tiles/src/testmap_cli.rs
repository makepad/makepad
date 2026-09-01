//! `makepad-map-tiles testmap` — the whole test-map recipe from a shell.
//!
//! The recipe itself is `makepad_map_build::testmap`; this is the CLI's
//! half: fetching the extract, and printing the same commentary the bake
//! passes already print (no progress sink is installed, so every
//! `step!`/`note!` goes straight to stdout).
//!
//! Apps do not use this fetcher. They download through the platform's own
//! HTTP stack, which streams to disk and reports progress into their window
//! (see `apps/route/src/testmap.rs`) and then run the same [`bake`] with
//! [`NoFetch`], because by then the extract is already on disk.

use makepad_map_build::testmap::{bake, BakeOptions, Fetch, TestMapPaths};
use makepad_network::blocking_http::{self, Limits, Request};
use makepad_network::digest::md5_hash;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// Refuse a body larger than this. The Amsterdam extract is ~143 MB; a
/// city extract that arrives an order of magnitude bigger than expected is
/// a wrong URL, not a busy day in Amsterdam.
const MAX_EXTRACT_BYTES: usize = 1024 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const MAX_REDIRECTS: usize = 5;

/// Blocking, whole-body fetch with an MD5 check against the sidecar the
/// mirrors publish next to each extract.
///
/// Whole-body because that is what `blocking_http` offers and a terminal
/// has nothing to animate anyway: the one thing a CLI user needs is to
/// know the file arrived intact.
pub struct SidecarFetch;

impl Fetch for SidecarFetch {
    fn fetch(
        &mut self,
        url: &str,
        dest: &Path,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        let body = get_following_redirects(url)?;
        on_progress(body.len() as u64, Some(body.len() as u64));
        if let Some(expected) = sidecar_md5(url) {
            let actual = hex(&md5_hash(&body));
            if actual != expected {
                return Err(format!(
                    "MD5 mismatch for {url}: expected {expected}, got {actual}"
                ));
            }
            makepad_map_build::note!("fetch", "  MD5 verified against {url}.md5");
        }
        // Written beside the destination and renamed, so an interrupted
        // run never leaves something that looks like a finished extract.
        let part = dest.with_extension("pbf.part");
        if let Some(dir) = part.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        fs::write(&part, &body).map_err(|e| format!("write {}: {e}", part.display()))?;
        fs::rename(&part, dest).map_err(|e| format!("rename {}: {e}", part.display()))
    }
}

fn limits() -> Limits {
    Limits {
        max_body_bytes: MAX_EXTRACT_BYTES,
        total_timeout: FETCH_TIMEOUT,
        ..Limits::default()
    }
}

fn get_following_redirects(url: &str) -> Result<Vec<u8>, String> {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let request = Request::get(current.clone()).limits(limits());
        let response = blocking_http::request_no_redirect(request)
            .map_err(|err| format!("GET {current}: {err}"))?;
        match response.status {
            200 => return Ok(response.body),
            301 | 302 | 303 | 307 | 308 => {
                let location = response
                    .header("location")
                    .ok_or_else(|| format!("{current}: redirect without location"))?;
                current = absolute(&current, location)?;
            }
            status => return Err(format!("GET {current}: HTTP {status}")),
        }
    }
    Err(format!("GET {url}: too many redirects"))
}

/// Resolve a Location header against the URL it came from. Only the two
/// forms mirrors actually send: an absolute URL, or an absolute path.
fn absolute(base: &str, location: &str) -> Result<String, String> {
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(location.to_string());
    }
    if !location.starts_with('/') {
        return Err(format!("{base}: unsupported relative redirect {location}"));
    }
    let scheme_end = base.find("://").ok_or_else(|| format!("bad url {base}"))? + 3;
    let host_end = base[scheme_end..]
        .find('/')
        .map(|i| scheme_end + i)
        .unwrap_or(base.len());
    Ok(format!("{}{}", &base[..host_end], location))
}

/// The `.md5` beside the extract, when the mirror publishes one. A missing
/// sidecar is not an error — it just means the size check is all we have.
fn sidecar_md5(url: &str) -> Option<String> {
    let request = Request::get(format!("{url}.md5")).limits(limits());
    let response = blocking_http::request_no_redirect(request).ok()?;
    if response.status != 200 {
        return None;
    }
    let text = String::from_utf8(response.body).ok()?;
    let digest = text.split_whitespace().next()?.to_ascii_lowercase();
    (digest.len() == 32 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digest)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `testmap [--dir DIR] [--name NAME] [--url URL] [--keep-store]`
pub fn run(args: &[String]) -> Result<(), String> {
    let mut options = BakeOptions::amsterdam();
    let mut dir = "local/maps".to_string();
    let mut name = "amsterdam".to_string();
    let mut iter = args[1..].iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dir" => dir = iter.next().ok_or("--dir needs a directory")?.clone(),
            "--name" => name = iter.next().ok_or("--name needs a name")?.clone(),
            "--url" => options.pbf_url = iter.next().ok_or("--url needs a URL")?.clone(),
            "--keep-store" => options.keep_store = true,
            other => return Err(format!("testmap: unexpected argument {other}")),
        }
    }
    options.paths = TestMapPaths::in_dir(&dir, &name);
    if options.paths.is_complete() {
        println!("test map already built:");
        report(&options.paths);
        return Ok(());
    }
    bake(&options, &mut SidecarFetch)?;
    report(&options.paths);
    Ok(())
}

fn report(paths: &TestMapPaths) {
    for path in [&paths.archive, &paths.graph(), &paths.search()] {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!("  {} ({:.1} MB)", path.display(), size as f64 / 1.0e6);
    }
}
