//! makepad-party-server: the party's HTTPS service without the big screen.
//!
//! By default it serves the party: the phone page and its API under
//! `/p/<code>/`. `--camtest` serves the camera check page that phase 0 uses
//! to find out what guests' phones can do instead. Options:
//!   --port N       listen port (default 8443)
//!   --code C       the party code (default: a new one each start)
//!   --name X       another name or address for the certificate (repeatable),
//!                  such as the router's public address
//!
//! `--check-hub [--hub URL]` measures what the party asks of this box's AI
//! hub instead (phase 0): the fast model per image and per live frame, the
//! vision model, the quality model, what stays resident and what each swap
//! costs; with `--sizes`, the fast model's live frame at sizes from 768 to
//! 1536 px and at more steps (what a larger loop costs); with `--compare PICTURE`,
//! a loop made from PICTURE redrawn at the smaller sizes and fewer steps a
//! faster loop could use, side by side in one sheet (`--out FILE`, default
//! party-compare.jpg in the temp folder) for a person to judge before any quality trade. It prints one line per step (to stderr, like everything here: the
//! log a box keeps of a process) and then stays, so the log can be read.
//!
//! `--live-ab PICTURE --out DIR [--tag T] [--seed N] [--fast] [--text-rows N|real]
//! [--prompt TEXT] [--live-only]` keeps the redraws of a tilt down PICTURE bit
//! for bit, for an A/B of the hub's compute or of two hubs (`redraws_for_ab`);
//! `--live-only` leaves out its two jobs. `--unload MODEL` (repeatable) takes
//! a bench's models off the hub again.

