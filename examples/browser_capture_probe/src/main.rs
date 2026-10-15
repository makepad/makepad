//! Lossless browser capture, end to end: a frame-counter page in the browser
//! host, captured through its frame ring (every frame, back-pressure instead
//! of losses) and audio ring, encoded into an MP4 straight from the shared
//! GPU textures (no CPU copy), then checked:
//!
//! - capture: the page's own counter (a bit strip it paints every frame,
//!   read back as ring probes) has no gap over the recording;
//! - file: decoding the MP4 gives the same frames, in order, none missing;
//! - A/V: each flash frame and the beep the page starts in the same frame
//!   are within one frame (16.7 ms) on the capture clock.
//!
//! `BROWSER_CAPTURE_SLOW_MS=40` makes the consumer slower than the page
//! (back-pressure: still no gap, the page runs at the consumer's pace).
//!
//! Two more modes, for recorders that take a page's video (Stage's
//! recording mode):
//! - `--make-video <dir> [fps]` writes `counter.mp4` (1280x720 at 60 fps or `fps`, 40 s:
//!   each frame's number in a 32-cell strip at its bottom, a flash frame
//!   every 60 frames with a 100 ms beep starting at it, H.264/AAC) and
//!   `counter.html` (plays `counter.webm` on a click). The browser host's
//!   Chromium has no H.264: make the WebM once with any VP9/Opus encoder,
//!   e.g. `ffmpeg -i counter.mp4 -c:v libvpx-vp9 -b:v 6M -c:a libopus counter.webm`;
//! - `--analyze <recording dir>` reads such a recording's segments
//!   (`v*.mp4` pictures, `a*.wav` sound, one timeline) and checks the same
//!   three things: no counter gap, pictures in order, beep within a frame of
//!   its flash.
//!
//! Prints PASS/FAIL lines and exits 0 on success. Run hidden:
//! `MAKEPAD_HIDE_WINDOWS=1 target/release/makepad-example-browser-capture-probe`
//! (the browser host must be built; MP4 at $BROWSER_CAPTURE_OUT or the temp dir).

pub use makepad_widgets;

use makepad_browser_view::{AudioRecord, BrowserViews, CaptureSpec, ViewId};
use makepad_widgets::makepad_platform::video_file::{VideoFileCodec, VideoFileDecoder, VideoFileEncoder, VideoFileEncoderOptions};
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(200, 100)
                body +: {}
            }
        }
    }
}

/// The page's picture and the strip under it, in pixels (dpi 1).
const W: u32 = 640;
const H: u32 = 360;
const STRIP: u32 = 16;
const CELLS: u32 = 32;
const CELL: u32 = W / CELLS;
const RECORD_SECONDS: f64 = 8.0;
const TIMEOUT: f64 = 90.0;
/// One frame at 60 Hz.
const FRAME_MS: f64 = 1000.0 / 60.0;

