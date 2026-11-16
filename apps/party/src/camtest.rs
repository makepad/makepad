//! The camera check (`/p/<code>/camtest`): one page that tries everything the
//! party page will rely on, on the phone in hand, and reports each result to
//! the box's own log (this process's stderr) and nowhere else: no route reads
//! it back. A line names the page's random session and the browser, never
//! the phone's address. Uploads are looked at (size, container, JPEG frames)
//! and dropped; nothing is written anywhere.

use crate::jpeg;
use crate::server::{html, json, limits, slow_down, text, ByteBudget, Handler, RateLimiter, Request};
use makepad_bounded_http::{Method, Resp};
use makepad_micro_serde::*;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub const PAGE: &str = include_str!("../web/camtest.html");

/// Report bodies are a few hundred bytes; video uploads are a short clip at
/// most, and only streamed through (never held).
const MAX_REPORT_BYTES: u64 = 64 * 1024;
const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
/// Picture uploads are read whole to look at their frames: at most what a
/// party loop may be.
const MAX_PICTURES_BYTES: u64 = 48 * 1024 * 1024;
/// The most a report adds to the log, per field and in all.
const MAX_LINE_CHARS: usize = 400;

#[derive(DeJson)]
struct Report {
    session: String,
    test: String,
    ok: bool,
    detail: String,
    ua: Option<String>,
}

pub struct CamTest {
    /// A run of the page sends a few dozen reports and a handful of uploads.
    reports: RateLimiter<IpAddr>,
    uploads: RateLimiter<IpAddr>,
    /// Lines from everyone together: past it, reports are counted, not
    /// logged, so the log cannot be flooded.
    logged: RateLimiter<()>,
    unlogged: AtomicU64,
    /// What picture uploads may hold at once, shared with the party's own.
    inflight: Arc<ByteBudget>,
}

impl Default for CamTest {
    fn default() -> Self {
        Self::new(Arc::new(ByteBudget::new(limits::memory_bytes() / 16)))
    }
}

impl CamTest {
    pub fn new(inflight: Arc<ByteBudget>) -> CamTest {
        CamTest {
            reports: RateLimiter::new(60.0, 2.0),
            uploads: RateLimiter::new(20.0, 1.0),
            logged: RateLimiter::new(300.0, 10.0),
            unlogged: AtomicU64::new(0),
            inflight,
        }
    }

