//! Lazy ANIMATED thumbnails for `vjeffect` assets.
//!
//! The bundled effect library seeds into the store with modest colored
//! placeholder JPEGs (effects/seed.rs). This widget replaces them, tile by
//! tile, with the real thing: it hosts a small bank of hidden slot-mode
//! [`VjFxView`]s (exactly the hosting contract in effects/CONTRACT.md), runs
//! documents offscreen at thumbnail resolution, and packs a second's worth
//! of frames into the SAME declared-cells sheet the grid already animates
//! for sprite actors and turntable icons — so a finished sheet re-enters
//! through the existing `DecodeJob::Thumb` lane and the grid needs no new
//! code path.
//!
//! HOW THE FRAMES ARE MADE (the fast path):
//!   * the effect's clock is HOST-DRIVEN ([`VjFxView::tick_manual`]): frame
//!     k renders at document time `preroll + k/30`, so a sheet is a pure
//!     function of the document — identical on a slow machine and a fast
//!     one, and identical on every screen (the offscreen pass is pinned to
//!     [`SLOT_W`]x[`SLOT_H`] physical pixels, dpi-corrected),
//!   * capture steps are BATCHED through the view's bake pool
//!     ([`VjFxView::bake_scene_step`]): a pass object executes once per
//!     paint, so the old one-step-per-frame loop paced a 34-step sheet at
//!     the display's beat (~9ms of idle wait per step — measured 85-95% of
//!     a sheet's 300-570ms wall time). Pooled steps let MANY captured
//!     frames share one paint; a stage-less document bakes its whole sheet
//!     in a couple of beats, a staged one alternates scene-encode and
//!     chain-run beats (pass sibling order is not encode order — see the
//!     ordering law on the view's `BakeStep`). Documents whose picture
//!     carries GPU state frame to frame (feedback/hold stages, sim fields)
//!     keep the classic sequential one-step-per-beat path bit for bit,
//!   * each captured frame is downscaled ON THE GPU straight into its cell
//!     of one sheet-sized target (2x supersample per cell), so the whole
//!     sheet costs ONE readback instead of thirty.
//!
//! SCHEDULING (one mechanism, event-driven):
//! - [`VjFxThumbs::enqueue`] admits ALWAYS — the pending set is deep (the
//!   whole library fits) and nothing is ever refused or retried on a clock.
//! - a free lane pulls by STRICT PRIORITY CLASS, decided at the pull
//!   against the live viewport the app hands over every tick
//!   ([`VjFxThumbs::set_priority`]): (1) tiles visible on screen right
//!   now, in pad order; (2) the rest of the currently open tab (EFFECT
//!   vs TRANSITION — the job's own `transition` flag); (3) other tabs,
//!   only when 1 and 2 are fully served. Never one mixed queue: an
//!   invisible tab's doc cannot hold a lane while anything visible is
//!   unbaked, a scroll or tab switch reclassifies instantly (newly
//!   visible tiles jump everything), and in-flight renders finish.
//! - failure is TERMINAL PER REVISION and loud: a doc whose load/render
//!   fails is marked failed for this session, painted as failed on its
//!   tile, and only a NEW REVISION (a content change, which is a new cache
//!   key and a new enqueue) bakes again. No retry timers, no backoff.
//!
//! Budget discipline (the VJ may be performing while this runs):
//! - the work is metered: at most [`LOADS_PER_FRAME`] document loads,
//!   [`STEP_BUDGET`] bake steps and [`READBACKS_PER_FRAME`] sheet
//!   readbacks per UI frame,
//! - a live set (output window up, a deck playing) drops the bank to a
//!   single lane AND a single step per frame — exactly the old polite
//!   cadence — see [`VjFxThumbs::set_full_speed`],
//! - the offscreen passes are thumbnail-sized ([`SLOT_W`]x[`SLOT_H`]), not
//!   program-sized, and the bake pools hand their memory back the moment
//!   the bank goes idle,
//! - video encode + cache write happen on a worker thread, never the UI
//!   thread.
//!
//! Cache: `<cache_parent>/cache-vjfx-thumbs-30/<revision>-rN.mp4`, keyed by
//! the immutable revision id (the content digest of the published revision)
//! plus [`RECIPE`]. THE ARTIFACT IS AN MP4 (H.264, 30 frames at cell
//! resolution) written and read through the platform-neutral makepad-video
//! seam — VideoToolbox on macOS, Media Foundation on Windows, GStreamer on
//! Linux — with the cell layout stamped as a trailer box the demuxer skips
//! (`anim_icon::stamp_layout_mp4`), so the file describes its own grid and
//! survives a byte copy. STORAGE ONLY: on load the file is decoded ONCE
//! into the same frames-in-a-sheet texture path the PNG used to feed, and
//! the decoder session is torn down — the runtime animates the atlas by
//! frame index exactly as before, zero codec work at draw time. A relaunch
//! decodes the file instead of re-rendering. Thin-client law: this is a
//! digest-keyed derived cache, never durable content state. Stale-recipe
//! files (including every legacy PNG sheet) are swept on startup.
//!
//! Fallback honesty: a document that fails to parse, a readback the
//! platform cannot do, a capture that comes back black, or an encoder the
//! platform cannot provide logs loudly, marks the revision failed for this
//! session (terminal — see above), and the tile paints the failure over its
//! seeded placeholder. Nothing on this path unwraps document data — a panic
//! in the frame path kills the whole app on macOS.

use crate::effects::shaders::DrawVjFxPresent;
use crate::effects::VjFxView;
use makepad_asset_data::{AssetId, AssetRevisionId, ThumbnailCells};
use makepad_asset_importer::anim_icon;
use makepad_widgets::makepad_platform::video_file::{
    VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
};
use makepad_widgets::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::clock::Instant;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The GPU downscale: one captured frame lands in one cell of the sheet,
    // box-filtered from the supersampled render. `self.pos` runs 0..1 over
    // the cell and maps 1:1 onto the source, so one output pixel covers
    // exactly `1/CELL_W x 1/CELL_H` of it; a 2x2 grid of BILINEAR taps
    // inside that footprint averages the 2x2 source texels exactly at the
    // pinned 2x supersample (each tap on a texel center), and a clean 4x4
    // average if a backend ever hands back a doubled target (each tap on a
    // 2x2 corner) — the same total box filter the CPU used to run, on the
    // GPU, at a fraction of the readback.
    set_type_default() do #(DrawVjFxThumbCell::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_out: texture_2d(float)

        tap: fn(uv: vec2) -> vec4 {
            return self.tex_out.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        pixel: fn() {
            let px = vec2(1.0 / 384.0, 1.0 / 240.0)
            let step = px * 0.5
            let base = self.pos - px * 0.25
            let sum = self.tap(base)
                + self.tap(base + vec2(step.x, 0.0))
                + self.tap(base + vec2(0.0, step.y))
                + self.tap(base + vec2(step.x, step.y))
            // Alpha forced opaque: render targets carry whatever alpha the
            // effect blended, and the thumbnail is a picture.
            return vec4((sum / 4.0).xyz, 1.0)
        }
    }

    mod.widgets.VjFxThumbsBase = #(VjFxThumbs::register_widget(vm))
    mod.widgets.VjFxThumbs = set_type_default() do mod.widgets.VjFxThumbsBase{
        width: 4
        height: 4
        // One hidden host per capture lane (see LANES).
        fx: mod.widgets.VjFxView{ composite: false }
        fx1: mod.widgets.VjFxView{ composite: false }
        fx2: mod.widgets.VjFxView{ composite: false }
        fx3: mod.widgets.VjFxView{ composite: false }
        fx4: mod.widgets.VjFxView{ composite: false }
        fx5: mod.widgets.VjFxView{ composite: false }
    }
}

/// One packed cell of the sheet, sized 1:1 against the REAL tile: at the
/// reference window (1680x1040 points on a 2x screen — the fullscreen-4K
/// class the operator runs), a grid pad measures 191.5x85.8 layout points
/// = 383x172 physical px (measured via the remote bridge's /snap). A
/// 384-wide cell is that width exactly; 16:10 height keeps the bake
/// framing the tiles have always had (the pad crops vertically, as it
/// always did). 30 cells pack 6x5 into a 2304x1200 sheet.
pub const CELL_W: usize = 384;
pub const CELL_H: usize = 240;
/// Sheet columns.
pub const SHEET_COLS: usize = 6;
pub const FRAME_COUNT: usize = 30;
const SHEET_ROWS: usize = FRAME_COUNT.div_ceil(SHEET_COLS);
/// The sheet target's exact texel size — the GPU packs the cells, so the
/// readback IS the finished sheet.
const SHEET_W: usize = SHEET_COLS * CELL_W;
const SHEET_H: usize = SHEET_ROWS * CELL_H;
/// Supersampling: the offscreen pass renders at 2x the cell on each axis,
/// PINNED as physical pixels on every screen (the host divides the slot's
/// logical size by the window dpi), so a sheet no longer depends on which
/// monitor the app happened to sit on.
const SUPERSAMPLE: usize = 2;
const SLOT_W: f64 = (CELL_W * SUPERSAMPLE) as f64;
const SLOT_H: f64 = (CELL_H * SUPERSAMPLE) as f64;
/// Seconds of effect time the sheet spans. One second at 30 frames: the
/// operator judged the earlier 6 fps sheet "a bit too low framerate" —
/// half a bar of genuinely smooth motion beats a whole bar of slideshow.
const CAPTURE_SPAN: f64 = 1.0;
/// Document time between captured frames — and the exact `dt` each rendered
/// frame is told passed, so the sheet plays back at the rate it was
/// simulated at, on any machine.
const FRAME_STEP: f64 = CAPTURE_SPAN / FRAME_COUNT as f64;
/// Document time to run before the first captured frame, so effects that
/// build (emitter plumes, feedback trails, growth ramps) are underway.
const PREROLL_SECS: f64 = 0.9;
/// Preroll frames for a document whose picture depends on the ITERATION
/// history of the frames before it ([`VjFxView::needs_stepped_time`]);
/// everything else jumps its clock straight to the moment in ONE frame.
const PREROLL_STEPS: usize = (PREROLL_SECS / FRAME_STEP) as usize;
/// SEQUENTIAL PATH ONLY — draw frames before the clock starts, so a
/// stateful document's shader compiles before its feedback/sim preroll
/// begins. Batched documents need no warmup: the Metal pipeline builds in
/// the same paint that executes the first batch's draws.
const WARMUP_DRAWS: u32 = 3;
/// Playback rate the sheet declares: frames over the span they cover, so
/// the tile replays at the speed the effect actually ran.
pub const SHEET_FPS: f32 = FRAME_COUNT as f32 / CAPTURE_SPAN as f32;

