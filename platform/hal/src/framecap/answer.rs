//! `cap ...`: the frame capture's inspect answers. Text first for an AI reader (profproto.md
//! section 5: the finding on line 1, one header row with units, 2-3 significant digits, what is
//! not shown, drill-down ids at the end) and small PNGs for a vision model where a picture says
//! more (render target after a draw, overdraw heatmap, a texture).
//!
//! Ids: `D<k>` draw k of the frame (also `F<n>.D<k>`), `P<k>` pass, `S<k>` pipeline (shader),
//! `X<k>` texture, `B<k>` buffer -- indices into the capture, stable for the capture.
//!
//! Live (`makepad inspect <app> cap ...`) answers use the app's newest capture (`cap grab` makes
//! one; any query grabs one when there is none); `makepad inspect file=<x.mkcap> cap ...` answers
//! from a file. The replay runs in this process, in a private HAL.

use super::replay::{Cut, Image, Opts, Replayer};
use super::*;
use makepad_remote::{Out, Req};
use std::time::Duration;

pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("cap grab", "[session=<n>] [window=<index>] [timeout=3000] [file=<path.mkcap>]", "record the app's next complete frame (every pass, draw, shader, buffer and texture) and save it"),
    ("cap frame", "[file=<x.mkcap>]", "the captured frame: passes, draws by owner, shaders, resources, what was not captured"),
    ("cap draws", "[pass=P<k>] [owner=<text>] [sort=order|pixels] [n=40]", "the draw list with owners, shaders and pixels each draw changed"),
    ("cap draw", "D<k>", "one draw: owner, shader and its source, state, bound textures and buffers, instances, pixels changed"),
    ("cap code", "D<k>|S<k> [full=1]", "a draw's pipeline: its shader's source location and the backend code it runs"),
    ("cap rt", "D<k> [png=<path>] [max=1000] [mark=0] [wire=1]", "the render target right after a draw, as a PNG for viewing (wire=1: the draw's triangle edges over it)"),
    ("cap pixel", "<x>,<y> [pass=P<k>]", "pixel history: the draws that changed a pixel, with the value before and after each"),
    ("cap overdraw", "[png=<path>] [max=1000]", "per-pixel count of draws that changed it, as a heatmap PNG and numbers"),
    ("cap mesh", "D<k> [n=8]", "a draw's vertex and instance inputs, decoded"),
    ("cap tex", "X<k> [png=<path>] [mip=0] [layer=0]", "a texture as the frame found it"),
    ("cap time", "[n=15] [repeat=16]", "per-draw GPU time measured in replay (each draw repeated in place)"),
    ("cap save", "<path.mkcap>", "write the newest capture to a file"),
    ("cap list", "", "the captures this process keeps"),
];

/// The replay of one capture and what has been measured on it, kept between questions.
struct Live {
    cap: Arc<Capture>,
    replayer: Replayer,
    coverage: Option<Coverage>,
}

/// Per-draw pixels changed (attachment 0 of the draw's pass), measured by replaying the frame
/// cut before and after every draw.
struct Coverage {
    /// per draw: changed pixel count and the bounding box of the change (x0, y0, x1, y1)
    draws: Vec<(u64, [u32; 4])>,
    /// window passes: per pixel, the number of draws that changed it
    overdraw: Vec<u16>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

fn source(req: &Req) -> Result<Arc<Capture>, String> {
    if let Some(path) = req.get("file") {
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        return file::decode(&bytes).map(Arc::new);
    }
    if let Some(c) = latest() {
        return Ok(c);
    }
    grab(req).map(|(c, _)| c)
}

/// Default session: the newest app session other than the host's, or the host's when it is alone.
fn default_session() -> Option<u32> {
    let ids = crate::Hal::global().with_world(|w| w.session_ids());
    ids.iter().copied().filter(|s| *s != crate::session::HOST).max().or(ids.first().copied())
}

fn grab(req: &Req) -> Result<(Arc<Capture>, Option<String>), String> {
    let hal = crate::Hal::global();
    let info = hal.backend_info().ok_or("this process has no GPU backend open")?;
    if info.backend == backend_id::HEADLESS {
        return Err("the headless backend draws nothing: capture needs a GPU backend (Metal, Vulkan)".into());
    }
    let session = req.get("session").and_then(|s| s.parse().ok()).or_else(default_session);
    let window = req.get("window").and_then(|s| s.parse().ok());
    // a complete frame: everything redrawn, cached passes too. The frame already being built when
    // the request arrives may predate it, so the first redraw is waited out and the next recorded.
    let sids: Vec<u32> = hal.with_world(|w| w.session_ids()).into_iter().filter(|s| session.is_none_or(|want| want == *s)).collect();
    // `owners`: the app also records which widget draws each instance range (the frame recorded)
    let redraw = |owners: bool| {
        for sid in &sids {
            hal.with_events(*sid, |q| q.redraw(Id::NONE, owners));
        }
        hal.wake();
    };
    redraw(false);
    if let Some(sid) = session {
        hal.await_frames(sid, 1, Duration::from_millis(req.num("timeout", 3000)));
    }
    let seq = arm(session, window);
    redraw(true);
    let timeout = Duration::from_millis(req.num("timeout", 3000));
    let Some(c) = wait(seq, timeout) else {
        disarm(seq);
        return Err(format!("no window frame of session {} within {} ms (is a window open and drawing?); try: cap grab session=<n> timeout=10000", session.map_or("any".into(), |s| s.to_string()), timeout.as_millis()));
    };
    // the backend fields the executor did not know
    let mut full = (*c).clone();
    full.backend = info.backend;
    full.dialect = if full.dialect == 0 { info.dialect } else { full.dialect };
    // the app's widgets as laid out now (the frame just drawn), in target pixels
    if let Some(reply) = hal.query(full.session, b"widgets", Duration::from_millis(req.num("widgets_ms", 5000))) {
        let scale = full.dpi.max(0.01);
        for line in String::from_utf8_lossy(&reply).lines() {
            let Some(r) = crate::automation::Row::parse(line) else { continue };
            if !r.visible || r.w <= 0.0 || r.h <= 0.0 || r.window.is_some_and(|w| w != full.target) {
                continue;
            }
            full.widgets.push(CapWidget { path: r.path, kind: r.kind, rect: [r.x as f32 * scale, r.y as f32 * scale, r.w as f32 * scale, r.h as f32 * scale], text: r.text });
        }
    } else {
        full.notes.push("the app did not answer the widgets query: draws are attributed to draw lists only".into());
    }
    let mut c = Arc::new(full);
    if !c.widgets.is_empty() {
        // attribution needs each draw's changed pixels: the replay measures them once, here
        let cov = session_for(&c, |s| coverage(s).map(|cv| cv.draws.clone()));
        let mut full = (*c).clone();
        match cov {
            Ok(cov) => {
                // widget rects are window pixels: only passes drawn in window space (the window's
                // own, or an offscreen target of the window's size) are attributed by them
                let window_space: Vec<bool> = c.passes.iter().map(|p| p.is_window() || c.texture(p.color[0]).is_some_and(|t| [t.w, t.h] == c.size)).collect();
                for (d, (n, bb)) in full.draws.iter_mut().zip(cov) {
                    if !window_space[d.pass as usize] {
                        continue;
                    }
                    let b = d.bounds;
                    let rect = if n > 0 { [bb[0] as f32, bb[1] as f32, bb[2] as f32, bb[3] as f32] } else if b.iter().all(|v| v.is_finite()) { b } else { continue };
                    d.widget = attribute(&c.widgets, rect);
                }
            }
            Err(e) => full.notes.push(format!("no attribution: the replay failed: {e}")),
        }
        c = Arc::new(full);
    }
    replace(seq, c.clone());
    let path = match req.get("file") {
        Some(p) => Some(p.to_string()),
        None => {
            let dir = std::env::temp_dir().join("makepad-framecap");
            let _ = std::fs::create_dir_all(&dir);
            Some(dir.join(format!("{}-{}-f{}.mkcap", sanitize(&c.app), std::process::id(), c.frame)).to_string_lossy().into_owned())
        }
    };
    if let Some(p) = &path {
        std::fs::write(p, file::encode(&c)).map_err(|e| format!("cannot write {p}: {e}"))?;
    }
    Ok((c, path))
}

/// The smallest widget holding all of `r` (x0, y0, x1, y1 px), with how many widgets lie inside it
/// when the draw covers several (a batched draw of many widgets).
fn attribute(widgets: &[CapWidget], r: [f32; 4]) -> String {
    let holds = |w: &CapWidget| w.rect[0] <= r[0] + 1.0 && w.rect[1] <= r[1] + 1.0 && w.rect[0] + w.rect[2] >= r[2] - 1.0 && w.rect[1] + w.rect[3] >= r[3] - 1.0;
    let area = |w: &CapWidget| w.rect[2] * w.rect[3];
    let Some(best) = widgets.iter().filter(|w| holds(w)).min_by(|a, b| area(a).total_cmp(&area(b))) else { return String::new() };
    let leaves = widgets
        .iter()
        .filter(|w| !std::ptr::eq(*w, best) && w.rect[0] >= r[0] - 1.0 && w.rect[1] >= r[1] - 1.0 && w.rect[0] + w.rect[2] <= r[2] + 1.0 && w.rect[1] + w.rect[3] <= r[3] + 1.0)
        .count();
    if leaves > 1 {
        format!("{} ({}, spans {leaves} widgets)", best.path, best.kind)
    } else {
        format!("{} ({})", best.path, best.kind)
    }
}

fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if t.is_empty() { "app".into() } else { t }
}

