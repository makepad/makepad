//! The TileGrid widget: a camera over a plane of image tiles.
//!
//! Every picture is one instance of one draw shader, batched per
//! (shard, level) atlas page — thousands of pictures are a handful of draw
//! calls. Pan and zoom are uniforms: while the camera glides inside the pad
//! of the last build, retained draw calls are re-presented with fresh
//! uniforms and the instance buffer is never re-uploaded. Atlas pages carry
//! continuous per-shard LOD with crossfades; a tile grown past its slot is
//! promoted to its own full-resolution mip-chain texture under byte budgets
//! with LRU eviction.
//!
//! IMPORTANT for hosts: wrap the grid in a `View` and give that view
//! `ViewOptimize::DrawList` once at startup —
//! ```ignore
//! if let Some(mut wrap) = self.ui.view(cx, ids!(grid_wrap)).borrow_mut() {
//!     wrap.set_optimize(cx, ViewOptimize::DrawList);
//! }
//! ```
//! — so a sibling widget's redraw cannot re-run this widget's draw and
//! re-upload a six-figure instance buffer. The uniform-only glide path
//! depends on the grid owning its draw list.

use crate::db::{self, ItemRow};
use crate::library::{ItemId, Library};
use crate::pack::{pack, Packing};
use crate::store::{self, StoreEvent, StoreHandle};
use crate::tape::{fit_dims, page_size, Planes, FullFrame, GRID, LEVELS, PYRAMID_LEVELS, SLOT};
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawTile::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_y: texture_2d(float)
        tex_uv: texture_2d(float)
        cam_pos: uniform(vec2(0.0, 0.0))
        cam_scale: uniform(100.0)
        view_center: uniform(vec2(0.0, 0.0))
        tile_pos: vec2(0.0, 0.0)
        tile_size: vec2(1.0, 1.0)
        // 1: tex_y is a BGRA mip chain (a full-resolution picture).
        rgba: 0.0
        uv0: vec2(0.0, 0.0)
        uv1: vec2(1.0, 1.0)
        fade: 1.0
        alpha_v: varying(float)

        vertex: fn() {
            // Geometry anti-aliasing, analytically: a quad is never allowed
            // to rasterize below one point. Zoomed all the way out a tile is
            // a fraction of a pixel, and thousands of hard-edged grains
            // beating against the pixel grid is moiré banding. The on-screen
            // size is clamped to a point per axis and the shrink is paid
            // back as alpha: exact area coverage, at no framebuffer cost.
            let want = self.tile_size * self.cam_scale
            let eff = max(want, vec2(1.0, 1.0))
            let cover = (want.x * want.y) / max(eff.x * eff.y, 0.000001)
            let size2 = eff / max(self.cam_scale, 0.000001)
            let world_xy = self.tile_pos + (self.tile_size - size2) * 0.5 + self.geom.pos * size2
            let scr = self.view_center + (world_xy - self.cam_pos) * self.cam_scale
            self.alpha_v = self.fade * cover
            self.pos = self.geom.pos
            self.world = self.draw_list.view_transform * vec4(scr.x, scr.y, self.draw_depth + self.draw_call.zbias, 1.)
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        pixel: fn() {
            let uv = self.uv0 + clamp(self.pos, vec2(0.0, 0.0), vec2(1.0, 1.0)) * (self.uv1 - self.uv0)
            // BT.709 limited range, the same math the tape encoder used.
            let yy = self.tex_y.sample(uv).x
            let cc = self.tex_uv.sample(uv).xy
            let c = (yy - 0.0627) * 1.1644
            let d = cc.x - 0.5
            let e = cc.y - 0.5
            let rgb = clamp(
                vec3(c + 1.7927 * e, c - 0.2132 * d - 0.5329 * e, c + 2.1124 * d),
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 1.0, 1.0)
            )
            let direct = self.tex_y.sample_as_bgra(uv)
            let lit = rgb.mix(direct.xyz, self.rgba)
            return vec4(lit * self.alpha_v, self.alpha_v)
        }
    }

    mod.widgets.TileGridBase = #(TileGrid::register_widget(vm))
    mod.widgets.TileGrid = set_type_default() do mod.widgets.TileGridBase{
        width: Fill
        height: Fill
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTile {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    tile_pos: Vec2f,
    #[live]
    tile_size: Vec2f,
    #[live]
    rgba: f32,
    #[live]
    uv0: Vec2f,
    #[live]
    uv1: Vec2f,
    #[live]
    fade: f32,
}

/// World-unit gap inside a cell: a tile fills at most this fraction.
const CELL_FILL: f32 = 0.92;
/// LOD / full-frame crossfade time.
const FADE_SECS: f64 = 0.35;
/// Resident atlas page budget (bytes of NV12).
const VRAM_BUDGET: usize = 512 * 1024 * 1024;
/// Resident full-resolution frames (bytes of BGRA + mips).
const FULL_BUDGET: usize = 512 * 1024 * 1024;
/// Above this on-screen tile size in device pixels the 128 px slot would be
/// magnified, so a frame from the picture's own pyramid is fetched. A tile
/// has to be worth several slots before it is worth a decode and megabytes
/// of texture.
const FULL_RES_PX: f64 = 320.0;
/// How long a decode that came back broken is left alone.
const RETRY_EMBARGO_SECS: f64 = 15.0;
/// Below this many tiles a rebuild is cheap enough that the uniform-only
/// glide path is not worth its bookkeeping.
const UNIFORM_ONLY_MIN_TILES: usize = 2_000;
/// A resize is one relayout, not a hundred: wait for the drag to rest.
const RESIZE_SETTLE_SECS: f64 = 0.35;

/// Seconds a picture takes to fly to its new place, shrink away, or grow
/// back when the query changes.
const TWEEN_SECS: f64 = 0.45;

/// easeOutQuint: fast out of the gate, settling softly — the motion the
/// desk's panes use, so the wall moves the way the rest of the desk does.
pub fn ease_out_quint(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u * u * u
}

fn lerp2(a: Vec2f, b: Vec2f, t: f32) -> Vec2f {
    vec2f(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// The display order for a query over `(title, link)` pairs in rank order:
/// with no words, every rank as it is; otherwise only the ranks whose
/// title or link holds EVERY word (case-insensitive), best first — a word
/// found in the title beats one found only in the link, a whole-word hit
/// beats a substring, and ties keep library order. Pure, so the wall's
/// order is testable without a grid.
pub fn order_for(items: &[(String, String)], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return (0..items.len()).collect();
    }
    let mut scored: Vec<(i32, usize)> = Vec::new();
    for (rank, (title, link)) in items.iter().enumerate() {
        let title = title.to_lowercase();
        let link = link.to_lowercase();
        let mut score = 0i32;
        let mut all = true;
        for w in &words {
            let in_title = title.contains(w.as_str());
            let in_link = link.contains(w.as_str());
            if !in_title && !in_link {
                all = false;
                break;
            }
            score += if in_title { 2 } else { 1 };
            if title.split(|c: char| !c.is_alphanumeric()).any(|t| t == w.as_str()) {
                score += 2;
            }
        }
        if all {
            scored.push((score, rank));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, rank)| rank).collect()
}


fn vec2f(x: f32, y: f32) -> Vec2f {
    Vec2f { x, y }
}

/// The grid a set of pictures hangs on: cut once for a view aspect and an
/// item count, then held while neither moves materially.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct GridPlan {
    pub cols: usize,
    pub rows: usize,
    pub origin: Vec2f,
    pub aspect: f32,
}

/// The grid for `count` pictures in a view of this aspect, centred on the
/// world origin.
pub fn grid_plan(count: usize, aspect: f32) -> GridPlan {
    let aspect = if aspect.is_finite() && aspect > 0.05 { aspect.clamp(0.35, 3.0) } else { 1.6 };
    let reserve = count.max(1);
    let cols = ((reserve as f32 * aspect).sqrt().ceil() as usize).max(1);
    let rows = reserve.div_ceil(cols).max(1);
    GridPlan { cols, rows, origin: vec2f(-(cols as f32) * 0.5, -(rows as f32) * 0.5), aspect }
}

/// Where rank `rank` hangs on this grid, its aspect-fit size centred in the
/// unit cell.
pub fn grid_slot(plan: &GridPlan, rank: usize, size: Vec2f) -> Vec2f {
    let cols = plan.cols.max(1);
    let col = (rank % cols) as f32;
    let row = (rank / cols) as f32;
    vec2f(plan.origin.x + col + (1.0 - size.x) * 0.5, plan.origin.y + row + (1.0 - size.y) * 0.5)
}

/// How the pictures hang: on unit cells, or packed edge to edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WallLayout {
    /// One picture per unit cell, aspect-fit inside it: even, gappy.
    Uniform,
    /// Justified rows (`pack.rs`): every picture at its own proportions,
    /// rows scaled to span the wall, no gaps — the default.
    Packed,
}

