//! Lazy ANIMATED thumbnails for `vjeffect` assets.
//!
//! The bundled effect library seeds into the store with modest colored
//! placeholder JPEGs (effects/seed.rs). This widget replaces them, tile by
//! tile, with the real thing: it hosts a small bank of hidden slot-mode
//! [`VjFxView`]s (exactly the hosting contract in effects/CONTRACT.md), runs
//! documents offscreen at thumbnail resolution, and packs a bar's worth of
//! frames into the SAME declared-cells sheet format the grid already
//! animates for sprite actors and turntable icons — so the finished sheet
//! re-enters through the existing `DecodeJob::Thumb` lane and the grid needs
//! no new code path.
//!
//! HOW THE FRAMES ARE MADE (the fast path):
//!   * the effect's clock is HOST-DRIVEN ([`VjFxView::tick_manual`]): frame
//!     k renders at document time `preroll + k/30`, so a sheet is one UI
//!     frame per captured frame instead of one per 33ms of wall clock — and
//!     it comes out identical on a slow machine and a fast one,
//!   * each captured frame is downscaled ON THE GPU straight into its cell
//!     of one 768x400 sheet target (a 4x4 grid of bilinear taps = the exact
//!     16/64-tap box filter the CPU used to run), so the whole sheet costs
//!     ONE readback instead of thirty,
//!   * several documents render side by side ([`LANES`]), each in its own
//!     view with its own passes, so a document's shader compile overlaps the
//!     next one's capture instead of serializing behind it.
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
//! - the work is spread over MANY responsive frames, never gulped: at most
//!   [`LOADS_PER_FRAME`] document loads (splash eval + shader compile) and
//!   [`READBACKS_PER_FRAME`] sheet readbacks per UI frame,
//! - a live set (output window up, a deck playing) drops the bank to a
//!   single lane — see [`VjFxThumbs::set_full_speed`],
//! - the offscreen passes are thumbnail-sized ([`SLOT_W`]x[`SLOT_H`]), not
//!   program-sized,
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
//! marks the revision failed for this session (terminal — see above), and
//! the tile paints the failure over its seeded placeholder. Nothing on this
//! path unwraps document data — a panic in the frame path kills the whole
//! app on macOS.

