//! The real-GPU app mode of the web test target: a built wasm app in headless CEF, driven by
//! steps (see the crate's module comment). The page side is `rig.js`.

use crate::{serve, start_cef, with_cef_profile, Site};
use makepad_cef::Browser;
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const RIG: &str = include_str!("rig.js");
pub const RIG_PATH: &str = "/.web-test/rig.js";

/// How long one step, or the app's start, may take before the run gives up on it.
const STEP_TIMEOUT: Duration = Duration::from_secs(120);

// Reports from the page: 0 is a page error, 1 the GPU description, 2 "rig loaded"; steps from 3.
const REPORT_ERROR: usize = 0;
const REPORT_DEVICE: usize = 1;
const REPORT_READY: usize = 2;
const FIRST_STEP: usize = 3;

const KEY_RAWKEYDOWN: i32 = 0;
const KEY_KEYUP: i32 = 2;
const KEY_CHAR: i32 = 3;

struct Options {
    app: PathBuf,
    out: PathBuf,
    size: (usize, usize),
    steps: Vec<Vec<String>>,
}

fn parse(argv: &[String]) -> Result<Options, String> {
    let (mut app, mut out, mut size, mut steps) = (None, None, (800, 600), None);
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--app" => app = Some(PathBuf::from(value("--app")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--size" => {
                let text = value("--size")?;
                let (w, h) = text.split_once('x').ok_or_else(|| format!("--size {text:?}: want WxH"))?;
                let dim = |s: &str| s.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| format!("--size {text:?}: want WxH"));
                size = (dim(w)?, dim(h)?);
            }
            "--steps" => steps = Some(value("--steps")?),
            // libtest's flags that cargo may pass
            _ => {}
        }
    }
    let steps: Vec<Vec<String>> = steps
        .ok_or("--steps is required")?
        .split(';')
        .map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|words| !words.is_empty())
        .collect();
    if steps.is_empty() {
        return Err("--steps names no step".into());
    }
    for step in &steps {
        check_step(step)?;
    }
    Ok(Options { app: app.ok_or("--app is required")?, out: out.ok_or("--out is required")?, size, steps })
}

fn check_step(step: &[String]) -> Result<(), String> {
    let want = match step[0].as_str() {
        "info" | "settle" | "lose" | "restore" => 0,
        "frames" | "key" | "grab" | "stats" => 1,
        "click" | "differ" | "same" => 2,
        "wheel" => 4,
        other => return Err(format!("unknown step {other:?}")),
    };
    let given = step.len() - 1;
    let ok = if step[0] == "same" { given == 2 || given == 3 } else { given == want };
    if !ok {
        return Err(format!("step {:?}: wrong number of arguments", step.join(" ")));
    }
    let number = |s: &String| s.parse::<i32>().map_err(|_| format!("step {:?}: {s:?} is not a number", step.join(" ")));
    match step[0].as_str() {
        "frames" => {
            number(&step[1])?;
        }
        "click" | "wheel" => {
            for arg in &step[1..] {
                number(arg)?;
            }
        }
        "grab" | "stats" => name_ok(&step[1])?,
        "same" | "differ" => {
            name_ok(&step[1])?;
            name_ok(&step[2])?;
            if let Some(tolerance) = step.get(3) {
                tolerance.parse::<u8>().map_err(|_| format!("tolerance {tolerance:?} is not 0..255"))?;
            }
        }
        "key" => key_codes(&step[1]).map(|_| ())?,
        _ => {}
    }
    Ok(())
}

fn name_ok(name: &str) -> Result<(), String> {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        Ok(())
    } else {
        Err(format!("{name:?}: a name is letters, digits, _ and -"))
    }
}

/// (Windows key code, character) of a key name or a single ASCII character.
fn key_codes(name: &str) -> Result<(i32, Option<u16>), String> {
    Ok(match name {
        "Enter" => (13, Some(13)),
        "Space" => (32, Some(32)),
        "Tab" => (9, None),
        "Escape" => (27, None),
        "Backspace" => (8, None),
        "Left" => (37, None),
        "Up" => (38, None),
        "Right" => (39, None),
        "Down" => (40, None),
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => (c.to_ascii_uppercase() as i32, Some(c as u16)),
                _ => return Err(format!("unknown key {name:?}")),
            }
        }
    })
}