// A native service: never built for wasm.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use makepad_app_party::hub::{self, Arithmetic, BoxHub, Compute, JobTime, LiveInput, FAST_MODEL, QUALITY_MODEL, VISION_MODEL};
use makepad_app_party::image::{self, Rgb};
use makepad_app_party::server::Handler;
use makepad_app_party::{camtest::CamTest, code, server::Server, start};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let mut port = 8443u16;
    let mut party_code = None;
    let mut extra = Vec::new();
    let mut camtest = false;
    let mut check_hub = false;
    let mut sizes = false;
    let mut hub_base = None;
    let mut compare = None;
    let mut live_steps = None;
    let mut live_costs = false;
    let mut live_ab = None;
    let mut compute = Compute::STORED;
    let mut ab_prompt = None;
    let mut ab_jobs = true;
    let mut seed = SEED;
    let mut tag = "a".to_string();
    let mut unload = Vec::new();
    let mut out = std::env::temp_dir().join("party-compare.jpg").to_string_lossy().into_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).unwrap_or_else(|| fail("--port needs a number")),
            "--code" => party_code = Some(args.next().unwrap_or_else(|| fail("--code needs a code"))),
            "--name" => extra.push(args.next().unwrap_or_else(|| fail("--name needs a name or address"))),
            "--camtest" => camtest = true,
            "--check-hub" => check_hub = true,
            "--sizes" => sizes = true,
            "--compare" => compare = Some(args.next().unwrap_or_else(|| fail("--compare needs a picture"))),
            "--live-steps" => live_steps = Some(args.next().unwrap_or_else(|| fail("--live-steps needs a picture"))),
            "--live-costs" => live_costs = true,
            "--live-ab" => live_ab = Some(args.next().unwrap_or_else(|| fail("--live-ab needs a picture"))),
            "--fast" => compute.arithmetic = Arithmetic::Fast,
            "--text-rows" => {
                compute.text_rows = match args.next().as_deref() {
                    Some("real") => Some(0),
                    Some(rows) => Some(rows.parse().unwrap_or_else(|_| fail("--text-rows needs a number or real"))),
                    None => fail("--text-rows needs a number or real"),
                }
            }
            "--prompt" => ab_prompt = Some(args.next().unwrap_or_else(|| fail("--prompt needs a text"))),
            "--live-only" => ab_jobs = false,
            "--seed" => seed = args.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| fail("--seed needs a number")),
            "--tag" => tag = args.next().unwrap_or_else(|| fail("--tag needs a name")),
            "--unload" => unload.push(args.next().unwrap_or_else(|| fail("--unload needs a model"))),
            "--out" => out = args.next().unwrap_or_else(|| fail("--out needs a file")),
            "--hub" => hub_base = Some(args.next().unwrap_or_else(|| fail("--hub needs a base URL"))),
            other => fail(&format!("unknown argument {other}")),
        }
    }
    if !unload.is_empty() {
        // A bench's models off the hub again, so the next one has its memory.
        let hub = BoxHub::locate(hub_base.as_deref()).unwrap_or_else(|e| fail(&e));
        for model in &unload {
            match hub.model(model, false) {
                Ok(took) => eprintln!("[unload] {model}: {:.1} s", took.as_secs_f64()),
                Err(e) => eprintln!("[unload] {model}: FAILED {e}"),
            }
        }
        eprintln!("[unload] {}", makepad_app_party::worker::gpu_line(&hub));
        return;
    }
    if live_costs {
        live_costs_by_size(hub_base.as_deref(), compute);
        return;
    }
    if let Some(picture) = &live_steps {
        compare_live_steps(hub_base.as_deref(), picture, &out, compute);
        return;
    }
    if let Some(picture) = &live_ab {
        redraws_for_ab(hub_base.as_deref(), picture, &out, &tag, compute, ab_prompt.as_deref(), ab_jobs, seed);
        return;
    }
    if check_hub {
        if let Some(picture) = &compare {
            compare_settings(hub_base.as_deref(), picture, &out);
        } else if sizes {
            check_sizes(hub_base.as_deref());
        } else {
            check(hub_base.as_deref());
        }
        // A session's log lives as long as its process: it stays for reading.
        loop {
            std::thread::park();
        }
    }
    let (server, urls, names, fingerprint) = if camtest {
        let party_code = party_code.unwrap_or_else(code::new_code);
        if !code::valid(&party_code) {
            fail("a party code is 6 to 32 of 23456789abcdefghjkmnpqrstuvwxyz");
        }
        let id = start::identity(&extra).unwrap_or_else(|e| fail(&e));
        let handler: Arc<dyn Handler> = Arc::new(CamTest::default());
        let server = Server::start(([0, 0, 0, 0], port).into(), id.tls, party_code.clone(), handler)
            .unwrap_or_else(|e| fail(&format!("listen on port {port}: {e}")));
        let urls = start::urls(id.lan, &extra, server.port, &party_code, "/camtest");
        (server, urls, id.names, id.fingerprint)
    } else {
        let running = start::party(port, party_code, &extra, hub_base.as_deref()).unwrap_or_else(|e| fail(&e));
        (running.server, running.urls, running.names, running.fingerprint)
    };
    if urls.is_empty() {
        eprintln!("[party] guests open: https://<this box>:{}/p/<code> (no LAN address found)", server.port);
    }
    for (i, url) in urls.iter().enumerate() {
        eprintln!("[party] {} {url}", if i == 0 { "guests open:" } else { "also:       " });
    }
    if let Some(url) = urls.first() {
        eprintln!("[party] the screen for a TV or projector: {url}/screen");
    }
    eprintln!("[party] certificate names: {names:?}");
    eprintln!("[party] certificate SHA-256: {fingerprint}");
    // The accept loop runs until the process ends.
    loop {
        std::thread::park();
    }
}

const PROMPT: &str = "a bright watercolour painting of the scene, soft evening light";
const SEED: u64 = 7;