// ------------------------------------------------------------------ text helpers

/// 2-3 significant digits.
fn sig(v: f64) -> String {
    let a = v.abs();
    if a == 0.0 {
        "0".into()
    } else if a >= 100.0 {
        format!("{:.0}", v)
    } else if a >= 10.0 {
        format!("{:.1}", v)
    } else if a >= 1.0 {
        format!("{:.2}", v)
    } else {
        format!("{:.3}", v)
    }
}

fn kb(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{} MB", sig(b as f64 / (1u64 << 20) as f64))
    } else {
        format!("{} KB", sig(b as f64 / 1024.0))
    }
}

fn ms(ns: u64) -> String {
    let m = ns as f64 / 1e6;
    if m < 0.1 { format!("{} µs", sig(ns as f64 / 1e3)) } else { format!("{} ms", sig(m)) }
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let tail: String = s.chars().rev().take(n - 1).collect::<Vec<_>>().into_iter().rev().collect();
        format!("…{tail}")
    }
}

fn owner_of(c: &Capture, d: &CapDraw) -> String {
    if !d.widget.is_empty() {
        return d.widget.clone();
    }
    match (c.s(d.owner), c.s(d.list_name)) {
        ("", "") => match c.passes.get(d.pass as usize).map(|p| c.s(p.name)).unwrap_or("") {
            "" => format!("list {}", d.list),
            pass => format!("{pass} pass, list {}", d.list),
        },
        (o, "") => o.to_string(),
        ("", l) => l.to_string(),
        (o, l) if o == l => o.to_string(),
        (o, l) => format!("{o} / {l}"),
    }
}

fn shader_of(c: &Capture, d: &CapDraw) -> (usize, String, String) {
    match c.pipelines.iter().position(|p| p.id == d.pipeline) {
        Some(i) => {
            let p = &c.pipelines[i];
            let src = c.s(p.desc.source).to_string();
            // unnamed shaders (anonymous script objects) go by their source file:line
            let name = match c.s(p.desc.name) {
                "" => src.rsplit('/').next().map(|f| f.rsplitn(2, ':').last().unwrap_or(f).to_string()).unwrap_or_default(),
                n => n.to_string(),
            };
            (i, name, src)
        }
        None => (usize::MAX, "?".into(), String::new()),
    }
}

fn instances(d: &CapDraw) -> u64 {
    if d.inst_ranges.is_empty() { d.inst_count as u64 } else { d.inst_ranges.iter().map(|r| r[1] as u64).sum() }
}

fn pass_name(c: &Capture, i: usize) -> String {
    let p = &c.passes[i];
    let n = c.s(p.name);
    let target = if p.is_window() {
        "window".to_string()
    } else {
        match c.textures.iter().position(|t| t.id == p.color[0]) {
            Some(x) => format!("X{x}"),
            None if p.color[0].is_none() => "depth only".into(),
            None => "?".into(),
        }
    };
    if n.is_empty() { target } else { format!("{n} -> {target}") }
}

fn parse_draw(c: &Capture, w: Option<&String>) -> Result<usize, String> {
    let w = w.ok_or("which draw? give D<k> (try: cap draws)")?;
    let k = w.rsplit('.').next().unwrap_or(w).trim_start_matches(['D', 'd']);
    match k.parse::<usize>() {
        Ok(k) if k < c.draws.len() => Ok(k),
        _ => Err(format!("no draw {w} (have D0..D{}); try: cap draws", c.draws.len().saturating_sub(1))),
    }
}