/// The packed wall's shape for `count` pictures whose aspects average
/// `mean_aspect`: wide enough that the whole wall has roughly the view's
/// aspect when its rows are one world unit tall. Rows are cut by the
/// packer; this only picks the width they are cut against.
pub fn packed_width(count: usize, mean_aspect: f32, view_aspect: f32) -> f32 {
    let n = count.max(1) as f32;
    let a = if mean_aspect.is_finite() && mean_aspect > 0.0 { mean_aspect.clamp(0.05, 20.0) } else { 1.0 };
    let v = if view_aspect.is_finite() && view_aspect > 0.05 { view_aspect.clamp(0.35, 3.0) } else { 1.6 };
    // Area ≈ n·a (each picture is a × 1 at the target height); a wall of
    // aspect v with that area is √(n·a·v) wide. Never narrower than the
    // widest sensible picture.
    (n * a * v).sqrt().max(a.min(4.0)).max(1.0)
}

/// A picture's world size inside its unit cell, keeping its own proportions.
pub fn cell_size(aspect: f32) -> Vec2f {
    let a = if aspect.is_finite() && aspect > 0.0 { aspect.clamp(0.05, 20.0) } else { 1.0 };
    if a >= 1.0 {
        vec2f(CELL_FILL, CELL_FILL / a)
    } else {
        vec2f(CELL_FILL * a, CELL_FILL)
    }
}

struct GridItem {
    id: ItemId,
    shard: i64,
    slot: u32,
    /// Fraction of the atlas slot the picture covers.
    uv1: Vec2f,
    title: Arc<str>,
    link: Arc<str>,
    url: Arc<str>,
    pos: Vec2f,
    size: Vec2f,
    aspect: f32,
    /// The place the picture is flying to (its slot in the current layout,
    /// or a point of no size when the query dropped it) and where it flew
    /// from; `pos`/`size`/`alpha` are what is drawn right now.
    to_pos: Vec2f,
    to_size: Vec2f,
    from_pos: Vec2f,
    from_size: Vec2f,
    alpha: f32,
    from_alpha: f32,
    to_alpha: f32,
}

struct PageTex {
    y: Texture,
    uv: Texture,
    arrived: f64,
    last_used: u64,
    bytes: usize,
}

struct FullTex {
    tex: Texture,
    arrived: f64,
    last_used: u64,
    bytes: usize,
    /// The long side this was decoded at, and whether anything finer exists.
    px: u32,
    finest: bool,
}

#[derive(Default)]
struct ShardView {
    /// The last level drawn fully opaque, so re-entering a shard does not
    /// restart a fade.
    shown: Option<usize>,
}

struct Pass {
    y: Texture,
    uv: Texture,
    fade: f32,
    tiles: Vec<(usize, Vec2f, Vec2f)>,
}

type PageKey = (i64, usize);

/// True while `key` rests after a failed decode; a rest that is over is
/// cleared as it is passed.
fn embargoed<K: std::hash::Hash + Eq + Copy>(failed: &mut HashMap<K, f64>, key: K, now: f64) -> bool {
    match failed.get(&key) {
        Some(&until) if now < until => true,
        Some(_) => {
            failed.remove(&key);
            false
        }
        None => false,
    }
}

#[derive(Clone, Debug, Default)]
pub enum TileGridAction {
    #[default]
    None,
    /// A picture was clicked: its item id and metadata from the library.
    Clicked { item: ItemId, title: String, link: String, url: String },
    /// The library opened (or failed to). Count is the pictures on the grid.
    Opened { count: usize, error: Option<String> },
}

#[derive(Script, ScriptHook, Widget)]
pub struct TileGrid {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_tile: DrawTile,
    /// Library root to open at startup; empty resolves the default
    /// (`IMAGE_TILES_HOME`, else the nearest `local/image-tiles`).
    #[live]
    library: String,

    #[rust]
    area: Area,
    #[rust]
    store: Option<StoreHandle>,
    #[rust]
    opened: bool,
    #[rust]
    items: Vec<GridItem>,
    #[rust]
    plan: Option<GridPlan>,
    /// Packed edge to edge (the default) or one picture per unit cell.
    #[live(true)]
    packed: bool,
    /// The packing behind the packed layout: the rows, for hit-testing.
    #[rust]
    packing: Option<Packing>,
    /// The wall's world extent, whichever layout: (min, max).
    #[rust]
    bounds: Option<(Vec2f, Vec2f)>,
    /// The words the wall is filtered by (empty: every picture).
    #[rust]
    query: String,
    /// The pictures on the wall right now, in display order (indices into
    /// `items`); the packing is cut over exactly these.
    #[rust]
    visible: Vec<usize>,
    /// A layout change in flight: when it started (Cx seconds); the items
    /// carry their from/to rects.
    #[rust]
    tween_start: Option<f64>,
    #[rust]
    start: Option<Instant>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: f64,
    #[rust]
    frame: u64,
    #[rust]
    view_rect: Rect,

    // ── camera ──
    #[rust]
    cam_pos: Vec2d,
    #[rust(1.0)]
    cam_scale: f64,
    #[rust]
    cam_pos_t: Vec2d,
    #[rust(1.0)]
    cam_scale_t: f64,
    #[rust]
    cam_ready: bool,
    #[rust(0.05)]
    min_scale: f64,
    #[rust]
    zoom_anchor: Option<(Vec2d, Vec2d)>,
    #[rust]
    drag: Option<(Vec2d, Vec2d, bool)>,
    #[rust]
    user_moved: bool,
    #[rust]
    resize_settle_at: Option<f64>,