/// How many documents render side by side at full speed. Each lane is a
/// whole [`VjFxView`] (its own passes, post chain, sim state), so lanes do
/// not interfere; the cost of one is one more effect's worth of GPU per
/// frame, which is why the pacing guard can hand them back.
pub const LANES: usize = 6;
/// Pooled bake steps per lane ([`VjFxView::bake_pool_ensure`]): the most
/// captured frames one lane can encode into one paint. Sized against the
/// pool's render-target memory (~1.5-3MB a step), freed at idle.
const BAKE_STEPS: usize = 12;
/// Bake steps (scene encodes + chain runs) all lanes may spend in one UI
/// frame at full speed. The polite budget is 1 — the old cadence exactly.
///
/// 16, not 48: the bake shares the UI's render thread, and the cold burst
/// stacks first-use pipeline compiles on top of the steps — at 48 the
/// opening paints fused into a MULTI-SECOND freeze of the whole app. At 16
/// a paint stays interactive and the library still bakes in ~half a
/// minute; the difference disappears into the decode overlap.
const STEP_BUDGET: usize = 16;
/// Documents that may START in one UI frame. A load is a splash eval plus
/// a script shader apply — staggering them is what keeps regen from ever
/// reading as a stall. Two per frame: with batching, starts — not steps —
/// are the cold-library floor.
const LOADS_PER_FRAME: usize = 2;
/// Sheet readbacks started per UI frame. On Metal a readback is an async
/// renderer-owned capture (request + completion-handler delivery); on the
/// fallback backends it blocks the UI thread on the GPU (a few ms at sheet
/// size). Two is the ceiling either way and the rest wait a frame.
const READBACKS_PER_FRAME: usize = 2;
/// H.264 bitrate for the cell-resolution cache stream. Sized for the
/// ALL-INTRA stream below (~0.7 bits/pixel at 384x240@30): every frame an
/// IDR costs bitrate over a GOP, but a 30-frame thumbnail gains nothing
/// from inter compression and an intra stream HAS no frame reordering —
/// no B-frames, no decode-order/presentation-order gap on any platform's
/// codec, which is one half of the frames-land-in-the-right-cell law
/// (the pts-keyed packing in media.rs is the other).
const THUMB_BITRATE_BPS: u32 = 2_000_000;

/// A render job: everything needed with no further lookups.
pub struct FxThumbJob {
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub title: String,
    pub source: String,
    /// Transition-tagged doc: the thumbnail PREVIEWS A TRANSITION — the
    /// effect gets a sweeping premix of two distinct test patterns as
    /// input0 while `p3` (the engage triangle) sweeps 0→1→0 across the
    /// captured second, so the tile shows one picture becoming another
    /// through the effect.
    pub transition: bool,
}

/// A finished sheet, already written to the cache; the app feeds it to the
/// decode pool exactly like a store-fetched thumbnail blob.
pub struct FxThumbSheet {
    pub revision: AssetRevisionId,
    pub path: PathBuf,
    pub cells: ThumbnailCells,
    pub fps: f32,
}

/// THE BAKE RECIPE, versioned.
///
/// The cache is keyed by REVISION — the document's content digest — which
/// answers "did the effect change" and nothing else. A change to how a sheet
/// is RENDERED (frame count, preroll, supersampling, the pictures a
/// transition is previewed against) produces a different picture from the
/// same revision, so without this the whole library would keep serving
/// yesterday's bake forever. These numbers are the missing half of the
/// identity: bump one and exactly the sheets it describes re-bake, once.
// 1: screen-engine docs bake against the structured stand-in instead of
//    the fallback blob — every sheet's picture changed.
// 2: cells doubled, the slot pinned to physical pixels, and the artifact
//    moved from stamped PNG to mp4 — every sheet re-bakes.
// 3: cells sized to the measured tile (384x240) and the batched-bake
//    geometry fix (recycle-upload emptied every pooled step after the
//    first: one good cell, twenty-nine black) — every r2 file re-bakes.
// 4: the stream went all-intra (no frame reordering anywhere), retiring
//    every sheet an intermediate build encoded with a GOP.
// 5: decode validation went strict (exactly 30 frames on 30 unique pts
//    slots, clean EOF — media.rs) and the readback moved onto the
//    producing GPU queue. An r4 artifact could be a racy partial bake
//    (one lit cell + twenty-nine black ones passes the whole-sheet-black
//    gate and decodes clean), so every r4 file retires and re-proves.
pub const RECIPE: u32 = 6;

/// The same lever for the TWO-DECK STAND-INS alone
/// ([`crate::effects::deck_pattern`]). A transition sheet is a picture OF
/// those; a generator's sheet has never seen them. Splitting the levers is
/// what lets calming the stand-ins re-bake seventy transition tiles without
/// throwing away a hundred and ninety perfectly good ones.
///
/// `1` = the quiet slates (the first pass' full-saturation primaries blew
/// out the rest of the UI).
// 2: deck A's stand-in moved from warm slate to the darkened key colour.
pub const DECK_RECIPE: u32 = 2;

/// The cache file for one revision. Transition previews render differently
/// (two-input sweep), so they key their own file — a static sheet cached
/// before the sweep existed is simply ignored, never shown. `-t3`: the
/// frame-indexed sweep (the whole transition edge to edge in one sheet).
pub fn cache_path(dir: &Path, revision: &AssetRevisionId, transition: bool) -> PathBuf {
    if transition {
        dir.join(format!("{revision}-t3-d{DECK_RECIPE}-r{RECIPE}.mp4"))
    } else {
        dir.join(format!("{revision}-r{RECIPE}.mp4"))
    }
}

/// Startup sweep: everything in the cache directory that is not a
/// current-recipe artifact — legacy PNG sheets, older recipes, orphaned
/// tmp files — is dead weight nothing will ever read again (the recipe is
/// part of the only name the app looks up). Deleting it here is what makes
/// a recipe bump a re-bake instead of a cache that doubles.
fn sweep_stale_cache(dir: &Path) {
    let keep = format!("-r{RECIPE}.mp4");
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dropped = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.ends_with(&keep) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            dropped += 1;
        }
    }
    if dropped > 0 {
        log!("fx thumb: cache sweep dropped {dropped} stale files (current recipe r{RECIPE}, mp4)");
    }
}

/// The declared layout every sheet this module writes carries.
pub fn sheet_cells() -> ThumbnailCells {
    ThumbnailCells {
        cols: SHEET_COLS as u32,
        cell_w: CELL_W as u32,
        cell_h: CELL_H as u32,
        first: 0,
        count: FRAME_COUNT as u32,
    }
}

/// Where cell `index` sits in the sheet, in sheet pixels.
fn cell_rect(index: usize) -> Rect {
    Rect {
        pos: dvec2(
            ((index % SHEET_COLS) * CELL_W) as f64,
            ((index / SHEET_COLS) * CELL_H) as f64,
        ),
        size: dvec2(CELL_W as f64, CELL_H as f64),
    }
}

/// The readback (BGRA, whatever size the platform's pass produced) as the
/// RGBA sheet the encoder wants. The GPU already packed the cells; all that
/// is left is the swizzle, opaque alpha, and — only if a backend ignored
/// the dpi override and handed back a scaled target — a box resample back
/// onto the declared grid, so the encoded cells always match the layout.
fn sheet_rgba_from_bgra(src: &[u8], sw: usize, sh: usize) -> Vec<u8> {
    let mut out = vec![0u8; SHEET_W * SHEET_H * 4];
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        return out;
    }
    for y in 0..SHEET_H {
        let y0 = y * sh / SHEET_H;
        let y1 = ((y + 1) * sh / SHEET_H).clamp(y0 + 1, sh);
        for x in 0..SHEET_W {
            let x0 = x * sw / SHEET_W;
            let x1 = ((x + 1) * sw / SHEET_W).clamp(x0 + 1, sw);
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let si = (sy * sw + sx) * 4;
                    // BGRA source.
                    b += src[si] as u32;
                    g += src[si + 1] as u32;
                    r += src[si + 2] as u32;
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            let di = (y * SHEET_W + x) * 4;
            out[di] = (r / count) as u8;
            out[di + 1] = (g / count) as u8;
            out[di + 2] = (b / count) as u8;
            out[di + 3] = 0xff;
        }
    }
    out
}

