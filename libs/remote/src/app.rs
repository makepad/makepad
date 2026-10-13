//! Driving an app's remote commands from a command line: `makepad-remote app <pid|app|socket>
//! <command> [k=v ...]` and `makepad inspect <pid> <command> ...` are two front ends that only
//! parse argv and call [`run`]. Every command is a `cmd` request; the ones that are images are
//! answered here, client side, from the `frames` service, and the client writes the PNG files
//! (default folder `$TMPDIR/makepad-remote/<app>-<pid>/`) and prints their paths:
//!
//! - `grab [w=ID] [scale=F] [out=FILE]`, `tweak grab ...`: one frame
//! - `gseq [n=8] [every_ms=50] [scale=F] [w=ID] [out=DIR]`: n frames at a cadence
//! - `gq [scale=F] [w=ID] [if_user_seq=N]`: a frame of every window, then `quit`
//! - `tweak final`: the app's answer, plus `"png"` (the composited window) when the user drew
//! - `tools`: the app's AI services and their tools (the `app` service's `list`)
//! - `call <service>.<tool> [<instance>] [<json args>]`: one AI tool call (progress lines, then
//!   the result)

use crate::discovery;
use crate::frames::{self, Record};
use crate::link::{Client, Error};
use std::path::{Path, PathBuf};

/// Frames of one folder kept per window (older ones are deleted when a new one is written).
const KEPT_PER_WINDOW: usize = 64;

/// Words (the command, its sub-command) and `k=v` arguments of an argv.
pub fn split_args(argv: &[String]) -> (Vec<String>, Vec<(String, String)>) {
    let (mut words, mut kv) = (Vec::new(), Vec::new());
    for a in argv {
        match a.split_once('=') {
            Some((k, v)) if !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) => kv.push((k.trim_start_matches("--").to_string(), v.to_string())),
            _ => words.push(a.clone()),
        }
    }
    (words, kv)
}

/// The request line for the `cmd` service: one flat JSON object, so values keep their spaces.
pub fn request_line(words: &[String], kv: &[(String, String)]) -> String {
    let mut line = format!("{{\"cmd\":{}", crate::json_str(&words.join(" ")));
    for (k, v) in kv {
        // `cmd` is the request's own field; the Command-key modifier of an input command goes by `logo`
        let k = if k == "cmd" { "logo" } else { k };
        line.push_str(&format!(",{}:{}", crate::json_str(k), crate::json_str(v)));
    }
    line.push('}');
    line
}

fn get<'a>(kv: &'a [(String, String)], keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| kv.iter().find(|x| x.0 == *k).map(|x| x.1.as_str()))
}

/// Like [`run`] for a request typed as one line (words split on spaces).
pub fn run_line(client: &Client, line: &str, each: &mut dyn FnMut(&str) -> bool) -> Result<bool, Error> {
    let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    run(client, &argv, each)
}

/// Runs one command: `each` gets every answer line (the `end` line left out; false stops the
/// reading). Ok(true) when the answer had an error.
pub fn run(client: &Client, argv: &[String], each: &mut dyn FnMut(&str) -> bool) -> Result<bool, Error> {
    run_inner(client, argv, each, false).map(|r| r.0)
}

/// As [`run`], also returning the `end` line of the answer (it carries the fields the app adds,
/// such as `"user_seq"`). The client-side commands have no end line of their own: the line of
/// a `ping` that follows them stands in, and `gq` has none (the app is gone).
pub fn run_end(client: &Client, argv: &[String], each: &mut dyn FnMut(&str) -> bool) -> Result<(bool, Option<String>), Error> {
    run_inner(client, argv, each, true)
}

fn run_inner(client: &Client, argv: &[String], each: &mut dyn FnMut(&str) -> bool, want_end: bool) -> Result<(bool, Option<String>), Error> {
    let (words, kv) = split_args(argv);
    let first = words.first().map(String::as_str).unwrap_or("help");
    let second = words.get(1).map(String::as_str);
    let reply = match (first, second) {
        ("grab", _) | ("tweak", Some("grab")) => grab(client, &kv),
        ("gseq", _) => gseq(client, &kv),
        ("rec", _) => rec(client, &kv),
        ("gq", _) => gq(client, &kv),
        ("tweak", Some("final")) => return tweak_final(client, &words, &kv, each),
        ("tools", _) => return app_request(client, "list", each),
        ("call", _) => {
            let Some(tool) = second else {
                return Ok((emit(Err("call <service>.<tool> [<instance>] [<json args>]".into()), each), None));
            };
            let rest = &words[2..];
            let (instance, args) = match rest.last() {
                Some(last) if last.trim_start().starts_with('{') => (&rest[..rest.len() - 1], last.as_str()),
                _ => (rest, "{}"),
            };
            let head = if instance.is_empty() { format!("call {tool}") } else { format!("call {tool} {}", instance.join(" ")) };
            return app_request(client, &format!("{head}\n{args}"), each);
        }
        _ => return forward(client, &words, &kv, each),
    };
    let err = emit(reply, each);
    let end = if want_end && first != "gq" { ask_end(client, "ping") } else { None };
    Ok((err, end))
}

