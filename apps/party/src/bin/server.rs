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
//! costs; with `--sizes`, the fast model's live frame at larger sizes and
//! more steps (what an upgrade of a loop costs). It prints one line per step (to stderr, like everything here: the
//! log a box keeps of a process) and then stays, so the log can be read.

// A native service: never built for wasm.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use makepad_app_party::hub::{self, BoxHub, JobTime, LiveInput, FAST_MODEL, QUALITY_MODEL, VISION_MODEL};
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
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).unwrap_or_else(|| fail("--port needs a number")),
            "--code" => party_code = Some(args.next().unwrap_or_else(|| fail("--code needs a code"))),
            "--name" => extra.push(args.next().unwrap_or_else(|| fail("--name needs a name or address"))),
            "--camtest" => camtest = true,
            "--check-hub" => check_hub = true,
            "--sizes" => sizes = true,
            "--hub" => hub_base = Some(args.next().unwrap_or_else(|| fail("--hub needs a base URL"))),
            other => fail(&format!("unknown argument {other}")),
        }
    }
    if check_hub {
        if sizes {
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
        let result = hub.edit(FAST_MODEL, &png, PROMPT, SEED, Some(4), Some(0.6), Some((512, 512)));
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
    match hub.live(FAST_MODEL, LiveInput::Raw, (512, 512), PROMPT, 4, 0.6, SEED) {
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
        let result = hub.describe(VISION_MODEL, &jpeg, "Describe this picture in one short sentence.", 60);
        if let Ok((text, _)) = &result {
            eprintln!("[check] vision says: {}", text.trim().replace('\n', " "));
        }
        job(&format!("vision describe #{i}"), result.map(|r| r.1));
        if i == 0 {
            residency(&hub, "with vision");
        }
    }
    job("klein edit after vision", hub.edit(FAST_MODEL, &png, PROMPT, SEED, Some(4), Some(0.6), Some((512, 512))).map(|r| r.1));
    residency(&hub, "klein back");

    // The quality models at 1024.
    let png_1024 = image::encode_png(&image::fit(&source, 1024, 1024)).unwrap_or_else(|e| fail(&e));
    for model in ["flux2-dev", QUALITY_MODEL] {
        for i in 0..2 {
            job(
                &format!("{model} edit 1024 20 steps #{i}"),
                hub.edit(model, &png_1024, PROMPT, SEED, Some(20), Some(0.75), Some((1024, 1024))).map(|r| r.1),
            );
        }
        residency(&hub, &format!("with {model}"));
    }
    job("klein edit after quality", hub.edit(FAST_MODEL, &png, PROMPT, SEED, Some(4), Some(0.6), Some((512, 512))).map(|r| r.1));
    residency(&hub, "at end");
    eprintln!("[check] done (the process stays so its log can be read)");
}

/// The fast model live at the sizes and step counts an upgrade could use.
fn check_sizes(explicit: Option<&str>) {
    let hub = BoxHub::locate(explicit).unwrap_or_else(|e| fail(&e));
    residency(&hub, "at start");
    for (side, steps) in [(512usize, 4u32), (512, 8), (768, 4), (768, 8), (1024, 4), (1024, 8)] {
        match hub.live(FAST_MODEL, LiveInput::Raw, (side as u32, side as u32), PROMPT, steps, 0.6, SEED) {
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

fn fail(message: &str) -> ! {
    eprintln!("makepad-party-server: {message}");
    std::process::exit(2);
}
