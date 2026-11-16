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
use crate::hub::{BoxHub, LiveInput, FAST_MODEL, QUALITY_MODEL, VISION_MODEL};
use crate::image::{self, Rgb};
use crate::store::{Jpeg, Quality, SourceKind, TakeId, Who};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// The fast model's live size: what it draws at interactive speed.
const LIVE_SIDE: usize = 512;
/// The longest side a photo is redrawn at by the fast model.
const PHOTO_SIDE: usize = 768;
/// The quality model's longest side, for a photo.
const BEST_SIDE: usize = 1024;
/// A loop's best quality: the fast model, larger and with more steps.
const BEST_LOOP_SIDE: usize = 768;
const BEST_LOOP_STEPS: u32 = 8;
const FAST_STEPS: u32 = 4;
const BEST_STEPS: u32 = 20;
/// How much of a loop frame the fast model redraws: below 1 it starts from
/// the frame itself, which keeps neighbouring frames alike. The hub starts
/// the sampler at step floor((1 - strength) * steps), so the same look at
/// another step count needs its own strength: 0.6 at 4 steps and 0.7 at 8
/// both start a quarter of the way in (step 1 of 4, step 2 of 8).
const LOOP_STRENGTH: f32 = 0.6;
const BEST_LOOP_STRENGTH: f32 = 0.7;
/// The live session's wait for one frame; the first includes the model's load.
const FRAME_TIMEOUT: Duration = Duration::from_secs(300);
const JPEG_QUALITY: u8 = 86;
/// A best-quality take stops between frames when a guest's take waits.
const YIELDED: &str = "gave way to a guest";

/// Starts the worker thread; it runs as long as the process.
pub fn start(party: Arc<Party>, hub: BoxHub) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("party-hub".into()).spawn(move || loop {
        // A guest up or live has the hub: the takes wait for their slot.
        party.live.wait_idle();
        let take = party.queue.pop_wait();
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

fn run(party: &Party, hub: &BoxHub, take: TakeId) {
    let job = {
        let mut store = party.store();
        let Some(t) = store.take(take) else { return };
        let (wish, quality, source) = (t.wish.clone(), t.quality, t.source);
        let Ok(s) = store.source(Who::Host, source) else { return };
        let prompt = match quality {
            Quality::Best => store.shown_take(s).and_then(|t| t.prompt.clone()),
            Quality::Fast => None,
        };
        let job = Job { kind: s.kind, frames: s.frames.clone(), wish, quality, prompt };
        if store.take_running(take).is_err() {
            return;
        }
        job
    };
    let result = redraw(party, hub, take, &job);
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
    let prompt = match &job.prompt {
        Some(prompt) => prompt.clone(),
        None => prompt_for(hub, first, job.wish.as_deref(), take),
    };
    party.store().take_prompt(take, prompt.clone()).map_err(|_| "gone")?;
    let seed = take.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 16;
    match (job.kind, job.quality) {
        (SourceKind::Loop { .. }, Quality::Fast) => {
            let mut live = hub.live(FAST_MODEL, LiveInput::Raw, (LIVE_SIDE as u32, LIVE_SIDE as u32), &prompt, FAST_STEPS, LOOP_STRENGTH, seed)?;
            for frame in &job.frames {
                let rgb = image::fit(&image::decode_jpeg(frame)?, LIVE_SIDE, LIVE_SIDE);
                let (out, _) = live.redraw(&rgb, FRAME_TIMEOUT)?;
                keep_frame(party, take, &out)?;
            }
        }
        (SourceKind::Photo, Quality::Fast) => {
            let rgb = sized(&image::decode_jpeg(first)?, PHOTO_SIDE);
            let png = image::encode_png(&rgb)?;
            let size = Some((rgb.width as u32, rgb.height as u32));
            let (out, _) = hub.edit(FAST_MODEL, &png, &prompt, seed, Some(FAST_STEPS), None, size)?;
            keep_frame(party, take, &image::decode_png(&out)?)?;
        }
        (SourceKind::Loop { .. }, Quality::Best) => {
            let side = BEST_LOOP_SIDE as u32;
            let mut live = hub.live(FAST_MODEL, LiveInput::Raw, (side, side), &prompt, BEST_LOOP_STEPS, BEST_LOOP_STRENGTH, seed)?;
            for frame in &job.frames {
                if !party.queue.is_empty() {
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
            let (out, _) = hub.edit(QUALITY_MODEL, &png, &prompt, seed, Some(BEST_STEPS), None, size)?;
            keep_frame(party, take, &image::decode_png(&out)?)?;
        }
    }
    Ok(())
}

fn keep_frame(party: &Party, take: TakeId, rgb: &Rgb) -> Result<(), String> {
    let jpeg = image::encode_jpeg(rgb, JPEG_QUALITY)?;
    party.store().take_frame(take, Arc::from(jpeg)).map_err(|_| "superseded by a newer wish, or its source deleted".to_string())
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
fn prompt_for(hub: &BoxHub, picture: &[u8], wish: Option<&str>, take: TakeId) -> String {
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
    match hub.describe(VISION_MODEL, picture, &ask, 96) {
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
mod tests {
    use super::*;

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
