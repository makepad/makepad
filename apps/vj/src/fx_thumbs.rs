//! Lazy ANIMATED thumbnails for `vjeffect` assets.
//!
//! The bundled effect library seeds into the store with modest colored
//! placeholder JPEGs (effects/seed.rs). This widget replaces them, tile by
//! tile, with the real thing: it hosts a hidden slot-mode [`VjFxView`]
//! (exactly the hosting contract in effects/CONTRACT.md), runs ONE effect at
//! a time offscreen at thumbnail resolution, captures a bar's worth of
//! frames spread across UI frames, and packs them into the SAME
//! declared-cells sheet format the grid already animates for sprite actors
//! and turntable icons — so the finished sheet re-enters through the
//! existing `DecodeJob::Thumb` lane and the grid needs no new code path.
//!
//! Budget discipline (the VJ may be performing while this runs):
//! - at most ONE effect is loaded/rendering at any moment,
//! - the offscreen pass is thumbnail-sized ([`SLOT_W`]x[`SLOT_H`]), not
//!   program-sized,
//! - at most ONE GPU readback per UI frame, and only on capture ticks
//!   (~6 a second), each measured and reported,
//! - PNG encode + cache write happen on a worker thread, never the UI
//!   thread.
//!
//! Cache: `<cache_parent>/cache-vjfx-thumbs-30/<revision>.png`, keyed by the
//! immutable revision id (the content digest of the published revision), a
//! layout-stamped PNG (`anim_icon::stamp_layout`) so the file describes its
//! own cell grid. A relaunch decodes the file instead of re-rendering.
//! Thin-client law: this is a digest-keyed derived cache, never durable
//! content state.
//!
//! Fallback honesty: a document that fails to parse, a readback the
//! platform cannot do, or a capture that comes back black logs loudly,
//! marks the revision failed for this session, and the tile keeps its
//! seeded placeholder. Nothing on this path unwraps document data — a panic
//! in the frame path kills the whole app on macOS.

use crate::effects::shaders::DrawVjFxPresent;
use crate::effects::VjFxView;
use makepad_asset_data::{AssetId, AssetRevisionId, ThumbnailCells};
use makepad_asset_importer::anim_icon;
use makepad_asset_importer::classic_import::encode_png_rgba;
use makepad_widgets::*;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.VjFxThumbsBase = #(VjFxThumbs::register_widget(vm))
    mod.widgets.VjFxThumbs = set_type_default() do mod.widgets.VjFxThumbsBase{
        width: 4
        height: 4
        fx: mod.widgets.VjFxView{ composite: false }
    }
}

/// One packed cell of the sheet — the grid tile is ~164x104 layout points
/// and thumbnails only ever draw at tile size, so a 128x80 cell (same
/// 16:10, ~1.3x upscale on the tile) is visually indistinguishable there
/// while cutting readback and sheet bytes by a third. 30 cells pack 6x5
/// into a 768x400 PNG.
pub const CELL_W: usize = 128;
pub const CELL_H: usize = 80;
/// Sheet columns.
pub const SHEET_COLS: usize = 6;
pub const FRAME_COUNT: usize = 30;
/// Supersampling: the offscreen pass renders at 4x the cell on each axis
/// and the box downscale below averages the whole 4x4 footprint per output
/// pixel — 16-tap SSAA, which is what keeps thin geometry (L-system twigs,
/// candle flames, chart wicks) from shimmering in a 128x80 tile.
const SUPERSAMPLE: usize = 4;
const SLOT_W: f64 = (CELL_W * SUPERSAMPLE) as f64;
const SLOT_H: f64 = (CELL_H * SUPERSAMPLE) as f64;
/// Seconds of effect time the sheet spans. One second at 30 frames: the
/// operator judged the earlier 6 fps sheet "a bit too low framerate" —
/// half a bar of genuinely smooth motion beats a whole bar of slideshow.
const CAPTURE_SPAN: f64 = 1.0;
const PREROLL_SECS: f64 = 0.9;
/// Draw frames before the clock starts: the pass must have rendered at
/// least once before the first readback can see anything.
const WARMUP_DRAWS: u32 = 3;
/// Playback rate the sheet declares: frames over the span they cover, so
/// the tile replays at the speed the effect actually ran.
pub const SHEET_FPS: f32 = FRAME_COUNT as f32 / CAPTURE_SPAN as f32;

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