fn emit(reply: Result<String, String>, each: &mut dyn FnMut(&str) -> bool) -> bool {
    match reply {
        Ok(json) => {
            each(&json);
            json.starts_with("{\"err\":")
        }
        Err(msg) => {
            each(&format!("{{\"err\":{}}}", crate::json_str(&msg)));
            true
        }
    }
}

fn forward(client: &Client, words: &[String], kv: &[(String, String)], each: &mut dyn FnMut(&str) -> bool) -> Result<(bool, Option<String>), Error> {
    let line = if words.is_empty() { "help".to_string() } else { request_line(words, kv) };
    let mut err = false;
    let mut lines = client.cmd(&line)?;
    while let Some(l) = lines.next() {
        let l = l?;
        err |= l.starts_with("{\"err\":");
        if !each(&l) {
            break;
        }
    }
    Ok((err, lines.end().map(str::to_string)))
}

/// One request of the `app` service (the app's AI tools): its answer lines, as they come. The
/// channel's end is the end of the answer; dropping it early cancels a call.
fn app_request(client: &Client, args: &str, each: &mut dyn FnMut(&str) -> bool) -> Result<(bool, Option<String>), Error> {
    use crate::conn::{Event, CLOSE_LINK_LOST, CLOSE_NORMAL};
    let ch = match client.open("app", args) {
        Ok(ch) => ch,
        Err(Error::Refused(_, why)) if why.starts_with("unsupported service") => {
            return Ok((emit(Err("this process serves no AI tools (no `app` service)".into()), each), None));
        }
        Err(e) => return Err(e),
    };
    let mut err = false;
    while let Some(ev) = ch.recv() {
        match ev {
            Event::Data(d) => {
                for line in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    if !each(line) {
                        return Ok((err, None));
                    }
                }
            }
            Event::Closed(CLOSE_NORMAL, _) => break,
            Event::Closed(CLOSE_LINK_LOST, why) => return Err(Error::LinkLost(why)),
            Event::Closed(_, why) => {
                err = true;
                each(&format!("{{\"err\":{}}}", crate::json_str(&why)));
                break;
            }
            Event::Opened(_) => {}
        }
    }
    Ok((err, None))
}

/// The end line of a command's answer.
fn ask_end(client: &Client, line: &str) -> Option<String> {
    let mut lines = client.cmd(line).ok()?;
    for l in lines.by_ref() {
        l.ok()?;
    }
    lines.end().map(str::to_string)
}

/// The one answer line of a command (the first line), for the commands this module chains.
fn ask_one(client: &Client, line: &str) -> Result<String, String> {
    let mut lines = client.cmd(line).map_err(|e| e.to_string())?;
    let first = lines.next().transpose().map_err(|e| e.to_string())?.unwrap_or_default();
    for l in lines {
        l.map_err(|e| e.to_string())?;
    }
    Ok(first)
}

fn window_of(kv: &[(String, String)]) -> Option<usize> {
    get(kv, &["w", "window"]).and_then(|v| v.parse().ok())
}

fn scale_of(kv: &[(String, String)]) -> f64 {
    get(kv, &["scale"]).and_then(|v| v.parse().ok()).unwrap_or(1.0)
}

/// `$TMPDIR/makepad-remote/<app>-<pid>/`, created.
fn grab_dir(client: &Client) -> Result<PathBuf, String> {
    let ping = ask_one(client, "ping").unwrap_or_default();
    let f = discovery::fields(&ping);
    let field = |k: &str| f.iter().find(|x| x.0 == k).map(|x| x.1.clone());
    let app = field("app").and_then(|r| discovery::string(r.as_bytes(), &mut 0)).unwrap_or_else(|| "app".into());
    let name = match field("pid") {
        Some(pid) => format!("{app}-{pid}"),
        None => app,
    };
    let dir = std::env::temp_dir().join("makepad-remote").join(name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("grab dir: {e}"))?;
    Ok(dir)
}