fn parse_index(w: &str, prefix: char, len: usize, what: &str) -> Result<usize, String> {
    match w.trim_start_matches([prefix, prefix.to_ascii_lowercase()]).parse::<usize>() {
        Ok(k) if k < len => Ok(k),
        _ => Err(format!("no {what} {w} (have {prefix}0..{prefix}{})", len.saturating_sub(1))),
    }
}

// ------------------------------------------------------------------ measurements

fn coverage(s: &mut Live) -> Result<&Coverage, String> {
    if s.coverage.is_none() {
        let c = s.cap.clone();
        let mut draws = vec![(0u64, [u32::MAX, u32::MAX, 0, 0]); c.draws.len()];
        let mut overdraw = vec![0u16; (c.size[0] * c.size[1]) as usize];
        for (pi, p) in c.passes.iter().enumerate() {
            if p.color[0].is_none() && !p.is_window() {
                continue;
            }
            let target = s.replayer.target_of(&c, pi);
            if target.is_none() {
                continue;
            }
            s.replayer.run(&c, Cut { pass: pi, draws: 0 }, Opts::default())?;
            let mut before = s.replayer.read(&c, target, None)?;
            for k in 0..p.draws.len as usize {
                let d = p.draws.off as usize + k;
                s.replayer.run(&c, Cut { pass: pi, draws: k + 1 }, Opts::default())?;
                let after = s.replayer.read(&c, target, None)?;
                let (mut n, mut bb) = (0u64, [u32::MAX, u32::MAX, 0, 0]);
                for y in 0..after.h.min(before.h) {
                    for x in 0..after.w.min(before.w) {
                        let i = ((y * after.w + x) * 4) as usize;
                        if after.rgba[i..i + 4] != before.rgba[i..i + 4] {
                            n += 1;
                            bb = [bb[0].min(x), bb[1].min(y), bb[2].max(x + 1), bb[3].max(y + 1)];
                            if p.is_window() && x < c.size[0] && y < c.size[1] {
                                let o = &mut overdraw[(y * c.size[0] + x) as usize];
                                *o = o.saturating_add(1);
                            }
                        }
                    }
                }
                draws[d] = (n, bb);
                before = after;
            }
        }
        s.coverage = Some(Coverage { draws, overdraw });
    }
    Ok(s.coverage.as_ref().unwrap())
}