/// True when the whole sheet is essentially black — the honesty gate: a
/// black wall of tiles is worse than the seeded placeholders.
fn all_black(rgba: &[u8]) -> bool {
    !rgba
        .chunks_exact(4)
        .any(|px| px[0] > 12 || px[1] > 12 || px[2] > 12)
}

/// Convert + gate + encode + stamp + atomically land the cache file. Runs
/// on a worker: the UI thread never touches a sheet's pixels again after
/// the readback.
///
/// The artifact: 30 cell-resolution frames hardware-encoded to H.264 in an
/// mp4 through makepad-video (the platform-neutral seam — VideoToolbox /
/// Media Foundation / GStreamer), with the cell layout appended as a
/// trailer box (`anim_icon::stamp_layout_mp4`) that any demuxer skips and
/// `anim_icon::read_layout` recovers. H.264 rather than H.265 because it
/// is the arm the realtime encode lane has proven on BOTH desktop
/// platforms; H.265 encode support is uneven across Media Foundation
/// machines and buys nothing at 75KB a sheet.
fn encode_and_write(
    bgra: Vec<u8>,
    sw: usize,
    sh: usize,
    path: PathBuf,
    revision: AssetRevisionId,
) -> Result<FxThumbSheet, String> {
    let sheet = sheet_rgba_from_bgra(&bgra, sw, sh);
    if all_black(&sheet) {
        return Err("rendered black".to_string());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cache dir: {e}"))?;
    }
    // The tmp file keeps the .mp4 extension: platform muxers use it as the
    // container type hint.
    let tmp = path.with_extension("tmp.mp4");
    let tmp_str = tmp
        .to_str()
        .ok_or_else(|| format!("non-utf8 cache path: {}", tmp.display()))?
        .to_string();
    let mut encoder = VideoFileEncoder::new(
        &tmp_str,
        VideoFileEncoderOptions {
            codec: VideoFileCodec::H264,
            width: CELL_W as u32,
            height: CELL_H as u32,
            fps_num: SHEET_FPS as u32,
            fps_den: 1,
            video_bitrate_bps: THUMB_BITRATE_BPS,
            audio: None,
            // ALL-INTRA: an IDR-only stream cannot reorder frames, so
            // emission order IS presentation order on every decoder.
            keyframe_only: true,
        },
    )
    .map_err(|e| format!("thumb video encoder: {e}"))?;
    let mut cell = vec![0u8; CELL_W * CELL_H * 4];
    for k in 0..FRAME_COUNT {
        let cx0 = (k % SHEET_COLS) * CELL_W;
        let cy0 = (k / SHEET_COLS) * CELL_H;
        for y in 0..CELL_H {
            let src = ((cy0 + y) * SHEET_W + cx0) * 4;
            cell[y * CELL_W * 4..(y + 1) * CELL_W * 4]
                .copy_from_slice(&sheet[src..src + CELL_W * 4]);
        }
        encoder
            .push_frame_rgba8(&cell, None)
            .map_err(|e| format!("thumb video frame {k}: {e}"))?;
    }
    encoder.finish().map_err(|e| format!("thumb video finish: {e}"))?;
    // The layout trailer: the file carries its own cell grid, exactly as
    // the PNG tEXt stamp did, and survives a byte copy.
    let bytes = std::fs::read(&tmp).map_err(|e| format!("cache read-back: {e}"))?;
    let bytes = anim_icon::stamp_layout_mp4(&bytes, sheet_cells(), SHEET_FPS);
    std::fs::write(&tmp, &bytes).map_err(|e| format!("cache write: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cache rename: {e}"))?;
    Ok(FxThumbSheet { revision, path, cells: sheet_cells(), fps: SHEET_FPS })
}

/// Where a lane's job is in its life. The clock only ever moves in
/// [`VjFxView::tick_manual`] steps, so these are frame counts, not seconds.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    /// SEQUENTIAL PATH ONLY: drawing so the document's shader compiles
    /// before its stateful preroll; the clock stands still. Batched jobs
    /// skip straight past this.
    Warmup(u32),
    /// Running the clock up to [`PREROLL_SECS`], in `left` steps of `dt`.
    Preroll { left: usize, dt: f64 },
    /// Rendering captured frame `k` into cell `k` of the sheet.
    Capture(usize),
    /// BATCHED PATH: every scene is encoded; staged chain runs are still
    /// flushing (one chain beat).
    Flush,
    /// All thirty cells drawn; the readback waits one frame for the paint
    /// that drew the last of them.
    Readback,
    /// RENDERER-OWNED CAPTURE in flight: the platform blits the sheet on
    /// the producing GPU queue's own command buffer and the bytes arrive
    /// via `take_render_texture_captures` once that buffer completed —
    /// they provably follow the last cell's render.
    AwaitRead,
    /// Worker thread encoding + writing the cache file.
    Encoding,
}

struct ActiveJob {
    job: FxThumbJob,
    phase: Phase,
    encoder: Option<std::thread::JoinHandle<Result<FxThumbSheet, String>>>,
    /// LIVECODING: the log position this document's load started at, so a
    /// draw-shader failure raised while the lane rendered can be attributed
    /// to it (see `crate::livecode`).
    mark: u64,
    /// BATCHED PATH: cells whose scene is staged in a pooled step and
    /// whose chain runs in the NEXT paint: (pool step, cell index).
    pending_chain: Vec<(usize, usize)>,
    /// SEQUENTIAL PATH: a scene encoded last paint awaits its chain beat;
    /// `Some(cell)` for a capture step, `Some(None)` for warmup/preroll.
    seq_pending: Option<Option<usize>>,
    /// The sheet target holds drawn cells (clear turns into load-preserve).
    sheet_cleared: bool,
    /// When the renderer-owned capture was requested ([`Phase::AwaitRead`]):
    /// its read time for the report line.
    await_started: Instant,
    /// Frames spent in [`Phase::AwaitRead`] — the timeout counts POLLS, not
    /// wall time, so a stalled clock (occlusion, machine sleep) can never
    /// spuriously fail a lane whose capture is simply parked with the app.
    await_polls: u32,
    // Honest numbers for the report line.
    started: Instant,
    frames: u32,
    load_ms: f32,
    read_ms: f32,
    /// CPU spent in tick_manual across the sheet (sim + clock).
    tick_ms: f32,
    /// CPU spent encoding the draws (scene + chain + cell blits).
    draw_ms: f32,
}

/// One lane's own render target: the sheet the GPU packs cell by cell, plus
/// the pass that owns it. The effect's passes are begun INSIDE this one
/// (`make_child_pass`), so the cell blits — recorded in the sheet pass
/// itself, the PARENT — always execute after the frames they sample.
struct SheetTarget {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
}

impl SheetTarget {
    fn new(cx: &mut Cx, name: &str) -> Self {
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed { width: SHEET_W, height: SHEET_H },
                initial: true,
            },
        );
        Self {
            pass: DrawPass::new_with_name(cx, name),
            draw_list: DrawList2d::new(cx),
            texture,
        }
    }
}

#[derive(Default)]
struct Lane {
    sheet: Option<SheetTarget>,
    job: Option<ActiveJob>,
    /// CPU premix textures for transition previews (two distinct patterns
    /// dissolved by the sweep) — one PAIR PER POOLED STEP: the pattern is
    /// rewritten per tick, and steps sharing one texture in one paint
    /// would all sample the LAST tick's picture. Premix docs use pair
    /// slot 0; two-deck (`duo`) docs get pattern A in 0 and B in 1.
    trans_tex: Vec<[Option<Texture>; 2]>,
    prepared: bool,
}