const PAGE: &str = r#"<!doctype html><html><body style="margin:0;background:#000;overflow:hidden">
<canvas id="c" width="640" height="376"></canvas><script>
const c = document.getElementById('c'), g = c.getContext('2d');
let n = 0, ac = null;
document.addEventListener('mousedown', () => { if (!ac) { ac = new AudioContext(); ac.resume(); setTimeout(() => console.log('audio latency: base ' + ac.baseLatency + ' s, output ' + ac.outputLatency + ' s'), 1000); } });
function beep() {
  if (!ac) return;
  const o = ac.createOscillator(), v = ac.createGain();
  o.frequency.value = 1000; v.gain.value = 0.5;
  o.connect(v).connect(ac.destination);
  const t = ac.currentTime; o.start(t); o.stop(t + 0.1);
}
function frame() {
  n++;
  const flash = n % 60 == 0;
  g.fillStyle = flash ? '#fff' : '#202020'; g.fillRect(0, 0, 640, 360);
  g.fillStyle = flash ? '#000' : '#fff'; g.font = '120px monospace'; g.fillText(String(n), 40, 220);
  for (let b = 0; b < 32; b++) {
    const bit = b < 24 ? (n >> (23 - b)) & 1 : (b == 24 ? (flash ? 1 : 0) : 0);
    g.fillStyle = bit ? '#fff' : '#000'; g.fillRect(b * 20, 360, 20, 16);
  }
  if (flash) beep();
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
</script></body></html>"#;

fn probes() -> Vec<[u32; 2]> {
    (0..CELLS).map(|b| [b * CELL + CELL / 2, H + STRIP / 2]).collect()
}

/// The counter and the flash bit of a strip, from one luminance per cell.
fn decode(cells: &[u8]) -> Option<(u32, bool)> {
    if cells.len() < CELLS as usize || cells.iter().all(|c| *c == 0) {
        return None;
    }
    let bit = |b: usize| cells[b] > 127;
    let counter = (0..24).fold(0u32, |n, b| (n << 1) | bit(b) as u32);
    if (25..CELLS as usize).any(bit) {
        return None;
    }
    Some((counter, bit(24)))
}

#[derive(PartialEq)]
enum Phase {
    Loading,
    Warm(f64),
    Recording(f64),
}

struct Captured {
    counter: u32,
    flash: bool,
    unix_ms: i64,
}

struct Probe {
    views: BrowserViews,
    view: ViewId,
    started: f64,
    phase: Phase,
    valid_run: u32,
    encoder: Option<VideoFileEncoder>,
    path: String,
    e0: Option<u64>,
    frames: Vec<Captured>,
    /// (pts_ms, samples interleaved stereo) of the recording.
    audio: Vec<(i64, Vec<f32>)>,
    timer: Timer,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    probe: Option<Probe>,
}

fn fail(probe: &mut Probe, cx: &mut Cx, message: &str) -> ! {
    println!("FAIL {message}");
    probe.views.shutdown(cx);
    std::process::exit(1);
}

impl Probe {
    fn start(cx: &mut Cx) -> Probe {
        let mut views = BrowserViews::new("makepad-example-browser-capture-probe");
        let url = format!("data:text/html;charset=utf-8,{}", percent_encode(PAGE));
        let Some(view) = views.open(&url, true, 0xff00_0000) else {
            println!("FAIL no browser host (build makepad-app-browser-host)");
            std::process::exit(1);
        };
        views.set_size(view, dvec2(W as f64, (H + STRIP) as f64), 1.0);
        views.set_capture(view, Some(CaptureSpec { slots: 8, probes: probes() }));
        views.set_audio(view, true);
        let path = std::env::var("BROWSER_CAPTURE_OUT")
            .unwrap_or_else(|_| std::env::temp_dir().join("browser-capture-probe.mp4").display().to_string());
        Probe {
            views,
            view,
            started: Cx::monotonic_now(),
            phase: Phase::Loading,
            valid_run: 0,
            encoder: None,
            path,
            e0: None,
            frames: Vec::new(),
            audio: Vec::new(),
            timer: cx.start_interval(0.01),
        }
    }

    fn step(&mut self, cx: &mut Cx) {
        let now = Cx::monotonic_now();
        if now - self.started > TIMEOUT {
            let phase = match self.phase {
                Phase::Loading => "the page never painted its strip",
                _ => "the recording never finished",
            };
            fail(self, cx, &format!("timeout: {phase}"));
        }
        if let Some(error) = self.views.state(self.view).and_then(|s| s.error.clone()) {
            fail(self, cx, &format!("page error: {error}"));
        }
        let leases = self.views.take_captured(self.view);
        let audio = self.views.take_audio(self.view);
        for line in self.views.take_console(self.view) {
            println!("page console: {}", line.message);
        }
        match self.phase {
            Phase::Loading => {
                for lease in &leases {
                    if lease.info.seq % 30 == 1 {
                        println!("frame {} {}x{} probes {:?}", lease.info.seq, lease.info.width, lease.info.height, &lease.info.probes);
                    }
                    if decode(&lease.info.probes).is_some() {
                        self.valid_run += 1;
                    } else {
                        self.valid_run = 0;
                    }
                }
                if self.valid_run >= 30 {
                    // A user gesture lets the page's AudioContext play.
                    if let Some(hosted) = self.views.hosted_mut(self.view) {
                        hosted.mouse_down(dvec2(10.0, 10.0), makepad_widgets::makepad_platform::studio::MouseButton::PRIMARY, Cx::time_now(), &KeyModifiers::default());
                        hosted.mouse_up(Some(dvec2(10.0, 10.0)), makepad_widgets::makepad_platform::studio::MouseButton::PRIMARY, Cx::time_now(), &KeyModifiers::default(), false);
                        hosted.ensure_beat(cx);
                    }
                    // The encoder opens now, while the page warms up: opening it takes long on some
                    // platforms (Windows' Media Foundation MFT: ~430 ms), and a consumer that stalls
                    // during the recording makes Chromium drop the page's frames.
                    let options = VideoFileEncoderOptions {
                        codec: VideoFileCodec::H264,
                        width: W,
                        height: H + STRIP,
                        fps_num: 60,
                        fps_den: 1,
                        video_bitrate_bps: 20_000_000,
                        audio: None,
                        keyframe_only: true,
                    };
                    let _ = std::fs::remove_file(&self.path);
                    let opened = std::time::Instant::now();
                    match VideoFileEncoder::new(&self.path, options) {
                        Ok(encoder) => self.encoder = Some(encoder),
                        Err(error) => fail(self, cx, &format!("encoder: {error}")),
                    }
                    println!("encoder opened in {} ms", opened.elapsed().as_millis());
                    self.phase = Phase::Warm(now);
                }
            }
            Phase::Warm(since) => {
                if now - since > 1.5 {
                    println!("recording {RECORD_SECONDS} s of the page into {}", self.path);
                    self.phase = Phase::Recording(now);
                }
            }
            Phase::Recording(since) => {
                for lease in leases {
                    let Some((counter, flash)) = decode(&lease.info.probes) else {
                        fail(self, cx, &format!("frame {} has no readable strip", lease.info.seq));
                    };
                    let e0 = *self.e0.get_or_insert(lease.info.elapsed_ns);
                    let pts = ((lease.info.elapsed_ns - e0) / 100) as i64;
                    let region = [0, 0, W, H + STRIP];
                    let pushed = self.encoder.as_mut().unwrap().push_frame_shared_texture(lease.handle, region, Some(pts));
                    if let Err(error) = pushed {
                        fail(self, cx, &format!("encoding frame {counter}: {error}"));
                    }
                    self.frames.push(Captured { counter, flash, unix_ms: lease.info.unix_ms });
                    // The slot goes back to the page here.
                    drop(lease);
                    // A slow consumer (BROWSER_CAPTURE_SLOW_MS): the page
                    // must wait for it instead of losing frames.
                    if let Some(ms) = std::env::var("BROWSER_CAPTURE_SLOW_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
                        std::thread::sleep(std::time::Duration::from_millis(ms));
                    }
                }
                for record in audio {
                    if let AudioRecord::Packet(packet) = record {
                        self.audio.push((packet.pts_ms, packet.samples));
                    }
                }
                if now - since > RECORD_SECONDS {
                    self.finish(cx);
                }
            }
        }
    }

    fn finish(&mut self, cx: &mut Cx) -> ! {
        self.views.set_capture(self.view, None);
        let encoder = self.encoder.take().unwrap();
        if let Err(error) = encoder.finish() {
            fail(self, cx, &format!("finishing the MP4: {error}"));
        }
        let mut ok = true;
        // Capture: the page's counter, frame after frame.
        let gaps: Vec<(u32, u32)> = self.frames.windows(2).filter(|w| w[1].counter != w[0].counter + 1).map(|w| (w[0].counter, w[1].counter)).collect();
        let span_ms = self.frames.last().map_or(0, |f| f.unix_ms) - self.frames.first().map_or(0, |f| f.unix_ms);
        let fps = (self.frames.len().saturating_sub(1)) as f64 * 1000.0 / span_ms.max(1) as f64;
        if gaps.is_empty() && self.frames.len() > 60 {
            println!("PASS capture: {} frames, counter {}..{} without a gap, {:.1} fps", self.frames.len(), self.frames[0].counter, self.frames.last().unwrap().counter, fps);
        } else {
            ok = false;
            println!("FAIL capture: {} frames, {} gaps (first {:?})", self.frames.len(), gaps.len(), gaps.first());
        }
        // File: decoded back, the same frames in the same order.
        match decoded_counters(&self.path) {
            Ok(counters) => {
                let expected: Vec<u32> = self.frames.iter().map(|f| f.counter).collect();
                if counters == expected {
                    println!("PASS file: {} frames decoded, all present and in order", counters.len());
                } else {
                    ok = false;
                    let first_diff = counters.iter().zip(&expected).position(|(a, b)| a != b);
                    println!("FAIL file: {} frames decoded vs {} captured (first difference at {:?})", counters.len(), expected.len(), first_diff);
                    let at = first_diff.unwrap_or(counters.len().min(expected.len())).saturating_sub(2);
                    println!("  decoded {:?}\n  captured {:?}", &counters[at.min(counters.len())..(at + 8).min(counters.len())], &expected[at.min(expected.len())..(at + 8).min(expected.len())]);
                    let mut distinct = counters.clone();
                    distinct.dedup();
                    println!("  decoded: {} distinct runs, last {:?}", distinct.len(), &counters[counters.len().saturating_sub(4)..]);
                }
            }
            Err(error) => {
                ok = false;
                println!("FAIL file: {error}");
            }
        }
        // How the packets' own stamps sit against a steady stream started at
        // the first one (what a recorder placing contiguous sound assumes).
        if self.audio.len() > 10 {
            let first = self.audio[0].0;
            let mut frames = 0usize;
            let residuals: Vec<f64> = self.audio.iter().map(|(pts, samples)| {
                let r = (*pts - first) as f64 - frames as f64 * 1000.0 / 48_000.0;
                frames += samples.len() / 2;
                r
            }).collect();
            let mut sorted = residuals.clone();
            sorted.sort_by(f64::total_cmp);
            println!("audio stamps vs a steady stream from the first packet: median {:+.1} ms, min {:+.1}, max {:+.1}", sorted[sorted.len() / 2], sorted[0], sorted[sorted.len() - 1]);
        }
        // A/V: flash frames against beep onsets.
        let flashes: Vec<f64> = self.frames.windows(2).filter(|w| w[1].flash && !w[0].flash).map(|w| w[1].unix_ms as f64).collect();
        let onsets = onsets(&self.audio);
        println!("flashes (unix ms) {:?}", flashes.iter().map(|f| *f as i64).collect::<Vec<_>>());
        println!("beep onsets (unix ms) {:?}", onsets.iter().map(|f| *f as i64).collect::<Vec<_>>());
        let mut offsets = Vec::new();
        for flash in &flashes {
            if let Some(onset) = onsets.iter().min_by(|a, b| (*a - flash).abs().total_cmp(&(*b - flash).abs())) {
                if (onset - flash).abs() < 500.0 {
                    offsets.push(onset - flash);
                }
            }
        }
        if offsets.is_empty() {
            ok = false;
            println!("FAIL a/v: {} flashes, {} beep onsets, none paired ({} audio packets)", flashes.len(), onsets.len(), self.audio.len());
        } else {
            let worst = offsets.iter().fold(0.0f64, |m, o| m.max(o.abs()));
            let mean = offsets.iter().sum::<f64>() / offsets.len() as f64;
            let line = format!("{} pairs, audio minus video mean {:+.1} ms, worst {:.1} ms (one frame is {:.1} ms)", offsets.len(), mean, worst, FRAME_MS);
            if worst <= FRAME_MS {
                println!("PASS a/v: {line}");
            } else {
                ok = false;
                println!("FAIL a/v: {line}");
            }
        }
        self.views.shutdown(cx);
        std::process::exit(if ok { 0 } else { 1 });
    }
}

/// Every frame of the MP4, its strip read from the luma plane.
fn decoded_counters(path: &str) -> Result<Vec<u32>, String> {
    let mut decoder = VideoFileDecoder::open(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(frame) = decoder.next_frame().map_err(|e| e.to_string())? {
        let (w, h) = (frame.width as usize, frame.height as usize);
        let scale_x = w as f64 / W as f64;
        let y = ((H + STRIP / 2) as f64 * h as f64 / (H + STRIP) as f64) as usize;
        let cells: Vec<u8> = (0..CELLS).map(|b| frame.nv12[y * w + ((b * CELL + CELL / 2) as f64 * scale_x) as usize]).collect();
        let (counter, _) = decode(&cells).ok_or_else(|| format!("decoded frame {} has no readable strip", out.len()))?;
        out.push(counter);
    }
    Ok(out)
}

/// When each beep starts: the first loud sample after a quiet stretch, in
/// Unix milliseconds.
fn onsets(audio: &[(i64, Vec<f32>)]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut quiet_since: Option<f64> = Some(f64::MIN);
    for (pts_ms, samples) in audio {
        for (i, frame) in samples.chunks_exact(2).enumerate() {
            let t = *pts_ms as f64 + i as f64 * 1000.0 / 48_000.0;
            let loud = frame[0].abs().max(frame[1].abs()) > 0.05;
            if loud {
                if quiet_since.is_some_and(|since| t - since > 200.0) {
                    out.push(t);
                }
                quiet_since = None;
            } else if quiet_since.is_none() {
                quiet_since = Some(t);
            }
        }
    }
    out
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

const VIDEO_W: u32 = 1280;
const VIDEO_H: u32 = 720;
const VIDEO_SECONDS: u32 = 40;
/// The strip of the video's frames: 32 cells of 40 px, its bottom 48 rows.
const VIDEO_STRIP: u32 = 48;

/// A video frame's strip bits: 24 of its number, then the flash bit.
fn strip_bits(n: u32, flash: bool) -> [bool; 32] {
    let mut bits = [false; 32];
    for (b, bit) in bits.iter_mut().enumerate().take(24) {
        *bit = (n >> (23 - b)) & 1 == 1;
    }
    bits[24] = flash;
    bits
}

fn make_video(dir: &std::path::Path, fps: u32) -> Result<(), String> {
    use makepad_widgets::makepad_platform::video_file::PcmAudioTrackOptions;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join("counter.mp4");
    let _ = std::fs::remove_file(&path);
    let options = VideoFileEncoderOptions {
        codec: VideoFileCodec::H264,
        width: VIDEO_W,
        height: VIDEO_H,
        fps_num: fps,
        fps_den: 1,
        video_bitrate_bps: 12_000_000,
        audio: Some(PcmAudioTrackOptions { sample_rate: 48_000, channels: 2, aac_bitrate_bps: 128_000 }),
        keyframe_only: false,
    };
    let mut encoder = VideoFileEncoder::new(path.to_str().unwrap(), options).map_err(|e| e.to_string())?;
    let mut rgb = vec![0u8; (VIDEO_W * VIDEO_H * 3) as usize];
    let samples_per_frame = (48_000 / fps) as usize;
    let mut beep_left = 0usize;
    let mut phase = 0.0f64;
    for frame in 0..VIDEO_SECONDS * fps {
        let n = frame + 1;
        let flash = n % 60 == 0;
        let ground = if flash { 255u8 } else { 32u8 };
        rgb.fill(ground);
        let bits = strip_bits(n, flash);
        for y in VIDEO_H - VIDEO_STRIP..VIDEO_H {
            for (b, bit) in bits.iter().enumerate() {
                let value = if *bit { 255 } else { 0 };
                let x0 = b as u32 * (VIDEO_W / 32);
                for x in x0..x0 + VIDEO_W / 32 {
                    let at = ((y * VIDEO_W + x) * 3) as usize;
                    rgb[at..at + 3].fill(value);
                }
            }
        }
        encoder.push_frame_rgb8(&rgb, Some(frame as i64 * 10_000_000 / fps as i64)).map_err(|e| e.to_string())?;
        if flash {
            beep_left = 4_800;
        }
        let mut audio = Vec::with_capacity(samples_per_frame * 2);
        for _ in 0..samples_per_frame {
            let v = if beep_left > 0 {
                beep_left -= 1;
                phase += 1000.0 / 48_000.0;
                ((phase * std::f64::consts::TAU).sin() * 16_000.0) as i16
            } else {
                0
            };
            audio.extend_from_slice(&[v, v]);
        }
        encoder.push_audio_i16(&audio).map_err(|e| e.to_string())?;
    }
    encoder.finish().map_err(|e| e.to_string())?;
    let html = r#"<!doctype html><html><body style="margin:0;background:#000">
<video id="v" src="counter.webm" playsinline style="width:100vw;height:100vh;object-fit:contain"></video>
<script>document.addEventListener('mousedown', () => { const v = document.getElementById('v'); v.currentTime = 0; v.play(); });</script>
</body></html>"#;
    std::fs::write(dir.join("counter.html"), html).map_err(|e| e.to_string())?;
    println!("wrote {} and counter.html; make counter.webm from it (VP9/Opus) next to them", path.display());
    Ok(())
}

/// A recording's segments in order: (picture file, sound file).
fn segments(dir: &std::path::Path) -> Result<Vec<(std::path::PathBuf, std::path::PathBuf)>, String> {
    let mut seqs: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter_map(|name| name.strip_prefix('a')?.strip_suffix(".wav").map(str::to_string))
        .collect();
    seqs.sort();
    Ok(seqs.into_iter().map(|seq| (dir.join(format!("v{seq}.mp4")), dir.join(format!("a{seq}.wav")))).collect())
}

/// The 16-bit stereo frames of a WAV sidecar, as one channel's floats.
fn wav_left(path: &std::path::Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let at = bytes.windows(4).position(|w| w == b"data").ok_or("no data chunk")? + 8;
    Ok(bytes[at..].chunks_exact(4).map(|f| i16::from_le_bytes([f[0], f[1]]) as f32 / 32768.0).collect())
}

fn analyze(dir: &std::path::Path) -> bool {
    let segments = match segments(dir) {
        Ok(segments) if !segments.is_empty() => segments,
        Ok(_) => {
            println!("FAIL no segments in {}", dir.display());
            return false;
        }
        Err(error) => {
            println!("FAIL {error}");
            return false;
        }
    };
    // One timeline: each segment starts where the sound before it ended.
    let mut audio: Vec<f32> = Vec::new();
    let mut frames: Vec<(f64, u32, bool)> = Vec::new();
    for (video, sound) in &segments {
        let start_s = audio.len() as f64 / 48_000.0;
        match wav_left(sound) {
            Ok(samples) => audio.extend(samples),
            Err(error) => {
                println!("FAIL {}: {error}", sound.display());
                return false;
            }
        }
        if !video.exists() {
            continue;
        }
        let mut decoder = match VideoFileDecoder::open(video.to_str().unwrap()) {
            Ok(decoder) => decoder,
            Err(error) => {
                println!("FAIL {}: {error}", video.display());
                return false;
            }
        };
        while let Ok(Some(frame)) = decoder.next_frame() {
            let (w, h) = (frame.width as usize, frame.height as usize);
            // The strip's rows, wherever the recording put the picture:
            // the video's bottom 48/720 of the picture height.
            let y = h - (h * (VIDEO_STRIP as usize / 2) / VIDEO_H as usize).max(1);
            let cells: Vec<u8> = (0..32).map(|b| frame.nv12[y * w + (b * w / 32 + w / 64)]).collect();
            let lit = |b: usize| cells[b] > 127;
            let n = (0..24).fold(0u32, |n, b| (n << 1) | lit(b) as u32);
            frames.push((start_s + frame.pts_100ns as f64 / 1e7, n, lit(24)));
        }
    }
    let mut ok = true;
    // Pictures: every source frame, once; a segment may open with the
    // picture on screen at its start (a repeat), never with a gap.
    let mut gaps = Vec::new();
    let mut unique = 0;
    for pair in frames.windows(2) {
        let (a, b) = (pair[0].1, pair[1].1);
        if b == a {
            continue;
        }
        unique += 1;
        if b != a + 1 {
            gaps.push((a, b));
        }
    }
    let first = frames.first().map_or(0, |f| f.1);
    let last = frames.last().map_or(0, |f| f.1);
    if gaps.is_empty() && unique > 60 {
        println!("PASS pictures: {} frames over {} segments, source frames {first}..{last} with no gap", frames.len(), segments.len());
    } else {
        ok = false;
        println!("FAIL pictures: {} frames, {} gaps (first {:?}), source frames {first}..{last}", frames.len(), gaps.len(), gaps.first());
    }
    let ordered = frames.windows(2).all(|pair| pair[1].0 > pair[0].0);
    if !ordered {
        ok = false;
        println!("FAIL pictures: timestamps not increasing");
    }
    // A/V: each flash picture against the beep starting nearest to it.
    let flashes: Vec<f64> = frames.windows(2).filter(|w| w[1].2 && !w[0].2).map(|w| w[1].0 * 1000.0).collect();
    let mut onsets = Vec::new();
    let mut quiet = 0usize;
    for (i, v) in audio.iter().enumerate() {
        if v.abs() > 0.05 {
            if quiet > 9_600 {
                onsets.push(i as f64 * 1000.0 / 48_000.0);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    let mut offsets = Vec::new();
    for flash in &flashes {
        if let Some(onset) = onsets.iter().min_by(|a, b| (*a - flash).abs().total_cmp(&(*b - flash).abs())) {
            if (onset - flash).abs() < 500.0 {
                offsets.push(onset - flash);
            }
        }
    }
    if offsets.is_empty() {
        ok = false;
        println!("FAIL a/v: {} flashes, {} beeps, none paired", flashes.len(), onsets.len());
    } else {
        let worst = offsets.iter().fold(0.0f64, |m, o| m.max(o.abs()));
        let mean = offsets.iter().sum::<f64>() / offsets.len() as f64;
        let line = format!("{} pairs, sound minus picture mean {:+.1} ms, worst {:.1} ms (one frame is {:.1} ms)", offsets.len(), mean, worst, FRAME_MS);
        if worst <= FRAME_MS {
            println!("PASS a/v: {line}");
        } else {
            ok = false;
            println!("FAIL a/v: {line}");
        }
    }
    ok
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            let args: Vec<String> = std::env::args().collect();
            if let Some(at) = args.iter().position(|a| a == "--make-video") {
                let dir = std::path::PathBuf::from(args.get(at + 1).cloned().unwrap_or_else(|| ".".into()));
                let fps = args.get(at + 2).and_then(|fps| fps.parse::<u32>().ok()).filter(|fps| (1..=120).contains(fps) && 48_000 % fps == 0).unwrap_or(60);
                match make_video(&dir, fps) {
                    Ok(()) => std::process::exit(0),
                    Err(error) => {
                        println!("FAIL make video: {error}");
                        std::process::exit(1);
                    }
                }
            }
            if let Some(at) = args.iter().position(|a| a == "--analyze") {
                let dir = std::path::PathBuf::from(args.get(at + 1).cloned().unwrap_or_else(|| ".".into()));
                std::process::exit(if analyze(&dir) { 0 } else { 1 });
            }
            self.probe = Some(Probe::start(cx));
        }
        if let Some(probe) = &mut self.probe {
            probe.views.handle_event(cx, event);
            if probe.timer.is_event(event).is_some() || matches!(event, Event::Signal) {
                probe.step(cx);
            }
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