/// A box-filtered copy at most `max` pixels on its longer side (vision-sized).
fn fit(img: &Image, max: u32) -> (Image, f32) {
    let long = img.w.max(img.h).max(1);
    if long <= max {
        return (img.clone(), 1.0);
    }
    let f = long as f32 / max as f32;
    let (w, h) = (((img.w as f32 / f).round() as u32).max(1), ((img.h as f32 / f).round() as u32).max(1));
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (x0, y0) = ((x as f32 * f) as u32, (y as f32 * f) as u32);
            let (x1, y1) = ((((x + 1) as f32 * f) as u32).min(img.w).max(x0 + 1), (((y + 1) as f32 * f) as u32).min(img.h).max(y0 + 1));
            let mut acc = [0u32; 4];
            let mut n = 0;
            for sy in y0..y1.min(img.h) {
                for sx in x0..x1.min(img.w) {
                    let p = img.px(sx, sy);
                    for c in 0..4 {
                        acc[c] += p[c] as u32;
                    }
                    n += 1;
                }
            }
            let i = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                out[i + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    (Image { w, h, rgba: out, format: img.format }, f)
}

/// Composites over a checkerboard so transparent targets read correctly.
fn flatten(img: &mut Image) {
    for y in 0..img.h {
        for x in 0..img.w {
            let i = ((y * img.w + x) * 4) as usize;
            let a = img.rgba[i + 3] as u32;
            let bg = if ((x / 8) + (y / 8)) % 2 == 0 { 200u32 } else { 150 };
            for c in 0..3 {
                // premultiplied colour over the background
                img.rgba[i + c] = (img.rgba[i + c] as u32 + bg * (255 - a) / 255).min(255) as u8;
            }
            img.rgba[i + 3] = 255;
        }
    }
}

fn outline(img: &mut Image, bb: [u32; 4], scale: f32) {
    if bb[0] >= bb[2] {
        return;
    }
    let [x0, y0, x1, y1] = bb.map(|v| (v as f32 / scale) as u32);
    let (x1, y1) = (x1.min(img.w.saturating_sub(1)), y1.min(img.h.saturating_sub(1)));
    let mut put = |x: u32, y: u32| {
        let i = ((y * img.w + x) * 4) as usize;
        if i + 4 <= img.rgba.len() {
            img.rgba[i..i + 4].copy_from_slice(&[255, 0, 255, 255]);
        }
    };
    for x in x0..=x1 {
        put(x, y0);
        put(x, y1);
    }
    for y in y0..=y1 {
        put(x0, y);
        put(x1, y);
    }
}

fn write_png(req: &Req, default: &str, img: &Image) -> Result<String, String> {
    let path = match req.get("png") {
        Some(p) => p.to_string(),
        None => {
            let dir = std::env::temp_dir().join("makepad-framecap");
            let _ = std::fs::create_dir_all(&dir);
            dir.join(default).to_string_lossy().into_owned()
        }
    };
    std::fs::write(&path, crate::automation::png_rgba(img.w, img.h, &img.rgba)).map_err(|e| format!("cannot write {path}: {e}"))?;
    Ok(path)
}

// ------------------------------------------------------------------ the commands

/// `cap ...`; true when this was a capture command.
pub fn dispatch(req: &Req, out: &mut Out) -> bool {
    if req.cmd != "cap" {
        return false;
    }
    let sub = req.pos.first().map(String::as_str).unwrap_or("frame");
    let what = format!("cap {sub}");
    let r = match sub {
        "grab" => grab(req).map(|(c, path)| {
            let mut t = frame_text(&c);
            if let Some(p) = path {
                t.push_str(&format!("\nsaved: {p}  ({} on disk)", kb(std::fs::metadata(&p).map_or(0, |m| m.len()))));
            }
            t
        }),
        "list" => Ok(list_text()),
        "save" => match (latest(), req.pos.get(1)) {
            (Some(c), Some(p)) => std::fs::write(p, file::encode(&c)).map(|_| format!("saved F{} of {} to {p}", c.frame, c.app)).map_err(|e| format!("cannot write {p}: {e}")),
            (None, _) => Err("no capture yet; try: cap grab".into()),
            (_, None) => Err("cap save <path.mkcap>".into()),
        },
        "frame" | "draws" | "draw" | "code" | "rt" | "pixel" | "overdraw" | "mesh" | "tex" | "time" => source(req).and_then(|c| match sub {
            "frame" => Ok(frame_text(&c)),
            "draws" => draws_text(&c, req),
            "draw" => draw_text(&c, req),
            "code" => code_text(&c, req),
            "mesh" => mesh_text(&c, req),
            "rt" => rt_text(&c, req),
            "pixel" => pixel_text(&c, req),
            "overdraw" => overdraw_text(&c, req),
            "tex" => tex_text(&c, req),
            _ => time_text(&c, req),
        }),
        _ => Err(format!("no cap command `{sub}`; try: cap grab | frame | draws | draw D<k> | code D<k> | rt D<k> | pixel x,y | overdraw | mesh D<k> | tex X<k> | time | save | list")),
    };
    let src = match sub {
        "grab" => "live",
        "list" | "save" => "process",
        _ if req.get("file").is_some() => "file",
        _ => "replay",
    };
    out.measured(&what, src, &[]);
    match r {
        Ok(text) => {
            if req.get("full") == Some("1") {
                out.limit = usize::MAX;
            }
            for l in text.lines() {
                out.line(l);
            }
            if out.truncated {
                out.always("(cut at the size limit; ask with full=1)");
            }
        }
        Err(e) => out.err(&e),
    }
    true
}

fn list_text() -> String {
    let st = state();
    if st.done.is_empty() {
        return "no captures in this process; try: cap grab".into();
    }
    let mut t = format!("{} captures kept (newest last)\n\n| req | app | frame | passes | draws | size |\n|---|---|---|---|---|---|\n", st.done.len());
    for (seq, c) in &st.done {
        t.push_str(&format!("| {seq} | {} | F{} | {} | {} | {} |\n", c.app, c.frame, c.passes.len(), c.draws.len(), kb(c.bytes())));
    }
    t
}

fn frame_text(c: &Capture) -> String {
    let total_inst: u64 = c.draws.iter().map(instances).sum();
    let mut t = format!(
        "F{} of {}: {} passes, {} draws ({} instances), {} shaders, {} textures, {} buffers; {}x{} px at dpi {} ({} captured)\n\n",
        c.frame, c.app, c.passes.len(), c.draws.len(), total_inst, c.pipelines.len(), c.textures.len(), c.buffers.len(), c.size[0], c.size[1], sig(c.dpi as f64), kb(c.bytes())
    );
    t.push_str("| pass | target | load | draws | instances |\n|---|---|---|---|---|\n");
    for (i, p) in c.passes.iter().enumerate() {
        let inst: u64 = c.draws[p.draws.off as usize..(p.draws.off + p.draws.len) as usize].iter().map(instances).sum();
        let load = match p.load[0] {
            pass_load::CLEAR => "clear",
            pass_load::LOAD => "load",
            _ => "dont-care",
        };
        t.push_str(&format!("| P{i} | {} | {load} | {} | {inst} |\n", pass_name(c, i), p.draws.len));
    }
    // draws by owner
    let mut owners: Vec<(String, u64, u64)> = Vec::new();
    for d in &c.draws {
        let o = owner_of(c, d);
        match owners.iter_mut().find(|x| x.0 == o) {
            Some(x) => {
                x.1 += 1;
                x.2 += instances(d);
            }
            None => owners.push((o, 1, instances(d))),
        }
    }
    owners.sort_by(|a, b| b.1.cmp(&a.1));
    t.push_str("\n| owner | draws | instances |\n|---|---|---|\n");
    for (o, n, i) in owners.iter().take(12) {
        t.push_str(&format!("| {} | {n} | {i} |\n", short(o, 60)));
    }
    if owners.len() > 12 {
        let rest: u64 = owners[12..].iter().map(|x| x.1).sum();
        t.push_str(&format!("+{} more owners, {rest} draws\n", owners.len() - 12));
    }
    let missing: Vec<&CapTexture> = c.textures.iter().filter(|x| x.contents.is_none() && x.note != "cleared by the frame before use").collect();
    if !missing.is_empty() || !c.notes.is_empty() {
        t.push_str("\nnot captured:\n");
        for x in missing.iter().take(6) {
            let i = c.textures.iter().position(|y| y.id == x.id).unwrap_or(0);
            t.push_str(&format!("- X{i} {}x{} {}: {}\n", x.w, x.h, format::name(x.format), x.note));
        }
        if missing.len() > 6 {
            t.push_str(&format!("- +{} more textures\n", missing.len() - 6));
        }
        for n in c.notes.iter().take(6) {
            t.push_str(&format!("- {n}\n"));
        }
    }
    t.push_str(&format!("\nmore: cap draws  cap draw D{}  cap overdraw  cap pixel <x>,<y>  cap time", c.draws.len().saturating_sub(1)));
    t
}

fn draws_text(c: &Arc<Capture>, req: &Req) -> Result<String, String> {
    let n = req.num("n", 40) as usize;
    let pass = req.get("pass").map(|p| parse_index(p, 'P', c.passes.len(), "pass")).transpose()?;
    let owner = req.get("owner");
    let by_pixels = req.get("sort") == Some("pixels");
    let cov: Option<Vec<(u64, [u32; 4])>> = if by_pixels || req.get("pixels") == Some("1") {
        Some(session_for(c, |s| coverage(s).map(|cv| cv.draws.clone()))?)
    } else {
        None
    };
    let mut rows: Vec<usize> = (0..c.draws.len())
        .filter(|i| pass.is_none_or(|p| c.draws[*i].pass as usize == p))
        .filter(|i| owner.is_none_or(|o| owner_of(c, &c.draws[*i]).contains(o)))
        .collect();
    if let (true, Some(cv)) = (by_pixels, &cov) {
        rows.sort_by(|a, b| cv[*b].0.cmp(&cv[*a].0));
    }
    let shown = rows.len().min(n);
    let mut t = format!("{} draws{}{}\n\n", rows.len(), pass.map_or(String::new(), |p| format!(" in P{p}")), owner.map_or(String::new(), |o| format!(" of owners matching `{o}`")));
    t.push_str(if cov.is_some() { "| draw | pass | owner | shader | inst | px changed |\n|---|---|---|---|---|---|\n" } else { "| draw | pass | owner | shader | inst |\n|---|---|---|---|---|\n" });
    for &i in &rows[..shown] {
        let d = &c.draws[i];
        let (si, name, _) = shader_of(c, d);
        let px = cov.as_ref().map_or(String::new(), |cv| format!(" {} |", cv[i].0));
        t.push_str(&format!("| D{i} | P{} | {} | S{si} {} | {} |{px}\n", d.pass, short(&owner_of(c, d), 48), short(&name, 32), instances(d)));
    }
    if rows.len() > shown {
        t.push_str(&format!("+{} more (n=<count>)\n", rows.len() - shown));
    }
    t.push_str("\nmore: cap draw D<k>  cap draws sort=pixels  cap draws owner=<text>");
    Ok(t)
}

/// The replay session of `cap`, made (pipelines compiled, resources uploaded) on first use and
/// kept for the next question about the same capture.
fn session_for<R>(cap: &Arc<Capture>, f: impl FnOnce(&mut Live) -> Result<R, String>) -> Result<R, String> {
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = live.as_mut().filter(|s| same_capture(&s.cap, cap)) {
        // the same frame, maybe with more filled in (attribution): answer from the newest
        s.cap = cap.clone();
    } else {
        *live = None;
        let replayer = Replayer::new(cap)?;
        *live = Some(Live { cap: cap.clone(), replayer, coverage: None });
    }
    f(live.as_mut().unwrap())
}

fn same_capture(a: &Capture, b: &Capture) -> bool {
    a.frame == b.frame && a.at_unix_ms == b.at_unix_ms && a.session == b.session && a.draws.len() == b.draws.len() && a.app == b.app
}

fn draw_text(c: &Arc<Capture>, req: &Req) -> Result<String, String> {
    let k = parse_draw(c, req.pos.get(1))?;
    let d = &c.draws[k];
    let (si, name, src) = shader_of(c, d);
    let (n, bb) = session_for(c, |s| coverage(s).map(|cv| cv.draws[k]))?;
    let mut t = if n == 0 {
        format!("D{k} ({}) changed no pixels of its target: hidden, clipped, transparent or drawn over by itself\n\n", owner_of(c, d))
    } else {
        format!("D{k} ({}) changed {n} px in [{},{} .. {},{}] of {}\n\n", owner_of(c, d), bb[0], bb[1], bb[2], bb[3], pass_name(c, d.pass as usize))
    };
    t.push_str("| field | value |\n|---|---|\n");
    t.push_str(&format!("| shader | S{si} {name} |\n| source | {} |\n", if src.is_empty() { "(not given)" } else { &src }));
    if !c.s(d.owner).is_empty() {
        t.push_str(&format!("| owner | {} |\n", c.s(d.owner)));
    }
    t.push_str(&format!("| draw list | {}list {}, item {} |\n", match c.s(d.list_name) { "" => String::new(), n => format!("{n}, ") }, d.list, d.item));
    t.push_str(&format!("| pass | P{} {} |\n", d.pass, pass_name(c, d.pass as usize)));
    let inst = if d.inst_kind == Inst::INLINE { format!("{} inline ({})", instances(d), kb(d.inst_data.len() as u64)) } else { format!("{} from buffer B{}", instances(d), c.buffers.iter().position(|b| b.id == d.inst_id).map_or("?".into(), |b| b.to_string())) };
    t.push_str(&format!("| instances | {inst}{} |\n", if d.inst_ranges.is_empty() { String::new() } else { format!(" in {} ranges", d.inst_ranges.len()) }));
    t.push_str(&format!("| indices | {} from {} |\n", d.index[1], d.index[0]));
    let mut flags = Vec::new();
    for (bit, name) in [(Draw::BLEND, "blend"), (Draw::DEPTH_WRITE, "depth-write"), (Draw::CULL, "cull back faces (front = counter-clockwise in clip space)"), (Draw::INDEX_U32, "u32 indices")] {
        if d.flags & bit != 0 {
            flags.push(name);
        }
    }
    t.push_str(&format!("| state | {} zbias {} |\n", if flags.is_empty() { "-".into() } else { flags.join(", ") }, sig(d.zbias as f64)));
    if d.scissor.iter().all(|v| v.is_finite()) {
        t.push_str(&format!("| scissor | {},{} {}x{} px |\n", sig(d.scissor[0] as f64), sig(d.scissor[1] as f64), sig(d.scissor[2] as f64), sig(d.scissor[3] as f64)));
    }
    if d.bounds.iter().all(|v| v.is_finite()) {
        t.push_str(&format!("| bounds | {},{} .. {},{} px |\n", sig(d.bounds[0] as f64), sig(d.bounds[1] as f64), sig(d.bounds[2] as f64), sig(d.bounds[3] as f64)));
    }
    let u: Vec<String> = d.uniforms.iter().zip(["draw", "list", "dyn", "scope"]).filter(|(u, _)| !u.is_empty()).map(|(u, n)| format!("{n} {} B", u.len())).collect();
    if !u.is_empty() {
        t.push_str(&format!("| uniforms | {} |\n", u.join(", ")));
    }
    for (slot, id) in d.tex.iter().enumerate() {
        if id.is_none() {
            t.push_str(&format!("| tex {slot} | none |\n"));
            continue;
        }
        match c.textures.iter().position(|x| x.id == *id) {
            Some(x) => {
                let tx = &c.textures[x];
                let who = [c.s(tx.label), c.s(tx.owner)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
                t.push_str(&format!("| tex {slot} | X{x} {}x{} {} {} |\n", tx.w, tx.h, format::name(tx.format), who));
            }
            None => t.push_str(&format!("| tex {slot} | {}:{} (not in the capture) |\n", id.index, id.gen)),
        }
    }
    for (what, id) in [("vertices", d.vb), ("indices", d.ib)] {
        if let Some(b) = c.buffers.iter().position(|b| b.id == id) {
            t.push_str(&format!("| {what} | B{b} {} |\n", kb(c.buffers[b].size)));
        }
    }
    t.push_str(&format!("\nmore: cap rt D{k}  cap code D{k}  cap mesh D{k}  cap draw D{}", (k + 1).min(c.draws.len() - 1)));
    Ok(t)
}

fn code_text(c: &Arc<Capture>, req: &Req) -> Result<String, String> {
    let w = req.pos.get(1).ok_or("cap code D<k> | S<k>")?;
    let si = if w.starts_with(['S', 's']) { parse_index(w, 'S', c.pipelines.len(), "pipeline")? } else { shader_of(c, &c.draws[parse_draw(c, Some(w))?]).0 };
    let p = c.pipelines.get(si).ok_or("the draw's pipeline is not in the capture")?;
    let blob = crate::buffers::PipelineBlob::parse(&p.blob).ok_or("the pipeline blob is malformed")?;
    let users = c.draws.iter().filter(|d| d.pipeline == p.id).count();
    let dialect = match p.dialect {
        dialect::MSL => "MSL",
        dialect::SPIRV => "SPIR-V",
        dialect::HLSL => "HLSL",
        dialect::GLSL => "GLSL",
        _ => "?",
    };
    let mut t = format!(
        "S{si} {}: {} draws use it; source {}; {} {} of backend code, {} floats per instance, {} varyings\n\n",
        c.draws.iter().find(|d| d.pipeline == p.id).map_or(String::new(), |d| shader_of(c, d).1),
        users,
        if c.s(p.desc.source).is_empty() { "(not given)" } else { c.s(p.desc.source) },
        kb(blob.code.len() as u64),
        dialect,
        p.desc.instance_stride,
        p.desc.varyings
    );
    if p.dialect == dialect::MSL || p.dialect == dialect::GLSL || p.dialect == dialect::HLSL {
        let code = String::from_utf8_lossy(blob.code);
        let lines: Vec<&str> = code.lines().collect();
        let n = if req.get("full") == Some("1") { lines.len() } else { lines.len().min(80) };
        t.push_str("```\n");
        for l in &lines[..n] {
            t.push_str(l);
            t.push('\n');
        }
        t.push_str("```\n");
        if lines.len() > n {
            t.push_str(&format!("+{} more lines (full=1)\n", lines.len() - n));
        }
    } else {
        t.push_str("(binary code; the shader's source location above is where to read it)\n");
    }
    Ok(t)
}

fn mesh_text(c: &Arc<Capture>, req: &Req) -> Result<String, String> {
    let k = parse_draw(c, req.pos.get(1))?;
    let n = req.num("n", 8) as usize;
    let d = &c.draws[k];
    let (si, name, _) = shader_of(c, d);
    let p = c.pipelines.get(si).ok_or("the draw's pipeline is not in the capture")?;
    let blob = crate::buffers::PipelineBlob::parse(&p.blob).ok_or("the pipeline blob is malformed")?;
    let stride = p.desc.instance_stride.max(1) as usize;
    let floats = |b: &[u8]| -> Vec<f32> { b.chunks_exact(4).map(|x| f32::from_le_bytes([x[0], x[1], x[2], x[3]])).collect() };
    let inst: Vec<f32> = if d.inst_kind == Inst::INLINE {
        floats(&d.inst_data)
    } else {
        c.buffer(d.inst_id).and_then(|b| b.contents.as_ref()).map(|b| floats(&b[(d.inst_off as usize).min(b.len())..])).unwrap_or_default()
    };
    let count = instances(d) as usize;
    let mut t = format!("D{k} S{si} {name}: {count} instances of {stride} floats, {} indices; vertex layout {:?}\n\n", d.index[1], blob.vertex_layout);
    t.push_str("instance inputs (floats in declaration order of the shader's instance fields):\n\n| inst | values |\n|---|---|\n");
    let firsts: Vec<usize> = if d.inst_ranges.is_empty() { (0..count).collect() } else { d.inst_ranges.iter().flat_map(|r| r[0] as usize..(r[0] + r[1]) as usize).collect() };
    for &i in firsts.iter().take(n) {
        let row = inst.get(i * stride..(i + 1) * stride).map_or("(past the data)".to_string(), |v| v.iter().map(|f| sig(*f as f64)).collect::<Vec<_>>().join(" "));
        t.push_str(&format!("| {i} | {row} |\n"));
    }
    if firsts.len() > n {
        t.push_str(&format!("+{} more instances (n=<count>)\n", firsts.len() - n));
    }
    if let Some(vb) = c.buffer(d.vb).and_then(|b| b.contents.as_ref()) {
        let vstride = blob.vertex_layout.first().copied().unwrap_or(16).max(4) as usize / 4;
        let v = floats(vb);
        t.push_str(&format!("\nvertices ({} floats each):\n\n| vtx | values |\n|---|---|\n", vstride));
        for i in 0..(v.len() / vstride).min(n) {
            t.push_str(&format!("| {i} | {} |\n", v[i * vstride..(i + 1) * vstride].iter().map(|f| sig(*f as f64)).collect::<Vec<_>>().join(" ")));
        }
        // Winding, from the captured inputs alone: the signed volume of the indexed triangles
        // with each vertex's first three floats as its position. On a closed mesh it is > 0
        // when the counter-clockwise fronts face outward (what back-face culling keeps) and
        // < 0 when the mesh draws inside-out.
        if vstride >= 3 {
            if let Some(ib) = c.buffer(d.ib).and_then(|b| b.contents.as_ref()) {
                let wide = d.flags & Draw::INDEX_U32 != 0;
                let at = |i: usize| -> Option<usize> {
                    let i = d.index[0] as usize + i;
                    if wide {
                        ib.get(i * 4..i * 4 + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize)
                    } else {
                        ib.get(i * 2..i * 2 + 2).map(|x| u16::from_le_bytes([x[0], x[1]]) as usize)
                    }
                };
                let pos = |i: usize| v.get(i * vstride..i * vstride + 3).map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]);
                let (mut volume, mut tris, mut outside) = (0.0f64, 0usize, 0usize);
                for tri in 0..d.index[1] as usize / 3 {
                    let corners = [at(tri * 3), at(tri * 3 + 1), at(tri * 3 + 2)];
                    let (Some(p), Some(q), Some(r)) = (corners[0].and_then(pos), corners[1].and_then(pos), corners[2].and_then(pos)) else {
                        outside += 1;
                        continue;
                    };
                    volume += (p[0] * (q[1] * r[2] - q[2] * r[1]) + p[1] * (q[2] * r[0] - q[0] * r[2]) + p[2] * (q[0] * r[1] - q[1] * r[0])) / 6.0;
                    tris += 1;
                }
                let verdict = if volume > 0.0 { "fronts face outward" } else if volume < 0.0 { "INSIDE-OUT: fronts face inward" } else { "flat or open" };
                t.push_str(&format!("\nwinding: object-space signed volume {} over {tris} triangles ({verdict}, if the mesh is closed and vertex floats 0..3 are its position){}\n", sig(volume), if outside > 0 { format!("; {outside} triangles index past the captured vertex bytes") } else { String::new() }));
            }
        }
    }
    t.push_str("\nnot shown: vertex shader outputs (needs a shader-side capture point)");
    Ok(t)
}