/// Writes `png` as the next `grab-w<window>-<seq>.png` of `dir` and deletes the older ones past
/// [`KEPT_PER_WINDOW`], except the paths in `pinned`.
fn write_grab(dir: &Path, window: usize, png: &[u8], pinned: &[PathBuf]) -> Result<PathBuf, String> {
    let prefix = format!("grab-w{window}-");
    let existing = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix))).collect())
            .unwrap_or_default()
    };
    let seq = |p: &Path| p.file_stem().and_then(|n| n.to_str()).and_then(|n| n.rsplit('-').next()).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
    let mut next = existing(dir).iter().map(|p| seq(p)).max().unwrap_or(0) + 1;
    let path = loop {
        let path = dir.join(format!("{prefix}{next:05}.png"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => break path,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => next += 1,
            Err(e) => return Err(format!("grab write: {e}")),
        }
    };
    std::fs::write(&path, png).map_err(|e| format!("grab write: {e}"))?;
    let mut mine: Vec<PathBuf> = existing(dir).into_iter().filter(|p| !pinned.contains(p)).collect();
    if mine.len() > KEPT_PER_WINDOW {
        mine.sort();
        let drop = mine.len() - KEPT_PER_WINDOW;
        for old in mine.into_iter().take(drop) {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(path)
}

/// `{"png":path,"w":window,"sz":[w,h],"capture_ms":..,"encode_ms":..,"capture_kind":..}`
fn grab_json(r: &Record, path: &Path) -> String {
    format!(
        "{{\"png\":{},\"w\":{},\"sz\":[{},{}],\"capture_ms\":{:.3},\"encode_ms\":{:.3},\"capture_kind\":{}}}",
        crate::json_str(&path.display().to_string()),
        r.num("window").unwrap_or(0.0),
        r.num("w").unwrap_or(0.0),
        r.num("h").unwrap_or(0.0),
        r.num("capture_ms").unwrap_or(0.0),
        r.num("encode_ms").unwrap_or(0.0),
        crate::json_str(&r.text("capture_kind").unwrap_or_default()),
    )
}

/// One frame of `window`, written to `out` or the next file of the grab folder.
fn grab_one(client: &Client, window: Option<usize>, scale: f64, out: Option<&str>) -> Result<(Record, PathBuf), String> {
    let req = frames::Request { window, scale: Some(scale), raw: false, codec: None, n: Some(1), fps: None, cap: false };
    let mut got = None;
    frames::fetch(client, &req, &mut |r| {
        got = Some(r);
        Ok(())
    })?;
    let record = got.ok_or("the app sent no frame")?;
    let path = match out {
        Some(file) => {
            std::fs::write(file, &record.bytes).map_err(|e| format!("grab write: {e}"))?;
            PathBuf::from(file)
        }
        None => write_grab(&grab_dir(client)?, record.num("window").unwrap_or(0.0) as usize, &record.bytes, &[])?,
    };
    Ok((record, path))
}

fn grab(client: &Client, kv: &[(String, String)]) -> Result<String, String> {
    let (record, path) = grab_one(client, window_of(kv), scale_of(kv), get(kv, &["out"]))?;
    Ok(grab_json(&record, &path))
}

/// `rec [out=DIR] [seconds=5] [fps=30] [codec=png|raw] [w=ID]`: capture by streaming. The app reads every
/// presented frame back and sends it here; this side writes `frame_<slot>.png` (or `.rgba`) into DIR and
/// counts the slots that never arrived, so a capture is checkable (`frames`, `missing`) and needs no
/// video encoder on the device. Encode the folder to a video with any encoder.
fn rec(client: &Client, kv: &[(String, String)]) -> Result<String, String> {
    let seconds: f64 = get(kv, &["seconds", "s"]).unwrap_or("5").parse().map_err(|_| "rec seconds must be a number".to_string())?;
    let fps: f64 = get(kv, &["fps"]).unwrap_or("30").parse().map_err(|_| "rec fps must be a number".to_string())?;
    let codec = get(kv, &["codec"]).unwrap_or("rle");
    if !matches!(codec, "rle" | "png" | "raw") { return Err("rec codec must be rle, png or raw".into()); }
    let raw = codec == "raw";
    let dir = match get(kv, &["out"]) {
        Some(d) => { std::fs::create_dir_all(d).map_err(|e| format!("rec out: {e}"))?; PathBuf::from(d) }
        None => grab_dir(client)?,
    };
    let req = frames::Request { window: window_of(kv), scale: Some(get(kv, &["scale"]).and_then(|v| v.parse().ok()).unwrap_or(0.5)), raw, codec: (codec == "rle").then_some("rle"), n: None, fps: Some(fps), cap: true };
    let started = std::time::Instant::now();
    let (mut count, mut first, mut last, mut size, mut dropped) = (0u64, None::<u64>, 0u64, (0u32, 0u32), 0u64);
    let (mut t_first, mut t_last) = (None::<f64>, 0f64);
    let result = frames::fetch(client, &req, &mut |r| {
        let slot = r.num("seq").unwrap_or(0.0) as u64;
        first.get_or_insert(slot);
        last = slot;
        size = (r.num("w").unwrap_or(0.0) as u32, r.num("h").unwrap_or(0.0) as u32);
        dropped = r.num("dropped").unwrap_or(0.0) as u64;
        let t = r.num("t_ns").unwrap_or(0.0);
        t_first.get_or_insert(t);
        t_last = t;
        let (name, bytes) = match codec {
            "rle" => (format!("frame_{slot:06}.bmp"), rle_to_bmp(size.0, size.1, &r.bytes)?),
            "raw" => (format!("frame_{slot:06}.rgba"), r.bytes.clone()),
            _ => (format!("frame_{slot:06}.png"), r.bytes.clone()),
        };
        std::fs::write(dir.join(name), bytes).map_err(|e| format!("rec write: {e}"))?;
        count += 1;
        if started.elapsed().as_secs_f64() >= seconds { return Err("\u{0}done".into()); }
        Ok(())
    });
    match result {
        Ok(()) => {}
        Err(e) if e == "\u{0}done" => {}
        Err(e) => return Err(format!("rec: {e}")),
    }
    let span = first.map_or(0, |f| last - f + 1);
    Ok(format!("{{\"dir\":{},\"frames\":{count},\"missing\":{},\"slots\":{span},\"fps\":{fps},\"sz\":[{},{}],\"device_dropped\":{dropped},\"device_ms\":{:.0}}}", crate::json_str(&dir.display().to_string()), span.saturating_sub(count), size.0, size.1, (t_last - t_first.unwrap_or(t_last)) / 1e6))
}

/// Run-length RGBA (see the app's `rle_encode`) to a 32-bit top-down BMP.
fn rle_to_bmp(w: u32, h: u32, rle: &[u8]) -> Result<Vec<u8>, String> {
    let mut px = Vec::with_capacity(w as usize * h as usize * 4);
    let mut i = 0;
    while i + 6 <= rle.len() {
        let run = u16::from_le_bytes([rle[i], rle[i + 1]]) as usize;
        let (r, g, b, a) = (rle[i + 2], rle[i + 3], rle[i + 4], rle[i + 5]);
        for _ in 0..run { px.extend_from_slice(&[b, g, r, a]); }
        i += 6;
    }
    if px.len() != w as usize * h as usize * 4 { return Err(format!("rec: a frame decoded to {} bytes, expected {}", px.len(), w as usize * h as usize * 4)); }
    let mut out = Vec::with_capacity(54 + px.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    out.extend_from_slice(&px);
    Ok(out)
}

fn gseq(client: &Client, kv: &[(String, String)]) -> Result<String, String> {
    let n = match get(kv, &["n"]).unwrap_or("8").parse::<usize>() {
        Ok(n @ 1..=64) => n,
        _ => return Err("gseq n must be an integer in 1..=64".into()),
    };
    let every_ms = match get(kv, &["every_ms"]).unwrap_or("50").parse::<u64>() {
        Ok(ms @ 8..=60_000) if ms * (n as u64 - 1) <= 60_000 => ms,
        _ => return Err("gseq every_ms must be 8..=60000; total cadence span must not exceed 60s".into()),
    };
    let dir = match get(kv, &["out"]) {
        Some(d) => {
            std::fs::create_dir_all(d).map_err(|e| format!("gseq out: {e}"))?;
            PathBuf::from(d)
        }
        None => grab_dir(client)?,
    };
    let req = frames::Request { window: window_of(kv), scale: Some(scale_of(kv)), raw: false, codec: None, n: Some(n), fps: Some(1000.0 / every_ms as f64), cap: false };
    let (mut paths, mut frame_json, mut pinned) = (Vec::new(), Vec::new(), Vec::new());
    frames::fetch(client, &req, &mut |r| {
        let index = frame_json.len();
        let path = write_grab(&dir, r.num("window").unwrap_or(0.0) as usize, &r.bytes, &pinned)?;
        paths.push(crate::json_str(&path.display().to_string()));
        let mut frame = grab_json(&r, &path);
        frame.pop();
        frame.push_str(&format!(",\"scheduled_ms\":{},\"pixels_ms\":{:.3}}}", index as u64 * every_ms, index as f64 * every_ms as f64 + r.num("capture_ms").unwrap_or(0.0)));
        frame_json.push(frame);
        pinned.push(path);
        Ok(())
    })
    .map_err(|e| format!("gseq frame {}: {e}", frame_json.len()))?;
    Ok(format!("{{\"png\":[{}],\"n\":{n},\"every_ms\":{every_ms},\"frames\":[{}]}}", paths.join(","), frame_json.join(",")))
}

/// The window ids of a `status` answer (`"w":[{"i":ID,...},...]`).
pub fn window_ids(status: &str) -> Vec<usize> {
    let Some(start) = status.find("\"w\":[") else { return Vec::new() };
    let b = status.as_bytes();
    let (mut i, mut depth, mut ids) = (start + 5, 1i32, Vec::new());
    while i < b.len() && depth > 0 {
        match b[i] {
            b'"' => {
                // a string: skip it (it may hold anything)
                let begin = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                // `"i":` directly inside a window object (depth 2)
                if depth == 2 && &status[begin..=i.min(b.len() - 1)] == "\"i\"" && b.get(i + 1) == Some(&b':') {
                    let digits: String = status[i + 2..].chars().take_while(char::is_ascii_digit).collect();
                    if let Ok(id) = digits.parse() {
                        ids.push(id);
                    }
                }
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    ids
}

/// A frame of every window (or `w=`), then `quit`: the last call of a session.
fn gq(client: &Client, kv: &[(String, String)]) -> Result<String, String> {
    let scale = scale_of(kv);
    let targets = match window_of(kv) {
        Some(w) => vec![w],
        None => window_ids(&ask_one(client, "status").unwrap_or_default()),
    };
    let (mut paths, mut problems) = (Vec::new(), Vec::new());
    for window in targets {
        match grab_one(client, Some(window), scale, None) {
            Ok((_, path)) => paths.push(crate::json_str(&path.display().to_string())),
            Err(msg) => problems.push(format!("w{window}: {msg}")),
        }
    }
    // Capture failure still permits cleanup, but human intervention does not.
    // a driver that pinned the user sequence passes it on: quit is refused once the person intervened
    let quit = match get(kv, &["if_user_seq"]) {
        Some(seq) => request_line(&["quit".to_string()], &[("if_user_seq".to_string(), seq.to_string())]),
        None => "quit".to_string(),
    };
    let reply = ask_one(client, &quit).unwrap_or_else(|e| format!("{{\"err\":{}}}", crate::json_str(&e)));
    if reply.starts_with("{\"err\":") {
        // the person intervened (or the app refused): nothing was quit
        return Ok(reply);
    }
    let mut out = format!("{{\"png\":[{}],\"quit\":{}", paths.join(","), if reply.starts_with("{\"ok\":1") { 1 } else { 0 });
    if !problems.is_empty() {
        out.push_str(&format!(",\"err\":{}", crate::json_str(&problems.join("; "))));
    }
    out.push('}');
    Ok(out)
}

/// The app's `tweak final`; when it says the user drew (`"drew":1`) a frame of the window is
/// written too and named in the same JSON.
fn tweak_final(client: &Client, words: &[String], kv: &[(String, String)], each: &mut dyn FnMut(&str) -> bool) -> Result<(bool, Option<String>), Error> {
    let mut lines = Vec::new();
    let mut err = false;
    let mut answer = client.cmd(&request_line(words, kv))?;
    while let Some(l) = answer.next() {
        let l = l?;
        err |= l.starts_with("{\"err\":");
        lines.push(l);
    }
    let end = answer.end().map(str::to_string);
    if let Some(json) = lines.first_mut() {
        if json.contains("\"drew\":1") && json.ends_with('}') {
            json.pop();
            match grab_one(client, window_of(kv), scale_of(kv), None) {
                Ok((_, path)) => json.push_str(&format!(",\"png\":{}}}", crate::json_str(&path.display().to_string()))),
                Err(msg) => json.push_str(&format!(",\"png_err\":{}}}", crate::json_str(&msg))),
            }
        }
    }
    for l in &lines {
        if !each(l) {
            break;
        }
    }
    Ok((err, end))
}