    fn log(&self, line: &str) {
        if self.logged.take(&(), 1.0).is_err() {
            self.unlogged.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let unlogged = self.unlogged.swap(0, Ordering::Relaxed);
        if unlogged > 0 {
            eprintln!("[camtest] ({unlogged} reports not logged: too many at once)");
        }
        eprintln!("[camtest] {line}");
    }

    /// One report from `peer`: logged (bounded), or refused for a while when
    /// that address sends too many.
    fn report(&self, peer: IpAddr, body: &[u8]) -> Resp {
        if let Err(wait) = self.reports.take(&peer, 1.0) {
            return slow_down(wait, "the camera check is sending a lot at once: trying again shortly");
        }
        match report_line(body) {
            Some(line) => {
                self.log(&line);
                json(200, "{}".into())
            }
            None => text(400, "not a report"),
        }
    }

    fn upload(&self, request: &mut Request<'_>) -> Resp {
        if let Err(wait) = self.uploads.take(&request.peer, 1.0) {
            return slow_down(wait, "the camera check is sending a lot at once: trying again shortly");
        }
        let kind = clean(request.query("kind").unwrap_or("?"), 16);
        let session = clean(request.query("session").unwrap_or("?"), 40);
        let content_type = clean(request.head.content_type.as_deref().unwrap_or("none"), 60);
        let summary = if kind == "loop" || kind == "frame" || kind == "photo" || kind == "recframe" || kind == "vframe" {
            let body = match request.body_within(MAX_PICTURES_BYTES, &self.inflight) {
                Ok(body) => body,
                Err(resp) => return resp,
            };
            pictures(&kind, &body)
        } else {
            let mut sniff = Sniff::default();
            match request.body_each(MAX_UPLOAD_BYTES, |chunk| sniff.feed(chunk)) {
                Ok(_) => sniff.summary(),
                Err(resp) => return resp,
            }
        };
        self.log(&format!("{session} upload {kind} ({content_type}): {summary}"));
        json(200, summary.serialize_json())
    }
}

impl Handler for CamTest {
    fn handle(&self, request: &mut Request<'_>) -> Resp {
        let route: Vec<&str> = request.route.iter().map(String::as_str).collect();
        match (request.method(), route.as_slice()) {
            (Method::Get | Method::Head, ["camtest"]) => html(PAGE),
            (Method::Post, ["camtest", "report"]) => match request.body(MAX_REPORT_BYTES) {
                Ok(body) => self.report(request.peer, &body),
                Err(resp) => resp,
            },
            (Method::Post, ["camtest", "upload"]) => self.upload(request),
            _ => text(404, "not found"),
        }
    }
}

/// A report as one log line: its session, test, verdict, detail and browser,
/// each cut short, the whole at most [`MAX_LINE_CHARS`].
fn report_line(body: &[u8]) -> Option<String> {
    let report = std::str::from_utf8(body).ok().and_then(|t| Report::deserialize_json(t).ok())?;
    let line = format!(
        "{} {} {} {}{}",
        clean(&report.session, 40),
        clean(&report.test, 60),
        if report.ok { "OK" } else { "FAIL" },
        clean(&report.detail, 200),
        report.ua.map(|ua| format!(" [{}]", clean(&ua, 160))).unwrap_or_default()
    );
    Some(line.chars().take(MAX_LINE_CHARS).collect())
}

/// Log-safe text: printable ASCII on one line, at most `max` characters.
fn clean(text: &str, max: usize) -> String {
    text.chars().take(max).map(|c| if c == '\n' { '|' } else if (' '..='~').contains(&c) { c } else { '?' }).collect()
}

fn pictures(kind: &str, body: &[u8]) -> String {
    let frames = if kind == "loop" {
        match jpeg::split_frames(body) {
            Ok(frames) => frames,
            Err(e) => return format!("{} bytes, {e}", body.len()),
        }
    } else {
        vec![body]
    };
    let infos: Vec<_> = frames.iter().map(|f| jpeg::info(f)).collect();
    let good = infos.iter().filter(|i| i.is_ok()).count();
    let metadata = infos.iter().filter(|i| matches!(i, Ok(info) if info.had_metadata)).count();
    let size = infos.iter().find_map(|i| i.as_ref().ok()).map(|i| format!("{}x{}", i.width, i.height)).unwrap_or_else(|| "-".into());
    let first_error = infos.iter().find_map(|i| i.as_ref().err()).map(|e| format!(", first bad: {e}")).unwrap_or_default();
    format!(
        "{} bytes, {good}/{} JPEG frames {size}, {metadata} with metadata{first_error}",
        body.len(),
        frames.len()
    )
}

/// What a video upload is, from its bytes: the container's first box and the
/// codec tags it holds.
#[derive(Default)]
struct Sniff {
    bytes: u64,
    head: Vec<u8>,
    /// The last few bytes of the previous chunk, so a tag split between two
    /// chunks is still found.
    carry: Vec<u8>,
    tags: Vec<&'static str>,
}

impl Sniff {
    const TAGS: [&'static str; 8] = ["avc1", "hvc1", "hev1", "av01", "vp09", "mp4a", "Opus", "V_VP8"];

    fn feed(&mut self, chunk: &[u8]) {
        self.bytes += chunk.len() as u64;
        if self.head.len() < 64 {
            let take = (64 - self.head.len()).min(chunk.len());
            self.head.extend_from_slice(&chunk[..take]);
        }
        let mut window = std::mem::take(&mut self.carry);
        window.extend_from_slice(chunk);
        for tag in Self::TAGS {
            if !self.tags.contains(&tag) && window.windows(tag.len()).any(|w| w == tag.as_bytes()) {
                self.tags.push(tag);
            }
        }
        let keep = window.len().min(8);
        self.carry = window[window.len() - keep..].to_vec();
    }

    fn summary(&self) -> String {
        let h = &self.head;
        let container = if h.len() >= 12 && &h[4..8] == b"ftyp" {
            let brand: String = h[8..12].iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' }).collect();
            format!("ISO-BMFF brand '{brand}'")
        } else if h.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            "Matroska/WebM".to_string()
        } else if h.is_empty() {
            "empty".to_string()
        } else {
            format!("unknown, starts {:02x?}", &h[..h.len().min(8)])
        };
        format!("{} bytes, {container}, tags [{}]", self.bytes, self.tags.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_names_containers_and_tags_split_across_chunks() {
        let mut mp4 = Sniff::default();
        mp4.feed(b"\x00\x00\x00\x18ftypqt  \x00\x00\x00\x00 moov ... hv");
        mp4.feed(b"c1 ... mp4a");
        let s = mp4.summary();
        assert!(s.contains("brand 'qt  '") && s.contains("hvc1") && s.contains("mp4a"), "{s}");
        let mut webm = Sniff::default();
        webm.feed(&[0x1A, 0x45, 0xDF, 0xA3, 0x01]);
        assert!(webm.summary().contains("Matroska/WebM"));
    }

    /// The log is the box's alone: no route reads it, a report adds one
    /// bounded line without the phone's address, an address that sends too
    /// many is slowed, and everyone together cannot flood it.
    #[test]
    fn reports_reach_only_the_box_log_and_only_so_many() {
        let long = "x".repeat(5000);
        let body = format!(r#"{{"session":"s1","test":"camera","ok":true,"detail":"{long}\nmore","ua":"{long}"}}"#);
        let line = report_line(body.as_bytes()).unwrap();
        assert!(line.chars().count() <= MAX_LINE_CHARS && !line.contains('\n'), "one bounded line: {} chars", line.len());
        assert!(line.starts_with("s1 camera OK x"), "{line}");
        assert!(report_line(b"not json").is_none());

        let camtest = CamTest::new(Arc::new(ByteBudget::new(1 << 20)));
        let (a, b): (IpAddr, IpAddr) = ("192.0.2.1".parse().unwrap(), "192.0.2.2".parse().unwrap());
        let report = br#"{"session":"s1","test":"camera","ok":true,"detail":"fine"}"#;
        for _ in 0..60 {
            assert_eq!(camtest.report(a, report).status, 200);
        }
        assert_eq!(camtest.report(a, report).status, 429, "one address is slowed past its burst");
        assert_eq!(camtest.report(b, report).status, 200, "another is not");

        let mut quiet = CamTest::new(Arc::new(ByteBudget::new(1 << 20)));
        quiet.logged = RateLimiter::new(2.0, 0.001);
        for i in 0..5u8 {
            assert_eq!(quiet.report(IpAddr::from([192, 0, 2, 10 + i]), report).status, 200, "a report past the log's rate is still taken");
        }
        assert_eq!(quiet.unlogged.load(Ordering::Relaxed), 3, "and counted instead of logged");

        // Over real TLS: the log route is gone; pictures wait for room.
        let tiny = CamTest::new(Arc::new(ByteBudget::new(100)));
        let (mut server, dir) = crate::server::tests::start("abcdefgh", Arc::new(tiny));
        let ask = |raw: String| String::from_utf8_lossy(&crate::server::tests::exchange_bytes(server.port, raw.as_bytes())).into_owned();
        let log = ask("GET /p/abcdefgh/camtest/log HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n".into());
        assert!(log.starts_with("HTTP/1.1 404"), "no route reads the log: {log}");
        let photo = vec![0u8; 1000];
        let mut raw = format!("POST /p/abcdefgh/camtest/upload?kind=photo HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", photo.len()).into_bytes();
        raw.extend_from_slice(&photo);
        let busy = String::from_utf8_lossy(&crate::server::tests::exchange_bytes(server.port, &raw)).into_owned();
        assert!(busy.starts_with("HTTP/1.1 503") && busy.contains("Retry-After: 5"), "no room: come back shortly: {busy}");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uploads_are_summed_up_in_printable_lines() {
        let frames = vec![jpeg::tests::sample(512, 288), jpeg::strip_metadata(&jpeg::tests::sample(512, 288)).unwrap()];
        let s = pictures("loop", &jpeg::tests::pack(&frames));
        assert!(s.contains("2/2 JPEG frames 512x288, 1 with metadata"), "{s}");
        assert!(pictures("photo", b"nope").contains("0/1"));

        let mut sniff = Sniff::default();
        sniff.feed(b"\0\0\0\x18ftyp\x1b[2Jmp42");
        assert!(sniff.summary().contains("brand '?[2J'"), "a brand is logged in printable characters only: {}", sniff.summary());
    }
}