fn rt_text(c: &Arc<Capture>, req: &Req) -> Result<String, String> {
    let k = parse_draw(c, req.pos.get(1))?;
    let max = req.num("max", 1000) as u32;
    let d = &c.draws[k];
    let pi = d.pass as usize;
    let (img, cov) = session_for(c, |s| {
        let cov = coverage(s)?.draws[k];
        let target = s.replayer.target_of(&s.cap.clone(), pi);
        if target.is_none() {
            return Err("this draw's pass has no colour target (depth only)".into());
        }
        let cap = s.cap.clone();
        let wire = (req.get("wire") == Some("1")).then_some(k);
        s.replayer.run(&cap, Cut::after(&cap, k), Opts { wire, ..Default::default() })?;
        Ok((s.replayer.read(&cap, target, None)?, cov))
    })?;
    let (mut small, scale) = fit(&img, max);
    flatten(&mut small);
    if req.get("mark") != Some("0") {
        outline(&mut small, cov.1, scale);
    }
    let path = write_png(req, &format!("rt-F{}-D{k}.png", c.frame), &small)?;
    Ok(format!(
        "{} after D{k} ({}): {}x{} px, shown {}x{}{}; D{k} changed {} px (magenta box)\npng: {path}\n\nmore: cap rt D{}  cap rt D{}  cap draw D{k}",
        pass_name(c, pi), owner_of(c, d), img.w, img.h, small.w, small.h, if scale > 1.0 { format!(" (1:{})", sig(scale as f64)) } else { String::new() }, cov.0,
        k.saturating_sub(1), (k + 1).min(c.draws.len() - 1)
    ))
}