/// The premix texture pair for pool step `step` (grown on demand). A free
/// fn so the lane's `job` can stay mutably borrowed alongside it.
fn trans_pair(trans_tex: &mut Vec<[Option<Texture>; 2]>, step: usize) -> &mut [Option<Texture>; 2] {
    while trans_tex.len() <= step {
        trans_tex.push([None, None]);
    }
    &mut trans_tex[step]
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VjFxThumbs {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The hidden slot-mode effect hosts, one per lane (CONTRACT.md's
    /// thumbnail hook).
    #[live]
    fx: VjFxView,
    #[live]
    fx1: VjFxView,
    #[live]
    fx2: VjFxView,
    #[live]
    fx3: VjFxView,
    #[live]
    fx4: VjFxView,
    #[live]
    fx5: VjFxView,
    /// Samples a sheet into this widget's 4x4 rect so the offscreen pass
    /// chain is a frame dependency (an unsampled render target never
    /// renders).
    #[live]
    draw_present: DrawVjFxPresent,
    /// The GPU downscale: one captured frame into one cell.
    #[live]
    draw_cell: DrawVjFxThumbCell,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    cache_dir: Option<PathBuf>,
    /// THE PENDING SET — one deep store (the whole library fits), pulled
    /// by [`Self::take_next`]'s strict classes. Order within the vec is
    /// enqueue order; the classes decide between entries at the pull, so
    /// membership can never go stale. Nothing in here expires.
    #[rust]
    pending: Vec<FxThumbJob>,
    /// CLASS 1: the revisions on screen this very tick, in pad order.
    #[rust]
    visible_now: Vec<AssetRevisionId>,
    /// CLASS 2: which effect tab is open — `Some(true)` TRANSITION,
    /// `Some(false)` EFFECT, `None` no effect tab (classes 2 and 3 merge).
    /// Matches [`FxThumbJob::transition`], which is the same split the two
    /// tabs draw.
    #[rust]
    open_tab: Option<bool>,
    #[rust]
    lanes: Vec<Lane>,
    #[rust]
    results: Vec<FxThumbSheet>,
    /// Revisions that failed to load or render — TERMINAL for this session:
    /// the pipeline is deterministic, so the same revision would fail the
    /// same way again. Only a new revision (a content change, hence a new
    /// key) bakes again. The tile paints the failure (see `take_failures`).
    #[rust]
    failed: HashMap<AssetRevisionId, String>,
    /// Failures not yet handed to the app (it mirrors them onto the tiles).
    #[rust]
    new_failures: Vec<AssetRevisionId>,
    /// Set once the platform proves it cannot read a render target back
    /// (web/headless); rendering is pointless then, cache decode still works.
    #[rust]
    disabled: Option<String>,
    /// Consecutive whole-job readback failures; one flaky texture must not
    /// switch the feature off, a platform that never answers must.
    #[rust]
    readback_failures: u32,
    /// Scratch for the transition premix patterns (one lane at a time).
    #[rust]
    trans_data: Vec<u32>,
    /// Politeness: a live set (output window up, a deck playing) keeps the
    /// bank at one lane and one step per frame.
    #[rust(true)]
    full_speed: bool,
    // Session totals for the report line.
    #[rust]
    done_count: usize,
    #[rust]
    done_ms: f32,
    #[rust]
    batch_started: Option<Instant>,
}

impl VjFxThumbs {
    pub fn set_cache_dir(&mut self, dir: PathBuf) {
        sweep_stale_cache(&dir);
        self.cache_dir = Some(dir);
    }

    pub fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }

    /// Nothing loaded, nothing pending: everything handed over is finished.
    pub fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.lanes.iter().all(|l| l.job.is_none())
    }

    /// A live set is running: keep the bank to one lane. At boot, idle, or a
    /// bulk regen it opens up to [`LANES`].
    pub fn set_full_speed(&mut self, full_speed: bool) {
        self.full_speed = full_speed;
    }

    fn lane_budget(&self) -> usize {
        if self.full_speed {
            // Nobody is performing: the bank owns the GPU.
            LANES
        } else {
            1
        }
    }

    /// THE PRIORITY FEED, one call per tick: what is on screen right now
    /// (in pad order) and which effect tab is open. [`Self::take_next`]
    /// classifies against exactly this, at the moment a lane frees — a
    /// scroll or tab switch reclassifies the whole pending set instantly,
    /// with no re-sorting to go stale. In-flight lanes are never touched.
    pub fn set_priority(&mut self, visible: &[AssetRevisionId], open_tab: Option<bool>) {
        self.visible_now.clear();
        self.visible_now.extend_from_slice(visible);
        self.open_tab = open_tab;
    }

    /// The next job a free lane renders, by STRICT CLASS — the user's law:
    /// "only generate icons for invisible things if the visible ones are
    /// done."
    fn take_next(&mut self) -> Option<FxThumbJob> {
        take_next_job(&mut self.pending, &self.visible_now, self.open_tab)
    }

    /// Terminal for this session: only a new revision renders again.
    pub fn is_failed(&self, revision: &AssetRevisionId) -> bool {
        self.failed.contains_key(revision)
    }

    /// Failures the app has not painted yet — the tile wears the failure
    /// (red ring + FAILED) instead of spinning forever.
    pub fn take_failures(&mut self) -> Vec<AssetRevisionId> {
        std::mem::take(&mut self.new_failures)
    }

    fn mark_failed(&mut self, revision: AssetRevisionId, error: String) {
        self.failed.insert(revision, error);
        self.new_failures.push(revision);
    }

    /// Already rendering or pending here. With several lanes in flight the
    /// app can reach a tile again while its document is still on a lane —
    /// without this it would fetch the source and render the same sheet
    /// twice.
    pub fn holds(&self, revision: &AssetRevisionId) -> bool {
        self.pending.iter().any(|j| &j.revision == revision)
            || self
                .lanes
                .iter()
                .any(|l| l.job.as_ref().is_some_and(|a| &a.job.revision == revision))
    }

    pub fn disabled_reason(&self) -> Option<&str> {
        self.disabled.as_deref()
    }

    pub fn take_results(&mut self) -> Vec<FxThumbSheet> {
        std::mem::take(&mut self.results)
    }

    /// Admission NEVER collapses: a job is refused only when the platform
    /// cannot render at all, its revision already failed (terminal), or the
    /// same revision is already pending/rendering. A REVISION CHANGE is an
    /// event: a job for the same asset under a new revision replaces the
    /// stale pending one (an in-flight render of the old revision finishes
    /// harmlessly — the cache is keyed by revision).
    pub fn enqueue(&mut self, cx: &mut Cx, job: FxThumbJob) {
        if self.disabled.is_some() || self.is_failed(&job.revision) || self.holds(&job.revision) {
            return;
        }
        self.pending.retain(|j| j.asset != job.asset);
        if self.batch_started.is_none() {
            self.batch_started = Some(Instant::now());
        }
        self.pending.push(job);
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }

    /// The lane bank, built on first use (one [`Lane`] per view field).
    fn ensure_lanes(&mut self) {
        while self.lanes.len() < LANES {
            self.lanes.push(Lane::default());
        }
    }

    /// The lane's view. Lanes are fixed struct fields (a widget's children
    /// are declared, not allocated), so the index maps by hand.
    fn view(&mut self, index: usize) -> &mut VjFxView {
        match index {
            0 => &mut self.fx,
            1 => &mut self.fx1,
            2 => &mut self.fx2,
            3 => &mut self.fx3,
            4 => &mut self.fx4,
            _ => &mut self.fx5,
        }
    }

    fn fail_lane(&mut self, cx: &mut Cx, index: usize, error: String) {
        if let Some(active) = self.lanes[index].job.take() {
            log!(
                "fx thumb: {} FAILED — {} (marked failed for this revision)",
                active.job.title,
                error
            );
            self.mark_failed(active.job.revision, error);
        }
        self.view(index).clear_effect(cx);
    }

    /// Pull pending documents onto free lanes, strictly by class
    /// ([`Self::take_next`]). At most [`LOADS_PER_FRAME`] a frame so a
    /// compile never lands on top of another one.
    fn start_jobs(&mut self, cx: &mut Cx) {
        // A platform that proved it cannot read back must stop PULLING:
        // without this the deep pending set kept rendering after disable
        // and marked the rest of the library failed.
        if self.disabled.is_some() {
            return;
        }
        let budget = self.lane_budget();
        let mut loads = 0;
        for index in 0..LANES {
            if loads >= LOADS_PER_FRAME || self.pending.is_empty() {
                return;
            }
            if index >= budget || self.lanes[index].job.is_some() {
                continue;
            }
            let running = self.lanes.iter().filter(|l| l.job.is_some()).count();
            if running >= budget {
                return;
            }
            let Some(job) = self.take_next() else { return };
            loads += 1;
            let prepared = self.lanes[index].prepared;
            let t0 = Instant::now();
            let view = self.view(index);
            if !prepared {
                view.set_manual_clock(true);
                view.set_bpm(120.0);
            }
            view.set_live(cx, true);
            // A previous transition job's sweep inputs/override must never
            // leak into the next document.
            view.set_input_texture(0, None);
            view.set_input_texture(1, None);
            view.set_user_override([None; 4]);
            let mark = crate::livecode::mark();
            let loaded = view.set_effect_source(cx, "vjfx_thumb", &job.source);
            self.lanes[index].prepared = true;
            match loaded {
                Ok(_) => {
                    self.lanes[index].job = Some(ActiveJob {
                        job,
                        phase: Phase::Warmup(0),
                        encoder: None,
                        mark,
                        pending_chain: Vec::new(),
                        seq_pending: None,
                        sheet_cleared: false,
                        await_started: t0,
                        await_polls: 0,
                        started: t0,
                        frames: 0,
                        load_ms: t0.elapsed().as_secs_f32() * 1000.0,
                        read_ms: 0.0,
                        tick_ms: 0.0,
                        draw_ms: 0.0,
                    });
                }
                Err(error) => {
                    log!(
                        "fx thumb: {} document failed to load — {error} (placeholder kept)",
                        job.title
                    );
                    // A parse failure is DEFINITE: report it as the answer
                    // rather than waiting to see what the shader does.
                    crate::livecode::report(
                        &job.revision.to_string(),
                        mark,
                        Err(error.clone()),
                    );
                    self.mark_failed(job.revision, error);
                }
            }
        }
    }

    /// Harvest finished encode workers without ever blocking the UI thread.
    fn poll_encoders(&mut self) {
        for index in 0..self.lanes.len() {
            let finished = self.lanes[index].job.as_ref().is_some_and(|a| {
                a.phase == Phase::Encoding && a.encoder.as_ref().is_some_and(|h| h.is_finished())
            });
            if !finished {
                continue;
            }
            let Some(mut active) = self.lanes[index].job.take() else { continue };
            let Some(handle) = active.encoder.take() else { continue };
            let total_ms = active.started.elapsed().as_secs_f32() * 1000.0;
            // LIVECODING: the document parsed and a full sheet was DRAWN, so
            // whatever the shader had to say has been said by now. A black
            // sheet is not itself a verdict — an input-shaping transition
            // doc renders black with no program behind it — so the answer
            // is whatever the compiler actually reported, and nothing means
            // it compiled.
            crate::livecode::report(&active.job.revision.to_string(), active.mark, Ok(()));
            match handle.join() {
                Ok(Ok(sheet)) => {
                    self.done_count += 1;
                    self.done_ms += total_ms;
                    log!(
                        "fx thumb: {} rendered — {} frames over {:.1}s in {:.0}ms ({} steps: tick {:.1}ms, encode {:.1}ms, load {:.1}ms, readback {:.1}ms); {} done, avg {:.0}ms, {:.1}s elapsed",
                        active.job.title,
                        FRAME_COUNT,
                        CAPTURE_SPAN,
                        total_ms,
                        active.frames,
                        active.tick_ms,
                        active.draw_ms,
                        active.load_ms,
                        active.read_ms,
                        self.done_count,
                        self.done_ms / self.done_count as f32,
                        self.batch_started
                            .map(|t| t.elapsed().as_secs_f32())
                            .unwrap_or(0.0),
                    );
                    self.results.push(sheet);
                }
                Ok(Err(error)) => {
                    log!(
                        "fx thumb: {} FAILED — {error} (marked failed for this revision)",
                        active.job.title
                    );
                    self.mark_failed(active.job.revision, error);
                }
                Err(_) => {
                    self.mark_failed(
                        active.job.revision,
                        "encode worker panicked".to_string(),
                    );
                }
            }
        }
    }

    /// Bind this capture step's inputs (transition sweep / screen stand-in)
    /// for the frame about to be encoded into pool step `step`. `k` is the
    /// cell being captured; document time, not wall time, drives the
    /// patterns so a sheet is reproducible.
    fn bind_step_inputs(
        cx: &mut Cx,
        view: &mut VjFxView,
        lane_trans: &mut [Option<Texture>; 2],
        scratch: &mut Vec<u32>,
        transition: bool,
        k: usize,
    ) {
        if transition {
            let m = (k as f32 / (FRAME_COUNT - 1) as f32).clamp(0.0, 1.0);
            let time = (PREROLL_SECS + k as f64 * FRAME_STEP) as f32;
            if view.wants_deck_inputs() {
                // Two-deck doc: the distinct patterns land on separate
                // inputs and p3 sweeps as the crossfader would — the
                // thumbnail replays the transition exactly.
                let tex_a = transition_input(cx, scratch, &mut lane_trans[0], 0.0, time);
                let tex_b = transition_input(cx, scratch, &mut lane_trans[1], 1.0, time);
                view.set_input_texture(0, Some(tex_a));
                view.set_input_texture(1, Some(tex_b));
                view.set_user_override([None, None, None, Some(m)]);
            } else {
                // Premix doc: one input carrying the sweeping dissolve,
                // intensity on the engage triangle.
                let tex = transition_input(cx, scratch, &mut lane_trans[0], m, time);
                view.set_input_texture(0, Some(tex));
                let tri = 1.0 - (2.0 * m - 1.0).abs();
                view.set_user_override([None, None, None, Some(tri)]);
            }
        } else if view.is_screen_engine() {
            // A screen doc is a remap OF ITS CONTENT: baked against the
            // engine's featureless fallback blob, a mirror looks blank and
            // a tile looks like mud — and baked against the deliberately
            // MUTED deck stand-in the whole family reads as a brown-gray
            // smudge at tile size. It gets the vivid test card instead:
            // the warp itself becomes the picture, in full color.
            let time = (PREROLL_SECS + k as f64 * FRAME_STEP) as f32;
            let tex = screen_input(cx, scratch, &mut lane_trans[0], time);
            view.set_input_texture(0, Some(tex));
        }
    }

    /// One lane's UI frame. Both paths run INSIDE the lane's sheet pass so
    /// the effect's own passes become its children (executed before it) and
    /// the cell blits recorded here read the frames of this very paint (or,
    /// for staged batches, of the paint their scenes settled in). Returns
    /// true when the sheet is complete and waiting for its readback.
    fn draw_lane(
        cx: &mut Cx2d,
        view: &mut VjFxView,
        lane: &mut Lane,
        cell: &mut DrawVjFxThumbCell,
        scratch: &mut Vec<u32>,
        budget: &mut usize,
    ) -> bool {
        match lane.job.as_ref().map(|a| a.phase) {
            None | Some(Phase::Encoding) => return false,
            Some(Phase::Readback) => return true,
            // Capture in flight: the sheet pass must NOT be re-begun —
            // its retained draw lists are what the requested repaint
            // re-executes together with the capture blit.
            Some(Phase::AwaitRead) => return false,
            _ => {}
        }
        if *budget == 0 {
            return false;
        }

        // Disjoint lane borrows for the step helpers below.
        let Lane { sheet, job, trans_tex, .. } = lane;

        // The sheet pass. dpi 1.0: the sheet target is exact texels, so the
        // readback IS the finished sheet.
        let sheet = sheet.get_or_insert_with(|| SheetTarget::new(cx.cx, "vjfx_thumb_sheet"));
        let size = dvec2(SHEET_W as f64, SHEET_H as f64);
        let cleared = job.as_ref().is_some_and(|a| a.sheet_cleared);
        sheet.pass.set_size(cx, size);
        sheet.pass.set_color_texture(
            cx.cx,
            &sheet.texture,
            if cleared {
                // Load: the cells already drawn stay drawn.
                DrawPassClearColor::InitWith(vec4(0.0, 0.0, 0.0, 1.0))
            } else {
                DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0))
            },
        );
        cx.make_child_pass(&sheet.pass);
        cx.begin_pass(&sheet.pass, Some(1.0));
        sheet.pass.set_size(cx, size);
        sheet.draw_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());

        let ready = if view.bake_batch_limit() > 1 {
            Self::lane_steps_batched(cx, view, job, trans_tex, cell, scratch, budget)
        } else {
            Self::lane_step_sequential(cx, view, job, trans_tex, cell, scratch, budget)
        };

        cx.end_pass_sized_turtle();
        sheet.draw_list.end(cx);
        cx.end_pass(&sheet.pass);
        ready
    }

    /// One sequential step is done (its chain ran, or it had no chain):
    /// advance the phase. `stepped` = [`VjFxView::needs_stepped_time`].
    fn advance_sequential_phase(phase: Phase, stepped: bool) -> Phase {
        match phase {
            Phase::Warmup(n) if n + 1 < WARMUP_DRAWS => Phase::Warmup(n + 1),
            Phase::Warmup(_) => {
                if stepped {
                    Phase::Preroll { left: PREROLL_STEPS, dt: FRAME_STEP }
                } else {
                    // Nothing in this document remembers the frames before
                    // it: one jump lands on the same picture.
                    Phase::Preroll { left: 1, dt: PREROLL_SECS }
                }
            }
            Phase::Preroll { left, dt } if left > 1 => Phase::Preroll { left: left - 1, dt },
            Phase::Preroll { .. } => Phase::Capture(0),
            Phase::Capture(k) if k + 1 < FRAME_COUNT => Phase::Capture(k + 1),
            Phase::Capture(_) => Phase::Readback,
            other => other,
        }
    }

    /// THE SEQUENTIAL PATH — one step at a time for documents whose
    /// picture carries GPU state from frame to frame (feedback/hold
    /// stages, sim fields): their chain and sim passes execute once per
    /// paint BY LAW. A step spans TWO paints when the document has a
    /// chain: scene (with the sim update) in one, the chain over the
    /// settled scene in the next — the same ordering law as the pooled
    /// steps, without which cells alternate between two time streams and
    /// the tile flickers. Stage-less steps finish in one paint.
    fn lane_step_sequential(
        cx: &mut Cx2d,
        view: &mut VjFxView,
        job: &mut Option<ActiveJob>,
        trans_tex: &mut Vec<[Option<Texture>; 2]>,
        cell: &mut DrawVjFxThumbCell,
        scratch: &mut Vec<u32>,
        budget: &mut usize,
    ) -> bool {
        let Some(active) = job.as_mut() else { return false };
        *budget = budget.saturating_sub(1);

        // CHAIN HALF: last paint's scene has settled; run the chain, blit
        // if this step captured, and only then advance the phase. With the
        // view's ping-pong scene targets the NEXT scene may encode in this
        // same paint (fall through below) — sim-field documents keep
        // strict alternation ([`VjFxView::bake_seq_pipeline`]).
        if let Some(capture) = active.seq_pending.take() {
            let t_draw = Instant::now();
            let texture = view.seq_chain_step(cx);
            if let (Some(k), Some(texture)) = (capture, texture) {
                cell.draw_vars.set_texture(0, &texture);
                cell.draw_abs(cx, cell_rect(k));
                active.sheet_cleared = true;
            }
            active.draw_ms += t_draw.elapsed().as_secs_f32() * 1000.0;
            active.phase =
                Self::advance_sequential_phase(active.phase, view.needs_stepped_time());
            if !view.bake_seq_pipeline()
                || matches!(active.phase, Phase::Readback | Phase::Flush | Phase::Encoding)
            {
                return false;
            }
        }

        // SCENE HALF. What this step stands for:
        let phase = active.phase;
        let (dt, capture) = match phase {
            Phase::Warmup(_) => (0.0, None),
            Phase::Preroll { dt, .. } => (dt, None),
            Phase::Capture(k) => (FRAME_STEP, Some(k)),
            _ => (0.0, None),
        };
        let t_tick = Instant::now();
        view.tick_manual(cx.cx, dt);
        active.tick_ms += t_tick.elapsed().as_secs_f32() * 1000.0;
        active.frames += 1;

        let t_draw = Instant::now();
        Self::bind_step_inputs(
            cx.cx,
            view,
            trans_pair(trans_tex, 0),
            scratch,
            active.job.transition,
            capture.unwrap_or(0),
        );
        view.seq_scene_step(cx);
        if view.bake_needs_chain_beat() {
            // The chain (and the blit, and the phase advance) run next
            // paint, over this scene once it has settled.
            active.seq_pending = Some(capture);
        } else {
            // No chain passes at all: the blit reads the scene texture
            // from the sheet pass, whose children always run first.
            let texture = view.seq_chain_step(cx);
            if let (Some(k), Some(texture)) = (capture, texture) {
                cell.draw_vars.set_texture(0, &texture);
                cell.draw_abs(cx, cell_rect(k));
                active.sheet_cleared = true;
            }
            active.phase =
                Self::advance_sequential_phase(phase, view.needs_stepped_time());
        }
        active.draw_ms += t_draw.elapsed().as_secs_f32() * 1000.0;
        false
    }

    /// THE BATCHED PATH — several capture steps share one paint through
    /// the view's bake pool. Alternation law (see `BakeStep` in view.rs):
    /// a paint either ENCODES SCENES (and, for a stage-less doc, blits
    /// them — the sheet pass is their parent and executes after them) or
    /// FLUSHES CHAINS staged the paint before; never both into one step's
    /// textures, because sibling pass order is not encode order.
    fn lane_steps_batched(
        cx: &mut Cx2d,
        view: &mut VjFxView,
        job: &mut Option<ActiveJob>,
        trans_tex: &mut Vec<[Option<Texture>; 2]>,
        cell: &mut DrawVjFxThumbCell,
        scratch: &mut Vec<u32>,
        budget: &mut usize,
    ) -> bool {
        let Some(active) = job.as_mut() else { return false };

        // CHAIN BEAT: the staged scenes settled at the last paint; run
        // their chains and blit. Nothing else this paint — the pool steps
        // must not be re-encoded under a chain that reads them.
        if !active.pending_chain.is_empty() {
            let t_draw = Instant::now();
            let pending = std::mem::take(&mut active.pending_chain);
            for (step, k) in pending {
                if let Some(texture) = view.bake_chain_step(cx, step) {
                    cell.draw_vars.set_texture(0, &texture);
                    cell.draw_abs(cx, cell_rect(k));
                    active.sheet_cleared = true;
                }
                *budget = budget.saturating_sub(1);
            }
            active.draw_ms += t_draw.elapsed().as_secs_f32() * 1000.0;
            if active.phase == Phase::Flush {
                active.phase = Phase::Readback;
            }
            return false;
        }
        if active.phase == Phase::Flush {
            active.phase = Phase::Readback;
            return true;
        }

        // Batched documents need no warmup: the pipeline builds in the
        // same paint that executes the first batch's draws.
        if let Phase::Warmup(_) = active.phase {
            active.phase = if view.needs_stepped_time() {
                Phase::Preroll { left: PREROLL_STEPS, dt: FRAME_STEP }
            } else {
                Phase::Preroll { left: 1, dt: PREROLL_SECS }
            };
        }

        // PREROLL, CPU-only: a batchable document's preroll renders were
        // never seen and fed no GPU state — the regen sequence (same dts)
        // is the whole effect. Runs to completion in one paint.
        if let Phase::Preroll { mut left, dt } = active.phase {
            let t_tick = Instant::now();
            while left > 0 {
                view.tick_manual(cx.cx, dt);
                view.bake_advance_only();
                active.frames += 1;
                left -= 1;
            }
            active.tick_ms += t_tick.elapsed().as_secs_f32() * 1000.0;
            *budget = budget.saturating_sub(1);
            active.phase = Phase::Capture(0);
        }

        // SCENE BEAT: encode as many captures as pool, budget and the
        // remaining cells allow.
        let deferred = view.bake_needs_chain_beat();
        let limit = view.bake_batch_limit();
        let mut used = 0usize;
        while let Phase::Capture(k) = active.phase {
            if *budget == 0 || used >= limit {
                break;
            }
            let t_tick = Instant::now();
            view.tick_manual(cx.cx, FRAME_STEP);
            active.tick_ms += t_tick.elapsed().as_secs_f32() * 1000.0;
            active.frames += 1;

            let t_draw = Instant::now();
            Self::bind_step_inputs(
                cx.cx,
                view,
                trans_pair(trans_tex, used),
                scratch,
                active.job.transition,
                k,
            );
            view.bake_scene_step(cx, used);
            if deferred {
                active.pending_chain.push((used, k));
            } else if let Some(texture) = view.bake_chain_step(cx, used) {
                cell.draw_vars.set_texture(0, &texture);
                cell.draw_abs(cx, cell_rect(k));
                active.sheet_cleared = true;
            }
            active.draw_ms += t_draw.elapsed().as_secs_f32() * 1000.0;

            used += 1;
            *budget = budget.saturating_sub(1);
            active.phase = if k + 1 < FRAME_COUNT {
                Phase::Capture(k + 1)
            } else if deferred {
                // Scenes all encoded; the last chains flush next paint.
                Phase::Flush
            } else {
                Phase::Readback
            };
        }
        false
    }

    /// The sheet is drawn: get its pixels off the GPU once. Preferred path
    /// is the RENDERER-OWNED capture — the platform encodes the readback
    /// blit after the sheet pass on the PRODUCING GPU queue and delivers
    /// the bytes only from that command buffer's completion handler, so
    /// they provably follow the last cell's render. The old direct
    /// `debug_read_render_texture` synchronizes on a DIFFERENT queue and
    /// could read the sheet before it finished rendering (the intermittent
    /// half-black sheets); it survives only as the fallback for backends
    /// whose readback is inherently ordered (GL/D3D11) or absent.
    fn finish_lane(&mut self, cx: &mut Cx, index: usize) {
        if self.cache_dir.is_none() {
            self.fail_lane(cx, index, "no cache dir configured".to_string());
            return;
        }
        let Some((texture, pass_id)) = self.lanes[index]
            .sheet
            .as_ref()
            .map(|s| (s.texture.clone(), s.pass.draw_pass_id()))
        else {
            self.fail_lane(cx, index, "no sheet target".to_string());
            return;
        };
        if cx.request_render_texture_capture(&texture) {
            // Repaint the sheet pass so the producing queue's next command
            // buffer carries the blit (its retained draw lists re-execute
            // — same cells, same pixels); the bytes arrive through
            // `take_render_texture_captures` next frame(s).
            cx.repaint_pass(pass_id);
            if let Some(active) = self.lanes[index].job.as_mut() {
                active.phase = Phase::AwaitRead;
                active.await_started = Instant::now();
                active.await_polls = 0;
            }
            // The effect can stop ticking now — every cell is encoded.
            self.view(index).clear_effect(cx);
            return;
        }
        let t0 = Instant::now();
        let Some((w, h, bytes)) = cx.debug_read_render_texture(&texture) else {
            self.readback_failures += 1;
            if self.readback_failures >= 3 {
                // Three different jobs in a row could not read back: this
                // platform has no readback (web/headless). Stop trying;
                // cached sheets still decode.
                self.disabled = Some("render-target readback unavailable".to_string());
            }
            self.fail_lane(cx, index, "render-target readback unavailable".to_string());
            return;
        };
        self.readback_failures = 0;
        let read_ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.spawn_encode(cx, index, w, h, bytes, read_ms);
    }

    /// The sheet bytes are on the CPU: hand them to the encode worker. The
    /// common tail of both readback paths.
    fn spawn_encode(
        &mut self,
        cx: &mut Cx,
        index: usize,
        w: usize,
        h: usize,
        bytes: Vec<u8>,
        read_ms: f32,
    ) {
        let Some(dir) = self.cache_dir.clone() else {
            self.fail_lane(cx, index, "no cache dir configured".to_string());
            return;
        };
        let Some(active) = self.lanes[index].job.as_mut() else { return };
        active.read_ms = read_ms;
        let revision = active.job.revision;
        let path = cache_path(&dir, &revision, active.job.transition);
        active.phase = Phase::Encoding;
        active.encoder = Some(std::thread::spawn(move || {
            encode_and_write(bytes, w, h, path, revision)
        }));
        // The effect can stop ticking now — the pixels are on the worker.
        self.view(index).clear_effect(cx);
    }

    /// Land finished renderer-owned captures on their waiting lanes, and
    /// time out a capture the platform never answered (fail the lane the
    /// same way an unavailable readback does).
    fn poll_captures(&mut self, cx: &mut Cx) {
        for (tid, w, h, bytes) in cx.take_render_texture_captures() {
            let found = (0..self.lanes.len()).find(|&i| {
                self.lanes[i]
                    .job
                    .as_ref()
                    .is_some_and(|a| a.phase == Phase::AwaitRead)
                    && self.lanes[i]
                        .sheet
                        .as_ref()
                        .is_some_and(|s| s.texture.texture_id() == tid)
            });
            let Some(index) = found else { continue };
            self.readback_failures = 0;
            let read_ms = self.lanes[index]
                .job
                .as_ref()
                .map(|a| a.await_started.elapsed().as_secs_f32() * 1000.0)
                .unwrap_or(0.0);
            self.spawn_encode(cx, index, w, h, bytes, read_ms);
        }
        for index in 0..self.lanes.len() {
            let timed_out = match self.lanes[index].job.as_mut() {
                Some(a) if a.phase == Phase::AwaitRead => {
                    a.await_polls += 1;
                    // ~10s of frames at 60Hz; polls, not wall time (above).
                    a.await_polls > 600
                }
                _ => false,
            };
            if timed_out {
                self.readback_failures += 1;
                if self.readback_failures >= 3 {
                    self.disabled = Some("render-target capture never completes".to_string());
                }
                self.fail_lane(cx, index, "render-target capture timed out".to_string());
            }
        }
    }

    /// TEMPORARY: env-gated view of the bank's real occupancy.
    fn debug_bank(&mut self) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static TICK: AtomicU32 = AtomicU32::new(0);
        if std::env::var("VJ_FX_BANK_DEBUG").is_err() {
            return;
        }
        let n = TICK.fetch_add(1, Ordering::Relaxed);
        if n % 20 != 0 {
            return;
        }
        let phases: String = self
            .lanes
            .iter()
            .map(|l| match l.job.as_ref().map(|a| a.phase) {
                None => '.',
                Some(Phase::Warmup(_)) => 'w',
                Some(Phase::Preroll { .. }) => 'p',
                Some(Phase::Capture(_)) => 'c',
                Some(Phase::Flush) => 'f',
                Some(Phase::Readback) => 'r',
                Some(Phase::AwaitRead) => 'a',
                Some(Phase::Encoding) => 'e',
            })
            .collect();
        log!(
            "fx bank: budget={} full_speed={} pending={} lanes=[{}]",
            self.lane_budget(),
            self.full_speed,
            self.pending.len(),
            phases
        );
    }
}