/// The cache file for one revision. Transition previews render differently
/// (two-input sweep), so they key their own file — a static sheet cached
/// before the sweep existed is simply ignored, never shown. `-t2`: the
/// first two-input sweep shipped with sheets rendered before input1
/// actually landed on some docs; bumping the key retired every one.
pub fn cache_path(dir: &Path, revision: &AssetRevisionId, transition: bool) -> PathBuf {
    if transition {
        // -t3: the sweep became frame-indexed (the whole transition edge
        // to edge in one sheet); -t2 retired the shader-collision sheets.
        dir.join(format!("{revision}-t3.png"))
    } else {
        dir.join(format!("{revision}.png"))
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

/// Box-filtered ASPECT-FILL downscale of a BGRA readback into one RGBA
/// cell: every output pixel averages its whole source footprint, which is
/// the "render 4x then downscale nicely" anti-aliasing. Cover, not
/// stretch: a source aspect that differs from the cell's crops instead of
/// squashing the picture.
fn cell_from_bgra(src: &[u8], sw: usize, sh: usize) -> Vec<u8> {
    let mut out = vec![0u8; CELL_W * CELL_H * 4];
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        return out;
    }
    // Source window that matches the cell aspect, centred.
    let cell_aspect = CELL_W as f64 / CELL_H as f64;
    let src_aspect = sw as f64 / sh as f64;
    let (win_w, win_h) = if src_aspect > cell_aspect {
        (sh as f64 * cell_aspect, sh as f64)
    } else {
        (sw as f64, sw as f64 / cell_aspect)
    };
    let (off_x, off_y) = ((sw as f64 - win_w) * 0.5, (sh as f64 - win_h) * 0.5);
    for y in 0..CELL_H {
        let y0 = (off_y + y as f64 * win_h / CELL_H as f64) as usize;
        let y1 = ((off_y + (y + 1) as f64 * win_h / CELL_H as f64) as usize)
            .clamp(y0 + 1, sh);
        for x in 0..CELL_W {
            let x0 = (off_x + x as f64 * win_w / CELL_W as f64) as usize;
            let x1 = ((off_x + (x + 1) as f64 * win_w / CELL_W as f64) as usize)
                .clamp(x0 + 1, sw);
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let si = (sy.min(sh - 1) * sw + sx.min(sw - 1)) * 4;
                    // BGRA source.
                    b += src[si] as u32;
                    g += src[si + 1] as u32;
                    r += src[si + 2] as u32;
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            let di = (y * CELL_W + x) * 4;
            // RGBA out, alpha forced opaque (render targets carry whatever
            // alpha the effect blended; the thumbnail is a picture).
            out[di] = (r / count) as u8;
            out[di + 1] = (g / count) as u8;
            out[di + 2] = (b / count) as u8;
            out[di + 3] = 0xff;
        }
    }
    out
}

/// True when every captured cell is essentially black — the honesty gate:
/// a black wall of tiles is worse than the seeded placeholders.
fn all_black(cells: &[Vec<u8>]) -> bool {
    !cells.iter().any(|cell| {
        cell.chunks_exact(4)
            .any(|px| px[0] > 12 || px[1] > 12 || px[2] > 12)
    })
}

/// Pack captured cells into one RGBA sheet (row-major, [`SHEET_COLS`] wide).
fn pack_sheet(cells: &[Vec<u8>]) -> (Vec<u8>, usize, usize) {
    let rows = cells.len().div_ceil(SHEET_COLS).max(1);
    let (w, h) = (SHEET_COLS * CELL_W, rows * CELL_H);
    let mut sheet = vec![0u8; w * h * 4];
    for (i, cell) in cells.iter().enumerate() {
        let (cx0, cy0) = ((i % SHEET_COLS) * CELL_W, (i / SHEET_COLS) * CELL_H);
        for y in 0..CELL_H {
            let src = y * CELL_W * 4;
            let dst = ((cy0 + y) * w + cx0) * 4;
            let n = CELL_W * 4;
            if cell.len() >= src + n && sheet.len() >= dst + n {
                sheet[dst..dst + n].copy_from_slice(&cell[src..src + n]);
            }
        }
    }
    (sheet, w, h)
}

/// Encode + stamp + atomically land the cache file. Runs on a worker.
fn encode_and_write(
    tiles: Vec<Vec<u8>>,
    path: PathBuf,
    revision: AssetRevisionId,
) -> Result<FxThumbSheet, String> {
    let (sheet, w, h) = pack_sheet(&tiles);
    let png = encode_png_rgba(&sheet, w as u32, h as u32)?;
    let png = anim_icon::stamp_layout(&png, sheet_cells(), SHEET_FPS);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cache dir: {e}"))?;
    }
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, &png).map_err(|e| format!("cache write: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cache rename: {e}"))?;
    Ok(FxThumbSheet { revision, path, cells: sheet_cells(), fps: SHEET_FPS })
}

