//! The hub worker: one long-lived thread that takes the queued takes to the
//! box's own hub one at a time. For each: the instruction the image model
//! follows (from the guest's wish, written by the vision model when the box
//! keeps it pinned beside the image model), then every frame redrawn, each
//! JPEG-encoded into the store as it comes so the phone and the screen see
//! the take grow.
//!
//! What runs where (measured on the party box, phase 0b): a loop's frames go
//! through one live session of the fast model (512 px, 4 steps: ~0.6 s a
//! frame); a photo is one job. The best quality redraws a loop on the fast
//! model at 768 px and 8 steps (~1.7 s a frame, no model swap) and a photo
//! on the quality model (~45 s at 1024 px, and a swap each way).
//!
//! Nothing here holds the store's lock across a decode, an encode or a hub
//! call: it snapshots the source, lets go, and comes back per frame. A take
//! whose source was deleted meanwhile finds itself gone and stops.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use crate::api::Party;
use crate::hub::{BoxHub, Compute, LiveInput, FAST_MODEL, QUALITY_MODEL, VISION_MODEL, VOICE_MODEL};
use crate::image::{self, Rgb};
use crate::store::{Jpeg, Quality, SourceKind, TakeId, Who};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A loop's side: the fast model draws it for the party's big screen, so
/// at a size a screen shows well; the phone's page sends its frames at this
/// size and plays them at [`PREVIEW_SIDE`].
pub const LOOP_SIDE: usize = 1024;
/// The side of the frames a phone is sent: its preview of the redraw.
pub const PREVIEW_SIDE: usize = 512;
/// The longest side a photo is redrawn at by the fast model.
const PHOTO_SIDE: usize = 1024;
/// The quality model's longest side, for a photo.
const BEST_SIDE: usize = 1024;
/// A loop's best quality: the fast model with more steps.
const BEST_LOOP_SIDE: usize = LOOP_SIDE;
const BEST_LOOP_STEPS: u32 = 8;
pub const FAST_STEPS: u32 = 4;
const BEST_STEPS: u32 = 20;
/// How much of a loop frame the fast model redraws: below 1 it starts from
/// the frame itself, which keeps neighbouring frames alike. The hub starts
/// the sampler at step floor((1 - strength) * steps), so the same look at
/// another step count needs its own strength: 0.6 at 4 steps and 0.7 at 8
/// both start a quarter of the way in (step 1 of 4, step 2 of 8).
pub const LOOP_STRENGTH: f32 = 0.6;
const BEST_LOOP_STRENGTH: f32 = 0.7;
/// The live session's wait for one frame; the first includes the model's load.
const FRAME_TIMEOUT: Duration = Duration::from_secs(300);
const JPEG_QUALITY: u8 = 86;
/// A best-quality take stops between frames when a guest's take waits.
const YIELDED: &str = "gave way to a guest";
/// A guest's take pauses between frames when a newer one is asked for: the
/// newer starts at once, and this one goes on from where it stopped after.
const PAUSED: &str = "paused for a newer take";

/// How long one hold of the redraw model lasts; renewed at a third of it.
const HOLD: Duration = Duration::from_secs(180);