/// THE CLASS PULL, standalone so the tests can pin it. Classes are decided
/// HERE, at the pull, against the live `visible`/`open_tab` — never baked
/// into the queue's order, which is what let the old FIFO go stale:
///
/// 1. visible on screen right now, in pad order;
/// 2. the rest of the open tab (`FxThumbJob::transition == open_tab`),
///    most recently enqueued first;
/// 3. other tabs, most recently enqueued first — touched only when
///    classes 1 and 2 are empty.
fn take_next_job(
    pending: &mut Vec<FxThumbJob>,
    visible: &[AssetRevisionId],
    open_tab: Option<bool>,
) -> Option<FxThumbJob> {
    for revision in visible {
        if let Some(at) = pending.iter().position(|j| &j.revision == revision) {
            return Some(pending.remove(at));
        }
    }
    if let Some(transition_tab) = open_tab {
        if let Some(at) = pending.iter().rposition(|j| j.transition == transition_tab) {
            return Some(pending.remove(at));
        }
    }
    pending.pop()
}

const TRANS_W: usize = crate::effects::deck_pattern::W;
const TRANS_H: usize = crate::effects::deck_pattern::H;

/// The transition preview's input: TWO visibly different deck stand-ins —
/// a dim warm slate with a drifting disc, and a dim cool slate ruled by a
/// grid and a counter-drifting bar — dissolved by `m` (the thumbnail's
/// sweeping crossfade), exactly the slot runtime's premix-as-input0
/// contract. `m = 0` is pure pattern A, `m = 1` pure pattern B.
///
/// The pixel math lives in [`crate::effects::deck_pattern`] so the gallery's
/// static preview cannot drift from what the tiles are actually baked with,
/// and so the "keep it quiet" ceiling is stated once. Change the look there
/// and bump [`RECIPE`] — the cache key carries it.
/// The SCREEN-ENGINE bake input: a VIVID test card. A screen doc is a
/// remap of its content, and against the deliberately muted two-deck
/// stand-in ([`crate::effects::deck_pattern`], peak 0.52 by law) every
/// rotozoom/stretch/crop tile read as the same brown-gray smudge. This
/// card is built to survive remapping instead: a saturated hue field (any
/// crop keeps color), a dark rule grid (any stretch or rotation shows its
/// geometry), and a drifting white ring (direction and scale read at a
/// glance). Live decks never see it — it exists only under the bake.
fn screen_input(
    cx: &mut Cx,
    data: &mut Vec<u32>,
    slot: &mut Option<Texture>,
    time: f32,
) -> Texture {
    const W: usize = TRANS_W;
    const H: usize = TRANS_H;
    data.resize(W * H, 0);
    let center = (0.5 + (time * 0.7).cos() * 0.22, 0.5 + (time * 0.53).sin() * 0.18);
    let aspect = W as f32 / H as f32;
    for y in 0..H {
        let v = y as f32 / H as f32;
        for x in 0..W {
            let u = x as f32 / W as f32;
            let hue = (u * 0.8 + v * 0.4 + time * 0.03).fract();
            let (mut r, mut g, mut b) = hue_rgb(hue);
            // Rule grid: dark lines so warped geometry stays legible.
            if (u * 8.0).fract() < 0.045 || (v * 4.5).fract() < 0.08 {
                r *= 0.22;
                g *= 0.22;
                b *= 0.22;
            }
            // Drifting white ring: direction and scale cue.
            let dx = (u - center.0) * aspect;
            let dy = v - center.1;
            let d = (dx * dx + dy * dy).sqrt();
            let ring = (1.0 - (d - 0.16).abs() * 28.0).clamp(0.0, 1.0);
            r += (1.0 - r) * ring;
            g += (1.0 - g) * ring;
            b += (1.0 - b) * ring;
            data[y * W + x] = 0xff00_0000
                | (((r * 255.0) as u32) << 16)
                | (((g * 255.0) as u32) << 8)
                | ((b * 255.0) as u32);
        }
    }
    match slot {
        Some(tex) => {
            tex.set_data_u32(cx, W, H, data.clone());
            tex.clone()
        }
        None => {
            let tex = Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: W,
                    height: H,
                    data: Some(data.clone()),
                    updated: TextureUpdated::Full,
                },
            );
            *slot = Some(tex.clone());
            tex
        }
    }
}