    // ── uniform-only glide bookkeeping ──
    #[rust]
    pushed_cam: Option<(Vec2d, f64)>,
    #[rust]
    pushed_all: bool,
    #[rust]
    cull_frac: f32,
    #[rust]
    beat_now: bool,
    #[rust]
    last_beat: f64,

    // ── resident textures ──
    #[rust]
    pages: HashMap<PageKey, PageTex>,
    #[rust]
    shard_views: HashMap<i64, ShardView>,
    #[rust]
    full: HashMap<ItemId, FullTex>,
    #[rust]
    requested: HashSet<PageKey>,
    #[rust]
    full_requested: HashSet<ItemId>,
    #[rust]
    page_failed: HashMap<PageKey, f64>,
    #[rust]
    full_failed: HashMap<ItemId, f64>,
}

impl TileGrid {
    fn time(&mut self) -> f64 {
        let start = *self.start.get_or_insert_with(Instant::now);
        start.elapsed().as_secs_f64()
    }

    /// Open a baked library: read the index, spawn the decode pool, lay the
    /// grid out and ask for every shard's coarsest page so the whole set
    /// shows the moment the first decodes land.
    pub fn open(&mut self, cx: &mut Cx, library: Library) {
        if let Some(store) = self.store.take() {
            store.shutdown();
        }
        self.items.clear();
        self.pages.clear();
        self.full.clear();
        self.shard_views.clear();
        self.requested.clear();
        self.full_requested.clear();
        self.page_failed.clear();
        self.full_failed.clear();
        self.plan = None;
        self.packing = None;
        self.bounds = None;
        self.query.clear();
        self.visible.clear();
        self.tween_start = None;
        self.opened = true;
        let uid = self.widget_uid();
        let (rows, shards) = match db::read_items(&library.db_path()) {
            Ok(v) => v,
            Err(e) => {
                log!("image-tiles: {e}");
                cx.widget_action(uid, TileGridAction::Opened { count: 0, error: Some(e) });
                return;
            }
        };
        self.items = rows.iter().filter_map(item_of).collect();
        self.visible = (0..self.items.len()).collect();
        let store = store::spawn(library, &cx.thread_spawner());
        for shard in shards.iter().filter(|s| s.sealed) {
            store.need_page(shard.id, LEVELS - 1, 1);
            self.requested.insert((shard.id, LEVELS - 1));
        }
        self.store = Some(store);
        self.relayout();
        self.fit_camera();
        self.cam_pos = self.cam_pos_t;
        self.cam_scale = self.cam_scale_t;
        cx.widget_action(uid, TileGridAction::Opened { count: self.items.len(), error: None });
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }

    /// Every picture on the grid: id, title, link — what a host searches
    /// over (the SMBC hover text rides in the title).
    pub fn items(&self) -> Vec<(ItemId, String, String)> {
        self.items.iter().map(|i| (i.id, i.title.to_string(), i.link.to_string())).collect()
    }

    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// The words the wall is filtered by (empty: everything shows).
    pub fn query(&self) -> &str {
        &self.query
    }

    /// How many pictures the wall shows under the current query.
    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    /// Filter and reorder the wall as the person types: the matches
    /// (best first) are packed afresh and every picture flies from where
    /// it is to where it goes; the others shrink away, and an empty query
    /// brings them all flying back. The camera re-frames the new wall.
    pub fn set_query(&mut self, cx: &mut Cx, query: &str) {
        let query = query.trim();
        if query == self.query {
            return;
        }
        self.query = query.to_string();
        let words: Vec<(String, String)> = self.items.iter().map(|i| (i.title.to_string(), i.link.to_string())).collect();
        self.visible = order_for(&words, &self.query);
        let now = cx.seconds_since_app_start();
        self.layout_targets();
        if self.view_rect.size.x >= 1.0 && self.cam_usable() {
            self.begin_tween(now);
            // The new wall has a new shape: glide the camera to frame it.
            self.user_moved = false;
            self.fit_camera();
        } else {
            self.snap();
        }
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }

    /// Every picture jumps to its target: the first layout, a resize.
    fn snap(&mut self) {
        for item in &mut self.items {
            item.pos = item.to_pos;
            item.size = item.to_size;
            item.alpha = item.to_alpha;
            item.from_pos = item.to_pos;
            item.from_size = item.to_size;
            item.from_alpha = item.to_alpha;
        }
        self.tween_start = None;
    }

    /// Start flying every picture from where it is drawn now to its target.
    fn begin_tween(&mut self, now: f64) {
        for item in &mut self.items {
            item.from_pos = item.pos;
            item.from_size = item.size;
            item.from_alpha = item.alpha;
        }
        self.tween_start = Some(now);
    }

    /// Advance the flight to `now`; true while pictures still move.
    fn step_tween(&mut self, now: f64) -> bool {
        let Some(start) = self.tween_start else { return false };
        let t = ((now - start) / TWEEN_SECS).clamp(0.0, 1.0) as f32;
        let e = ease_out_quint(t);
        for item in &mut self.items {
            item.pos = lerp2(item.from_pos, item.to_pos, e);
            item.size = lerp2(item.from_size, item.to_size, e);
            item.alpha = item.from_alpha + (item.to_alpha - item.from_alpha) * e;
        }
        if t >= 1.0 {
            self.snap();
            return false;
        }
        true
    }

    /// Glide the camera onto one picture so it fills most of the view.
    /// False when the id is not on the grid.
    pub fn show_item(&mut self, cx: &mut Cx, item: ItemId) -> bool {
        let Some(found) = self.items.iter().find(|i| i.id == item) else {
            return false;
        };
        let (pos, size) = (found.to_pos, found.to_size);
        if self.view_rect.size.x < 1.0 || self.view_rect.size.y < 1.0 {
            return false;
        }
        let fit_x = self.view_rect.size.x * 0.7 / size.x.max(0.01) as f64;
        let fit_y = self.view_rect.size.y * 0.7 / size.y.max(0.01) as f64;
        self.zoom_anchor = None;
        self.user_moved = true;
        self.cam_scale_t = fit_x.min(fit_y).clamp(self.min_scale, 6000.0);
        self.cam_pos_t = Vec2d { x: (pos.x + size.x * 0.5) as f64, y: (pos.y + size.y * 0.5) as f64 };
        if !self.cam_ready {
            self.cam_pos = self.cam_pos_t;
            self.cam_scale = self.cam_scale_t;
            self.cam_ready = true;
        }
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
        true
    }

    fn ensure_open(&mut self, cx: &mut Cx) {
        if self.opened {
            return;
        }
        let library =
            if self.library.is_empty() { Library::resolve() } else { Library::new(self.library.clone()) };
        self.open(cx, library);
    }

    fn view_aspect(&self) -> f32 {
        if self.view_rect.size.y > 1.0 {
            (self.view_rect.size.x / self.view_rect.size.y) as f32
        } else {
            1.6
        }
    }