/// Keeps the redraw model ready while guests are about, on a thread of its
/// own for as long as the process runs. When a guest comes after a quiet
/// while, or starts their camera, and nothing is being redrawn, it redraws
/// one grey frame the way a take does: that loads the model and readies the
/// box's GPU for the loop's size (6-7 s the first time after the hub
/// starts, under half a second when it is ready), so a guest's first take
/// does not pay for it. It holds the model on the hub while guests keep
/// coming and lets it go once nobody has asked anything for a few minutes.
/// The hub can still unload a held model when the desktop's UI or a load
/// needs the memory: the next take loads it.
pub fn keep_warm(party: Arc<Party>, hub: BoxHub) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("party-warm".into()).spawn(move || {
        let mut holding = false;
        loop {
            let wanted = party.next_keep(holding.then_some(HOLD / 3));
            if party.stopped() { break; }
            if !wanted.about {
                if holding {
                    holding = false;
                    for model in [FAST_MODEL, VOICE_MODEL] {
                        if let Err(e) = hub.hold(model, None) {
                            eprintln!("[party] {} letting {model} go: {e}", clock());
                        }
                    }
                    eprintln!("[party] {} no guest for a while: the hub may let {FAST_MODEL} and {VOICE_MODEL} go", clock());
                }
                continue;
            }
            for model in [FAST_MODEL, VOICE_MODEL] {
                let held = hub.hold(model, Some(HOLD));
                if !holding {
                    if let Err(e) = &held {
                        eprintln!("[party] {} holding {model}: {e}", clock());
                    }
                }
            }
            holding = true;
            // The screen's redraw is warm already, and a second session beside it would not fit.
            let redrawing = || party.live.redrawing.load(std::sync::atomic::Ordering::SeqCst);
            let idle = || !party.stopped() && !party.store().busy() && !party.live.slots().busy() && !redrawing();
            if wanted.voice && idle() {
                // A moment of silence, heard the way a spoken prompt is.
                let started = Instant::now();
                match hub.transcribe(&silence_wav(), &|| party.stopped()) {
                    Ok(_) => eprintln!("[party] {} the voice button: {VOICE_MODEL} ready in {:.1} s; {}", clock(), started.elapsed().as_secs_f64(), gpu_line(&hub)),
                    Err(e) => eprintln!("[party] {} the voice button, but {VOICE_MODEL} is not ready: {e}", clock()),
                }
            }
            // A box that takes live calls warms the model in the screen's one
            // redraw (it opens with the first source: the box camera, a phone's
            // offer); a warm-up session beside it would not fit the GPU.
            if wanted.image && !party.live.available() && idle() {
                let started = Instant::now();
                let grey = Rgb { width: LOOP_SIDE, height: LOOP_SIDE, data: vec![128; LOOP_SIDE * LOOP_SIDE * 3] };
                let ready = hub
                    .live(FAST_MODEL, LiveInput::Raw, (LOOP_SIDE as u32, LOOP_SIDE as u32), "a calm grey picture", FAST_STEPS, LOOP_STRENGTH, 0, Compute::STORED)
                    .and_then(|mut live| live.redraw(&grey, FRAME_TIMEOUT));
                match ready {
                    Ok(_) => eprintln!("[party] {} a guest is here: {FAST_MODEL} ready in {:.1} s", clock(), started.elapsed().as_secs_f64()),
                    Err(e) => eprintln!("[party] {} a guest is here, but {FAST_MODEL} is not ready: {e}", clock()),
                }
            }
        }
    })
}

/// Half a second of silence as a 16 kHz mono WAV file.
fn silence_wav() -> Vec<u8> {
    let samples = 8000u32;
    let mut wav = Vec::with_capacity(44 + samples as usize * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16000u32.to_le_bytes());
    wav.extend_from_slice(&32000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples * 2).to_le_bytes());
    wav.resize(44 + samples as usize * 2, 0);
    wav
}

/// The box's GPU memory and what the hub holds in it, for the log.
pub fn gpu_line(hub: &BoxHub) -> String {
    match hub.control() {
        Ok(control) => {
            let (models, gpu) = crate::hub::residency(&control);
            let gpu = gpu.map(|(used, total)| format!("GPU {used}/{total} MB")).unwrap_or_else(|| "GPU ?".into());
            let models: Vec<String> = models
                .iter()
                .map(|(id, state, mb)| format!("{id} {state}{}", mb.map(|mb| format!(" {mb} MB")).unwrap_or_default()))
                .collect();
            format!("{gpu}; on the hub: {}", if models.is_empty() { "nothing".into() } else { models.join(", ") })
        }
        Err(e) => format!("GPU ?: {e}"),
    }
}