use crate::effects::shaders::DrawVjFxPresent;
use crate::effects::VjFxView;
use makepad_asset_data::{AssetId, AssetRevisionId, ThumbnailCells};
use makepad_asset_importer::anim_icon;
use makepad_asset_importer::classic_import::encode_png_rgba;
use makepad_widgets::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The GPU downscale: one captured frame lands in one cell of the sheet,
    // box-filtered from the supersampled render. `self.pos` runs 0..1 over
    // the cell and maps 1:1 onto the source, so one output pixel covers
    // exactly `1/CELL_W x 1/CELL_H` of it; a 4x4 grid of BILINEAR taps
    // inside that footprint averages 4x4 source texels at 4x supersample
    // and 8x8 at 8x (each tap sitting on a 2x2 corner) — the same total
    // average the CPU box filter used to compute, on the GPU, at a
    // sixteenth of the readback.
    set_type_default() do #(DrawVjFxThumbCell::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_out: texture_2d(float)

        tap: fn(uv: vec2) -> vec4 {
            return self.tex_out.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        pixel: fn() {
            let px = vec2(1.0 / 128.0, 1.0 / 80.0)
            let step = px * 0.25
            let base = self.pos - px * 0.5 + step * 0.5
            let sum = self.tap(base)
                + self.tap(base + vec2(step.x, 0.0))
                + self.tap(base + vec2(step.x * 2.0, 0.0))
                + self.tap(base + vec2(step.x * 3.0, 0.0))
                + self.tap(base + vec2(0.0, step.y))
                + self.tap(base + vec2(step.x, step.y))
                + self.tap(base + vec2(step.x * 2.0, step.y))
                + self.tap(base + vec2(step.x * 3.0, step.y))
                + self.tap(base + vec2(0.0, step.y * 2.0))
                + self.tap(base + vec2(step.x, step.y * 2.0))
                + self.tap(base + vec2(step.x * 2.0, step.y * 2.0))
                + self.tap(base + vec2(step.x * 3.0, step.y * 2.0))
                + self.tap(base + vec2(0.0, step.y * 3.0))
                + self.tap(base + vec2(step.x, step.y * 3.0))
                + self.tap(base + vec2(step.x * 2.0, step.y * 3.0))
                + self.tap(base + vec2(step.x * 3.0, step.y * 3.0))
            // Alpha forced opaque: render targets carry whatever alpha the
            // effect blended, and the thumbnail is a picture.
            return vec4((sum / 16.0).xyz, 1.0)
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
const SHEET_ROWS: usize = FRAME_COUNT.div_ceil(SHEET_COLS);
/// The sheet target's exact texel size — the GPU packs the cells, so the
/// readback IS the finished sheet.
const SHEET_W: usize = SHEET_COLS * CELL_W;
const SHEET_H: usize = SHEET_ROWS * CELL_H;
/// Supersampling: the offscreen pass renders at 4x the cell on each axis
/// (8x on a retina screen, where the child pass inherits the window's dpi)
/// and the cell shader averages the whole footprint per output pixel —
/// 16/64-tap SSAA, which is what keeps thin geometry (L-system twigs,
/// candle flames, chart wicks) from shimmering in a 128x80 tile.
const SUPERSAMPLE: usize = 4;
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
/// Draw frames before the clock starts: the document's shader compiles
/// after the first draw that asks for it, so the first frames of a fresh
/// document are the compile landing, not the effect.
const WARMUP_DRAWS: u32 = 3;
/// Playback rate the sheet declares: frames over the span they cover, so
/// the tile replays at the speed the effect actually ran.
pub const SHEET_FPS: f32 = FRAME_COUNT as f32 / CAPTURE_SPAN as f32;

/// How many documents render side by side at full speed. Each lane is a
/// whole [`VjFxView`] (its own passes, post chain, sim state), so lanes do
/// not interfere; the cost of one is one more effect's worth of GPU per
/// frame, which is why the pacing guard can hand them back.
pub const LANES: usize = 6;
/// Documents that may START in one UI frame. A load is a splash eval plus
/// (usually) a shader compile — staggering them is what keeps regen from
/// ever reading as a stall.
const LOADS_PER_FRAME: usize = 1;
/// Sheet readbacks per UI frame. A readback blocks the UI thread on the GPU
/// (~3ms), so two is the ceiling and the rest wait a frame.
const READBACKS_PER_FRAME: usize = 2;

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
///
/// `0` means "the original recipe" and adds nothing to the name, so
/// introducing the lever orphaned no existing file.
// 1: screen-engine docs bake against the structured stand-in instead of
//    the fallback blob — every sheet's picture changed.
pub const RECIPE: u32 = 1;

/// The same lever for the TWO-DECK STAND-INS alone
/// ([`crate::effects::deck_pattern`]). A transition sheet is a picture OF
/// those; a generator's sheet has never seen them. Splitting the levers is
/// what lets calming the stand-ins re-bake seventy transition tiles without
/// throwing away a hundred and ninety perfectly good ones.
///
/// `1` = the quiet slates (the first pass' full-saturation primaries blew
/// out the rest of the UI).
pub const DECK_RECIPE: u32 = 1;

/// The cache file for one revision. Transition previews render differently
/// (two-input sweep), so they key their own file — a static sheet cached
/// before the sweep existed is simply ignored, never shown. `-t2`: the
/// first two-input sweep shipped with sheets rendered before input1
/// actually landed on some docs; bumping the key retired every one.
pub fn cache_path(dir: &Path, revision: &AssetRevisionId, transition: bool) -> PathBuf {
    let recipe = if RECIPE == 0 { String::new() } else { format!("-r{RECIPE}") };
    if transition {
        // -t3: the sweep became frame-indexed (the whole transition edge
        // to edge in one sheet); -t2 retired the shader-collision sheets.
        let deck = if DECK_RECIPE == 0 { String::new() } else { format!("-d{DECK_RECIPE}") };
        dir.join(format!("{revision}-t3{deck}{recipe}.png"))
    } else {
        dir.join(format!("{revision}{recipe}.png"))
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
/// RGBA sheet the PNG wants. The GPU already packed the cells; all that is
/// left is the swizzle, opaque alpha, and — only if a backend ignored the
/// dpi override and handed back a scaled target — a box resample back onto
/// the declared grid, so a stamped sheet always describes its own pixels.
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
    let png = encode_png_rgba(&sheet, SHEET_W as u32, SHEET_H as u32)?;
    let png = anim_icon::stamp_layout(&png, sheet_cells(), SHEET_FPS);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cache dir: {e}"))?;
    }
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, &png).map_err(|e| format!("cache write: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cache rename: {e}"))?;
    Ok(FxThumbSheet { revision, path, cells: sheet_cells(), fps: SHEET_FPS })
}

/// Where a lane's job is in its life. The clock only ever moves in
/// [`VjFxView::tick_manual`] steps, so these are frame counts, not seconds.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    /// Drawing so the document's shader compiles; the clock stands still.
    Warmup(u32),
    /// Running the clock up to [`PREROLL_SECS`], in `left` steps of `dt`.
    Preroll { left: usize, dt: f64 },
    /// Rendering captured frame `k` into cell `k` of the sheet.
    Capture(usize),
    /// All thirty cells drawn; the readback waits one frame for the paint
    /// that drew the last of them.
    Readback,
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
    // Honest numbers for the report line.
    started: Instant,
    frames: u32,
    load_ms: f32,
    read_ms: f32,
}