    /// Which layout this grid hangs its pictures on.
    pub fn wall_layout(&self) -> WallLayout {
        if self.packed {
            WallLayout::Packed
        } else {
            WallLayout::Uniform
        }
    }

    /// Lay the wall out and put every picture on its slot at once: the
    /// first layout, a resize.
    fn relayout(&mut self) {
        self.layout_targets();
        self.snap();
    }

    /// Cut the layout and write every picture's TARGET: the pictures on
    /// the wall get their slot; the ones the query dropped shrink to a
    /// point at their own centre and fade out, so leaving reads as
    /// vanishing where they stood and coming back as growing from there.
    fn layout_targets(&mut self) {
        match self.wall_layout() {
            WallLayout::Uniform => {
                // The uniform grid hangs every picture, query or not.
                let plan = grid_plan(self.items.len(), self.view_aspect());
                for (rank, item) in self.items.iter_mut().enumerate() {
                    item.to_size = cell_size(item.aspect);
                    item.to_pos = grid_slot(&plan, rank, item.to_size);
                    item.to_alpha = 1.0;
                }
                let cols = plan.cols.max(1) as f32;
                let rows = plan.rows.max(1) as f32;
                self.bounds = Some((plan.origin, vec2f(plan.origin.x + cols, plan.origin.y + rows)));
                self.plan = Some(plan);
                self.packing = None;
            }
            WallLayout::Packed => {
                // Rows one world unit tall — the unit the slot pyramid and
                // the LOD maths already think in — cut against a width that
                // gives the whole wall roughly the view's shape, then centred
                // on the world origin like the uniform grid. Only the
                // pictures the query keeps are packed, in display order.
                let aspects: Vec<f32> = self.visible.iter().map(|&i| self.items[i].aspect).collect();
                let mean = if aspects.is_empty() {
                    1.0
                } else {
                    aspects.iter().map(|a| a.clamp(0.05, 20.0)).sum::<f32>() / aspects.len() as f32
                };
                let width = packed_width(aspects.len(), mean, self.view_aspect());
                let packing = pack(&aspects, width, 1.0);
                let origin = vec2f(-packing.width * 0.5, -packing.height * 0.5);
                for item in &mut self.items {
                    // Dropped by the query: a point at its own centre.
                    let centre = vec2f(item.pos.x + item.size.x * 0.5, item.pos.y + item.size.y * 0.5);
                    item.to_pos = centre;
                    item.to_size = vec2f(0.0, 0.0);
                    item.to_alpha = 0.0;
                }
                for (&i, rect) in self.visible.iter().zip(&packing.rects) {
                    let item = &mut self.items[i];
                    item.to_pos = vec2f(origin.x + rect.x, origin.y + rect.y);
                    item.to_size = vec2f(rect.w, rect.h);
                    item.to_alpha = 1.0;
                }
                self.bounds = Some((origin, vec2f(origin.x + packing.width, origin.y + packing.height)));
                self.packing = Some(packing);
                self.plan = None;
            }
        }
    }

    // ── camera ─────────────────────────────────────────────────────────

    fn view_center(&self) -> Vec2d {
        self.view_rect.pos + self.view_rect.size * 0.5
    }

    fn world_to_screen(&self, p: Vec2f) -> Vec2d {
        let c = self.view_center();
        Vec2d { x: (p.x as f64 - self.cam_pos.x) * self.cam_scale + c.x, y: (p.y as f64 - self.cam_pos.y) * self.cam_scale + c.y }
    }

    fn screen_to_world(&self, s: Vec2d) -> Vec2d {
        let c = self.view_center();
        Vec2d { x: (s.x - c.x) / self.cam_scale + self.cam_pos.x, y: (s.y - c.y) / self.cam_scale + self.cam_pos.y }
    }

    fn cam_usable(&self) -> bool {
        self.cam_ready
            && self.cam_scale.is_finite()
            && self.cam_scale > 0.0
            && self.cam_pos.x.is_finite()
            && self.cam_pos.y.is_finite()
    }

    /// Frame the whole grid in the viewport.
    fn fit_camera(&mut self) {
        let Some((lo, hi)) = self.bounds else {
            self.cam_scale = 1.0;
            self.cam_scale_t = 1.0;
            self.cam_pos = Vec2d::default();
            self.cam_pos_t = Vec2d::default();
            self.cam_ready = true;
            return;
        };
        if self.view_rect.size.x < 1.0 {
            return;
        }
        // The wall's world extent, whichever layout cut it.
        let cols = (hi.x - lo.x).max(0.01) as f64;
        let rows = (hi.y - lo.y).max(0.01) as f64;
        let scale = (self.view_rect.size.x * 0.94 / (cols + 0.6)).min(self.view_rect.size.y * 0.94 / (rows + 0.8));
        let scale = scale.clamp(0.01, 6000.0);
        // The fit is as far back as an auto-move goes; the wheel may pull a
        // little further to see the set with room around it.
        self.min_scale = scale * 0.2;
        self.zoom_anchor = None;
        self.cam_scale_t = scale;
        self.cam_pos_t = Vec2d::default();
        self.cam_ready = true;
    }

    /// Wheel zoom: the world point under the cursor stays under the cursor
    /// on every frame of the smoothing, not just at the end.
    fn zoom_at(&mut self, cursor: Vec2d, factor: f64) {
        if !self.cam_usable() {
            return;
        }
        let world = match self.zoom_anchor {
            Some((screen, world)) if (screen - cursor).length() < 1.0 => world,
            _ => self.screen_to_world(cursor),
        };
        self.zoom_anchor = Some((cursor, world));
        self.cam_scale_t = (self.cam_scale_t * factor).clamp(self.min_scale, 6000.0);
        let c = self.view_center();
        self.cam_pos_t = Vec2d { x: world.x - (cursor.x - c.x) / self.cam_scale_t, y: world.y - (cursor.y - c.y) / self.cam_scale_t };
    }

    /// Ease the live camera toward its target: scale in log space, and while
    /// a zoom anchor stands, the anchored world point is held under its
    /// screen point on every frame. Returns whether it is still moving.
    fn step_camera(&mut self, dt: f64) -> bool {
        if !self.cam_scale.is_finite() || self.cam_scale <= 0.0 || !self.cam_pos.x.is_finite() || !self.cam_pos.y.is_finite() {
            self.cam_scale = if self.cam_scale_t.is_finite() && self.cam_scale_t > 0.0 { self.cam_scale_t } else { 1.0 };
            self.cam_pos = if self.cam_pos_t.x.is_finite() && self.cam_pos_t.y.is_finite() { self.cam_pos_t } else { Vec2d::default() };
            self.cam_scale_t = self.cam_scale;
            self.cam_pos_t = self.cam_pos;
            self.zoom_anchor = None;
            return false;
        }
        let k = 1.0 - (-dt * 12.0).exp();
        let dp0 = self.cam_pos_t - self.cam_pos;
        let ds = self.cam_scale_t.ln() - self.cam_scale.ln();
        let was_apart = dp0.x.abs() * self.cam_scale > 0.05 || dp0.y.abs() * self.cam_scale > 0.05 || ds.abs() > 0.0005;
        self.cam_scale = (self.cam_scale.ln() + ds * k).exp();
        if let Some((screen, world)) = self.zoom_anchor {
            let c = self.view_center();
            self.cam_pos = Vec2d { x: world.x - (screen.x - c.x) / self.cam_scale, y: world.y - (screen.y - c.y) / self.cam_scale };
        } else if self.drag.is_some() {
            // Under a held drag the grid is BOLTED to the finger: easing here
            // makes the pan trail by a rubber-band beat.
            self.cam_pos = self.cam_pos_t;
        } else {
            let dp = self.cam_pos_t - self.cam_pos;
            self.cam_pos = self.cam_pos + dp * k;
        }
        let dp = self.cam_pos_t - self.cam_pos;
        let ds = self.cam_scale_t.ln() - self.cam_scale.ln();
        let moving = dp.x.abs() * self.cam_scale > 0.05 || dp.y.abs() * self.cam_scale > 0.05 || ds.abs() > 0.0005;
        if !moving {
            self.cam_pos = self.cam_pos_t;
            self.cam_scale = self.cam_scale_t;
            self.zoom_anchor = None;
            if was_apart {
                // The camera just settled: one beat re-asks LOD at rest.
                self.beat_now = true;
            }
        }
        moving
    }