#[derive(PartialEq)]
enum Phase {
    /// Waiting for the pass to have drawn a few times.
    Warmup,
    /// Running the effect and capturing on schedule.
    Capturing,
    /// Worker thread encoding + writing the cache file.
    Encoding,
}

struct ActiveJob {
    job: FxThumbJob,
    phase: Phase,
    draws: u32,
    /// App-clock time the capture window opened (set when warmup ends).
    started: f64,
    next_capture: f64,
    captures: Vec<Vec<u8>>,
    encoder: Option<std::thread::JoinHandle<Result<FxThumbSheet, String>>>,
    // Honest numbers for the report line.
    load_ms: f32,
    capture_ms_max: f32,
    capture_ms_sum: f32,
    draw_ms_max: f32,
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
    /// The hidden slot-mode effect host (CONTRACT.md's thumbnail hook).
    #[live]
    fx: VjFxView,
    /// Samples the effect's output into this widget's 4x4 rect so the
    /// offscreen pass chain is a frame dependency (an unsampled render
    /// target never renders).
    #[live]
    draw_present: DrawVjFxPresent,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    cache_dir: Option<PathBuf>,
    #[rust]
    queue: VecDeque<FxThumbJob>,
    #[rust]
    active: Option<ActiveJob>,
    #[rust]
    results: Vec<FxThumbSheet>,
    /// Revisions that failed to render this session — never retried, their
    /// tiles keep the seeded placeholder.
    #[rust]
    failed: HashMap<AssetRevisionId, String>,
    /// Set once the platform proves it cannot read a render target back
    /// (web/headless); rendering is pointless then, cache decode still works.
    #[rust]
    disabled: Option<String>,
    /// Consecutive whole-job readback failures; one flaky texture must not
    /// switch the feature off, a platform that never answers must.
    #[rust]
    readback_failures: u32,
    #[rust(false)]
    slot_sized: bool,
    /// CPU premix for transition previews (two distinct patterns dissolved
    /// by the sweep) — reused buffer + textures, like the runtime fallback.
    /// Two texture slots: premix docs use slot 0; two-deck (`duo`) docs get
    /// pattern A in slot 0 and pattern B in slot 1.
    #[rust]
    trans_data: Vec<u32>,
    #[rust]
    trans_tex: [Option<Texture>; 2],
}

impl VjFxThumbs {
    pub fn set_cache_dir(&mut self, dir: PathBuf) {
        self.cache_dir = Some(dir);
    }