/// Starts the worker thread for this party.
pub fn start(party: Arc<Party>, hub: BoxHub) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("party-hub".into()).spawn(move || loop {
        // A guest up or live has the hub: the takes wait for their slot, and
        // for a guest's quick ask (their spoken prompt, ideas) to be answered.
        party.live.wait_idle(|| party.stopped());
        party.asks.wait_none(|| party.stopped());
        let Some(take) = party.queue.pop_wait(|| party.stopped()) else { break };
        party.live.take_running.store(true, Ordering::SeqCst);
        if party.live.slots().busy() {
            party.live.take_running.store(false, Ordering::SeqCst);
            party.queue.push_front(take);
            continue;
        }
        run(&party, &hub, take);
        // A guest waiting for the hub gets it now.
        party.live.take_running.store(false, Ordering::SeqCst);
    })
}

/// What a take needs, copied out of the store (the frames are shared).
struct Job {
    kind: SourceKind,
    frames: Vec<Jpeg>,
    wish: Option<String>,
    quality: Quality,
    /// An upgrade's instruction: the one the take it betters was drawn with,
    /// so the screen shows the same picture, better, not another one.
    prompt: Option<String>,
}

/// The wall clock as UTC `HH:MM:SS.t`, so the box's lines can be read beside
/// the phone's reports.
pub fn clock() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let s = t.as_secs();
    format!("{:02}:{:02}:{:02}.{}", s / 3600 % 24, s / 60 % 60, s % 60, t.subsec_millis() / 100)
}

fn run(party: &Party, hub: &BoxHub, take: TakeId) {
    eprintln!("[party] {} take {take}: starts", clock());
    let job = {
        let mut store = party.store();
        let Some(t) = store.take(take) else { return };
        let (wish, quality, source) = (t.wish.clone(), t.quality, t.source);
        let Ok(s) = store.source(Who::Host, source) else { return };
        let prompt = match quality {
            Quality::Best => store.shown_take(s).and_then(|t| t.prompt.clone()),
            // A paused take goes on with the instruction it started with.
            Quality::Fast => t.prompt.clone(),
        };
        let job = Job { kind: s.kind, frames: s.frames.clone(), wish, quality, prompt };
        if store.take_running(take).is_err() {
            return;
        }
        job
    };
    let result = redraw(party, hub, take, &job);
    if result.as_ref().is_err_and(|e| e == PAUSED) {
        return;
    }
    if result.as_ref().is_err_and(|e| e == YIELDED) {
        // An upgrade gave way to a guest: the screen asks for it again later.
        party.store().remove_take(take);
        return;
    }
    if let Err(e) = &result {
        eprintln!("[party] take {take}: {e}");
    }
    let persist = party.store().take_finished(take, result);
    if let Ok(persist) = persist {
        party.persist(persist);
    }
}