    fn item_at(&self, world: Vec2d) -> Option<usize> {
        if let (Some(packing), Some((lo, _))) = (&self.packing, self.bounds) {
            // Packed: the rows know who is where — a rank in the packing is
            // a place in the display order, not an item index.
            let rank = packing.hit(world.x as f32 - lo.x, world.y as f32 - lo.y, 0.02)?;
            return self.visible.get(rank).copied();
        }
        let plan = self.plan?;
        let col = (world.x as f32 - plan.origin.x).floor();
        let row = (world.y as f32 - plan.origin.y).floor();
        if col < 0.0 || row < 0.0 || col as usize >= plan.cols {
            return None;
        }
        let rank = row as usize * plan.cols + col as usize;
        let item = self.items.get(rank)?;
        let (x, y) = (world.x as f32, world.y as f32);
        let slop = 0.04;
        (x >= item.pos.x - slop
            && x <= item.pos.x + item.size.x + slop
            && y >= item.pos.y - slop
            && y <= item.pos.y + item.size.y + slop)
            .then_some(rank)
    }

    // ── store events ───────────────────────────────────────────────────

    fn drain_store(&mut self, cx: &mut Cx) {
        let Some(store) = self.store.take() else { return };
        let mut redraw = false;
        while let Ok(event) = store.events.try_recv() {
            redraw = true;
            match event {
                StoreEvent::Page { shard, level, planes } => self.on_page(cx, shard, level, planes),
                StoreEvent::Full { item, px, finest, frame } => self.on_full(cx, item, px, finest, frame),
                StoreEvent::PageFailed { shard, level } => {
                    let until = self.time() + RETRY_EMBARGO_SECS;
                    self.requested.remove(&(shard, level));
                    self.page_failed.insert((shard, level), until);
                }
                StoreEvent::FullFailed { item } => {
                    let until = self.time() + RETRY_EMBARGO_SECS;
                    self.full_requested.remove(&item);
                    self.full_failed.insert(item, until);
                }
            }
        }
        self.store = Some(store);
        if redraw {
            self.next_frame = cx.new_next_frame();
            self.area.redraw(cx);
        }
    }

    fn make_page(cx: &mut Cx, planes: Planes, now: f64) -> PageTex {
        let (w, h) = (planes.width as usize, planes.height as usize);
        let bytes = planes.y.len() + planes.uv.len();
        let y = Texture::new_with_format(
            cx,
            TextureFormat::VecRu8 { width: w, height: h, data: Some(planes.y), unpack_row_length: None, updated: TextureUpdated::Full },
        );
        let uv = Texture::new_with_format(
            cx,
            TextureFormat::VecRGu8 {
                width: w / 2,
                height: h / 2,
                data: Some(planes.uv),
                unpack_row_length: None,
                updated: TextureUpdated::Full,
            },
        );
        PageTex { y, uv, arrived: now, last_used: 0, bytes }
    }

    fn on_page(&mut self, cx: &mut Cx, shard: i64, level: usize, planes: Planes) {
        let now = self.time();
        self.requested.remove(&(shard, level));
        if planes.width != page_size(level) {
            log!("image-tiles: page {shard} L{level}: unexpected size {}", planes.width);
            return;
        }
        let page = Self::make_page(cx, planes, now);
        self.pages.insert((shard, level), page);
    }

    fn on_full(&mut self, cx: &mut Cx, item: ItemId, px: u32, finest: bool, frame: FullFrame) {
        let now = self.time();
        self.full_requested.remove(&item);
        // A frame never replaces a finer one already resident: decodes come
        // back in whatever order the pool finishes them.
        if let Some(have) = self.full.get(&item) {
            if have.px > px {
                return;
            }
        }
        // Only a picture's FIRST frame fades in; a finer one swaps in where
        // the fade already stands, or the tile bounces sharp-soft-sharp.
        let arrived = self.full.get(&item).map_or(now, |have| have.arrived);
        let bytes = frame.bgra.len() * 4;
        let tex = Texture::new_with_format(
            cx,
            TextureFormat::VecMipBGRAu8_32 {
                width: frame.width as usize,
                height: frame.height as usize,
                data: Some(frame.bgra),
                max_level: Some(frame.max_level),
                wrap: TextureWrap::ClampToEdge,
                updated: TextureUpdated::Full,
            },
        );
        self.full.insert(item, FullTex { tex, arrived, last_used: self.frame, bytes, px, finest });
    }

    fn evict(&mut self) {
        let mut used: usize = self.pages.values().map(|p| p.bytes).sum();
        if used > VRAM_BUDGET {
            let frame = self.frame;
            // Fine levels only: the coarse tail is cheap and always kept, so
            // a shard never goes fully dark. Nothing drawn this frame or
            // last is touched.
            let mut victims: Vec<(PageKey, u64, usize)> = self
                .pages
                .iter()
                .filter(|((_, level), p)| *level <= 2 && p.last_used + 2 < frame)
                .map(|(k, p)| (*k, p.last_used, p.bytes))
                .collect();
            victims.sort_by_key(|(_, last, _)| *last);
            for (key, _, bytes) in victims {
                if used <= VRAM_BUDGET {
                    break;
                }
                self.pages.remove(&key);
                if let Some(view) = self.shard_views.get_mut(&key.0) {
                    if view.shown == Some(key.1) {
                        view.shown = None;
                    }
                }
                used -= bytes;
            }
        }
        let mut full_used: usize = self.full.values().map(|f| f.bytes).sum();
        if full_used > FULL_BUDGET {
            let frame = self.frame;
            let mut order: Vec<(ItemId, u64, usize)> =
                self.full.iter().filter(|(_, f)| f.last_used + 2 < frame).map(|(k, f)| (*k, f.last_used, f.bytes)).collect();
            order.sort_by_key(|(_, last, _)| *last);
            for (key, _, bytes) in order {
                if full_used <= FULL_BUDGET {
                    break;
                }
                self.full.remove(&key);
                full_used -= bytes;
            }
        }
    }