    pub fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }

    /// Nothing loaded, nothing queued: the app may hand over the next job.
    pub fn is_idle(&self) -> bool {
        self.active.is_none() && self.queue.is_empty()
    }

    pub fn is_failed(&self, revision: &AssetRevisionId) -> bool {
        self.failed.contains_key(revision)
    }

    pub fn disabled_reason(&self) -> Option<&str> {
        self.disabled.as_deref()
    }

    pub fn take_results(&mut self) -> Vec<FxThumbSheet> {
        std::mem::take(&mut self.results)
    }

    pub fn enqueue(&mut self, cx: &mut Cx, job: FxThumbJob) {
        if self.disabled.is_some() || self.failed.contains_key(&job.revision) {
            return;
        }
        let busy = self
            .active
            .as_ref()
            .is_some_and(|a| a.job.revision == job.revision)
            || self.queue.iter().any(|j| j.revision == job.revision);
        if busy {
            return;
        }
        self.queue.push_back(job);
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }

    fn fail_active(&mut self, cx: &mut Cx, error: String) {
        if let Some(active) = self.active.take() {
            log!(
                "fx thumb: {} FAILED — {} (placeholder kept)",
                active.job.title,
                error
            );
            self.failed.insert(active.job.revision, error);
        }
        self.fx.clear_effect(cx);
    }

    /// Load the next queued document into the hidden view.
    fn start_next(&mut self, cx: &mut Cx) {
        while self.active.is_none() {
            let Some(job) = self.queue.pop_front() else { return };
            if !self.slot_sized {
                self.slot_sized = true;
                self.fx.set_slot_size(dvec2(SLOT_W, SLOT_H));
            }
            self.fx.set_bpm(120.0);
            self.fx.set_live(cx, true);
            // A previous transition job's sweep inputs/override must never
            // leak into the next document.
            self.fx.set_input_texture(0, None);
            self.fx.set_input_texture(1, None);
            self.fx.set_user_override([None; 4]);
            let t0 = std::time::Instant::now();
            match self.fx.set_effect_source(cx, "vjfx_thumb", &job.source) {
                Ok(_) => {
                    self.active = Some(ActiveJob {
                        job,
                        phase: Phase::Warmup,
                        draws: 0,
                        started: 0.0,
                        next_capture: 0.0,
                        captures: Vec::new(),
                        encoder: None,
                        load_ms: t0.elapsed().as_secs_f32() * 1000.0,
                        capture_ms_max: 0.0,
                        capture_ms_sum: 0.0,
                        draw_ms_max: 0.0,
                    });
                }
                Err(error) => {
                    log!(
                        "fx thumb: {} document failed to load — {error} (placeholder kept)",
                        job.title
                    );
                    self.failed.insert(job.revision, error);
                }
            }
        }
    }

    /// Harvest the encode worker without ever blocking the UI thread.
    fn poll_encoder(&mut self, _cx: &mut Cx) {
        let finished = self
            .active
            .as_ref()
            .is_some_and(|a| a.phase == Phase::Encoding && a.encoder.as_ref().is_some_and(|h| h.is_finished()));
        if !finished {
            return;
        }
        let Some(mut active) = self.active.take() else { return };
        let Some(handle) = active.encoder.take() else { return };
        match handle.join() {
            Ok(Ok(sheet)) => {
                log!(
                    "fx thumb: {} rendered — {} frames over {:.1}s, load {:.1}ms, capture avg {:.2}ms max {:.2}ms, fx draw max {:.2}ms",
                    active.job.title,
                    FRAME_COUNT,
                    CAPTURE_SPAN,
                    active.load_ms,
                    active.capture_ms_sum / FRAME_COUNT as f32,
                    active.capture_ms_max,
                    active.draw_ms_max,
                );
                self.results.push(sheet);
            }
            Ok(Err(error)) => {
                log!(
                    "fx thumb: {} encode failed — {error} (placeholder kept)",
                    active.job.title
                );
                self.failed.insert(active.job.revision, error);
            }
            Err(_) => {
                self.failed
                    .insert(active.job.revision, "encode worker panicked".to_string());
            }
        }
    }

    /// One capture if it is due. At most one readback per UI frame by
    /// construction (this runs once per draw).
    fn try_capture(&mut self, cx: &mut Cx, now: f64) {
        let due = {
            let Some(active) = self.active.as_ref() else { return };
            active.phase == Phase::Capturing && now >= active.next_capture
        };
        if !due {
            return;
        }
        let Some(texture) = self.fx.output_texture() else { return };
        let t0 = std::time::Instant::now();
        let Some((w, h, bytes)) = cx.debug_read_render_texture(&texture) else {
            self.readback_failures += 1;
            if self.readback_failures >= 3 {
                // Three different jobs in a row could not read back: this
                // platform has no readback (web/headless). Stop trying;
                // cached sheets still decode.
                self.disabled = Some("render-target readback unavailable".to_string());
            }
            self.fail_active(cx, "render-target readback unavailable".to_string());
            return;
        };
        self.readback_failures = 0;
        let cell = cell_from_bgra(&bytes, w, h);
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        let done = {
            let Some(active) = self.active.as_mut() else { return };
            active.capture_ms_max = active.capture_ms_max.max(ms);
            active.capture_ms_sum += ms;
            active.captures.push(cell);
            active.next_capture = now + CAPTURE_SPAN / FRAME_COUNT as f64;
            active.captures.len() >= FRAME_COUNT
        };
        if done {
            self.finish_captures(cx);
        }
    }

    /// All frames in hand: stop the effect, hand the pixels to a worker.
    fn finish_captures(&mut self, cx: &mut Cx) {
        let Some(dir) = self.cache_dir.clone() else {
            self.fail_active(cx, "no cache dir configured".to_string());
            return;
        };
        let tiles = match self.active.as_mut() {
            Some(active) => std::mem::take(&mut active.captures),
            None => return,
        };
        if all_black(&tiles) {
            self.fail_active(cx, "rendered black".to_string());
            return;
        }
        let Some(active) = self.active.as_mut() else { return };
        let revision = active.job.revision;
        let path = cache_path(&dir, &revision, active.job.transition);
        active.phase = Phase::Encoding;
        active.encoder = Some(std::thread::spawn(move || {
            encode_and_write(tiles, path, revision)
        }));
        // The effect can stop ticking now — the pixels are on the worker.
        self.fx.clear_effect(cx);
    }
}