/// Bright six-segment hue wheel (S=1, V=1) — the test card wants
/// saturation, not colorimetric fidelity.
fn hue_rgb(h: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(1.0) * 6.0;
    let x = 1.0 - (h.rem_euclid(2.0) - 1.0).abs();
    match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    }
}

fn transition_input(
    cx: &mut Cx,
    data: &mut Vec<u32>,
    slot: &mut Option<Texture>,
    m: f32,
    time: f32,
) -> Texture {
    use crate::effects::deck_pattern;
    const W: usize = TRANS_W;
    const H: usize = TRANS_H;
    data.resize(W * H, 0);
    let drift = ((time * 0.7).cos() * 0.25, (time * 0.53).sin() * 0.2);
    let bar = (time * 0.4).fract();
    for y in 0..H {
        let v = y as f32 / H as f32;
        for x in 0..W {
            let u = x as f32 / W as f32;
            data[y * W + x] = deck_pattern::texel_bgra(u, v, m, drift, bar);
        }
    }
    match slot {
        Some(tex) => {
            tex.set_data_u32(cx, W, H, data.clone());
            tex.clone()
        }
        None => {
            let tex = Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: W,
                    height: H,
                    data: Some(data.clone()),
                    updated: TextureUpdated::Full,
                },
            );
            *slot = Some(tex.clone());
            tex
        }
    }
}