/// A grab: top-down RGBA.
struct Grab {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

struct Session {
    browser: Browser,
    reports: mpsc::Receiver<(usize, Vec<u8>)>,
    device: Option<String>,
    ready: bool,
}

impl Session {
    /// Runs the browser until report `want` arrives. A page error or a console error ends it.
    fn wait(&mut self, want: Option<usize>, label: &str) -> Result<Vec<u8>, String> {
        let started = Instant::now();
        loop {
            makepad_cef::do_message_loop_work();
            if let Some(error) = self.browser.take_error() {
                return Err(format!("the browser failed: {error}"));
            }
            let errors: Vec<_> = self.browser.take_console_messages().into_iter().filter(|m| m.level >= 4)
                .map(|m| format!("{} ({}:{})", m.message, m.source, m.line)).collect();
            if !errors.is_empty() {
                return Err(format!("console error: {}", errors.join("\n")));
            }
            while let Ok((id, body)) = self.reports.try_recv() {
                match id {
                    REPORT_ERROR => return Err(format!("page error: {}", String::from_utf8_lossy(&body))),
                    REPORT_DEVICE => self.device = Some(String::from_utf8_lossy(&body).to_string()),
                    REPORT_READY => self.ready = true,
                    id if Some(id) == want => return Ok(body),
                    _ => {}
                }
            }
            if want.is_none() && self.ready {
                return Ok(Vec::new());
            }
            if started.elapsed() > STEP_TIMEOUT {
                return Err(format!("{label}: no answer within {} s", STEP_TIMEOUT.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A step the page runs: its command and count in, its report out.
    fn page(&mut self, id: usize, command: &str, count: i32) -> Result<Vec<u8>, String> {
        self.browser.execute_javascript(&format!("makepadRigStep({id}, \"{command}\", {count});")).map_err(|e| e.to_string())?;
        self.wait(Some(id), command)
    }
}

fn flip(width: usize, height: usize, bottom_up: &[u8]) -> Vec<u8> {
    let row = width * 4;
    let mut out = Vec::with_capacity(bottom_up.len());
    for y in (0..height).rev() {
        out.extend_from_slice(&bottom_up[y * row..(y + 1) * row]);
    }
    out
}

fn decode_grab(body: &[u8]) -> Result<Grab, String> {
    if body.len() < 8 {
        return Err("a grab report is too short".into());
    }
    let width = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(body[4..8].try_into().unwrap()) as usize;
    if width == 0 || height == 0 || body.len() != 8 + width * height * 4 {
        return Err(format!("a grab report of {} bytes does not fit {width}x{height}", body.len()));
    }
    Ok(Grab { width, height, rgba: flip(width, height, &body[8..]) })
}

fn encode_png(grab: &Grab) -> Result<Vec<u8>, String> {
    use makepad_zune_png::{
        makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
        PngEncoder,
    };
    let options = EncoderOptions::default()
        .set_width(grab.width)
        .set_height(grab.height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    let mut out = Vec::new();
    PngEncoder::new(&grab.rgba, options).encode(&mut out).map_err(|e| format!("png encode failed: {e:?}"))?;
    Ok(out)
}

/// RGB compare, alpha ignored: (largest channel difference, differing pixels over the tolerance
/// is what `over` counts, first such pixel).
struct Diff {
    largest: u8,
    differing: usize,
    over: usize,
    first_over: Option<(usize, usize)>,
}

fn compare(a: &Grab, b: &Grab, tolerance: u8) -> Result<Diff, String> {
    if (a.width, a.height) != (b.width, b.height) {
        return Err(format!("sizes differ: {}x{} and {}x{}", a.width, a.height, b.width, b.height));
    }
    let mut diff = Diff { largest: 0, differing: 0, over: 0, first_over: None };
    for (i, (p, q)) in a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)).enumerate() {
        let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0);
        if d > 0 {
            diff.differing += 1;
        }
        if d > tolerance {
            diff.over += 1;
            diff.first_over.get_or_insert((i % a.width, i / a.width));
        }
        diff.largest = diff.largest.max(d);
    }
    Ok(diff)
}

fn run_steps(session: &mut Session, options: &Options) -> Result<usize, String> {
    let mut grabs: HashMap<String, Grab> = HashMap::new();
    let mut failures = 0;
    let mut next_id = FIRST_STEP;
    let mut id = || {
        next_id += 1;
        next_id - 1
    };
    let text = |body: Vec<u8>| String::from_utf8_lossy(&body).to_string();
    for (n, step) in options.steps.iter().enumerate() {
        let label = step.join(" ");
        let arg = |i: usize| step[i].parse::<i32>().unwrap_or(0);
        let outcome: Result<String, String> = (|| match step[0].as_str() {
            "info" => session.page(id(), "info", 0).map(text),
            "frames" => session.page(id(), "frames", arg(1)).map(text),
            "settle" => session.page(id(), "settle", 0).map(text),
            "lose" => session.page(id(), "lose", 0).map(text),
            "restore" => session.page(id(), "restore", 0).map(text),
            "grab" => {
                let grab = decode_grab(&session.page(id(), "grab", 0)?)?;
                let path = options.out.join(format!("{}.png", step[1]));
                std::fs::write(&path, encode_png(&grab)?).map_err(|e| format!("{}: {e}", path.display()))?;
                let line = format!("{}x{} -> {}.png", grab.width, grab.height, step[1]);
                grabs.insert(step[1].clone(), grab);
                Ok(line)
            }
            "stats" => {
                let json = text(session.page(id(), "stats", 0)?);
                let value = JsonValue::deserialize_json(&json).map_err(|e| format!("stats are not JSON: {e:?}"))?;
                let frames = value.get("stats").and_then(|s| s.get("frames")).and_then(JsonValue::as_u64).unwrap_or(0);
                let path = options.out.join(format!("{}.json", step[1]));
                std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(format!("GPU frames={frames} -> {}.json", step[1]))
            }
            "click" => {
                let (x, y) = (arg(1), arg(2));
                let browser = &mut session.browser;
                browser.send_mouse_move(x, y, 0, false).map_err(|e| e.to_string())?;
                browser.send_mouse_click(x, y, 0, 0, false, 1).map_err(|e| e.to_string())?;
                browser.send_mouse_click(x, y, 0, 0, true, 1).map_err(|e| e.to_string())?;
                session.page(id(), "ack", 0).map(|_| format!("at {x},{y}"))
            }
            "wheel" => {
                let (x, y) = (arg(1), arg(2));
                session.browser.send_mouse_move(x, y, 0, false).map_err(|e| e.to_string())?;
                session.browser.send_mouse_wheel(x, y, 0, arg(3), arg(4)).map_err(|e| e.to_string())?;
                session.page(id(), "ack", 0).map(|_| format!("at {x},{y} by {},{}", arg(3), arg(4)))
            }
            "key" => {
                let (code, character) = key_codes(&step[1])?;
                let c = character.unwrap_or(0);
                let browser = &mut session.browser;
                browser.send_key_event(KEY_RAWKEYDOWN, 0, code, code, c, c, false).map_err(|e| e.to_string())?;
                if character.is_some() {
                    browser.send_key_event(KEY_CHAR, 0, code, code, c, c, false).map_err(|e| e.to_string())?;
                }
                browser.send_key_event(KEY_KEYUP, 0, code, code, c, c, false).map_err(|e| e.to_string())?;
                session.page(id(), "ack", 0).map(|_| step[1].clone())
            }
            "same" | "differ" => {
                let get = |name: &String| grabs.get(name).ok_or_else(|| format!("no grab named {name:?} yet"));
                let tolerance = step.get(3).map(|t| t.parse::<u8>().unwrap_or(0)).unwrap_or(0);
                let diff = compare(get(&step[1])?, get(&step[2])?, tolerance)?;
                let detail = format!("largest difference {}, {} differing pixels", diff.largest, diff.differing);
                if step[0] == "same" {
                    match diff.first_over {
                        None => Ok(detail),
                        Some((x, y)) => Err(format!("{detail}; {} over tolerance {tolerance}, first at {x},{y}", diff.over)),
                    }
                } else if diff.differing == 0 {
                    Err("the grabs are equal in RGB".into())
                } else {
                    Ok(detail)
                }
            }
            other => Err(format!("unknown step {other:?}")),
        })();
        match outcome {
            Ok(line) => println!("step {} {label}: ok: {line}", n + 1),
            Err(error) => {
                println!("step {} {label}: FAILED: {error}", n + 1);
                // A compare failure leaves the page sound, so later steps still tell something;
                // any other failure leaves the page in an unknown state.
                if !matches!(step[0].as_str(), "same" | "differ") {
                    return Err(format!("step {} {label}: {error}", n + 1));
                }
                failures += 1;
            }
        }
    }
    // A last round trip, so an error right after the final step is seen.
    session.page(id(), "ack", 0)?;
    Ok(failures)
}

/// `index.html` with the rig's script as the first element of `<head>`.
fn inject_rig(index: &str) -> Result<String, String> {
    let lower = index.to_ascii_lowercase();
    let head = lower.find("<head").ok_or("index.html has no <head>")?;
    let close = head + lower[head..].find('>').ok_or("index.html: unterminated <head>")? + 1;
    Ok(format!("{}<script src=\"{RIG_PATH}\"></script>{}", &index[..close], &index[close..]))
}

pub fn run(argv: &[String]) -> Result<(), String> {
    let options = parse(argv)?;
    let root = options.app.canonicalize().map_err(|e| format!("{}: {e}", options.app.display()))?;
    let index = std::fs::read_to_string(root.join("index.html")).map_err(|e| format!("{}/index.html: {e}", root.display()))?;
    let index = inject_rig(&index)?;
    std::fs::create_dir_all(&options.out).map_err(|e| format!("{}: {e}", options.out.display()))?;
    let (tx, reports) = mpsc::channel();
    let port = serve(root, Site::App { index }, tx)?;
    with_cef_profile(|| {
        start_cef()?;
        let url = format!("http://127.0.0.1:{port}/");
        let (w, h) = options.size;
        let browser = Browser::new(&url, w, h, 1.0).map_err(|e| format!("no browser: {e}"))?;
        let mut session = Session { browser, reports, device: None, ready: false };
        let _ = session.browser.set_focus(true);
        println!("     Running {} in CEF {} at {w}x{h}", options.app.display(), makepad_cef::cef_version());
        let result = session.wait(None, "the app's start").and_then(|_| run_steps(&mut session, &options));
        drop(session);
        match result {
            Ok(0) => {
                println!("web app test: {} steps ok", options.steps.len());
                Ok(())
            }
            Ok(failures) => Err(format!("{failures} compare step(s) failed")),
            Err(error) => Err(error),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rig_helpers() {
        // arguments and steps
        let ok = parse(&args(&["--app", "a", "--out", "o", "--size", "320x200", "--steps", "info; same a b 3; key Enter; click 1 2; ;"])).unwrap();
        assert_eq!(ok.size, (320, 200), "size parsed");
        assert_eq!(ok.steps.len(), 4, "empty steps are dropped");
        for bad in ["bogus", "frames", "grab ../x", "same a", "key F13", "same a b 300", "click 1 x"] {
            assert!(parse(&args(&["--app", "a", "--out", "o", "--steps", bad])).is_err(), "step {bad:?} must be refused");
        }
        assert!(parse(&args(&["--out", "o", "--steps", "info"])).is_err(), "--app is required");

        // the rig script goes first in <head>
        let page = inject_rig("<!doctype html><HTML><HEAD lang=x><title>t</title></HEAD></HTML>").unwrap();
        assert!(page.contains(&format!("<HEAD lang=x><script src=\"{RIG_PATH}\"></script><title>")), "rig is the first head element");
        assert!(inject_rig("<body></body>").is_err(), "a page without a head is refused");

        // grabs: bottom-up rows flip, and compare ignores alpha
        let mut body = vec![2, 0, 0, 0, 2, 0, 0, 0];
        body.extend_from_slice(&[1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255, 4, 4, 4, 255]);
        let a = decode_grab(&body).unwrap();
        assert_eq!(&a.rgba[..4], &[3, 3, 3, 255], "top row is the last one read");
        assert!(decode_grab(&body[..10]).is_err(), "a short grab is refused");
        let mut b = Grab { width: a.width, height: a.height, rgba: a.rgba.clone() };
        b.rgba[3] = 0;
        b.rgba[4] += 5;
        let diff = compare(&a, &b, 0).unwrap();
        assert_eq!((diff.largest, diff.differing, diff.over, diff.first_over), (5, 1, 1, Some((1, 0))), "alpha ignored, one RGB pixel differs");
        assert!(compare(&a, &b, 5).unwrap().first_over.is_none(), "within tolerance");
    }
}