impl VjFxThumbs {
    const TRANS_W: usize = 192;
    const TRANS_H: usize = 120;

    /// The transition preview's input0: TWO visibly different test patterns
    /// — a warm gradient with a drifting disc, and a cool grid with a
    /// counter-drifting bar — dissolved by `m` (the thumbnail's sweeping
    /// crossfade), exactly the slot runtime's premix-as-input0 contract.
    fn transition_input(&mut self, cx: &mut Cx, m: f32, time: f32) -> Texture {
        self.transition_input_into(cx, m, time, 0)
    }

    /// Render the premix at sweep position `m` into texture slot `slot`
    /// (m = 0 → pure pattern A, m = 1 → pure pattern B).
    fn transition_input_into(&mut self, cx: &mut Cx, m: f32, time: f32, slot: usize) -> Texture {
        const W: usize = VjFxThumbs::TRANS_W;
        const H: usize = VjFxThumbs::TRANS_H;
        self.trans_data.resize(W * H, 0);
        let m = m.clamp(0.0, 1.0);
        let (dx, dy) = ((time * 0.7).cos() * 0.25, (time * 0.53).sin() * 0.2);
        let bar = (time * 0.4).fract();
        for y in 0..H {
            let v = y as f32 / H as f32;
            for x in 0..W {
                let u = x as f32 / W as f32;
                // A: warm diagonal gradient + travelling disc.
                let mut ar = 0.85 - 0.5 * v;
                let mut ag = 0.45 + 0.3 * u;
                let mut ab = 0.15 + 0.2 * (1.0 - u);
                let ddx = u - 0.5 - dx;
                let ddy = v - 0.5 - dy;
                if ddx * ddx + ddy * ddy < 0.02 {
                    ar = 1.0;
                    ag = 0.95;
                    ab = 0.7;
                }
                // B: cool grid + counter-drifting vertical bar.
                let gx = x % 24 < 2 || y % 24 < 2;
                let mut br = 0.08;
                let mut bg = 0.25 + 0.35 * v;
                let mut bb = 0.7 + 0.3 * (1.0 - v);
                if gx {
                    br = 0.4;
                    bg = 0.9;
                    bb = 1.0;
                }
                if ((u + 1.0 - bar).fract() - 0.5).abs() < 0.03 {
                    br = 0.9;
                    bg = 1.0;
                    bb = 1.0;
                }
                let r = ar + (br - ar) * m;
                let g = ag + (bg - ag) * m;
                let b = ab + (bb - ab) * m;
                let (r8, g8, b8) = (
                    (r.clamp(0.0, 1.0) * 255.0) as u32,
                    (g.clamp(0.0, 1.0) * 255.0) as u32,
                    (b.clamp(0.0, 1.0) * 255.0) as u32,
                );
                self.trans_data[y * W + x] = 0xff00_0000 | (r8 << 16) | (g8 << 8) | b8;
            }
        }
        match &self.trans_tex[slot.min(1)] {
            Some(tex) => {
                tex.set_data_u32(cx, W, H, self.trans_data.clone());
                tex.clone()
            }
            None => {
                let tex = Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        width: W,
                        height: H,
                        data: Some(self.trans_data.clone()),
                        updated: TextureUpdated::Full,
                    },
                );
                self.trans_tex[slot.min(1)] = Some(tex.clone());
                tex
            }
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
        // The hidden view runs its own NextFrame clock while a document is
        // loaded; forwarding every event is what keeps it ticking.
        self.fx.handle_event(cx, event, scope);
        if self.next_frame.is_event(event).is_some() {
            if self.active.is_some() || !self.queue.is_empty() {
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.poll_encoder(cx.cx);
        self.start_next(cx.cx);

        let now = cx.cx.seconds_since_app_start();
        // Phase bookkeeping BEFORE this frame's pass: the readback sees the
        // pixels the previous draw committed.
        if let Some(active) = self.active.as_mut() {
            if active.phase == Phase::Warmup {
                active.draws += 1;
                if active.draws >= WARMUP_DRAWS {
                    active.phase = Phase::Capturing;
                    active.started = now;
                    active.next_capture = now + PREROLL_SECS;
                }
            }
        }
        self.try_capture(cx.cx, now);

        // The effect pass itself (a no-op walk when no document is loaded).
        let rendering = self
            .active
            .as_ref()
            .is_some_and(|a| a.phase != Phase::Encoding);
        if rendering {
            // Transition previews: the sweeping two-pattern premix lands in
            // input0 and the engage triangle in p3, every frame — the tile
            // then SHOWS one picture becoming another through the effect.
            let sweep = self.active.as_ref().and_then(|active| {
                if !active.job.transition {
                    return None;
                }
                // The sheet holds the COMPLETE transition, edge to edge:
                // the frame that capture k will grab renders p3 exactly at
                // k/(N-1) — first frame pure deck A, last pure deck B —
                // instead of whatever slice of the sweep a wall-clock
                // window happened to sample. (Capture cadence stays timed;
                // the VALUE is index-driven, so jitter cannot bend the
                // sweep.)
                let m = if active.phase == Phase::Capturing {
                    (active.captures.len() as f64 / (FRAME_COUNT - 1) as f64)
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                };
                Some(m as f32)
            });
            if let Some(m) = sweep {
                if self.fx.wants_deck_inputs() {
                    // Two-deck doc: the distinct patterns land on separate
                    // inputs and p3 sweeps as the crossfader would — the
                    // thumbnail replays the transition exactly.
                    let tex_a = self.transition_input_into(cx.cx, 0.0, now as f32, 0);
                    let tex_b = self.transition_input_into(cx.cx, 1.0, now as f32, 1);
                    self.fx.set_input_texture(0, Some(tex_a));
                    self.fx.set_input_texture(1, Some(tex_b));
                    self.fx.set_user_override([None, None, None, Some(m)]);
                } else {
                    // Premix doc: one input carrying the sweeping dissolve,
                    // intensity on the engage triangle.
                    let tex = self.transition_input(cx.cx, m, now as f32);
                    self.fx.set_input_texture(0, Some(tex));
                    let tri = 1.0 - (2.0 * m - 1.0).abs();
                    self.fx.set_user_override([None, None, None, Some(tri)]);
                }
            }
            let t0 = std::time::Instant::now();
            let _ = self.fx.draw_walk(cx, scope, Walk::fill());
            let ms = t0.elapsed().as_secs_f32() * 1000.0;
            if let Some(active) = self.active.as_mut() {
                active.draw_ms_max = active.draw_ms_max.max(ms);
            }
            // Sample the output into our 4x4 rect: the frame dependency
            // that makes the offscreen chain actually render.
            if let Some(texture) = self.fx.output_texture() {
                self.draw_present.draw_vars.set_texture(0, &texture);
                self.draw_present.draw_abs(cx, rect);
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        if self.active.is_some() || !self.queue.is_empty() {
            self.area.redraw(cx.cx);
            self.next_frame = cx.cx.new_next_frame();
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_geometry_declares_exactly_what_pack_sheet_writes() {
        let cells = sheet_cells();
        let tiles: Vec<Vec<u8>> = (0..FRAME_COUNT)
            .map(|i| vec![i as u8; CELL_W * CELL_H * 4])
            .collect();
        let (sheet, w, h) = pack_sheet(&tiles);
        // The declared grid fits the sheet exactly (the same validation the
        // store applies to a published thumbnail's views).
        let last = cells.first + cells.count - 1;
        let right = ((last % cells.cols) + 1) * cells.cell_w;
        let bottom = ((last / cells.cols) + 1) * cells.cell_h;
        assert!(right as usize <= w && bottom as usize <= h, "{right}x{bottom} vs {w}x{h}");
        assert_eq!(sheet.len(), w * h * 4);
        // Cell 5 sits at (1 * CELL_W, 1 * CELL_H): its first pixel carries
        // its tile's fill byte.
        let (x, y) = ((5 % SHEET_COLS) * CELL_W, (5 / SHEET_COLS) * CELL_H);
        assert_eq!(sheet[(y * w + x) * 4], 5);
        // Playback covers the capture span in real time.
        assert!((SHEET_FPS * CAPTURE_SPAN as f32 - FRAME_COUNT as f32).abs() < 1e-6);
    }

    #[test]
    fn cell_resample_is_cover_bgra_to_rgba_and_never_panics_on_junk() {
        // A 2x-dpi readback (320x200) of solid blue-ish BGRA.
        let (sw, sh) = (CELL_W * 2, CELL_H * 2);
        let mut src = vec![0u8; sw * sh * 4];
        for px in src.chunks_exact_mut(4) {
            px.copy_from_slice(&[200, 100, 50, 255]); // B G R A
        }
        let cell = cell_from_bgra(&src, sw, sh);
        assert_eq!(cell.len(), CELL_W * CELL_H * 4);
        assert_eq!(&cell[..4], &[50, 100, 200, 255], "BGRA -> RGBA swizzle");
        // Wildly wrong aspect still fills the whole cell (cover-crop).
        let tall = vec![128u8; 40 * 400 * 4];
        let cell = cell_from_bgra(&tall, 40, 400);
        assert!(cell.chunks_exact(4).all(|px| px[0] == 128 && px[3] == 255));
        // Degenerate inputs return an empty cell rather than panicking.
        let junk = cell_from_bgra(&[1, 2, 3], 999, 999);
        assert_eq!(junk.len(), CELL_W * CELL_H * 4);
        let zero = cell_from_bgra(&[], 0, 0);
        assert_eq!(zero.len(), CELL_W * CELL_H * 4);
    }

    #[test]
    fn the_black_gate_and_the_stamped_layout_round_trip() {
        let black = vec![vec![0u8; CELL_W * CELL_H * 4]; 3];
        assert!(all_black(&black), "black frames must be refused");
        let mut lit = black.clone();
        lit[1][0] = 200;
        assert!(!all_black(&lit));

        // What encode_and_write stamps, decode-side read_layout must
        // recover — this is the whole cache-file contract.
        let (sheet, w, h) = pack_sheet(&lit);
        let png = encode_png_rgba(&sheet, w as u32, h as u32).expect("png");
        let png = anim_icon::stamp_layout(&png, sheet_cells(), SHEET_FPS);
        let (cells, fps) = anim_icon::read_layout(&png).expect("stamped layout");
        assert_eq!(cells, sheet_cells());
        assert_eq!(fps, SHEET_FPS);
    }
}