/// Every frame of a take, into the store as each comes.
fn redraw(party: &Party, hub: &BoxHub, take: TakeId, job: &Job) -> Result<(), String> {
    if let Some(wish) = &job.wish {
        if !kind_wish(wish) {
            return Err("the box skips that wish; try another".into());
        }
    }
    let first = job.frames.first().ok_or("no picture")?;
    let cancelled = || party.stopped() || party.store().take(take).is_none();
    let asked = Instant::now();
    let prompt = match &job.prompt {
        Some(prompt) => prompt.clone(),
        None => prompt_for(hub, first, job.wish.as_deref(), take, &cancelled),
    };
    if cancelled() { return Err("cancelled".into()); }
    let prompted = asked.elapsed();
    party.store().take_prompt(take, prompt.clone()).map_err(|_| "gone")?;
    let seed = take.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 16;
    match (job.kind, job.quality) {
        (SourceKind::Loop { .. }, Quality::Fast) => {
            let opening = Instant::now();
            let mut live = hub.live(FAST_MODEL, LiveInput::Raw, (LOOP_SIDE as u32, LOOP_SIDE as u32), &prompt, FAST_STEPS, LOOP_STRENGTH, seed, Compute::STORED)?;
            let opened = opening.elapsed();
            // A take that was paused goes on after the frames it has.
            let from = party.store().take(take).map_or(0, |t| t.frames.len());
            let n = job.frames.len();
            let decode = |i: usize| -> Result<Rgb, String> { Ok(image::fit(&image::decode_jpeg(&job.frames[i])?, LOOP_SIDE, LOOP_SIDE)) };
            // The box draws frame i while this thread decodes frame i + 1 and
            // encodes the redraw of frame i - 1: the next goes as soon as a
            // redraw comes back.
            let mut stage = Stages::default();
            let mut next = if from < n { Some(decode(from)?) } else { None };
            let mut drawn: Option<Rgb> = None;
            for i in from..n {
                if party.stopped() { return Err("cancelled".into()); }
                let why = if i == from {
                    None
                } else if party.asks.waiting() {
                    // The hub answers a guest's quick ask (their spoken
                    // prompt, ideas) first; this take goes on right after.
                    party.queue.push_front(take);
                    Some("a guest's quick ask")
                } else if give_way(party, take) {
                    Some("a newer take")
                } else {
                    None
                };
                if let Some(why) = why {
                    if let Some(out) = drawn.take() {
                        keep_frame(party, take, &out)?;
                    }
                    eprintln!("[party] {} take {take}: paused after {i} frames for {why}", clock());
                    return Err(PAUSED.into());
                }
                let sent = Instant::now();
                live.send(&next.take().ok_or("no frame")?)?;
                if i + 1 < n {
                    let t = Instant::now();
                    next = Some(decode(i + 1)?);
                    stage.decode += t.elapsed();
                }
                if let Some(out) = drawn.take() {
                    let t = Instant::now();
                    keep_frame(party, take, &out)?;
                    stage.keep += t.elapsed();
                }
                let (out, time) = live.receive(FRAME_TIMEOUT)?;
                if i == from {
                    eprintln!(
                        "[party] {} take {take}: first frame {:.1} s after it started (prompt {:.1} s, session {:.1} s, frame {:.1} s: box {:.0} ms, model {:.0} ms)",
                        clock(), asked.elapsed().as_secs_f64(), prompted.as_secs_f64(), opened.as_secs_f64(), sent.elapsed().as_secs_f64(), time.frame_ms, time.model_ms
                    );
                } else {
                    stage.add(sent.elapsed(), &time);
                }
                drawn = Some(out);
            }
            if let Some(out) = drawn.take() {
                keep_frame(party, take, &out)?;
            }
            eprintln!("[party] {} take {take}: {} frames in {:.1} s; {}", clock(), n - from, asked.elapsed().as_secs_f64(), stage.line());
        }
        (SourceKind::Photo, Quality::Fast) => {
            let rgb = sized(&image::decode_jpeg(first)?, PHOTO_SIDE);
            let png = image::encode_png(&rgb)?;
            let size = Some((rgb.width as u32, rgb.height as u32));
            let (out, _) = hub.edit(FAST_MODEL, &png, &prompt, seed, Some(FAST_STEPS), None, size, &cancelled)?;
            keep_frame(party, take, &image::decode_png(&out)?)?;
        }
        (SourceKind::Loop { .. }, Quality::Best) => {
            let side = BEST_LOOP_SIDE as u32;
            let mut live = hub.live(FAST_MODEL, LiveInput::Raw, (side, side), &prompt, BEST_LOOP_STEPS, BEST_LOOP_STRENGTH, seed, Compute::STORED)?;
            for frame in &job.frames {
                if party.stopped() { return Err("cancelled".into()); }
                if !party.queue.is_empty() || party.asks.waiting() {
                    return Err(YIELDED.into());
                }
                let rgb = image::fit(&image::decode_jpeg(frame)?, BEST_LOOP_SIDE, BEST_LOOP_SIDE);
                let (out, _) = live.redraw(&rgb, FRAME_TIMEOUT)?;
                keep_frame(party, take, &out)?;
            }
        }
        (SourceKind::Photo, Quality::Best) => {
            let rgb = sized(&image::decode_jpeg(first)?, BEST_SIDE);
            let png = image::encode_png(&rgb)?;
            let size = Some((rgb.width as u32, rgb.height as u32));
            // A plain edit, as for the fast model: the photo is the model's
            // reference, and the picture is drawn from noise.
            let (out, _) = hub.edit(QUALITY_MODEL, &png, &prompt, seed, Some(BEST_STEPS), None, size, &cancelled)?;
            keep_frame(party, take, &image::decode_png(&out)?)?;
        }
    }
    Ok(())
}