    fn push_instance(&mut self, cx: &mut Cx2d, i: usize, uv0: Vec2f, uv1: Vec2f, fade: f32, rgba: bool) {
        let item = &self.items[i];
        let p = self.world_to_screen(item.pos);
        let s = Vec2d { x: item.size.x as f64 * self.cam_scale, y: item.size.y as f64 * self.cam_scale };
        let dt = &mut self.draw_tile;
        dt.tile_pos = item.pos;
        dt.tile_size = item.size;
        dt.rgba = if rgba { 1.0 } else { 0.0 };
        dt.uv0 = uv0;
        dt.uv1 = uv1;
        dt.fade = fade * item.alpha;
        dt.draw_abs(cx, Rect { pos: p, size: s });
    }
}

fn item_of(row: &ItemRow) -> Option<GridItem> {
    let shard = row.shard?;
    let slot = row.slot? as u32;
    let fit = fit_dims(row.width.max(1) as u32, row.height.max(1) as u32, SLOT);
    Some(GridItem {
        id: row.id,
        shard,
        slot,
        uv1: vec2f(fit.0 as f32 / SLOT as f32, fit.1 as f32 / SLOT as f32),
        title: row.title.as_str().into(),
        link: row.link.as_str().into(),
        url: row.url.as_str().into(),
        pos: Vec2f::default(),
        size: vec2f(CELL_FILL, CELL_FILL),
        aspect: row.aspect as f32,
        to_pos: Vec2f::default(),
        to_size: vec2f(CELL_FILL, CELL_FILL),
        from_pos: Vec2f::default(),
        from_size: vec2f(CELL_FILL, CELL_FILL),
        alpha: 1.0,
        from_alpha: 1.0,
        to_alpha: 1.0,
    })
}