fn pixel_text(c: &Arc<Capture>, req: &Req) -> Result<String, String> {
    let w = req.pos.get(1).ok_or("cap pixel <x>,<y> (target pixels)")?;
    let (x, y) = w.split_once(',').and_then(|(a, b)| Some((a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?))).ok_or("cap pixel <x>,<y>")?;
    let pass = req.get("pass").map(|p| parse_index(p, 'P', c.passes.len(), "pass")).transpose()?;
    session_for(c, |s| {
        let cap = s.cap.clone();
        let cov = coverage(s)?.draws.clone();
        let mut rows = Vec::new();
        let mut covered = Vec::new();
        for (pi, p) in cap.passes.iter().enumerate() {
            let window = p.is_window();
            if pass.map_or(!window, |want| want != pi) {
                continue;
            }
            let target = s.replayer.target_of(&cap, pi);
            if target.is_none() {
                continue;
            }
            for k in 0..p.draws.len as usize {
                let d = p.draws.off as usize + k;
                let (_, bb) = cov[d];
                if !(x >= bb[0] && x < bb[2] && y >= bb[1] && y < bb[3]) {
                    let b = cap.draws[d].bounds;
                    if b.iter().all(|v| v.is_finite()) && (x as f32) >= b[0] && (x as f32) < b[2] && (y as f32) >= b[1] && (y as f32) < b[3] {
                        covered.push(d);
                    }
                    continue;
                }
                s.replayer.run(&cap, Cut { pass: pi, draws: k }, Opts::default())?;
                let before = s.replayer.read(&cap, target, Some([x, y, 1, 1]))?.px(0, 0);
                s.replayer.run(&cap, Cut { pass: pi, draws: k + 1 }, Opts::default())?;
                let after = s.replayer.read(&cap, target, Some([x, y, 1, 1]))?.px(0, 0);
                if before != after {
                    rows.push((d, before, after));
                } else {
                    covered.push(d);
                }
            }
        }
        let hex = |p: [u8; 4]| format!("#{:02x}{:02x}{:02x}{:02x}", p[0], p[1], p[2], p[3]);
        let mut t = match rows.last() {
            Some((d, _, after)) => format!("pixel {x},{y}: {} draws changed it; the last is D{d} ({}) leaving {}\n\n", rows.len(), owner_of(&cap, &cap.draws[*d]), hex(*after)),
            None => format!("pixel {x},{y}: no draw changed it{}\n\n", pass.map_or(" in the window passes".to_string(), |p| format!(" in P{p}"))),
        };
        if !rows.is_empty() {
            t.push_str("| draw | owner | shader | before (rgba) | after (rgba) |\n|---|---|---|---|---|\n");
            for (d, b, a) in &rows {
                let (si, name, _) = shader_of(&cap, &cap.draws[*d]);
                t.push_str(&format!("| D{d} | {} | S{si} {} | {} | {} |\n", short(&owner_of(&cap, &cap.draws[*d]), 40), short(&name, 28), hex(*b), hex(*a)));
            }
        }
        if !covered.is_empty() {
            let list: Vec<String> = covered.iter().take(12).map(|d| format!("D{d}")).collect();
            t.push_str(&format!("\ncovered it without a visible change: {}{}\n", list.join(" "), if covered.len() > 12 { format!(" +{} more", covered.len() - 12) } else { String::new() }));
        }
        let here = attribute(&cap.widgets, [x as f32, y as f32, x as f32 + 1.0, y as f32 + 1.0]);
        if !here.is_empty() {
            t.push_str(&format!("\nwidget there: {here}\n"));
        }
        t.push_str("\nnot shown: fragment shader outputs before blending (values are after each draw's blend)\nmore: cap draw D<k>  cap rt D<k>");
        Ok(t)
    })
}