/// Where the time of a take's frames after its first goes, for its log line.
#[derive(Default)]
struct Stages {
    frames: u32,
    /// From sending a frame to its redraw coming back.
    round: Duration,
    box_ms: f64,
    model_ms: f64,
    prep_ms: f64,
    /// This thread's own work, done while the box draws.
    decode: Duration,
    keep: Duration,
}

impl Stages {
    fn add(&mut self, round: Duration, time: &crate::hub::LiveTime) {
        self.frames += 1;
        self.round += round;
        self.box_ms += time.frame_ms;
        self.model_ms += time.model_ms;
        self.prep_ms += time.prep_ms;
    }

    fn line(&self) -> String {
        let n = self.frames.max(1) as f64;
        let ms = |d: Duration| d.as_secs_f64() * 1000.0 / n;
        format!(
            "after the first {:.0} ms a frame: the box {:.0} ms of it (input wait and prep {:.0} ms, model {:.0} ms); meanwhile here decode {:.0} ms, encode and keep {:.0} ms",
            ms(self.round), self.box_ms / n, self.prep_ms / n, self.model_ms / n, ms(self.decode), ms(self.keep)
        )
    }
}

/// Whether `take` gives way to a take asked for after it (ids grow), which
/// then goes first, with `take` right after it. One it paused earlier waits
/// its turn, and so does an upgrade to the best quality.
fn give_way(party: &Party, take: TakeId) -> bool {
    loop {
        let Some(newer) = party.queue.pop_newest() else { return false };
        // A take superseded while it waited is no longer anyone's.
        let Some(quality) = party.store().take(newer).map(|t| t.quality) else { continue };
        if newer < take || quality == Quality::Best {
            party.queue.push(newer);
            return false;
        }
        party.queue.push_front(take);
        party.queue.push_front(newer);
        return true;
    }
}

/// Keeps a redrawn frame, with its preview for the phone (a few ms beside
/// the model's frame: a box-filtered half and its JPEG).
fn keep_frame(party: &Party, take: TakeId, rgb: &Rgb) -> Result<(), String> {
    let frame: Jpeg = Arc::from(image::encode_jpeg(rgb, JPEG_QUALITY)?);
    let preview: Jpeg = if rgb.width.max(rgb.height) > PREVIEW_SIDE {
        let (w, h) = crate::wall_plan::fit_within(rgb.width, rgb.height, PREVIEW_SIDE, PREVIEW_SIDE);
        Arc::from(image::encode_jpeg(&image::resize(rgb, w, h), JPEG_QUALITY)?)
    } else {
        frame.clone()
    };
    party.store().take_frame(take, frame, preview).map_err(|_| "superseded by a newer wish, or its source deleted".to_string())
}

/// `rgb` scaled so its longer side is `side`, both sides a multiple of 16
/// (what the image models draw), cropped by at most 15 px.
fn sized(rgb: &Rgb, side: usize) -> Rgb {
    let scale = side as f64 / rgb.width.max(rgb.height) as f64;
    let w = ((rgb.width as f64 * scale) as usize / 16 * 16).max(16);
    let h = ((rgb.height as f64 * scale) as usize / 16 * 16).max(16);
    image::fit(rgb, w, h)
}

/// Styles a take without a wish picks from, in turn.
const STYLES: &[&str] = &[
    "a bright watercolour painting",
    "a storybook illustration in soft colours",
    "a stage show under warm spotlights",
    "a sunny beach at golden hour",
    "a paper cut-out collage",
    "an oil painting in thick, happy brush strokes",
    "a snowy mountain hut with fairy lights",
    "a retro comic panel",
];

/// The wish (or the `turn`th style) in a fixed sentence: what the image
/// model follows when no vision model writes it (a live camera, always).
pub fn plain_prompt(wish: Option<&str>, turn: u64) -> String {
    match wish {
        Some(wish) => format!("Redraw this picture: {wish}. Keep the people, their faces, poses and the framing."),
        None => format!("Redraw this picture as {}. Keep the people, their faces, poses and the framing.", STYLES[turn as usize % STYLES.len()]),
    }
}