impl WidgetNode for VjFxThumbs {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for VjFxThumbs {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The hosts run on the manual clock (this widget drives every frame
        // they render), but they are still widgets: forward the event.
        self.fx.handle_event(cx, event, scope);
        self.fx1.handle_event(cx, event, scope);
        self.fx2.handle_event(cx, event, scope);
        self.fx3.handle_event(cx, event, scope);
        self.fx4.handle_event(cx, event, scope);
        self.fx5.handle_event(cx, event, scope);
        if self.next_frame.is_event(event).is_some() && !self.is_idle() {
            self.area.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        // The slot's LOGICAL size, chosen so its PHYSICAL size is exactly
        // [`SLOT_W`]x[`SLOT_H`] on any screen — the bake is screen-blind.
        let dpi = cx.current_dpi_factor().max(0.5);
        let slot = dvec2(SLOT_W / dpi, SLOT_H / dpi);
        self.ensure_lanes();
        self.poll_encoders();
        self.poll_captures(cx.cx);
        self.start_jobs(cx.cx);
        self.debug_bank();

        let mut budget = if self.full_speed { STEP_BUDGET } else { 1 };
        let mut ready: Vec<usize> = Vec::new();
        let mut last_sheet: Option<Texture> = None;
        for index in 0..LANES {
            if self.lanes[index].job.is_none() {
                continue;
            }
            // Disjoint borrows: the lane's view field, its lane state, the
            // shared cell shader and the premix scratch.
            let (view, lane): (&mut VjFxView, &mut Lane) = match index {
                0 => (&mut self.fx, &mut self.lanes[0]),
                1 => (&mut self.fx1, &mut self.lanes[1]),
                2 => (&mut self.fx2, &mut self.lanes[2]),
                3 => (&mut self.fx3, &mut self.lanes[3]),
                4 => (&mut self.fx4, &mut self.lanes[4]),
                _ => (&mut self.fx5, &mut self.lanes[5]),
            };
            view.set_slot_size(slot);
            if self.full_speed {
                view.bake_pool_ensure(cx.cx, BAKE_STEPS);
            }
            if Self::draw_lane(cx, view, lane, &mut self.draw_cell, &mut self.trans_data, &mut budget) {
                ready.push(index);
            }
            if let Some(sheet) = self.lanes[index].sheet.as_ref() {
                last_sheet = Some(sheet.texture.clone());
            }
        }
        // Readbacks are the one blocking thing here: bounded per frame.
        for index in ready.into_iter().take(READBACKS_PER_FRAME) {
            self.finish_lane(cx.cx, index);
        }
        // Sample a sheet into our 4x4 rect: the frame dependency that makes
        // the offscreen chain actually render.
        if let Some(texture) = last_sheet {
            self.draw_present.draw_vars.set_texture(0, &texture);
            self.draw_present.draw_abs(cx, rect);
        }
        cx.end_turtle_with_area(&mut self.area);
        if !self.is_idle() {
            self.area.redraw(cx.cx);
            self.next_frame = cx.cx.new_next_frame();
        } else {
            self.batch_started = None;
            // Hand the bake pools' MEMORY back the moment the bank rests —
            // never their pass identity (see bake_pool_release_memory: pass
            // ids recycle into execution order, so pool objects are
            // permanent by law).
            for index in 0..LANES {
                self.view(index).bake_pool_release_memory(cx.cx);
            }
            for lane in &mut self.lanes {
                lane.trans_tex.clear();
            }
        }
        DrawStep::done()
    }
}

/// The GPU cell downscale (see the shader in `script_mod!` above).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxThumbCell {
    #[deref]
    pub draw_super: DrawQuad,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_geometry_declares_exactly_what_the_gpu_packs() {
        let cells = sheet_cells();
        // The declared grid fits the sheet target exactly (the same
        // validation the store applies to a published thumbnail's views).
        let last = cells.first + cells.count - 1;
        let right = ((last % cells.cols) + 1) * cells.cell_w;
        let bottom = ((last / cells.cols) + 1) * cells.cell_h;
        assert!(
            right as usize <= SHEET_W && bottom as usize <= SHEET_H,
            "{right}x{bottom} vs {SHEET_W}x{SHEET_H}"
        );
        // Cells match the measured reference tile (383x172 physical px)
        // 1:1 across the width, at 2x supersample.
        assert_eq!((CELL_W, CELL_H), (384, 240));
        assert_eq!((SLOT_W as usize, SLOT_H as usize), (768, 480));
        // Every cell rect lands inside the sheet, on its own row/column.
        let r5 = cell_rect(5);
        assert_eq!(r5.pos, dvec2((5 * CELL_W) as f64, 0.0));
        let r6 = cell_rect(6);
        assert_eq!(r6.pos, dvec2(0.0, CELL_H as f64));
        let last_rect = cell_rect(FRAME_COUNT - 1);
        assert!(last_rect.pos.x + last_rect.size.x <= SHEET_W as f64);
        assert!(last_rect.pos.y + last_rect.size.y <= SHEET_H as f64);
        // Playback covers the capture span in real time, and the clock the
        // capture runs on is exactly that playback rate.
        assert!((SHEET_FPS * CAPTURE_SPAN as f32 - FRAME_COUNT as f32).abs() < 1e-6);
        assert!((FRAME_STEP * SHEET_FPS as f64 - 1.0).abs() < 1e-9);
        assert_eq!(PREROLL_STEPS, 27);
        // The cache artifact is mp4, and the recipe is part of the name —
        // the startup sweep keys off exactly this suffix.
        let rev = AssetRevisionId::from_bytes([7; 32]);
        let path = cache_path(Path::new("/tmp"), &rev, false);
        assert!(path.to_str().unwrap().ends_with(&format!("-r{RECIPE}.mp4")));
        let path = cache_path(Path::new("/tmp"), &rev, true);
        assert!(path
            .to_str()
            .unwrap()
            .ends_with(&format!("-t3-d{DECK_RECIPE}-r{RECIPE}.mp4")));
    }