fn heat(v: u16, max: u16) -> [u8; 4] {
    if v == 0 {
        return [0, 0, 0, 255];
    }
    let t = (v as f32 / max.max(1) as f32).clamp(0.0, 1.0);
    // dark blue -> cyan -> yellow -> red
    let stops = [(0.0, [20.0, 30.0, 140.0]), (0.33, [0.0, 200.0, 220.0]), (0.66, [250.0, 230.0, 40.0]), (1.0, [230.0, 30.0, 30.0])];
    let mut c = stops[3].1;
    for w in stops.windows(2) {
        if t <= w[1].0 {
            let f = (t - w[0].0) / (w[1].0 - w[0].0);
            c = [0, 1, 2].map(|i| w[0].1[i] + (w[1].1[i] - w[0].1[i]) * f);
            break;
        }
    }
    [c[0] as u8, c[1] as u8, c[2] as u8, 255]
}

fn overdraw_text(c: &Arc<Capture>, req: &Req) -> Result<String, String> {
    if c.size[0] == 0 {
        return Err("this capture has no window passes".into());
    }
    let max = req.num("max", 1000) as u32;
    let od = session_for(c, |s| coverage(s).map(|cv| cv.overdraw.clone()))?;
    let peak = od.iter().copied().max().unwrap_or(0);
    let touched = od.iter().filter(|v| **v > 0).count() as f64;
    let total = od.len().max(1) as f64;
    let mean = if touched > 0.0 { od.iter().map(|v| *v as f64).sum::<f64>() / touched } else { 0.0 };
    let over3 = od.iter().filter(|v| **v >= 3).count() as f64;
    let mut hist = [0usize; 6];
    for v in &od {
        hist[(*v as usize).min(5)] += 1;
    }
    let mut img = Image { w: c.size[0], h: c.size[1], rgba: Vec::with_capacity(od.len() * 4), format: format::RGBA8 };
    let scale_max = peak.min(8);
    for v in &od {
        img.rgba.extend_from_slice(&heat((*v).min(scale_max), scale_max));
    }
    let (small, scale) = fit(&img, max);
    let path = write_png(req, &format!("overdraw-F{}.png", c.frame), &small)?;
    let mut t = format!(
        "window: {}% of pixels changed by 3+ draws; mean {} draws per drawn pixel, peak {}\npng: {path} ({}x{}{}; black 0, blue 1 .. red {}+)\n\n| draws | % of pixels |\n|---|---|\n",
        sig(100.0 * over3 / total), sig(mean), peak, small.w, small.h, if scale > 1.0 { format!(", 1:{}", sig(scale as f64)) } else { String::new() }, scale_max
    );
    for (i, n) in hist.iter().enumerate() {
        t.push_str(&format!("| {}{} | {} |\n", i, if i == 5 { "+" } else { "" }, sig(100.0 * *n as f64 / total)));
    }
    t.push_str("\nnot shown: writes that left a pixel unchanged; offscreen passes\nmore: cap pixel <x>,<y>  cap draws sort=pixels");
    Ok(t)
}