/// A stand-in for a guest's picture: a gradient and a disc that moves with
/// `shift`, so live frames differ the way a short loop's do.
fn sample(width: usize, height: usize, shift: usize) -> Rgb {
    let mut data = Vec::with_capacity(width * height * 3);
    let (cx, cy, r) = ((width / 3 + shift * 4) as i64, (height / 2) as i64, (width / 5) as i64);
    for y in 0..height {
        for x in 0..width {
            let (dx, dy) = (x as i64 - cx, y as i64 - cy);
            if dx * dx + dy * dy < r * r {
                data.extend_from_slice(&[230, 180, 60]);
            } else {
                data.extend_from_slice(&[(x * 160 / width) as u8 + 40, (y * 120 / height) as u8 + 60, 150]);
            }
        }
    }
    Rgb { width, height, data }
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

fn job(label: &str, result: Result<JobTime, String>) {
    match result {
        Ok(t) => eprintln!("[check] {label}: {} ms (queued {} ms, ran {} ms)", ms(t.wall), ms(t.queued), ms(t.run)),
        Err(e) => eprintln!("[check] {label}: FAILED {e}"),
    }
}

fn residency(hub: &BoxHub, label: &str) {
    match hub.control() {
        Ok(control) => {
            let (models, gpu) = hub::residency(&control);
            let gpu = gpu.map(|(used, total)| format!("GPU {used}/{total} MB")).unwrap_or_else(|| "GPU ?".into());
            let models: Vec<String> = models
                .iter()
                .map(|(id, state, mb)| format!("{id} {state}{}", mb.map(|mb| format!(" {mb} MB")).unwrap_or_default()))
                .collect();
            eprintln!("[check] residency {label}: {gpu}; {}", if models.is_empty() { "nothing loaded".into() } else { models.join(", ") });
        }
        Err(e) => eprintln!("[check] residency {label}: FAILED {e}"),
    }
}

/// What the party asks of the hub, measured: see the module comment.
fn check(explicit: Option<&str>) {
    let hub = BoxHub::locate(explicit).unwrap_or_else(|e| fail(&e));
    eprintln!("[check] hub {}", hub.base);
    if let Ok(control) = hub.control() {
        if let Some(models) = control.get("models").and_then(|m| m.as_arr()) {
            for m in models {
                let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                if [FAST_MODEL, QUALITY_MODEL, VISION_MODEL, "flux2-dev", "qwen3.5-9b-vision"].contains(&id) {
                    eprintln!("[check] model {id}: {}", m.to_json());
                }
            }
        }
    }
    residency(&hub, "at start");
    let source = sample(512, 512, 0);
    let png = image::encode_png(&source).unwrap_or_else(|e| fail(&e));
    let jpeg = image::encode_jpeg(&source, 85).unwrap_or_else(|e| fail(&e));

    // The fast model, one image per job, as a take's frame without a live session.
    for i in 0..6 {
        let result = hub.edit(FAST_MODEL, &png, PROMPT, SEED, Some(4), Some(0.6), Some((512, 512)), &|| false);
        if i == 0 {
            if let Ok((png, _)) = &result {
                if let Ok(rgb) = image::decode_png(png) {
                    eprintln!("[check] klein answer {}x{}", rgb.width, rgb.height);
                }
            }
        }
        job(&format!("klein edit 512 4 steps #{i}"), result.map(|r| r.1));
    }
    residency(&hub, "after klein jobs");

    // The fast model live: a 2 s loop's 24 frames, one at a time.
    let opened = Instant::now();
    match hub.live(FAST_MODEL, LiveInput::Raw, (512, 512), PROMPT, 4, 0.6, SEED, Compute::STORED) {
        Ok(mut live) => {
            eprintln!("[check] live open: {} ms", ms(opened.elapsed()));
            let mut steady = Vec::new();
            for i in 0..24 {
                match live.redraw(&sample(512, 512, i), Duration::from_secs(300)) {
                    Ok((out, t)) => {
                        eprintln!(
                            "[check] live frame #{i}: {} ms here, box {:.0} ms (model {:.0}, text {:.0}) {}x{}",
                            ms(t.wall),
                            t.frame_ms,
                            t.model_ms,
                            t.text_encode_ms,
                            out.width,
                            out.height
                        );
                        if i >= 2 {
                            steady.push(t.wall);
                        }
                    }
                    Err(e) => {
                        eprintln!("[check] live frame #{i}: FAILED {e}");
                        break;
                    }
                }
                if i == 12 {
                    residency(&hub, "during live");
                }
            }
            if !steady.is_empty() {
                let mean = steady.iter().sum::<Duration>() / steady.len() as u32;
                eprintln!("[check] live steady: {} ms per frame over {} frames", ms(mean), steady.len());
            }
        }
        Err(e) => eprintln!("[check] live open: FAILED {e}"),
    }
    residency(&hub, "after live");

    // The vision model: a prompt from a picture.
    for i in 0..3 {
        let result = hub.describe(VISION_MODEL, &jpeg, "Describe this picture in one short sentence.", 60, &|| false);
        if let Ok((text, _)) = &result {
            eprintln!("[check] vision says: {}", text.trim().replace('\n', " "));
        }
        job(&format!("vision describe #{i}"), result.map(|r| r.1));
        if i == 0 {
            residency(&hub, "with vision");
        }
    }
    job("klein edit after vision", hub.edit(FAST_MODEL, &png, PROMPT, SEED, Some(4), Some(0.6), Some((512, 512)), &|| false).map(|r| r.1));
    residency(&hub, "klein back");

    // The quality models at 1024.
    let png_1024 = image::encode_png(&image::fit(&source, 1024, 1024)).unwrap_or_else(|e| fail(&e));
    for model in ["flux2-dev", QUALITY_MODEL] {
        for i in 0..2 {
            job(
                &format!("{model} edit 1024 20 steps #{i}"),
                hub.edit(model, &png_1024, PROMPT, SEED, Some(20), Some(0.75), Some((1024, 1024)), &|| false).map(|r| r.1),
            );
        }
        residency(&hub, &format!("with {model}"));
    }
    job("klein edit after quality", hub.edit(FAST_MODEL, &png, PROMPT, SEED, Some(4), Some(0.6), Some((512, 512)), &|| false).map(|r| r.1));
    residency(&hub, "at end");
    eprintln!("[check] done (the process stays so its log can be read)");
}

/// The fast model live at the loop's sizes, from the phone's preview size
/// up to past what a big screen shows, and at more steps.
fn check_sizes(explicit: Option<&str>) {
    let hub = BoxHub::locate(explicit).unwrap_or_else(|e| fail(&e));
    residency(&hub, "at start");
    for (side, steps) in [(768usize, 4u32), (1024, 4), (1280, 4), (1536, 4), (1024, 8)] {
        match hub.live(FAST_MODEL, LiveInput::Raw, (side as u32, side as u32), PROMPT, steps, 0.6, SEED, Compute::STORED) {
            Ok(mut live) => {
                let mut times = Vec::new();
                for i in 0..8 {
                    match live.redraw(&sample(side, side, i), Duration::from_secs(300)) {
                        Ok((_, t)) => times.push((t.wall, t.model_ms)),
                        Err(e) => {
                            eprintln!("[check] live {side} {steps} steps #{i}: FAILED {e}");
                            break;
                        }
                    }
                }
                let steady = &times[times.len().min(2)..];
                if !steady.is_empty() {
                    let wall = steady.iter().map(|t| t.0).sum::<Duration>() / steady.len() as u32;
                    let model = steady.iter().map(|t| t.1).sum::<f64>() / steady.len() as f64;
                    eprintln!("[check] live {side}x{side} {steps} steps: {} ms per frame (model {model:.0} ms) over {} frames", ms(wall), steady.len());
                }
            }
            Err(e) => eprintln!("[check] live {side} {steps} steps: FAILED {e}"),
        }
        residency(&hub, &format!("after live {side} {steps} steps"));
    }
    eprintln!("[check] done (the process stays so its log can be read)");
}

/// The loop redraw at the settings a faster loop could use, side by side. A
/// 24-frame loop panned across `picture` goes through each setting; the
/// sheet's first row is the loop, then one row per setting (six frames of
/// it), and the log says what each row is and what a frame of it cost.
fn compare_settings(explicit: Option<&str>, picture: &str, out: &str) {
    const FRAMES: usize = 24;
    const SHOWN: [usize; 6] = [0, 4, 8, 12, 16, 20];
    const TILE: usize = 192;
    let hub = BoxHub::locate(explicit).unwrap_or_else(|e| fail(&e));
    let source = std::fs::read(picture).map_err(|e| e.to_string()).and_then(|b| image::decode_jpeg(&b)).unwrap_or_else(|e| fail(&format!("{picture}: {e}")));
    // A square window slides across the picture: a slow pan, as a phone held still-ish makes.
    let side = source.width.min(source.height) * 4 / 5;
    let frames: Vec<Rgb> = (0..FRAMES)
        .map(|i| {
            let x0 = (source.width - side) * i / (FRAMES - 1);
            let y0 = (source.height - side) / 2;
            let mut data = Vec::with_capacity(side * side * 3);
            for y in y0..y0 + side {
                let row = (y * source.width + x0) * 3;
                data.extend_from_slice(&source.data[row..row + side * 3]);
            }
            image::fit(&Rgb { width: side, height: side, data }, 512, 512)
        })
        .collect();
    let settings: [(usize, u32, f32); 4] = [(512, 4, 0.6), (512, 3, 0.6), (384, 4, 0.6), (384, 3, 0.6)];
    let mut sheet = Rgb { width: TILE * SHOWN.len(), height: TILE * (settings.len() + 1), data: vec![0; TILE * SHOWN.len() * TILE * (settings.len() + 1) * 3] };
    let mut place = |row: usize, column: usize, rgb: &Rgb| {
        let tile = image::fit(rgb, TILE, TILE);
        for y in 0..TILE {
            let to = ((row * TILE + y) * sheet.width + column * TILE) * 3;
            sheet.data[to..to + TILE * 3].copy_from_slice(&tile.data[y * TILE * 3..(y + 1) * TILE * 3]);
        }
    };
    for (column, &i) in SHOWN.iter().enumerate() {
        place(0, column, &frames[i]);
    }
    eprintln!("[compare] row 1: the loop, frames {SHOWN:?} of {FRAMES}");
    for (k, &(side, steps, strength)) in settings.iter().enumerate() {
        let run = steps - ((1.0 - strength) * steps as f32).floor() as u32;
        let mut live = match hub.live(FAST_MODEL, LiveInput::Raw, (side as u32, side as u32), PROMPT, steps, strength, SEED, Compute::STORED) {
            Ok(live) => live,
            Err(e) => {
                eprintln!("[compare] {side} px {steps} steps: FAILED {e}");
                continue;
            }
        };
        let (mut wall, mut model, mut counted) = (Duration::ZERO, 0.0, 0u32);
        for (i, frame) in frames.iter().enumerate() {
            match live.redraw(&image::fit(frame, side, side), Duration::from_secs(300)) {
                Ok((drawn, time)) => {
                    // The first frame of a session pays for its prompt and warm-up.
                    if i > 0 {
                        (wall, model, counted) = (wall + time.wall, model + time.model_ms, counted + 1);
                    }
                    if let Some(column) = SHOWN.iter().position(|&s| s == i) {
                        place(k + 1, column, &drawn);
                    }
                }
                Err(e) => {
                    eprintln!("[compare] {side} px {steps} steps frame {i}: FAILED {e}");
                    break;
                }
            }
        }
        let n = counted.max(1);
        eprintln!(
            "[compare] row {}: {side} px, {steps} steps at strength {strength} ({run} model steps): {} ms a frame (model {:.0} ms) over {counted} frames",
            k + 2,
            ms(wall / n),
            model / n as f64
        );
    }
    match image::encode_jpeg(&sheet, 88).map_err(|e| e.to_string()).and_then(|jpeg| std::fs::write(out, jpeg).map_err(|e| e.to_string())) {
        Ok(()) => eprintln!("[compare] sheet: {}", std::fs::canonicalize(out).map_or(out.to_string(), |p| p.display().to_string())),
        Err(e) => eprintln!("[compare] sheet {out}: FAILED {e}"),
    }
    eprintln!("[compare] done (the process stays so its log can be read)");
}

/// Live mode's redraw at the step counts it could use, on the same frames:
/// a slow tilt down `picture` at the live size goes through each setting in
/// turn, one frame at a time. The log gives, per setting, the box's time a
/// frame (what sets the frames a second a call gets) and the round trip of a
/// frame; `out` (a directory) gets the redraws of the same moments, one file
/// per moment and setting, and a sheet per moment (the camera's frame on
/// top, then the settings in the order logged).
fn compare_live_steps(explicit: Option<&str>, picture: &str, out: &str, compute: Compute) {
    const FRAMES: usize = 40;
    const WARM: usize = 3;
    const MOMENTS: [usize; 3] = [12, 26, 39];
    let settings: [(u32, f32); 4] = [(4, 0.6), (3, 0.6), (2, 0.6), (2, 0.5)];
    let hub = BoxHub::locate(explicit).unwrap_or_else(|e| fail(&e));
    let source = std::fs::read(picture).map_err(|e| e.to_string()).and_then(|b| image::decode_jpeg(&b)).unwrap_or_else(|e| fail(&format!("{picture}: {e}")));
    std::fs::create_dir_all(out).unwrap_or_else(|e| fail(&format!("{out}: {e}")));
    let (width, height) = makepad_app_party::live::live_size(1280, 720);
    let (width, height) = (width as usize, height as usize);
    let frames = tilt(&source, FRAMES, width, height);
    let prompt = makepad_app_party::worker::plain_prompt(None, 0);
    eprintln!("[steps] {FRAMES} frames at {width}x{height}, \"{prompt}\"; the first {WARM} of each setting warm it up");
    let save = |name: String, rgb: &Rgb| {
        let path = std::path::Path::new(out).join(name);
        if let Err(e) = image::encode_jpeg(rgb, 92).map_err(|e| e.to_string()).and_then(|jpeg| std::fs::write(&path, jpeg).map_err(|e| e.to_string())) {
            eprintln!("[steps] {}: FAILED {e}", path.display());
        }
    };
    let mut sheets: Vec<Rgb> = MOMENTS.iter().map(|&i| Rgb { width, height: height * (settings.len() + 1), data: frames[i].data.clone() }).collect();
    for (&i, sheet) in MOMENTS.iter().zip(&mut sheets) {
        save(format!("frame{i:02}-camera.jpg"), &frames[i]);
        sheet.data.resize(width * height * 3 * (settings.len() + 1), 0);
    }
    for (k, &(steps, strength)) in settings.iter().enumerate() {
        let run = steps - makepad_ai_hub::backend::img2img_start_step(strength, steps);
        let mut live = match hub.live(FAST_MODEL, LiveInput::Raw, (width as u32, height as u32), &prompt, steps, strength, SEED, compute) {
            Ok(live) => live,
            Err(e) => {
                eprintln!("[steps] {steps} steps at {strength}: FAILED {e}");
                continue;
            }
        };
        let (mut box_ms, mut model_ms, mut trips) = (Vec::new(), Vec::new(), Vec::new());
        for (i, frame) in frames.iter().enumerate() {
            match live.redraw(frame, Duration::from_secs(300)) {
                Ok((drawn, time)) => {
                    if i >= WARM {
                        box_ms.push(time.frame_ms);
                        model_ms.push(time.model_ms);
                        trips.push(time.wall.as_secs_f64() * 1000.0);
                    }
                    if let Some(m) = MOMENTS.iter().position(|&at| at == i) {
                        save(format!("frame{i:02}-{steps}steps-{:02}.jpg", (strength * 100.0).round() as u32), &drawn);
                        let at = width * height * 3 * (k + 1);
                        if drawn.width == width && drawn.height == height {
                            sheets[m].data[at..at + width * height * 3].copy_from_slice(&drawn.data);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[steps] {steps} steps at {strength} frame {i}: FAILED {e}");
                    break;
                }
            }
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let median = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(0.0)
        };
        let (frame, model) = (mean(&box_ms), mean(&model_ms));
        let longest = trips.iter().copied().fold(0.0, f64::max);
        eprintln!(
            "[steps] {steps} steps at strength {strength} ({run} model steps): the box {frame:.0} ms a frame (model {model:.0} ms) = {:.1} fps; round trip median {:.0} ms, longest {longest:.0} ms, over {} frames",
            1000.0 / frame.max(1.0),
            median(&mut trips),
            box_ms.len()
        );
    }
    for (&i, sheet) in MOMENTS.iter().zip(&sheets) {
        save(format!("sheet-frame{i:02}.jpg"), sheet);
    }
    eprintln!("[steps] done: {out}");
}

/// What a live frame costs the box by size and by the model steps it runs:
/// per size, the model's time at 0 to 3 steps (0: the camera's frame only
/// goes through the image encoder and decoder) and a straight line through
/// them, the cost of a frame before any step and of each step.
fn live_costs_by_size(explicit: Option<&str>, compute: Compute) {
    const FRAMES: usize = 16;
    const WARM: usize = 3;
    // (steps, strength) running 0, 1, 2 and 3 model steps.
    let settings: [(u32, f32); 4] = [(4, 0.0), (2, 0.5), (3, 0.6), (4, 0.6)];
    let hub = BoxHub::locate(explicit).unwrap_or_else(|e| fail(&e));
    let prompt = makepad_app_party::worker::plain_prompt(None, 0);
    for (width, height) in [(384usize, 216usize), (512, 288), (768, 432)] {
        let mut points = Vec::new();
        for &(steps, strength) in &settings {
            let run = steps - makepad_ai_hub::backend::img2img_start_step(strength, steps);
            let mut live = match hub.live(FAST_MODEL, LiveInput::Raw, (width as u32, height as u32), &prompt, steps, strength, SEED, compute) {
                Ok(live) => live,
                Err(e) => {
                    eprintln!("[costs] {width}x{height} {run} steps: FAILED {e}");
                    continue;
                }
            };
            let (mut model, mut frame, mut prep, mut counted) = (0.0, 0.0, 0.0, 0usize);
            let (mut encode, mut denoise, mut decode) = (0.0, 0.0, 0.0);
            for i in 0..FRAMES {
                match live.redraw(&sample(width, height, i), Duration::from_secs(300)) {
                    Ok((_, time)) if i >= WARM => {
                        (model, frame, prep, counted) = (model + time.model_ms, frame + time.frame_ms, prep + time.prep_ms, counted + 1);
                        (encode, denoise, decode) = (encode + time.encode_ms, denoise + time.denoise_ms, decode + time.decode_ms);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("[costs] {width}x{height} {run} steps frame {i}: FAILED {e}");
                        break;
                    }
                }
            }
            let n = counted.max(1) as f64;
            eprintln!(
                "[costs] {width}x{height}, {run} model steps ({steps} at {strength}): model {:.0} ms (encode {:.1}, steps {:.1}, decode {:.1}, the rest {:.1}), frame {:.0} ms, prep {:.1} ms over {counted} frames",
                model / n,
                encode / n,
                denoise / n,
                decode / n,
                (model - encode - denoise - decode) / n,
                frame / n,
                prep / n
            );
            if counted > 0 {
                points.push((run as f64, model / n));
            }
        }
        if points.len() >= 2 {
            let n = points.len() as f64;
            let (sx, sy) = (points.iter().map(|p| p.0).sum::<f64>(), points.iter().map(|p| p.1).sum::<f64>());
            let sxx = points.iter().map(|p| p.0 * p.0).sum::<f64>();
            let sxy = points.iter().map(|p| p.0 * p.1).sum::<f64>();
            let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
            let fixed = (sy - slope * sx) / n;
            eprintln!("[costs] {width}x{height}: {fixed:.0} ms a frame before any step, {slope:.0} ms a step");
        }
    }
    eprintln!("[costs] done");
}

/// `frames` frames of a window of `width:height`'s shape tilting down
/// `source` from top to bottom, as a phone held at a party moves.
fn tilt(source: &Rgb, frames: usize, width: usize, height: usize) -> Vec<Rgb> {
    let window = (source.width * height / width).min(source.height);
    (0..frames)
        .map(|i| {
            let y0 = (source.height - window) * i / (frames - 1).max(1);
            let rows = &source.data[y0 * source.width * 3..(y0 + window) * source.width * 3];
            image::resize(&Rgb { width: source.width, height: window, data: rows.to_vec() }, width, height)
        })
        .collect()
}

/// The redraws of the same frames kept bit for bit, for an A/B of the hub's
/// compute (arithmetic, text rows) or of a hub before and after a change: a
/// tilt down `picture` through live mode's settings after `prompt` (the
/// party's plain one when `None`) and, computed as stored, through a
/// recorded loop's; then (with `jobs`) one klein job and one flux2-dev job
/// of the picture, which run as stored whatever the session before them ran in.
/// `out` gets `<tag>-live.rgb` and `<tag>-loop.rgb` (raw RGB frames one
/// after another), `<tag>-klein-job.png`, `<tag>-dev-job.png` and
/// `camera-live.rgb`, `camera-loop.rgb` (the frames sent); the log gives
/// sizes and the stage times.
#[allow(clippy::too_many_arguments)]
fn redraws_for_ab(
    explicit: Option<&str>,
    picture: &str,
    out: &str,
    tag: &str,
    compute: Compute,
    prompt: Option<&str>,
    jobs: bool,
    seed: u64,
) {
    use makepad_app_party::live::{live_size, LIVE_STEPS, LIVE_STRENGTH};
    use makepad_app_party::worker::{FAST_STEPS, LOOP_SIDE, LOOP_STRENGTH};
    let hub = BoxHub::locate(explicit).unwrap_or_else(|e| fail(&e));
    let source = std::fs::read(picture).map_err(|e| e.to_string()).and_then(|b| image::decode_jpeg(&b)).unwrap_or_else(|e| fail(&format!("{picture}: {e}")));
    std::fs::create_dir_all(out).unwrap_or_else(|e| fail(&format!("{out}: {e}")));
    let prompt = prompt.map_or_else(|| makepad_app_party::worker::plain_prompt(None, 0), str::to_string);
    let (lw, lh) = live_size(1280, 720);
    let mut runs = vec![("live", (lw as usize, lh as usize), 40usize, LIVE_STEPS, LIVE_STRENGTH)];
    if compute == Compute::STORED {
        runs.push(("loop", (LOOP_SIDE, LOOP_SIDE), 8, FAST_STEPS, LOOP_STRENGTH));
    }
    for (name, (width, height), count, steps, strength) in runs {
        let frames = tilt(&source, count, width, height);
        let camera: Vec<u8> = frames.iter().flat_map(|f| f.data.iter().copied()).collect();
        let _ = std::fs::write(std::path::Path::new(out).join(format!("camera-{name}.rgb")), camera);
        let mut live = match hub.live(FAST_MODEL, LiveInput::Raw, (width as u32, height as u32), &prompt, steps, strength, seed, compute) {
            Ok(live) => live,
            Err(e) => {
                eprintln!("[ab] {name}: FAILED {e}");
                continue;
            }
        };
        let (mut drawn, mut times) = (Vec::with_capacity(width * height * 3 * count), Vec::new());
        for (i, frame) in frames.iter().enumerate() {
            match live.redraw(frame, Duration::from_secs(300)) {
                Ok((rgb, time)) if rgb.width == width && rgb.height == height => {
                    drawn.extend_from_slice(&rgb.data);
                    if i >= 2 {
                        times.push(time);
                    }
                }
                Ok((rgb, _)) => {
                    eprintln!("[ab] {name} frame {i}: {}x{} where {width}x{height} was asked", rgb.width, rgb.height);
                    break;
                }
                Err(e) => {
                    eprintln!("[ab] {name} frame {i}: FAILED {e}");
                    break;
                }
            }
        }
        let path = std::path::Path::new(out).join(format!("{tag}-{name}.rgb"));
        if let Err(e) = std::fs::write(&path, &drawn) {
            eprintln!("[ab] {}: FAILED {e}", path.display());
        }
        let n = times.len().max(1) as f64;
        let mean = |f: fn(&hub::LiveTime) -> f64| times.iter().map(f).sum::<f64>() / n;
        eprintln!(
            "[ab] {tag} {name}: {} frames at {width}x{height}, {steps} @ {strength}, {:?}, text rows {}, seed {seed}: model {:.0} ms (encode {:.1}, steps {:.1}, decode {:.1}), frame {:.0} ms",
            drawn.len() / (width * height * 3),
            compute.arithmetic,
            compute.text_rows.map_or("all".to_string(), |rows| rows.to_string()),
            mean(|t| t.model_ms),
            mean(|t| t.encode_ms),
            mean(|t| t.denoise_ms),
            mean(|t| t.decode_ms),
            mean(|t| t.frame_ms)
        );
        drop(live);
        eprintln!("[ab] {tag} after {name}: {}", makepad_app_party::worker::gpu_line(&hub));
    }
    let still = image::resize(&source, 512, 512);
    let png = image::encode_png(&still).unwrap_or_else(|e| fail(&e));
    let job_runs = if jobs { vec![("klein-job", FAST_MODEL, FAST_STEPS), ("dev-job", QUALITY_MODEL, 8)] } else { Vec::new() };
    for (name, model, steps) in job_runs {
        match hub.edit(model, &png, &prompt, seed, Some(steps), None, Some((512, 512)), &|| false) {
            Ok((bytes, time)) => {
                let path = std::path::Path::new(out).join(format!("{tag}-{name}.png"));
                if let Err(e) = std::fs::write(&path, &bytes) {
                    eprintln!("[ab] {}: FAILED {e}", path.display());
                }
                eprintln!("[ab] {tag} {name}: {model}, {steps} steps at 512x512, seed {seed}: run {:.1} s", time.run.as_secs_f64());
                eprintln!("[ab] {tag} after {name}: {}", makepad_app_party::worker::gpu_line(&hub));
            }
            Err(e) => eprintln!("[ab] {tag} {name}: FAILED {e}"),
        }
    }
    eprintln!("[ab] done: {out}");
}

fn fail(message: &str) -> ! {
    eprintln!("makepad-party-server: {message}");
    std::process::exit(2);
}