impl Widget for TileGrid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::Startup = event {
            self.ensure_open(cx);
        }
        if let Event::Signal = event {
            self.drain_store(cx);
        }
        if let Event::Shutdown = event {
            if let Some(store) = self.store.take() {
                store.shutdown();
            }
        }
        if let Some(nf) = self.next_frame.is_event(event) {
            let now_time = nf.time;
            let dt = (now_time - self.last_time).clamp(0.0, 0.1);
            self.last_time = now_time;
            let moving = self.step_camera(dt) || self.drag.is_some();
            // Pictures in flight: their drawn rects move every frame, so the
            // instance buffer is rebuilt until they land.
            let tweening = self.step_tween(cx.seconds_since_app_start());
            let now = self.time();
            if let Some(at) = self.resize_settle_at {
                if now >= at {
                    self.resize_settle_at = None;
                    self.relayout();
                    if !self.user_moved {
                        self.fit_camera();
                    }
                    self.area.redraw(cx);
                    self.next_frame = cx.new_next_frame();
                }
            }
            let animating = self.pages.values().any(|p| now - p.arrived < FADE_SECS)
                || self.full.values().any(|f| now - f.arrived < FADE_SECS);
            if moving || animating || tweening || self.resize_settle_at.is_some() {
                self.next_frame = cx.new_next_frame();
            }
            // A settle beat exists to ask for the settled camera's tiles;
            // between beats, glide frames ride the uniforms.
            let beat_gap = if moving { 1.5 } else { 0.25 };
            let beat_due = self.beat_now && now - self.last_beat > beat_gap;
            if beat_due {
                self.last_beat = now;
            }
            let within_pad = self.pushed_cam.map_or(false, |(pos, scale)| {
                let ratio = self.cam_scale / scale.max(1e-9);
                let view_world = self.view_rect.size.x / self.cam_scale.max(1e-9);
                let d = self.cam_pos - pos;
                // Zoom is a uniform too: instances are world-space, so scale
                // changes cost nothing until the CULL SET is wrong. With
                // everything pushed any zoom-in is safe; zooming out reveals
                // unpushed tiles and rebuilds past the pad.
                let ratio_ok = if self.pushed_all { ratio > 0.25 && ratio < 4.0 } else { ratio > 0.9 && ratio < 4.0 };
                ratio_ok && (self.pushed_all || (d.x.abs() < view_world * 0.09 && d.y.abs() < view_world * 0.09))
            });
            if !beat_due && !tweening && (moving || animating) && self.items.len() > UNIFORM_ONLY_MIN_TILES && within_pad {
                // Re-present the retained draw calls under fresh camera
                // uniforms: no draw_walk, no instance re-upload.
                let center = self.view_center();
                let area = self.area;
                let dv = &mut self.draw_tile.draw_vars;
                dv.set_uniform_on_draw_list(cx, area, id!(cam_pos), &[self.cam_pos.x as f32, self.cam_pos.y as f32]);
                dv.set_uniform_on_draw_list(cx, area, id!(cam_scale), &[self.cam_scale as f32]);
                dv.set_uniform_on_draw_list(cx, area, id!(view_center), &[center.x as f32, center.y as f32]);
            } else if moving || animating || tweening || beat_due {
                self.area.redraw(cx);
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerScroll(fs) => {
                self.user_moved = true;
                if fs.scroll.x.abs() > fs.scroll.y.abs() * 1.2 && self.cam_usable() {
                    self.zoom_anchor = None;
                    self.cam_pos_t.x += fs.scroll.x / self.cam_scale_t;
                } else {
                    let factor = (-fs.scroll.y * 0.0025).exp();
                    self.zoom_at(fs.abs, factor);
                }
                self.next_frame = cx.new_next_frame();
            }
            Hit::FingerDown(fd) => {
                self.drag = Some((fd.abs, self.cam_pos_t, false));
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fm) => {
                if let Some((start, cam, moved)) = self.drag {
                    let delta = fm.abs - start;
                    let moved = moved || delta.length() > 3.0;
                    self.drag = Some((start, cam, moved));
                    if moved && self.cam_usable() {
                        self.user_moved = true;
                        self.zoom_anchor = None;
                        self.cam_pos_t = Vec2d { x: cam.x - delta.x / self.cam_scale_t, y: cam.y - delta.y / self.cam_scale_t };
                        self.next_frame = cx.new_next_frame();
                    }
                }
            }
            Hit::FingerUp(fu) => {
                cx.set_cursor(MouseCursor::Default);
                if let Some((_, _, moved)) = self.drag.take() {
                    if !moved {
                        let world = self.screen_to_world(fu.abs);
                        if let Some(rank) = self.item_at(world) {
                            let item = &self.items[rank];
                            let uid = self.widget_uid();
                            cx.widget_action(
                                uid,
                                TileGridAction::Clicked {
                                    item: item.id,
                                    title: item.title.to_string(),
                                    link: item.link.to_string(),
                                    url: item.url.to_string(),
                                },
                            );
                        }
                    }
                    self.next_frame = cx.new_next_frame();
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        let first = self.view_rect.size.x < 1.0 && rect.size.x >= 1.0;
        let resized = !first
            && rect.size.x >= 1.0
            && (rect.size.x - self.view_rect.size.x).abs() + (rect.size.y - self.view_rect.size.y).abs() > 0.5;
        self.view_rect = rect;
        if resized {
            self.resize_settle_at = Some(self.time() + RESIZE_SETTLE_SECS);
            self.next_frame = cx.new_next_frame();
        }
        if first {
            self.ensure_open(cx.cx);
            self.relayout();
            if !self.user_moved {
                self.fit_camera();
                self.cam_pos = self.cam_pos_t;
                self.cam_scale = self.cam_scale_t;
            }
        }
        self.frame += 1;
        if !self.cam_usable() || self.items.is_empty() {
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        }
        let now = self.time();
        let frame = self.frame;
        let center = self.view_center();
        self.pushed_cam = Some((self.cam_pos, self.cam_scale));
        self.draw_tile.set_uniform(cx, id!(cam_pos), &[self.cam_pos.x as f32, self.cam_pos.y as f32]);
        self.draw_tile.set_uniform(cx, id!(cam_scale), &[self.cam_scale as f32]);
        self.draw_tile.set_uniform(cx, id!(view_center), &[center.x as f32, center.y as f32]);

        // Visible world rectangle, padded a cell plus a twelfth of the view
        // on every side: glide frames re-present this build under a camera
        // that has moved a little, and the pad keeps the leading edge
        // populated until the next rebuild.
        let w0 = self.screen_to_world(rect.pos);
        let w1 = self.screen_to_world(rect.pos + rect.size);
        let (pad_x, pad_y) = ((w1.x - w0.x) as f32 * 0.12, (w1.y - w0.y) as f32 * 0.12);
        let (vx0, vy0, vx1, vy1) =
            (w0.x as f32 - 1.0 - pad_x, w0.y as f32 - 1.0 - pad_y, w1.x as f32 + 1.0 + pad_x, w1.y as f32 + 1.0 + pad_y);

        let mut by_shard: HashMap<i64, Vec<usize>> = HashMap::new();
        let mut want_pages: HashMap<PageKey, u64> = HashMap::new();
        let mut culled = 0usize;
        let cull_off = self.cull_frac < 0.10;
        for (i, item) in self.items.iter().enumerate() {
            if item.alpha <= 0.0 {
                // Dropped by the query and done shrinking: not on the wall.
                continue;
            }
            let (p, s) = (item.pos, item.size);
            if p.x + s.x < vx0 || p.x > vx1 || p.y + s.y < vy0 || p.y > vy1 {
                // The count is the truth either way — it decides whether the
                // NEXT build culls — but the skip itself is what hysteresis
                // turns off.
                culled += 1;
                if !cull_off {
                    continue;
                }
            }
            by_shard.entry(item.shard).or_default().push(i);
        }
        // A build that culled nothing holds the whole grid: no amount of
        // panning invalidates it, so glide frames stay uniform-only until
        // the zoom changes. Below 10% culled, culling buys nothing and costs
        // pan-proofness — stop culling.
        self.pushed_all = culled == 0;
        self.cull_frac = culled as f32 / self.items.len().max(1) as f32;

        // Level wanted for the on-screen slot size in DEVICE pixels: the
        // finer neighbour, so a source pixel is never stretched over more
        // than one screen pixel.
        let px = self.cam_scale * cx.current_dpi_factor().max(1.0);
        // The level for a tile `side` world units across: a packed wall
        // hangs pictures of many sizes, so each shard asks for the page its
        // BIGGEST visible tile needs; on the uniform grid every side is a
        // cell and this is the old single answer.
        let desired_for = |side: f32| -> usize {
            let tile_px = px * side.max(0.05) as f64;
            let lod = (SLOT as f64 / tile_px.max(1.0)).log2();
            lod.floor().clamp(0.0, (LEVELS - 1) as f64) as usize
        };

        let mut passes: Vec<Pass> = Vec::new();
        let mut shards: Vec<i64> = by_shard.keys().copied().collect();
        shards.sort_unstable();
        let inv = 1.0 / GRID as f32;
        for key in shards {
            let indices = by_shard.remove(&key).unwrap();
            let side = indices
                .iter()
                .map(|&i| self.items[i].size.x.max(self.items[i].size.y))
                .fold(CELL_FILL, f32::max);
            let desired = desired_for(side);
            let resident: Vec<usize> = (0..LEVELS).filter(|l| self.pages.contains_key(&(key, *l))).collect();
            if !self.pages.contains_key(&(key, desired)) {
                // What this page would paint: every visible tile of the
                // shard, at the cell size this zoom draws them.
                let cell = (px * CELL_FILL as f64) * (px * CELL_FILL as f64);
                want_pages.insert((key, desired), (indices.len() as f64 * cell).max(1.0) as u64);
            }
            if resident.is_empty() {
                continue;
            }
            let fine = *resident
                .iter()
                .min_by_key(|l| (**l as i64 - desired as i64).abs() * 2 - if **l >= desired { 1 } else { 0 })
                .unwrap();
            let fade = ((now - self.pages[&(key, fine)].arrived) / FADE_SECS).clamp(0.0, 1.0) as f32;
            let shown = self.shard_views.get(&key).and_then(|v| v.shown);
            let tiles: Vec<(usize, Vec2f, Vec2f)> = indices
                .iter()
                .map(|&i| {
                    let item = &self.items[i];
                    let (sx, sy) = ((item.slot % GRID) as f32 * inv, (item.slot / GRID) as f32 * inv);
                    (i, vec2f(sx, sy), vec2f(sx + item.uv1.x * inv, sy + item.uv1.y * inv))
                })
                .collect();
            if fade < 1.0 {
                // Whatever was drawn before stays underneath until the new
                // page is in: transitions are crossfades, never a blink.
                let under = shown
                    .filter(|p| *p != fine && self.pages.contains_key(&(key, *p)))
                    .or_else(|| resident.iter().copied().filter(|l| *l != fine).max());
                if let Some(prev) = under {
                    let page = &self.pages[&(key, prev)];
                    passes.push(Pass { y: page.y.clone(), uv: page.uv.clone(), fade: 1.0, tiles: tiles.clone() });
                    self.pages.get_mut(&(key, prev)).unwrap().last_used = frame;
                }
            } else {
                self.shard_views.entry(key).or_default().shown = Some(fine);
            }
            let page = self.pages.get_mut(&(key, fine)).unwrap();
            page.last_used = frame;
            passes.push(Pass { y: page.y.clone(), uv: page.uv.clone(), fade, tiles });
        }
        for (&key, &priority) in &want_pages {
            if embargoed(&mut self.page_failed, key, now) {
                continue;
            }
            if let Some(store) = &self.store {
                if self.requested.insert(key) {
                    store.need_page(key.0, key.1, priority);
                }
            }
        }

        // Coarse (opaque) passes first, then the fading fine ones on top.
        passes.sort_by(|a, b| b.fade.partial_cmp(&a.fade).unwrap_or(std::cmp::Ordering::Equal));
        let mut full_needs: Vec<(ItemId, f64, u32)> = Vec::new();
        // Every visible picture that wants a frame at all, asked or not:
        // this is what "still wanted" means to the store on the wants beat.
        let mut full_visible: Vec<(ItemId, u64)> = Vec::new();
        let mut full_draw: Vec<(usize, Texture, f32)> = Vec::new();
        for pass in &passes {
            self.draw_tile.draw_vars.set_texture(0, &pass.y);
            self.draw_tile.draw_vars.set_texture(1, &pass.uv);
            // One batch per pass: the pass holds its textures constant,
            // which is exactly the condition for batching — thousands of
            // tiles become a memcpy into one instance buffer and one call.
            self.draw_tile.begin_many_instances(cx);
            for &(i, uv0, uv1) in &pass.tiles {
                let item_id = self.items[i].id;
                let (s, p) = (self.items[i].size, self.items[i].pos);
                let area = ((s.x * s.y) as f64 * self.cam_scale * self.cam_scale).max(1.0);
                let tile_px = ((s.x.max(s.y)) as f64 * px).ceil().max(1.0) as u32;
                let on_screen = p.x + s.x >= vx0 && p.x <= vx1 && p.y + s.y >= vy0 && p.y <= vy1;
                if on_screen && tile_px as f64 >= FULL_RES_PX {
                    full_visible.push((item_id, area as u64));
                    match self.full.get_mut(&item_id) {
                        Some(tex) => {
                            tex.last_used = frame;
                            // Outgrown what is held: ask finer, draw this one
                            // until it lands.
                            if !tex.finest && tile_px > tex.px && !self.full_requested.contains(&item_id) {
                                full_needs.push((item_id, area, tile_px));
                            }
                            let ffade = ((now - tex.arrived) / FADE_SECS).clamp(0.0, 1.0) as f32;
                            if ffade >= 1.0 && pass.fade >= 1.0 {
                                full_draw.push((i, tex.tex.clone(), 1.0));
                                continue;
                            }
                            full_draw.push((i, tex.tex.clone(), ffade));
                        }
                        None => {
                            if !self.full_requested.contains(&item_id) {
                                full_needs.push((item_id, area, tile_px));
                            }
                        }
                    }
                }
                self.push_instance(cx, i, uv0, uv1, pass.fade, false);
            }
            self.draw_tile.end_many_instances(cx);
        }
        full_draw.sort_by_key(|(i, ..)| *i);
        full_draw.dedup_by_key(|(i, ..)| *i);
        for (i, tex, ffade) in full_draw {
            self.draw_tile.draw_vars.set_texture(0, &tex);
            self.draw_tile.draw_vars.set_texture(1, &tex);
            self.push_instance(cx, i, vec2f(0.0, 0.0), vec2f(1.0, 1.0), ffade, true);
        }

        // Biggest on screen first, and only what the budget can hold: past
        // it, every ask evicts something just fetched and the pool spends
        // itself re-fetching. The few biggest are asked WHATEVER the budget
        // says — the picture being stared at must never be the one refused;
        // making room for it is eviction's job, not the ask's.
        full_needs.sort_by(|a, b| b.1.total_cmp(&a.1));
        let visible_keys: HashSet<ItemId> = full_visible.iter().map(|(k, _)| *k).collect();
        let resident_visible: usize = self.full.iter().filter(|(k, _)| visible_keys.contains(*k)).map(|(_, f)| f.bytes).sum();
        let mut planned = resident_visible;
        let level_bytes = |want: u32| {
            let level = PYRAMID_LEVELS.iter().rev().find(|p| **p >= want).copied().unwrap_or(want);
            (level as usize).pow(2) * 4 * 4 / 3
        };
        const ALWAYS_ASK: usize = 4;
        let affordable: Vec<(ItemId, f64, u32)> = full_needs
            .into_iter()
            .enumerate()
            .take_while(|(n, (_, _, want))| {
                planned += level_bytes(*want);
                *n < ALWAYS_ASK || planned <= FULL_BUDGET
            })
            .map(|(_, v)| v)
            .collect();
        for (key, area, want) in affordable.into_iter().take(8) {
            if embargoed(&mut self.full_failed, key, now) {
                continue;
            }
            if let Some(store) = &self.store {
                if self.full_requested.insert(key) {
                    store.need_full(key, want, area as u64);
                }
            }
        }

        // Every wants beat, tell the store exactly what decode work is still
        // worth doing: queued work not named here has left the view and is
        // dropped, and the claims on it are struck so it is askable again
        // the moment it comes back.
        if self.frame % 15 == 0 || self.beat_now {
            self.beat_now = false;
            if let Some(store) = &self.store {
                let fulls: HashMap<ItemId, u64> = full_visible.iter().copied().collect();
                store.wants(&want_pages, &fulls);
                self.requested.retain(|k| want_pages.contains_key(k));
                self.full_requested.retain(|k| fulls.contains_key(k));
            }
        }

        self.evict();
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl TileGridRef {
    /// Open (or re-open) a library at `root`.
    pub fn open(&self, cx: &mut Cx, root: &std::path::Path) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx, Library::new(root));
        }
    }

    /// The clicked item, if any of `actions` is ours.
    pub fn clicked(&self, actions: &Actions) -> Option<(ItemId, String, String, String)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let TileGridAction::Clicked { item, title, link, url } = item.cast() {
                return Some((item, title, link, url));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> Vec<(String, String)> {
        vec![
            ("2026-01-02: A robot learns to love".into(), "https://smbc/1".into()),
            ("2026-01-03: Physics of a cat".into(), "https://smbc/2".into()),
            ("2026-01-04: The Robot uprising, again".into(), "https://smbc/3".into()),
            ("2026-01-05: robotics for cats".into(), "https://smbc/robot".into()),
        ]
    }

    #[test]
    fn an_empty_query_is_library_order_and_a_query_ranks_the_matches() {
        let w = wall();
        assert_eq!(order_for(&w, ""), vec![0, 1, 2, 3]);
        assert_eq!(order_for(&w, "   "), vec![0, 1, 2, 3]);
        // Whole-word title hits first (ties in rank order), then the
        // substring hit; a link-only hit would come last.
        assert_eq!(order_for(&w, "robot"), vec![0, 2, 3]);
        // Every word must appear somewhere.
        assert_eq!(order_for(&w, "robot cat"), vec![3]);
        assert!(order_for(&w, "dog").is_empty());
        // Case does not matter; the link counts.
        assert_eq!(order_for(&w, "SMBC/2"), vec![1]);
    }

    #[test]
    fn the_flight_starts_where_it_is_and_lands_where_it_goes() {
        assert_eq!(ease_out_quint(0.0), 0.0);
        assert_eq!(ease_out_quint(1.0), 1.0);
        assert_eq!(ease_out_quint(2.0), 1.0);
        let mid = ease_out_quint(0.5);
        assert!(mid > 0.9 && mid < 1.0, "fast out of the gate: {mid}");
        let a = vec2f(0.0, 0.0);
        let b = vec2f(4.0, -2.0);
        assert_eq!(lerp2(a, b, 0.0), a);
        assert_eq!(lerp2(a, b, 1.0), b);
        let half = lerp2(a, b, 0.5);
        assert!((half.x - 2.0).abs() < 1e-6 && (half.y + 1.0).abs() < 1e-6);
        // Monotone: later is never further from the target.
        let mut last = 0.0f32;
        for i in 1..=20 {
            let e = ease_out_quint(i as f32 / 20.0);
            assert!(e >= last);
            last = e;
        }
    }
}