fn tex_text(c: &Arc<Capture>, req: &Req) -> Result<String, String> {
    let w = req.pos.get(1).ok_or("cap tex X<k>")?;
    let k = parse_index(w, 'X', c.textures.len(), "texture")?;
    let x = &c.textures[k];
    let users: Vec<String> = c.draws.iter().enumerate().filter(|(_, d)| d.tex.contains(&x.id)).take(8).map(|(i, _)| format!("D{i}")).collect();
    let who = [c.s(x.label), c.s(x.owner), c.s(x.site)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
    let Some(bytes) = &x.contents else {
        return Ok(format!("X{k} {}x{} {} ({who}): not captured: {}\nsampled by: {}", x.w, x.h, format::name(x.format), x.note, users.join(" ")));
    };
    if format::is_block(x.format) {
        return Ok(format!("X{k} {}x{} {} ({who}): {} captured; block-compressed, no preview\nsampled by: {}", x.w, x.h, format::name(x.format), kb(bytes.len() as u64), users.join(" ")));
    }
    let n = format::level_bytes(x.format, x.w, x.h) as usize;
    let px = &bytes[..n.min(bytes.len())];
    let rgba = match x.format {
        format::RGBA8 => px.to_vec(),
        format::BGRA8 => px.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect(),
        f => super::replay::to_rgba8(f, px),
    };
    let img = Image { w: x.w, h: x.h, rgba, format: x.format };
    let (mut small, scale) = fit(&img, req.num("max", 1000) as u32);
    flatten(&mut small);
    let path = write_png(req, &format!("tex-F{}-X{k}.png", c.frame), &small)?;
    Ok(format!(
        "X{k} {}x{} {} ({who}), mip 0 as the frame found it\npng: {path} ({}x{}{})\nsampled by: {}{}",
        x.w, x.h, format::name(x.format), small.w, small.h, if scale > 1.0 { format!(", 1:{}", sig(scale as f64)) } else { String::new() },
        if users.is_empty() { "no draw (a render target)".into() } else { users.join(" ") },
        if x.mips > 1 || x.depth > 1 { format!("\nnot shown: {} more mips/layers", x.mips * x.depth - 1) } else { String::new() }
    ))
}

fn time_text(c: &Arc<Capture>, req: &Req) -> Result<String, String> {
    let n = req.num("n", 15) as usize;
    let repeat = req.num("repeat", 16).clamp(2, 256) as u32;
    session_for(c, |s| {
        let cap = s.cap.clone();
        let all = Cut::all(&cap);
        // the frame's own time, best of three
        let mut base = u64::MAX;
        for _ in 0..3 {
            s.replayer.run(&cap, all, Opts::default())?;
            base = base.min(s.replayer.last_gpu_ns);
        }
        if base == 0 {
            return Err("this backend reports no GPU time for replayed frames".into());
        }
        let mut per: Vec<(usize, u64)> = Vec::new();
        for d in 0..cap.draws.len() {
            let mut best = u64::MAX;
            for _ in 0..2 {
                s.replayer.run(&cap, all, Opts { repeat: Some((d, repeat)), ..Default::default() })?;
                best = best.min(s.replayer.last_gpu_ns);
            }
            per.push((d, best.saturating_sub(base) / (repeat as u64 - 1)));
        }
        let sum: u64 = per.iter().map(|x| x.1).sum::<u64>().max(1);
        per.sort_by(|a, b| b.1.cmp(&a.1));
        let mut t = format!(
            "replayed frame: {} GPU; the costliest draw is D{} ({}) at {} alone, {}% of the draws' summed cost\n\n| draw | owner | shader | GPU alone | % of sum |\n|---|---|---|---|---|\n",
            ms(base), per.first().map_or(0, |x| x.0), per.first().map_or(String::new(), |x| owner_of(&cap, &cap.draws[x.0])), per.first().map_or("0".into(), |x| ms(x.1)),
            per.first().map_or("0".into(), |x| sig(100.0 * x.1 as f64 / sum as f64))
        );
        for (d, ns) in per.iter().take(n) {
            let (si, name, _) = shader_of(&cap, &cap.draws[*d]);
            t.push_str(&format!("| D{d} | {} | S{si} {} | {} | {} |\n", short(&owner_of(&cap, &cap.draws[*d]), 40), short(&name, 28), ms(*ns), sig(100.0 * *ns as f64 / sum as f64)));
        }
        if per.len() > n {
            let rest: u64 = per[n..].iter().map(|x| x.1).sum();
            t.push_str(&format!("+{} more draws, {} total\n", per.len() - n, ms(rest)));
        }
        t.push_str(&format!("\nmethod: each draw repeated {repeat}x in place in the replayed frame, (frame - base) / {}; best of 2. A draw alone costs more than its share of the frame (the GPU overlaps draws; the sum is {} against a {} frame): rank by it, do not add it up. Replay timing, not the live frame's (live: gpu prof).\nmore: cap draw D<k>", repeat - 1, ms(sum), ms(base)));
        Ok(t)
    })
}