    #[test]
    fn readback_becomes_rgba_and_never_panics_on_junk() {
        // An exact-size readback of solid blue-ish BGRA.
        let mut src = vec![0u8; SHEET_W * SHEET_H * 4];
        for px in src.chunks_exact_mut(4) {
            px.copy_from_slice(&[200, 100, 50, 128]); // B G R A
        }
        let sheet = sheet_rgba_from_bgra(&src, SHEET_W, SHEET_H);
        assert_eq!(sheet.len(), SHEET_W * SHEET_H * 4);
        assert_eq!(&sheet[..4], &[50, 100, 200, 255], "BGRA -> opaque RGBA");
        // A backend that ignored the dpi override still lands on the grid.
        let big = vec![128u8; SHEET_W * 2 * SHEET_H * 2 * 4];
        let sheet = sheet_rgba_from_bgra(&big, SHEET_W * 2, SHEET_H * 2);
        assert!(sheet.chunks_exact(4).all(|px| px[0] == 128 && px[3] == 255));
        // Degenerate inputs return an empty sheet rather than panicking.
        let junk = sheet_rgba_from_bgra(&[1, 2, 3], 999, 999);
        assert_eq!(junk.len(), SHEET_W * SHEET_H * 4);
        let zero = sheet_rgba_from_bgra(&[], 0, 0);
        assert_eq!(zero.len(), SHEET_W * SHEET_H * 4);
    }

    /// THE QUEUE LAW: "only generate icons for invisible things if the
    /// visible ones are done." Three strict classes, decided at the pull —
    /// this test is the regression tripwire against the one-mixed-queue
    /// failure (visible tiles waiting behind an invisible tab's work).
    #[test]
    fn lanes_pull_visible_first_then_open_tab_then_the_rest() {
        let rev = |n: u8| AssetRevisionId::from_bytes([n; 32]);
        let job = |n: u8, transition: bool| FxThumbJob {
            asset: AssetId::from_bytes([n; 16]),
            revision: rev(n),
            title: format!("doc {n}"),
            source: String::new(),
            transition,
        };
        // Enqueue order: transition docs first, then effect docs — the
        // WRONG order for an open EFFECT tab, so only the classes can fix
        // it. Doc 5 (effect) and doc 2 (transition) are on screen.
        let mut pending = vec![job(1, true), job(2, true), job(3, false), job(4, false), job(5, false)];
        let visible = [rev(5), rev(2)];
        let effect_tab = Some(false);
        let order: Vec<u8> = std::iter::from_fn(|| {
            take_next_job(&mut pending, &visible, effect_tab).map(|j| j.revision.as_bytes()[0])
        })
        .collect();
        // Class 1 in pad order (5 then 2 — even though 2 is the other
        // tab's doc, VISIBLE wins over everything), then class 2 (the open
        // EFFECT tab: 4, 3 — most recent first), then class 3 (1).
        assert_eq!(order, vec![5, 2, 4, 3, 1], "strict class order broke");
        // A tab switch reclassifies instantly: same pending, TRANSITION
        // tab open, nothing visible — the transition doc leads.
        let mut pending = vec![job(1, true), job(3, false)];
        let first = take_next_job(&mut pending, &[], Some(true)).unwrap();
        assert!(first.transition, "open tab must outrank other tabs");
        // No effect tab open: pure recency.
        let mut pending = vec![job(1, true), job(3, false)];
        let first = take_next_job(&mut pending, &[], None).unwrap();
        assert_eq!(first.revision, rev(3));
    }

    /// The black gate refuses empty sheets, and the mp4 layout trailer —
    /// the whole cache-file contract — round-trips through the same
    /// `read_layout` the app calls on the cache bytes. (A synthetic ftyp
    /// box stands in for the container: the stamp is a top-level box
    /// append, codec-independent by design.)
    #[test]
    fn the_black_gate_and_the_mp4_layout_stamp_round_trip() {
        let black = vec![0u8; SHEET_W * SHEET_H * 4];
        assert!(all_black(&black), "black sheets must be refused");
        let mut lit = black.clone();
        lit[4] = 200;
        assert!(!all_black(&lit));

        // A minimal mp4 skeleton: an ftyp box alone is enough for the
        // trailer scan.
        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&16u32.to_be_bytes());
        mp4.extend_from_slice(b"ftypisom");
        mp4.extend_from_slice(&[0u8; 4]);
        let stamped = anim_icon::stamp_layout_mp4(&mp4, sheet_cells(), SHEET_FPS);
        let (cells, fps) = anim_icon::read_layout(&stamped).expect("stamped layout");
        assert_eq!(cells, sheet_cells());
        assert_eq!(fps, SHEET_FPS);
        // Restamping replaces rather than stacks.
        let restamped = anim_icon::stamp_layout_mp4(&stamped, sheet_cells(), SHEET_FPS);
        assert_eq!(restamped.len(), stamped.len());
    }

    /// The startup sweep keeps exactly the current recipe's artifacts.
    #[test]
    fn cache_sweep_drops_everything_but_the_current_recipe() {
        let dir = std::env::temp_dir().join(format!(
            "vjfx-thumbs-sweep-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rev = AssetRevisionId::from_bytes([9; 32]);
        let keep = cache_path(&dir, &rev, false);
        let keep_t = cache_path(&dir, &rev, true);
        std::fs::write(&keep, b"x").unwrap();
        std::fs::write(&keep_t, b"x").unwrap();
        for stale in ["a.png", "b-r1.png", "c-t3-d2-r1.png", "d-r0.mp4", "e.tmp.mp4"] {
            std::fs::write(dir.join(stale), b"x").unwrap();
        }
        sweep_stale_cache(&dir);
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left.len(), 2, "sweep left: {left:?}");
        assert!(keep.exists() && keep_t.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