/// One lane's own render target: the sheet the GPU packs cell by cell, plus
/// the pass that owns it. The effect's own passes are begun INSIDE this one
/// (`make_child_pass`), which is what guarantees the cell blit reads the
/// frame that was just rendered rather than the previous one — child passes
/// are ordered before their parent.
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
    /// CPU premix for transition previews (two distinct patterns dissolved
    /// by the sweep) — per lane, since lanes sit on different frames.
    /// Premix docs use slot 0; two-deck (`duo`) docs get pattern A in slot
    /// 0 and pattern B in slot 1.
    trans_tex: [Option<Texture>; 2],
    prepared: bool,
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
    /// bank at one lane.
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
                view.set_slot_size(dvec2(SLOT_W, SLOT_H));
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
                        started: t0,
                        frames: 0,
                        load_ms: t0.elapsed().as_secs_f32() * 1000.0,
                        read_ms: 0.0,
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
                        "fx thumb: {} rendered — {} frames over {:.1}s in {:.0}ms ({} draws, load {:.1}ms, readback {:.1}ms); {} done, avg {:.0}ms, {:.1}s elapsed",
                        active.job.title,
                        FRAME_COUNT,
                        CAPTURE_SPAN,
                        total_ms,
                        active.frames,
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

    /// One lane's frame: advance its clock by exactly the step this frame
    /// stands for, render the effect INSIDE the lane's sheet pass, and (on
    /// a capture frame) blit it into its cell. Returns true when the sheet
    /// is complete and waiting for its readback.
    fn draw_lane(cx: &mut Cx2d, view: &mut VjFxView, lane: &mut Lane, cell: &mut DrawVjFxThumbCell, scratch: &mut Vec<u32>) -> bool {
        let Some(active) = lane.job.as_mut() else { return false };
        let phase = active.phase;
        match phase {
            Phase::Readback => return true,
            Phase::Encoding => return false,
            _ => {}
        }
        // What this frame stands for.
        let (dt, capture) = match phase {
            Phase::Warmup(_) => (0.0, None),
            Phase::Preroll { dt, .. } => (dt, None),
            Phase::Capture(k) => (FRAME_STEP, Some(k)),
            _ => (0.0, None),
        };
        let clear = !matches!(phase, Phase::Capture(k) if k > 0);
        view.tick_manual(cx.cx, dt);
        active.frames += 1;

        // Transition previews: the sweeping two-pattern premix lands in
        // input0 and the engage triangle in p3 — the tile then SHOWS one
        // picture becoming another through the effect. The sheet holds the
        // COMPLETE transition, edge to edge: the frame captured into cell k
        // renders p3 exactly at k/(N-1), first frame pure deck A, last pure
        // deck B.
        if active.job.transition {
            let k = capture.unwrap_or(0);
            let m = (k as f32 / (FRAME_COUNT - 1) as f32).clamp(0.0, 1.0);
            // Document time, not wall time: the patterns drift the same way
            // every run, so a sheet is reproducible.
            let time = (PREROLL_SECS + k as f64 * FRAME_STEP) as f32;
            if view.wants_deck_inputs() {
                // Two-deck doc: the distinct patterns land on separate
                // inputs and p3 sweeps as the crossfader would — the
                // thumbnail replays the transition exactly.
                let tex_a = transition_input(cx.cx, scratch, &mut lane.trans_tex[0], 0.0, time);
                let tex_b = transition_input(cx.cx, scratch, &mut lane.trans_tex[1], 1.0, time);
                view.set_input_texture(0, Some(tex_a));
                view.set_input_texture(1, Some(tex_b));
                view.set_user_override([None, None, None, Some(m)]);
            } else {
                // Premix doc: one input carrying the sweeping dissolve,
                // intensity on the engage triangle.
                let tex = transition_input(cx.cx, scratch, &mut lane.trans_tex[0], m, time);
                view.set_input_texture(0, Some(tex));
                let tri = 1.0 - (2.0 * m - 1.0).abs();
                view.set_user_override([None, None, None, Some(tri)]);
            }
        } else if view.is_screen_engine() {
            // A screen doc is a remap OF ITS CONTENT: baked against the
            // engine's featureless fallback blob, a mirror looks blank and
            // a tile looks like mud. Give it the structured stand-in (a
            // fixed A/B blend so both the disc and the grid features are in
            // frame) — the warp itself becomes the picture.
            let k = capture.unwrap_or(0);
            let time = (PREROLL_SECS + k as f64 * FRAME_STEP) as f32;
            let tex = transition_input(cx.cx, scratch, &mut lane.trans_tex[0], 0.35, time);
            view.set_input_texture(0, Some(tex));
        }

        // The sheet pass. The effect's own passes are begun inside it, so
        // they are its CHILDREN and therefore execute first — the cell blit
        // below reads the frame this very draw produced.
        let sheet = lane
            .sheet
            .get_or_insert_with(|| SheetTarget::new(cx.cx, "vjfx_thumb_sheet"));
        let size = dvec2(SHEET_W as f64, SHEET_H as f64);
        sheet.pass.set_size(cx, size);
        sheet.pass.set_color_texture(
            cx.cx,
            &sheet.texture,
            if clear {
                DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0))
            } else {
                // Load: the cells already drawn stay drawn.
                DrawPassClearColor::InitWith(vec4(0.0, 0.0, 0.0, 1.0))
            },
        );
        cx.make_child_pass(&sheet.pass);
        // dpi 1.0: the sheet target is exact texels, so the readback IS the
        // PNG (the effect's own pass keeps the window's dpi — it delegates
        // up to the window, which is where the extra supersampling on a
        // retina screen comes from).
        cx.begin_pass(&sheet.pass, Some(1.0));
        sheet.pass.set_size(cx, size);
        sheet.draw_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        let _ = view.draw_walk(cx, &mut Scope::empty(), Walk::fill());
        if let (Some(k), Some(texture)) = (capture, view.output_texture()) {
            cell.draw_vars.set_texture(0, &texture);
            cell.draw_abs(cx, cell_rect(k));
        }
        cx.end_pass_sized_turtle();
        sheet.draw_list.end(cx);
        cx.end_pass(&sheet.pass);

        // Advance the phase for the next frame.
        let stepped = view.needs_stepped_time();
        let Some(active) = lane.job.as_mut() else { return false };
        active.phase = match phase {
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
        };
        false
    }

    /// The sheet is drawn: read it back once and hand the pixels to a
    /// worker. This is the only GPU->CPU transfer a thumbnail costs.
    fn finish_lane(&mut self, cx: &mut Cx, index: usize) {
        let Some(dir) = self.cache_dir.clone() else {
            self.fail_lane(cx, index, "no cache dir configured".to_string());
            return;
        };
        let Some(texture) = self.lanes[index].sheet.as_ref().map(|s| s.texture.clone()) else {
            self.fail_lane(cx, index, "no sheet target".to_string());
            return;
        };
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
                Some(Phase::Readback) => 'r',
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
        self.ensure_lanes();
        self.poll_encoders();
        self.start_jobs(cx.cx);
        self.debug_bank();

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
            if Self::draw_lane(cx, view, lane, &mut self.draw_cell, &mut self.trans_data) {
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

    #[test]
    fn the_black_gate_and_the_stamped_layout_round_trip() {
        let black = vec![0u8; SHEET_W * SHEET_H * 4];
        assert!(all_black(&black), "black sheets must be refused");
        let mut lit = black.clone();
        lit[4] = 200;
        assert!(!all_black(&lit));

        // What encode_and_write stamps, decode-side read_layout must
        // recover — this is the whole cache-file contract.
        let png = encode_png_rgba(&lit, SHEET_W as u32, SHEET_H as u32).expect("png");
        let png = anim_icon::stamp_layout(&png, sheet_cells(), SHEET_FPS);
        let (cells, fps) = anim_icon::read_layout(&png).expect("stamped layout");
        assert_eq!(cells, sheet_cells());
        assert_eq!(fps, SHEET_FPS);
    }
}