/// The instruction the image model (an editing model) follows. With the
/// vision model pinned beside it on the box, the vision model writes it
/// from the picture and the wish; otherwise, so that no take costs two model
/// swaps, it is the wish (or the next style) in a fixed sentence.
fn prompt_for(hub: &BoxHub, picture: &[u8], wish: Option<&str>, take: TakeId, cancelled: &dyn Fn() -> bool) -> String {
    let style = STYLES[take as usize % STYLES.len()];
    let fixed = plain_prompt(wish, take);
    if !hub.pinned(VISION_MODEL) {
        return fixed;
    }
    let goal = match wish {
        Some(wish) => format!("the way a guest asked: \"{wish}\""),
        None => format!("as {style}, or another playful style that suits it better"),
    };
    let ask = format!(
        "This is a picture from a party. Write one instruction for an image editing model that redraws it {goal}. \
         Keep the people, their faces, poses and the framing, and keep it kind. Answer with the instruction only, at most 40 words."
    );
    match hub.describe(VISION_MODEL, picture, &ask, 96, cancelled) {
        Ok((text, _)) if !text.trim().is_empty() => text.trim().trim_matches('"').to_string(),
        Ok(_) => fixed,
        Err(e) => {
            eprintln!("[party] vision: {e}");
            fixed
        }
    }
}

/// A first look at a typed wish: words that make a take unkind are skipped
/// before the models see them. The host can skip anything else on the screen.
pub fn kind_wish(wish: &str) -> bool {
    const SKIPPED: &[&str] = &[
        "nude", "naked", "nsfw", "sexy", "porn", "gore", "blood", "bloody", "corpse", "kill", "murder", "gun", "weapon",
        "knife", "wound", "ugly", "fat", "hate", "racist", "nazi", "drunk", "drugs", "vomit",
    ];
    let lower = wish.to_lowercase();
    !lower.split(|c: char| !c.is_alphanumeric()).any(|word| SKIPPED.contains(&word))
}

#[cfg(test)]
#[path = "../../../libs/ai/hub/src/speech/tests/refusing_hub.rs"]
mod refusing_hub;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_the_party_server_ends_the_workers_admission_wait() {
        use crate::api::Api;
        use crate::store::{Budget, Keep, TakeState};
        let hub = refusing_hub::RefusingHub::start(|path| {
            assert_eq!(path, "/control");
            r#"{"models":[{"id":"qwen3.5-4b-vision","pinned":true}]}"#
        });
        let party = Arc::new(Party::new(Default::default(), Budget::UNLIMITED));
        let (server, dir) = crate::server::tests::start("abcdefgh", Arc::new(Api::new(party.clone())));
        let take = {
            let mut store = party.store();
            let guest = store.join("guest".into());
            let (source, _) = store.add_source(guest, [127, 0, 0, 1].into(), SourceKind::Photo,
                vec![Arc::from([0u8; 4])], None, Keep::Me).unwrap();
            store.add_take(source, Quality::Fast).unwrap()
        };
        let worker = start(party.clone(), BoxHub::locate(Some(&hub.base)).unwrap()).unwrap();
        party.queue.push(take);
        hub.wait_for_retry();
        let (done, ended) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(server);
            worker.join().unwrap();
            done.send(()).unwrap();
        });
        ended.recv_timeout(Duration::from_secs(2)).expect("the party worker must exit");
        assert_eq!(party.store().take(take).unwrap().state, TakeState::Failed("cancelled".into()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wishes_and_sizes() {
        assert!(kind_wish("as a watercolour painting on a beach"));
        assert!(!kind_wish("make them Naked"), "words are matched whatever their case");
        assert!(kind_wish("a bloodhound at a picnic"), "whole words only");
        let photo = Rgb { width: 1280, height: 960, data: vec![0; 1280 * 960 * 3] };
        let s = sized(&photo, 768);
        assert_eq!((s.width, s.height), (768, 576), "the longer side, both multiples of 16");
        let tall = sized(&Rgb { width: 721, height: 1280, data: vec![0; 721 * 1280 * 3] }, 1024);
        assert_eq!((tall.width % 16, tall.height), (0, 1024));
    }
}
