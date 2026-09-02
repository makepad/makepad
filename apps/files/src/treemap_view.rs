//! The treemap: a spatial map of where a folder's bytes actually are.
//!
//! Every rectangle's area is its bytes, all the way down to the individual
//! file: a 4 GB video is visibly four thousand times the block a 1 MB photo
//! gets, and the folder it sits in is the frame drawn around it. There is no
//! depth limit — the map stops where a rectangle stops being visible, which on
//! a big window is at the file and on a small one is a few folders up.
//!
//! Three things make it readable rather than a field of colour. Each tile is a
//! shaded cushion (Van Wijk), so ten thousand rectangles read as ten thousand
//! things. Hue says what kind of thing it is, and a folder borrows the hue of
//! its own heaviest content, so a folder full of video reads as video without
//! being opened. And nesting is drawn as a frame that narrows with depth,
//! which is what keeps the borders from eating the bytes they surround.
//!
//! The scan never runs on the UI thread and never makes anyone wait for all of
//! it: a worker streams the tree back as it walks (see [`crate::treemap`]), the
//! map is drawable after the first `read_dir`, and it sharpens as the walk goes
//! deeper. The layout itself is pure arithmetic and runs inline, throttled
//! while a scan is still feeding it.

use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use crate::{
    model::FileKind,
    theme::Palette,
    treemap::{self, Cell, MapStyle, Node, Query, Rect as MapRect, ScanStep},
};

/// The strip along the top carrying the zoom breadcrumb and the scan state.
const CRUMB_H: f64 = 21.0;
/// The strip along the bottom carrying the persistent selection readout.
const FOOT_H: f64 = 21.0;
/// A rectangle needs this much room before its name is worth drawing, and
/// this much again before its size goes on a second line.
const LABEL_MIN: DVec2 = DVec2 { x: 50.0, y: 19.0 };
const LABEL_TWO_LINE_H: f64 = 32.0;
/// A ceiling on labels per frame. Past a few hundred names nobody is reading
/// them and every one costs a text layout.
const LABEL_BUDGET: usize = 700;
/// The map is only re-laid-out this often while a scan is still feeding it —
/// the tree changes hundreds of times a second and the picture does not need
/// to.
const RELAYOUT_EVERY: Duration = Duration::from_millis(110);
/// How often the worker wakes the UI. The steps themselves queue freely; this
/// only bounds the signals.
const SIGNAL_EVERY: Duration = Duration::from_millis(45);
/// The kind tag [`treemap::layout`] gives the "N smaller items" rectangle.
const KIND_BUNDLE: u8 = u8::MAX;
/// The palette class everything unrecognised falls into.
const OTHER_CLASS: usize = 6;
/// How long a filter change morphs the map from the old cell set to the new.
const TWEEN: Duration = Duration::from_millis(200);
/// Points of elevation one nesting level is worth at camera scale 1 — the
/// whole meaning of the raised projections: height is depth. Big enough
/// that a nested plate clears its parent's label line.
const RISE: f64 = 11.0;
/// The perspective eye's distance from the pivot along the view axis, in the
/// same points. Large on purpose: the 3d mode is the ortho map breathing,
/// not a flyover.
const PERSP_EYE: f64 = 1500.0;
/// Where the orbit starts and where Esc returns it: enough tilt that height
/// reads immediately, nowhere near enough to hide the map behind itself.
const DEFAULT_PITCH: f64 = 0.66;
/// The grazing end of the tilt. Past this the plane degenerates into a
/// horizon and a disk-use instrument stops being one.
const MAX_PITCH: f64 = 1.15;
/// Radians of yaw per point of leftward drag, and of pitch per point down.
const ORBIT_PER_PT: f64 = 0.010;
const PITCH_PER_PT: f64 = 0.008;
/// A press that stays within this many points is a click; past it, the
/// button's drag gesture — and never both.
const DRAG_THRESHOLD: f64 = 4.0;
/// The tile size at which the cushion is at full strength. A cushion lives in
/// the tile's own 0..1 space, so left alone a huge rectangle gets a huge soft
/// gradient that reads as a spotlight rather than as a surface. The shading is
/// there to separate small neighbours, so it fades out on the big ones, where
/// there is a border and a label doing the same job.
const CUSHION_FULL_AT: f64 = 44.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    /** One face of the map: a Van Wijk cushion — a shallow pillow lit from
     * the upper left — inside a hard border. The cushion is what makes a
     * dense map readable: adjacent tiles of the same hue are separated by
     * their own shading even where there is no room for a border line.
     *
     * The geometry is a free QUAD, not a rect: the orbit camera hands four
     * projected screen corners per instance (c0 top-left, c1 top-right, c2
     * bottom-right, c3 bottom-left) and the vertex stage interpolates them
     * bilinearly, so one shared instance batch draws the flat map, the tilted
     * plates and the prism walls alike. Because the corners are free, the
     * usual vertex-clamp scissor would deform the shape — clipping happens in
     * the fragment against the same draw_clip instead. */
    set_type_default() do #(DrawMapTile::script_shader(vm)) {
        ..mod.draw.DrawQuad
        /** the tile's own colour */
        color: #x40507a
        /** the border drawn around the tile */
        edge: #x16161e
        /** cushion depth 0..1 step 0.05 */
        cushion: 0.55
        /** border thickness in points 0..3 step 0.25 */
        border: 1.0
        scr: varying(vec2f)
        qsize: varying(vec2f)
        vertex: fn() {
            let p = mix(
                mix(self.c0, self.c1, self.geom.pos.x)
                mix(self.c3, self.c2, self.geom.pos.x)
                self.geom.pos.y
            )
            self.pos = self.geom.pos
            self.scr = p
            self.qsize = vec2(
                max(length(self.c1 - self.c0), 1.0)
                max(length(self.c3 - self.c0), 1.0)
            )
            let ps = p + self.draw_list.view_shift
            self.world = self.draw_list.view_transform * vec4(
                ps.x
                ps.y
                self.draw_depth + self.draw_call.zbias
                1.0
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        pixel: fn() {
            // The fragment scissor the free-quad geometry needs: outside the
            // clip the fragment simply is not there.
            if self.scr.x < self.draw_clip.x || self.scr.y < self.draw_clip.y
                || self.scr.x > self.draw_clip.z || self.scr.y > self.draw_clip.w {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let p = self.pos * self.qsize
            let d = min(min(p.x, p.y), min(self.qsize.x - p.x, self.qsize.y - p.y))
            // The pillow's surface normal. The height field is the classic
            // x(1-x)·y(1-y) parabola, so its slope is linear in the position
            // and costs two multiplies.
            let nx = self.cushion * (1.0 - 2.0 * self.pos.x)
            let ny = self.cushion * (1.0 - 2.0 * self.pos.y)
            let n = normalize(vec3(-nx, -ny, 1.0))
            let l = normalize(vec3(-0.45, -0.62, 0.64))
            let h = normalize(l + vec3(0.0, 0.0, 1.0))
            let diff = clamp(dot(n, l), 0.0, 1.0)
            let spec = pow(clamp(dot(n, h), 0.0, 1.0), /**highlight tightness 4..64 step 2*/ 26.0)
            let lit = self.color.rgb * (/**ambient 0.2..1 step 0.02*/ 0.56 + /**diffuse 0..1.5 step 0.02*/ 0.68 * diff)
                + vec3(spec, spec, spec) * /**highlight 0..0.6 step 0.02*/ 0.18
            let cov = clamp((self.border - d) * 2.0 + 0.5, 0.0, 1.0)
            let c = mix(vec4(lit, self.color.w), self.edge, cov)
            return vec4(c.rgb * c.w, c.w)
        }
    }

    mod.widgets.MpfTreemapBase = #(TreemapView::register_widget(vm))
    mod.widgets.MpfTreemap = set_type_default() do mod.widgets.MpfTreemapBase{
        width: Fill
        height: Fill
        draw_bg +: {color: mod.mpf.bg}
        draw_tile +: {}
        draw_text +: {
            color: mod.mpf.fg
            text_style: theme.font_regular{font_size: 8.0}
        }
        draw_bold +: {
            color: mod.mpf.fg_bright
            text_style: theme.font_bold{font_size: 8.5}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMapTile {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    cushion: f32,
    #[live]
    border: f32,
    /// The projected screen corners of this face, clockwise from top-left.
    /// Every face the map draws — flat tile, tilted plate, prism wall — is
    /// these four points; `rect_pos`/`rect_size` only carry the bounding box.
    #[live]
    c0: Vec2f,
    #[live]
    c1: Vec2f,
    #[live]
    c2: Vec2f,
    #[live]
    c3: Vec2f,
}

/// What a press on the map means to the folder view around it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum TreemapAction {
    /// A rectangle was picked. The map keeps showing it; the browser may
    /// select it too when it happens to be in the current listing.
    Selected(PathBuf),
    /// What was picked is not on the disk any more; the map has dropped it.
    Vanished(PathBuf),
    /// The ✕ on the filter chip: the map is unfiltered again, and whoever
    /// owns the filter controls should show them cleared.
    FilterCleared,
    /// A secondary press released without dragging: the context menu's
    /// moment, at this window point. A secondary press that dragged was a
    /// pan and asks for nothing.
    Context(DVec2),
    #[default]
    None,
}

/// The kind class a file's colour comes from: the index into
/// [`Palette::kinds`]. Kept here rather than on `FileKind` because it is a
/// property of *this picture*, not of the file.
pub fn kind_class(kind: FileKind) -> u8 {
    match kind {
        FileKind::Video => 0,
        FileKind::Image => 1,
        FileKind::Audio => 2,
        FileKind::Code => 3,
        // A PDF reads as a document, which is what the text hue means here.
        FileKind::Text | FileKind::Pdf => 4,
        FileKind::Archive => 5,
        FileKind::Folder | FileKind::Generic => 6,
    }
}

/// One message from the scan worker. `generation` is the request it answers,
/// so a scan the user already navigated away from is dropped rather than
/// folded into the folder they are looking at now.
struct ScanMessage {
    generation: u64,
    step: Option<ScanStep>,
    finished: Option<Outcome>,
}

/// How a request for a folder's map ended.
enum Outcome {
    /// The disk was walked. The tree in hand is fresh, and worth saving.
    Scanned,
    /// The saved map was good and is what got delivered — nothing was read
    /// off the disk at all, which is the whole point of keeping it.
    Loaded { scanned_at: u64 },
    /// Cancelled, or the folder could not be read.
    Failed,
}

/// What the footer keeps saying after a click — held apart from the cell list
/// because a relayout throws every cell away and the selection must survive
/// it.
#[derive(Clone, Debug, PartialEq)]
struct Pick {
    path: PathBuf,
    size: u64,
    files: u32,
    is_dir: bool,
    bundle: u32,
}

/// A name waiting to be drawn on top of the finished tiles.
struct Label {
    at: DVec2,
    room: f64,
    line: String,
    below: Option<String>,
    ink: Vec4f,
    /// True for a name that floats over other tiles (a group's) and brings
    /// its own dim plate for contrast. A leaf's name sits on the leaf's own
    /// cushion and needs none.
    scrim: bool,
}

/// A flat-map label frozen at its relayout: text already fitted, width
/// already measured, anchor in layout space. Drawing translates the anchor
/// through the live camera remap and nothing else — deriving names from the
/// remapped rects every frame made each one flicker through its own
/// truncation points and re-stack its stagger row all through a zoom glide.
struct FrozenLabel {
    at: DVec2,
    line: String,
    below: Option<String>,
    width: f64,
    ink: Vec4f,
    scrim: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TreemapView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    // The whole panel, not one of the draw calls inside it: a redraw of this
    // view has to invalidate the map, and the last thing any of the shaders
    // below touched is a strip at one edge of it.
    #[redraw]
    #[area]
    area: Area,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_tile: DrawMapTile,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,

    /// The folder the map is of — the browser's folder.
    #[rust]
    root: PathBuf,
    /// The names between the mapped folder and the one the map is zoomed
    /// into. Empty means the whole scan is on screen.
    #[rust]
    zoom: Vec<String>,
    #[rust]
    tree: Node,
    #[rust]
    style: MapStyle,
    #[rust]
    cells: Vec<Cell>,
    /// The rect `cells` was laid out for; a different one means re-layout.
    #[rust]
    laid_out: Rect,
    #[rust]
    stale: bool,
    #[rust]
    last_layout: Option<f64>,
    #[rust]
    frame: NextFrame,

    /// The visual camera over the map: 1.0 shows the whole focused folder
    /// fitted to the panel; larger blows it up that many times, with
    /// `cam_off` saying how far the window has slid into the blown-up map
    /// (in points, from its top-left). Purely a way of *looking* — the
    /// breadcrumb, the browser and the scan never move with it.
    #[rust]
    cam_scale: f64,
    #[rust]
    cam_off: DVec2,
    /// The orbit half of the camera, raised projections only: `yaw` spins
    /// the map plane about the panel's centre, `pitch` tilts the eye from
    /// straight down (0) toward grazing. The flat map ignores both.
    #[rust]
    yaw: f64,
    #[rust]
    pitch: f64,
    /// A press of either button waiting to learn whether it is a click or
    /// that button's drag gesture. The click itself is decided on release —
    /// a press that moved is a gesture and picks nothing, opens nothing, so
    /// dragging across the map never changes the selection and never opens
    /// the menu.
    #[rust]
    drag: Option<Drag>,

    /// How the map is drawn: flat, extruded, or in perspective.
    #[rust]
    projection: MapProjection,
    /// The order cells paint in for the raised projections. Empty for the
    /// flat map, whose own vector is already painter's order.
    #[rust]
    paint_order: Vec<usize>,

    /// The live filter. None (or an empty query) is the whole disk.
    #[rust]
    filter: Option<Query>,
    /// What the filter matched under the focused folder: (bytes, files).
    #[rust]
    filtered: Option<(u64, u32)>,
    /// Where the filter chip's ✕ was drawn, for the click that clears it.
    #[rust]
    filter_hit: Rect,
    /// Byte totals per kind tag — the legend's numbers, recomputed lazily.
    #[rust]
    totals: [u64; 16],
    #[rust]
    totals_dirty: bool,
    /// The filter's weights, measured once per (tree revision, query) and
    /// reused by every relayout since — the camera relayouts every frame of
    /// an orbit, and re-walking six hundred thousand nodes each frame was
    /// exactly the frame rate the user was feeling. None while unfiltered.
    #[rust]
    measure: Option<treemap::Measure>,
    #[rust]
    measure_rev: u64,
    #[rust]
    measure_query: Option<Query>,
    /// Bumped whenever the measured tree itself changes — scan steps landing,
    /// moves absorbed, a re-root — never by the camera.
    #[rust]
    tree_rev: u64,

    /// The camera `cells` were laid out at. While a gesture moves the live
    /// camera, the draw path remaps every rect from this camera to that one —
    /// the picture rides along as one rigid sheet — and the layout is only
    /// rebuilt when the motion settles (or on a coarse cadence during a long
    /// one), morphing there. This is what keeps a wheel zoom visually
    /// constant: the layout is not scale-invariant (insets and header strips
    /// are fixed point sizes, the bundle floor moves with area), so
    /// re-laying-out every glide frame made tiles swim and jump mid-zoom.
    #[rust]
    layout_scale: f64,
    #[rust]
    layout_off: DVec2,
    #[rust]
    layout_yaw: f64,
    #[rust]
    layout_pitch: f64,
    /// The ground region the current cells were laid out over (the padded
    /// cull). As long as the live camera still looks inside it — and has not
    /// zoomed in past what the layout resolves — the layout is not remade at
    /// all: a settling gesture keeps the exact arrangement on screen instead
    /// of buying a fresh packing nobody asked for.
    #[rust]
    laid_cull: MapRect,

    /// The wheel's glide: the scale it is headed for, and the ground point
    /// pinned under the cursor for the whole ride. Each wheel step retargets;
    /// the camera eases there over a few frames instead of jumping.
    #[rust]
    zoom_glide: Option<ZoomGlide>,
    /// Q/E's glide: the yaw the orbit is headed for, and the last tick.
    #[rust]
    yaw_glide: Option<(f64, f64)>,

    /// The filter tween: where each surviving path was, the cells that are
    /// leaving (with the rect they were last seen at), and when it started.
    #[rust]
    tween_from: HashMap<PathBuf, TweenFrom>,
    #[rust]
    tween_leavers: Vec<(Cell, MapRect, f64)>,
    #[rust]
    tween_start: Option<f64>,
    /// A snapshot of the map as it looks right now, taken when the filter
    /// changes, consumed by the next relayout to aim the tween.
    #[rust]
    tween_capture: Option<Vec<(Cell, MapRect, f64)>>,
    /// True when the pending capture was taken for a settle the *camera*
    /// asked for. The picture on screen is already true then — the layout is
    /// merely catching up — so detail the refresh brings in must simply be
    /// there, not fade in at the user; a zoom is not data appearing. Filter
    /// edits and scan changes keep the arrival ceremony.
    #[rust]
    tween_calm: bool,
    /// Bumped by every relayout; keys the frozen labels to their layout.
    #[rust]
    layout_rev: u64,
    /// The flat map's printed names, derived once per relayout against the
    /// settled rects and merely translated while a gesture is in flight.
    #[rust]
    frozen_labels: Vec<FrozenLabel>,
    /// Which relayout `frozen_labels` was derived from.
    #[rust]
    frozen_rev: Option<u64>,

    #[rust]
    generation: u64,
    #[rust]
    cancel: Option<Arc<AtomicBool>>,
    #[rust]
    scanning: bool,
    /// Folders the walk has not opened yet. A scan cannot know its own
    /// denominator before it has walked the tree, so this is a count, not a
    /// percentage — and unlike a percentage it is true.
    #[rust]
    folders_left: u32,
    /// When the numbers on screen were measured, in seconds since the epoch.
    /// A cached map is only safe to show if it says how old it is.
    #[rust]
    scanned_at: u64,
    /// Folders the scan was refused, named so the total's shortfall is
    /// admitted rather than hidden.
    #[rust]
    denied: Vec<String>,
    /// Where "rescan" was drawn, for the click that starts one.
    #[rust]
    rescan_hit: Rect,
    #[rust]
    error: Option<String>,

    #[rust]
    sender: Option<Sender<ScanMessage>>,
    #[rust]
    receiver: Option<Receiver<ScanMessage>>,

    #[rust]
    hover: Option<usize>,
    #[rust]
    pick: Option<Pick>,
    /// Where each breadcrumb segment was drawn, and how many names of `zoom`
    /// it stands for.
    #[rust]
    crumbs: Vec<CrumbHit>,
}

/// One clickable breadcrumb segment.
#[derive(Clone, Copy)]
struct CrumbHit {
    rect: Rect,
    depth: usize,
}

/// A wheel zoom in flight: eased toward `target` a frame at a time, always
/// about the same map-ground `anchor`, so the point under the cursor stays
/// put for the whole glide. Each further wheel step just retargets it.
#[derive(Clone, Copy)]
struct ZoomGlide {
    target: f64,
    anchor: DVec2,
    last: f64,
}

/// How fast a glide closes on its target: the ease-out's time constant.
/// 45ms settles ~95% of the way in ~135ms — smooth, never floaty.
/// How far past the layout's own scale the camera may zoom IN before the
/// map is worth re-laying-out. Inside this band the detail floor is at most
/// this factor coarser than ideal — imperceptible — and keeping the cells
/// in hand keeps the arrangement rock steady.
const DETAIL_SLACK: f64 = 1.3;
const GLIDE_TAU: f64 = 0.045;
/// How often a long, still-running camera gesture may refresh the layout
/// underneath itself. Coarse on purpose: between refreshes the picture rides
/// a rigid remap of the last layout — visually constant by construction —
/// and each refresh arrives as a morph, never a per-frame reshuffle.
const MOTION_RELAYOUT: Duration = Duration::from_millis(150);
/// How far past the panel the flat cull reaches, as a fraction of the panel
/// per side: the slack that lets a pan or an out-zoom ride the remap without
/// exposing unlaid ground before the next refresh.
const MOTION_CULL_PAD: f64 = 0.25;

/// A press waiting to learn whether it is a click or its button's drag
/// gesture: primary orbits (pans, on the flat map), secondary pans.
#[derive(Clone, Copy)]
struct Drag {
    from: DVec2,
    cam_off: DVec2,
    yaw: f64,
    pitch: f64,
    taps: u32,
    secondary: bool,
    /// Crossed the threshold: this press is a gesture now and will never be
    /// a click, however close to `from` it releases.
    moved: bool,
}

/// The frozen trigonometry of the orbit camera for one frame: the map plane
/// spun by yaw about `pivot`, tilted by pitch, and — in perspective — pushed
/// through an eye [`PERSP_EYE`] points up the view axis. The flat map is the
/// same camera at yaw 0, pitch 0, which projects to the identity.
#[derive(Clone, Copy)]
struct Cam {
    pivot: DVec2,
    sin_yaw: f64,
    cos_yaw: f64,
    sin_pitch: f64,
    cos_pitch: f64,
    persp: bool,
}

impl Cam {
    /// The layout point `p` at elevation `z`, on screen.
    fn project(&self, p: DVec2, z: f64) -> DVec2 {
        let dx = p.x - self.pivot.x;
        let dy = p.y - self.pivot.y;
        let xr = dx * self.cos_yaw - dy * self.sin_yaw;
        let yr = dx * self.sin_yaw + dy * self.cos_yaw;
        let vx = xr;
        let vy = yr * self.cos_pitch - z * self.sin_pitch;
        if !self.persp {
            return dvec2(self.pivot.x + vx, self.pivot.y + vy);
        }
        let depth = yr * self.sin_pitch + z * self.cos_pitch;
        let s = (PERSP_EYE / (PERSP_EYE - depth)).clamp(0.5, 2.5);
        dvec2(self.pivot.x + vx * s, self.pivot.y + vy * s)
    }

    /// The ground point (z = 0) that projects to screen point `s` — the
    /// exact inverse of [`Cam::project`], for both projections.
    fn unproject_ground(&self, s: DVec2) -> DVec2 {
        self.unproject_at(s, 0.0)
    }

    /// The point on the plane at elevation `z` that projects to screen
    /// point `s`. The cursor in a raised projection rests on a tile *top*,
    /// not the ground behind it — anchoring a zoom at z = 0 under a tall
    /// tower drifts by the tower's own parallax.
    fn unproject_at(&self, s: DVec2, z: f64) -> DVec2 {
        let sx = s.x - self.pivot.x;
        let sy = s.y - self.pivot.y;
        let (xr, yr);
        if !self.persp {
            // sy = yr·cosφ − z·sinφ.
            yr = if self.cos_pitch.abs() < 1e-4 {
                0.0
            } else {
                (sy + z * self.sin_pitch) / self.cos_pitch
            };
            xr = sx;
        } else {
            // vy·s = sy with s = E/(E − yr·sinφ − z·cosφ) and
            // vy = yr·cosφ − z·sinφ is linear in yr once multiplied out.
            let denom = PERSP_EYE * self.cos_pitch + sy * self.sin_pitch;
            yr = if denom.abs() < 1e-6 {
                0.0
            } else {
                (sy * PERSP_EYE - z * (sy * self.cos_pitch - PERSP_EYE * self.sin_pitch))
                    / denom
            };
            let sc = (PERSP_EYE / (PERSP_EYE - yr * self.sin_pitch - z * self.cos_pitch))
                .clamp(0.5, 2.5);
            xr = sx / sc;
        }
        let dx = xr * self.cos_yaw + yr * self.sin_yaw;
        let dy = -xr * self.sin_yaw + yr * self.cos_yaw;
        dvec2(self.pivot.x + dx, self.pivot.y + dy)
    }
}

/// One projected face: four screen corners, top-left first, clockwise.
#[derive(Clone, Copy)]
struct Quad {
    p: [DVec2; 4],
}

impl Quad {
    fn of_rect(cam: &Cam, r: &MapRect, z: f64) -> Quad {
        Quad {
            p: [
                cam.project(dvec2(r.x, r.y), z),
                cam.project(dvec2(r.x + r.w, r.y), z),
                cam.project(dvec2(r.x + r.w, r.y + r.h), z),
                cam.project(dvec2(r.x, r.y + r.h), z),
            ],
        }
    }

    fn bounds(&self) -> Rect {
        let mut min = self.p[0];
        let mut max = self.p[0];
        for p in &self.p[1..] {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
        Rect { pos: min, size: max - min }
    }

    /// Whether `at` is inside this (convex) face, either winding.
    fn contains(&self, at: DVec2) -> bool {
        let mut sign = 0.0f64;
        for i in 0..4 {
            let a = self.p[i];
            let b = self.p[(i + 1) % 4];
            let cross = (b.x - a.x) * (at.y - a.y) - (b.y - a.y) * (at.x - a.x);
            if cross.abs() < 1e-9 {
                continue;
            }
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
        sign != 0.0
    }
}

/// How the map is projected onto the panel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MapProjection {
    /// The flat map — exactly the 2D treemap.
    #[default]
    Flat,
    /// 2.5D: every cell extrudes straight up by its nesting depth, showing a
    /// darker riser below its plate. Deep tangles read as towers.
    Ortho,
    /// The same prisms through a gentle straight-down perspective: higher
    /// plates swell and lean away from the middle of the panel.
    Persp,
}

/// Where a cell was when a filter tween started, so it can glide to where it
/// is now.
struct TweenFrom {
    rect: MapRect,
    depth: f64,
}

impl TreemapView {
    /// The folder the map is currently of.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder the map is zoomed into — the root itself when it is not.
    pub fn focus_path(&self) -> PathBuf {
        let mut path = self.root.clone();
        for name in &self.zoom {
            path.push(name);
        }
        path
    }

    /// The node the map is drawing, following the zoom as far as it still
    /// resolves.
    fn focused(&self) -> &Node {
        let mut node = &self.tree;
        for name in &self.zoom {
            match node.child_named(name) {
                Some(next) => node = next,
                None => break,
            }
        }
        node
    }

    /// Map `path`, from the saved map when there is one. A scan already
    /// running for another folder is cancelled first — the user asked for
    /// this folder, not that one.
    pub fn set_root(&mut self, cx: &mut Cx, path: &Path) {
        // Asking for the folder already on screen is not a request to measure
        // it again. The browser re-lists its folder after every operation, and
        // the map it just corrected by arithmetic must survive that — throwing
        // it away would undo the whole point of keeping one.
        if path == self.root && !self.tree.children.is_empty() && self.error.is_none() {
            return;
        }
        self.begin(cx, path, false);
    }

    /// Re-open the current root under whatever the scan rules now say —
    /// the scope checkbox's move. The saved map for the *new* scope is
    /// welcome (that is what makes flipping back instant); the tree in hand
    /// was measured under the old rules and is not.
    pub fn remap(&mut self, cx: &mut Cx) {
        let root = self.root.clone();
        if root.as_os_str().is_empty() {
            return;
        }
        self.begin(cx, &root, false);
    }

    /// Measure the disk again and replace the saved map, whatever its age.
    /// The one thing that makes a cached map safe to trust: it is never more
    /// than a keystroke away from being made true.
    pub fn rescan(&mut self, cx: &mut Cx) {
        let root = self.root.clone();
        if root.as_os_str().is_empty() {
            return;
        }
        crate::sizecache::forget(&root);
        self.begin(cx, &root, true);
    }

    fn begin(&mut self, cx: &mut Cx, path: &Path, fresh: bool) {
        if self.sender.is_none() {
            let (sender, receiver) = channel();
            self.sender = Some(sender);
            self.receiver = Some(receiver);
        }
        self.stop(cx);
        // Re-measuring the folder already on screen is not a reason to lose
        // what the user had picked: the selection is a path, and the path is
        // as true after the rescan as before it. A different folder is a
        // different picture, and there the old pick would be a lie.
        let keep_pick = if path == self.root { self.pick.take() } else { None };
        self.root = path.to_path_buf();
        self.zoom.clear();
        self.tree = Node::dir(crate::model::display_name(path), FileKind::Folder as u8);
        self.cells.clear();
        self.laid_out = Rect::default();
        self.stale = true;
        self.last_layout = None;
        self.hover = None;
        self.pick = keep_pick;
        self.error = None;
        self.folders_left = 0;
        self.scanned_at = 0;
        self.scanning = true;
        self.cam_scale = 1.0;
        self.cam_off = DVec2::default();
        self.yaw = 0.0;
        self.pitch = DEFAULT_PITCH;
        // The camera rests: the remap is the identity until the first layout
        // of the new map records itself here.
        self.layout_scale = 1.0;
        self.layout_off = DVec2::default();
        self.layout_yaw = 0.0;
        self.layout_pitch = DEFAULT_PITCH;
        self.drag = None;
        self.zoom_glide = None;
        self.yaw_glide = None;
        self.filtered = None;
        self.totals_dirty = true;
        self.tree_rev = self.tree_rev.wrapping_add(1);
        self.tween_capture = None;
        self.tween_calm = false;
        self.tween_start = None;
        self.tween_from.clear();
        self.tween_leavers.clear();
        self.frozen_labels.clear();
        self.frozen_rev = None;

        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let Some(sender) = self.sender.clone() else {
            return;
        };
        let root = self.root.clone();
        let instant = crate::vfs::vfs().is_instant();
        let scan = move || {
            // The four scan threads all report through here, so the channel
            // and the signal clock live behind one lock. Waking the UI is the
            // expensive half and is what gets rate-limited; the steps
            // themselves queue as fast as the disk produces them.
            let gate = Mutex::new(Cx::monotonic_now());
            let sink = |step: ScanStep| {
                if sender
                    .send(ScanMessage {
                        generation,
                        step: Some(step),
                        finished: None,
                    })
                    .is_err()
                {
                    return;
                }
                let mut due = gate.lock().unwrap_or_else(|e| e.into_inner());
                let now = Cx::monotonic_now();
                if now >= *due {
                    *due = now + SIGNAL_EVERY.as_secs_f64();
                    SignalToUI::set_ui_signal();
                }
            };
            // The saved map first, and off the UI thread: decoding a home
            // directory's worth of tree is a tenth of a second of work that
            // has no business happening between two frames.
            let cached = if fresh || crate::vfs::is_demo() {
                None
            } else {
                crate::sizecache::load(&root)
            };
            if let Some(cached) = cached {
                let _ = sender.send(ScanMessage {
                    generation,
                    step: Some(ScanStep::Closed {
                        at: Vec::new(),
                        node: cached.tree,
                    }),
                    finished: None,
                });
                let _ = sender.send(ScanMessage {
                    generation,
                    step: None,
                    finished: Some(Outcome::Loaded {
                        scanned_at: cached.scanned_at,
                    }),
                });
                SignalToUI::set_ui_signal();
                return;
            }
            let ok = crate::vfs::vfs().scan_stream(&root, &cancel, &sink);
            let _ = sender.send(ScanMessage {
                generation,
                step: None,
                finished: Some(if ok { Outcome::Scanned } else { Outcome::Failed }),
            });
            SignalToUI::set_ui_signal();
        };
        if instant {
            scan();
            self.drain(cx);
        } else {
            thread::spawn(scan);
        }
        self.redraw(cx);
    }

    /// Stop whatever scan is running. Called when the view is left, when the
    /// folder changes, and when the window goes away.
    pub fn stop(&mut self, cx: &mut Cx) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if self.scanning {
            self.scanning = false;
            self.redraw(cx);
        }
    }

    /// The status line for the map: what it is showing, or how far the scan
    /// has got, and what the last click landed on.
    pub fn status(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        let node = self.focused();
        let where_it_is = self
            .focus_path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.focus_path().display().to_string());
        if self.scanning {
            return format!(
                "Scanning {where_it_is} — {} files · {} so far · {} folder{} still open",
                self.tree.files,
                treemap::format_bytes(self.tree.size),
                self.folders_left,
                if self.folders_left == 1 { "" } else { "s" },
            );
        }
        let picked = match &self.pick {
            Some(pick) => format!(" · picked {}", crate::model::display_name(&pick.path)),
            None => String::new(),
        };
        format!(
            "{where_it_is} — {} in {} files · scroll zooms, drag pans, Esc backs out{picked}",
            treemap::format_bytes(node.size),
            node.files,
        )
    }

    /// Take everything the worker sent. True when the view needs a redraw.
    pub fn drain(&mut self, cx: &mut Cx) -> bool {
        let messages: Vec<ScanMessage> = self
            .receiver
            .as_ref()
            .map(|r| r.try_iter().collect())
            .unwrap_or_default();
        if messages.is_empty() {
            return false;
        }
        let mut finished = false;
        for message in messages {
            if message.generation != self.generation {
                continue;
            }
            if let Some(step) = message.step {
                if let ScanStep::Pace { folders_left } = &step {
                    // Cheap and constant: no tree walk, just the walk's own
                    // count of folders it has not opened yet.
                    self.folders_left = *folders_left;
                    continue;
                }
                self.tree.apply(step);
                self.stale = true;
                self.totals_dirty = true;
                self.tree_rev = self.tree_rev.wrapping_add(1);
            }
            if let Some(outcome) = message.finished {
                self.scanning = false;
                self.cancel = None;
                self.stale = true;
                self.folders_left = 0;
                finished = true;
                // Nothing is growing any more, so nothing is still pending.
                self.tree.seal();
                self.denied = self.tree.denied_paths(4);
                match outcome {
                    Outcome::Scanned => {
                        self.scanned_at = crate::sizecache::now();
                        self.save_cache();
                    }
                    Outcome::Loaded { scanned_at } => self.scanned_at = scanned_at,
                    Outcome::Failed => {
                        if self.tree.children.is_empty() {
                            self.error = Some(format!(
                                "Could not map {}",
                                crate::model::display_name(&self.root)
                            ));
                        }
                    }
                }
            }
        }
        // While the walk is running the tree changes far faster than the
        // picture needs to; a finished scan always redraws at once.
        if finished || self.layout_is_due(cx.seconds_since_app_start()) {
            self.redraw(cx);
        } else {
            // Nothing gets lost: the trailing update is picked up on the next
            // frame, once the throttle has expired.
            self.frame = cx.new_next_frame();
        }
        true
    }

    /// Whether the picture may be rebuilt now. The throttle exists only to
    /// keep a running scan from re-laying out the map hundreds of times a
    /// second; once nothing is feeding it any more there is nothing to
    /// throttle, and a map still showing a mid-scan snapshot after the walk
    /// has finished would be quietly, plausibly wrong.
    fn layout_is_due(&self, now: f64) -> bool {
        if !self.scanning {
            return true;
        }
        match self.last_layout {
            Some(at) => now - at >= RELAYOUT_EVERY.as_secs_f64(),
            None => true,
        }
    }

    /// Which path is highlighted on the map.
    pub fn set_selected(&mut self, cx: &mut Cx, path: Option<PathBuf>) {
        let same = match (&self.pick, &path) {
            (Some(pick), Some(path)) => &pick.path == path,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.pick = path.map(|path| Pick {
            path,
            size: 0,
            files: 0,
            is_dir: false,
            bundle: 0,
        });
        // The real numbers come from the cell when there is one, so a reveal
        // from the list view reads the same as a click on the map.
        if let Some(pick) = &self.pick {
            if let Some(cell) = self.cells.iter().find(|c| c.path == pick.path) {
                self.pick = Some(pick_of(cell));
            }
        }
        self.redraw(cx);
    }

    /// The path the last click landed on.
    pub fn selection(&self) -> Option<PathBuf> {
        self.pick.as_ref().map(|p| p.path.clone())
    }

    /// Step the view back out. The camera first — Esc un-orbits and un-zooms
    /// what the eye did before it re-roots what a reveal did. False when
    /// there is nowhere left to go.
    pub fn zoom_out(&mut self, cx: &mut Cx) -> bool {
        // Whatever is still gliding stops where Esc found it.
        self.zoom_glide = None;
        self.yaw_glide = None;
        if self.projection != MapProjection::Flat
            && (self.yaw.abs() > 0.01 || (self.pitch - DEFAULT_PITCH).abs() > 0.01)
        {
            self.set_orbit(cx, 0.0, DEFAULT_PITCH);
            return true;
        }
        if self.cam_scale > 1.001 {
            self.set_camera(cx, 1.0, DVec2::default());
            return true;
        }
        if self.zoom.pop().is_none() {
            return false;
        }
        self.after_zoom(cx);
        true
    }

    /// Zoom to `depth` names deep, for a breadcrumb click.
    fn zoom_to(&mut self, cx: &mut Cx, depth: usize) {
        if depth >= self.zoom.len() {
            return;
        }
        self.zoom.truncate(depth);
        self.after_zoom(cx);
    }

    /// Zoom into `path`, which must be under the mapped folder. False when it
    /// is not, or is not a folder the scan knows about.
    pub fn zoom_into(&mut self, cx: &mut Cx, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        let names: Vec<String> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if names.is_empty() {
            return false;
        }
        let mut node = &self.tree;
        for name in &names {
            match node.child_named(name) {
                Some(next) if next.is_dir => node = next,
                _ => return false,
            }
        }
        self.zoom = names;
        self.after_zoom(cx);
        true
    }

    fn after_zoom(&mut self, cx: &mut Cx) {
        self.hover = None;
        self.stale = true;
        self.last_layout = None;
        self.laid_out = Rect::default();
        // A re-root measures a different subtree — the filter cache must not
        // outlive the folder it was measured against.
        self.tree_rev = self.tree_rev.wrapping_add(1);
        // A re-root is a new picture; the camera starts over on it.
        self.cam_scale = 1.0;
        self.cam_off = DVec2::default();
        self.zoom_glide = None;
        self.yaw_glide = None;
        self.yaw = 0.0;
        self.pitch = DEFAULT_PITCH;
        self.layout_scale = 1.0;
        self.layout_off = DVec2::default();
        self.layout_yaw = 0.0;
        self.layout_pitch = DEFAULT_PITCH;
        self.redraw(cx);
    }

    // ------------------------------------------------------------- camera

    /// The rigid ride from where `cells` were laid out to where the camera
    /// is now: `screen = laid·k + b`. Identity whenever the camera rests on
    /// its own layout.
    fn cam_remap(&self) -> (f64, DVec2) {
        remap_params(
            self.laid_out,
            self.layout_scale,
            self.layout_off,
            self.cam_scale,
            self.cam_off,
        )
    }


    /// Whether a camera gesture still owns the frame. While one does, camera
    /// changes ride the remap and never relayout — that is the visual
    /// constancy the whole scheme exists for.
    fn cam_in_motion(&self) -> bool {
        self.zoom_glide.is_some()
            || self.yaw_glide.is_some()
            || self.drag.map_or(false, |d| d.moved)
    }

    /// Whether the layout no longer honestly covers what the camera shows —
    /// the only reason a camera move is ever allowed to remake the map.
    ///
    /// This is deliberately a wide band, not an equality test. The layout is
    /// not scale-invariant (the bundle floor moves with area, insets are
    /// fixed point sizes), so *any* relayout at a slightly different camera
    /// repacks groups and reads as tiles randomly reordering. Within the
    /// band the rigid remap of the cells in hand is visually
    /// indistinguishable from a fresh layout — so the fresh layout is not
    /// bought. Spent means: zoomed in past what the layout resolves
    /// ([`DETAIL_SLACK`]), or looking at ground outside the laid cull.
    fn layout_spent(&self) -> bool {
        // Spent cuts both ways: zoomed IN past the layout's detail floor,
        // or zoomed OUT so far the layout in hand is a microscopic patch —
        // a one-sided test here is how a zoom-out once froze the old
        // zoomed-in layout on screen forever as wrong-scale ghost plates.
        let ratio = self.cam_scale.max(1.0) / self.layout_scale.max(1.0);
        if ratio > DETAIL_SLACK || ratio < 1.0 / DETAIL_SLACK {
            return true;
        }
        self.view_escaped_cull()
    }

    /// The hard half of [`Self::layout_spent`]: the live view is showing
    /// ground the layout never laid. Spent-for-detail can wait for the
    /// motion cadence — the picture is merely coarse; escaped cannot wait
    /// for anything, because what it shows is *bare background*.
    fn view_escaped_cull(&self) -> bool {
        let body = self.laid_out;
        if body.size.x <= 0.0 || self.laid_cull.w <= 0.0 {
            return true;
        }
        // What the live camera can see, in the layout's own ground space.
        let corners = [
            body.pos,
            dvec2(body.pos.x + body.size.x, body.pos.y),
            dvec2(body.pos.x + body.size.x, body.pos.y + body.size.y),
            dvec2(body.pos.x, body.pos.y + body.size.y),
        ];
        let mut min = dvec2(f64::MAX, f64::MAX);
        let mut max = dvec2(f64::MIN, f64::MIN);
        match self.projection {
            MapProjection::Flat => {
                let (k, b) = self.cam_remap();
                for corner in corners {
                    let g = dvec2((corner.x - b.x) / k, (corner.y - b.y) / k);
                    min.x = min.x.min(g.x);
                    min.y = min.y.min(g.y);
                    max.x = max.x.max(g.x);
                    max.y = max.y.max(g.y);
                }
            }
            _ => {
                // The draw path is screen = Cam(remap(ground)): undoing the
                // projection alone leaves the point in the LIVE camera's
                // frame, and the cull lives in the LAYOUT's. Skipping the
                // second inverse is how a raised-mode zoom-out once compared
                // a body-sized live footprint against a giant stale cull,
                // never noticed the escape, and froze ghosts on screen.
                let cam = self.cam_at(body);
                let (k, b) = self.cam_remap();
                for corner in corners {
                    let live = cam.unproject_ground(corner);
                    let g = dvec2((live.x - b.x) / k, (live.y - b.y) / k);
                    min.x = min.x.min(g.x);
                    min.y = min.y.min(g.y);
                    max.x = max.x.max(g.x);
                    max.y = max.y.max(g.y);
                }
                // The cull was a rotation-proof square around the *layout*
                // camera's footprint plus the lean reach; the live footprint
                // needs that same reach to stay honestly inside.
                let reach = self.elev(24) + 40.0;
                min.x -= reach;
                min.y -= reach;
                max.x += reach;
                max.y += reach;
            }
        }
        min.x < self.laid_cull.x
            || min.y < self.laid_cull.y
            || max.x > self.laid_cull.x + self.laid_cull.w
            || max.y > self.laid_cull.y + self.laid_cull.h
    }

    /// Lay the map out at the camera's resting place, morphing there from
    /// wherever the picture visually stands. A no-op when the layout in hand
    /// still covers the view — which is exactly what keeps a small zoom or
    /// pan visually constant end to end.
    fn settle(&mut self, cx: &mut Cx) {
        if self.tree.children.is_empty() || (!self.stale && !self.layout_spent()) {
            return;
        }
        if self.tween_capture.is_none() {
            // Calm unless the tree itself changed underneath the gesture —
            // a camera settle re-derives the same picture at more detail.
            self.tween_calm = !self.stale;
            self.tween_capture = Some(self.visual_snapshot(cx.seconds_since_app_start()));
        }
        self.stale = true;
        self.last_layout = None;
        self.redraw(cx);
    }

    /// Mid-gesture, whether the coarse layout refresh may run: something to
    /// refresh — the layout spent, or the tree changed under the scan — and
    /// the cadence has passed.
    fn motion_refresh_due(&self, now: f64) -> bool {
        if !self.layout_spent() && !self.stale {
            return false;
        }
        match self.last_layout {
            Some(at) => now - at >= MOTION_RELAYOUT.as_secs_f64(),
            None => true,
        }
    }

    /// Move the camera. Mid-gesture the picture rides the remap — one rigid
    /// sheet, nothing re-flows — and the layout catches up when the motion
    /// settles or on the coarse mid-motion cadence, arriving as a morph. A
    /// discrete jump (a double-click fit, Esc) settles at once, so the new
    /// detail — bundles dissolving into the things they stood for — morphs
    /// in rather than popping.
    fn set_camera(&mut self, cx: &mut Cx, scale: f64, off: DVec2) {
        let body = self.laid_out;
        let scale = scale.clamp(1.0, 512.0);
        let off = dvec2(
            off.x.clamp(0.0, (body.size.x * (scale - 1.0)).max(0.0)),
            off.y.clamp(0.0, (body.size.y * (scale - 1.0)).max(0.0)),
        );
        if (scale - self.cam_scale).abs() < 1e-9 && (off - self.cam_off).length() < 1e-9 {
            return;
        }
        self.cam_scale = scale;
        self.cam_off = off;
        self.hover = None;
        if !self.cam_in_motion() {
            self.settle(cx);
        }
        self.redraw(cx);
    }

    /// Zoom by `factor`, keeping the map point under `at` exactly where it
    /// is — the anchor rule every map application follows.
    fn zoom_at(&mut self, cx: &mut Cx, at: DVec2, factor: f64) {
        let body = self.laid_out;
        if body.size.x <= 0.0 || body.size.y <= 0.0 {
            return;
        }
        let old = self.cam_scale.max(1.0);
        let new = (old * factor).clamp(1.0, 512.0);
        let factor = new / old;
        let anchor = at - body.pos;
        self.set_camera(
            cx,
            new,
            dvec2(
                (self.cam_off.x + anchor.x) * factor - anchor.x,
                (self.cam_off.y + anchor.y) * factor - anchor.y,
            ),
        );
    }

    // -------------------------------------------------- projection & filter

    /// One nesting level's worth of elevation, in on-screen points. Grows
    /// with the square root of the camera so towers stay proud when zoomed
    /// without ever dwarfing the tiles.
    fn rise(&self) -> f64 {
        RISE * self.cam_scale.max(1.0).sqrt()
    }

    /// The elevation of a plate at `depth`. The top level sits on the floor
    /// — exactly where the flat map has it — and every nesting level steps
    /// up one rise from there.
    fn elev(&self, depth: usize) -> f64 {
        depth.min(24) as f64 * self.rise()
    }

    fn elev_f(&self, depth: f64) -> f64 {
        depth.min(24.0) * self.rise()
    }

    /// The orbit camera for a map drawn into `body`. The flat projection is
    /// the same camera pinned straight down and un-spun, which makes it the
    /// identity — one code path for all three.
    fn cam_at(&self, body: Rect) -> Cam {
        let (yaw, pitch) = match self.projection {
            MapProjection::Flat => (0.0, 0.0),
            _ => (self.yaw, self.pitch),
        };
        Cam {
            pivot: dvec2(
                body.pos.x + body.size.x * 0.5,
                body.pos.y + body.size.y * 0.5,
            ),
            sin_yaw: yaw.sin(),
            cos_yaw: yaw.cos(),
            sin_pitch: pitch.sin(),
            cos_pitch: pitch.cos(),
            persp: self.projection == MapProjection::Persp,
        }
    }

    /// The layout-space direction a raised prism drifts in as it gains
    /// elevation — where towers lean, and therefore what the painter's
    /// order must follow. Screen-up, un-spun by the yaw.
    fn lean(&self) -> DVec2 {
        dvec2(-self.yaw.sin(), -self.yaw.cos())
    }

    /// Point the orbit somewhere. The cells stay put — only the projection
    /// of them moves — so no relayout mid-gesture; the paint order alone
    /// must follow the new lean at once, or towers overlap wrongly the very
    /// frame the yaw crosses a quadrant.
    fn set_orbit(&mut self, cx: &mut Cx, yaw: f64, pitch: f64) {
        let yaw = wrap_angle(yaw);
        let pitch = pitch.clamp(0.0, MAX_PITCH);
        if (yaw - self.yaw).abs() < 1e-9 && (pitch - self.pitch).abs() < 1e-9 {
            return;
        }
        self.yaw = yaw;
        self.pitch = pitch;
        self.hover = None;
        self.paint_order = match self.projection {
            MapProjection::Flat => Vec::new(),
            _ => view_order(&self.cells, self.lean()),
        };
        if !self.cam_in_motion() {
            self.settle(cx);
        }
        self.redraw(cx);
    }

    /// Nudge the orbit — the keyboard's Q/E. A yaw-only nudge glides there
    /// rather than snapping, and a second tap mid-glide just aims further.
    pub fn orbit_by(&mut self, cx: &mut Cx, dyaw: f64, dpitch: f64) {
        if self.projection == MapProjection::Flat {
            return;
        }
        if dpitch == 0.0 {
            let base = self.yaw_glide.map_or(self.yaw, |(target, _)| target);
            self.yaw_glide = Some((wrap_angle(base + dyaw), cx.seconds_since_app_start()));
            self.frame = cx.new_next_frame();
            return;
        }
        self.set_orbit(cx, self.yaw + dyaw, self.pitch + dpitch);
    }

    /// One frame of whichever glides are running: ease toward the target,
    /// keep the frame clock alive until both arrive.
    fn step_glides(&mut self, cx: &mut Cx) {
        if let Some(mut glide) = self.zoom_glide.take() {
            let now = cx.seconds_since_app_start();
            let dt = (now - glide.last).clamp(0.0, 0.1);
            glide.last = now;
            let current = self.cam_scale.max(1.0);
            // Zoom lives in ratio space: equal glide time closes an equal
            // *proportion* of the remaining ratio, in or out alike.
            let remaining = (glide.target / current).ln();
            if remaining.abs() < 0.002 {
                // Arrived: the last step runs un-glided, and the layout
                // settles under wherever the ride ended.
                self.zoom_at(cx, glide.anchor, glide.target / current);
                self.settle(cx);
            } else {
                let k = 1.0 - (-dt / GLIDE_TAU).exp();
                // Restored before the step, so the camera change knows a
                // glide still owns it and rides the remap.
                self.zoom_glide = Some(glide);
                self.zoom_at(cx, glide.anchor, (remaining * k).exp());
                self.frame = cx.new_next_frame();
            }
        }
        if let Some((target, last)) = self.yaw_glide.take() {
            let now = cx.seconds_since_app_start();
            let dt = (now - last).clamp(0.0, 0.1);
            let remaining = wrap_angle(target - self.yaw);
            if remaining.abs() < 0.002 {
                self.set_orbit(cx, target, self.pitch);
                self.settle(cx);
            } else {
                let k = 1.0 - (-dt / GLIDE_TAU).exp();
                self.yaw_glide = Some((target, now));
                self.set_orbit(cx, self.yaw + remaining * k, self.pitch);
                self.frame = cx.new_next_frame();
            }
        }
    }

    /// Change how the map projects. The layout itself never changes — only
    /// what is done with it on the way to the screen.
    pub fn set_projection(&mut self, cx: &mut Cx, projection: MapProjection) {
        if self.projection == projection {
            return;
        }
        self.projection = projection;
        if self.pitch <= 0.0 {
            self.pitch = DEFAULT_PITCH;
        }
        self.hover = None;
        self.stale = true;
        self.last_layout = None;
        self.redraw(cx);
    }

    /// Apply (or clear) the live filter, morphing from the picture on screen.
    pub fn set_filter(&mut self, cx: &mut Cx, filter: Option<Query>) {
        let filter = filter.filter(|q| !q.is_empty());
        if filter == self.filter {
            return;
        }
        // Aim the tween from wherever things visually are right now — a
        // slider mid-drag retargets smoothly instead of jumping.
        self.tween_capture = Some(self.visual_snapshot(cx.seconds_since_app_start()));
        self.tween_calm = false;
        self.filter = filter;
        self.stale = true;
        self.last_layout = None;
        self.hover = None;
        self.redraw(cx);
    }

    /// Whether a filter is active, and what it matched: (bytes, files).
    pub fn filter_matched(&self) -> Option<(u64, u32)> {
        self.filter.as_ref()?;
        self.filtered
    }

    /// Byte totals per kind tag under the mapped folder — the legend's
    /// numbers. Recounted only after the tree actually changed.
    pub fn kind_totals(&mut self) -> [u64; 16] {
        if self.totals_dirty {
            self.totals = treemap::kind_totals(&self.tree);
            self.totals_dirty = false;
        }
        self.totals
    }

    /// Eased tween progress, or None when nothing is morphing.
    fn tween_t(&self, now: f64) -> Option<f64> {
        let start = self.tween_start?;
        let t = (now - start).max(0.0) / TWEEN.as_secs_f64();
        if t >= 1.0 {
            return None;
        }
        // Smoothstep: no snap at either end.
        Some(t * t * (3.0 - 2.0 * t))
    }

    /// Every cell's current on-screen truth — the rect it is visually at,
    /// mid-tween and mid-gesture alike, and its fractional depth — plus the
    /// leavers still fading out. Remapped through the live camera, so a
    /// tween aimed from here starts exactly where the eye left off.
    fn visual_snapshot(&self, now: f64) -> Vec<(Cell, MapRect, f64)> {
        let t = self.tween_t(now);
        let (rk, rb) = self.cam_remap();
        let mut out: Vec<(Cell, MapRect, f64)> = Vec::with_capacity(self.cells.len());
        for cell in &self.cells {
            let (rect, depth, alive) = self.tweened(cell, t);
            if alive > 0.0 {
                out.push((cell.clone(), remap_rect(&rect, rk, rb), depth));
            }
        }
        if let Some(t) = t {
            for (cell, rect, _) in &self.tween_leavers {
                if 1.0 - t > 0.05 {
                    out.push((cell.clone(), remap_rect(rect, rk, rb), cell.depth as f64));
                }
            }
        }
        out
    }

    /// Where `cell` is right now: (layout rect, fractional depth, alpha).
    fn tweened(&self, cell: &Cell, t: Option<f64>) -> (MapRect, f64, f64) {
        let Some(t) = t else {
            return (cell.rect, cell.depth as f64, 1.0);
        };
        match self.tween_from.get(&cell.path) {
            Some(from) => (
                lerp_rect(&from.rect, &cell.rect, t),
                from.depth + (cell.depth as f64 - from.depth) * t,
                1.0,
            ),
            None => {
                // An arriver: grows out of its own footprint.
                let grown = 0.7 + 0.3 * t;
                let rect = MapRect {
                    x: cell.rect.x + cell.rect.w * (1.0 - grown) * 0.5,
                    y: cell.rect.y + cell.rect.h * (1.0 - grown) * 0.5,
                    w: cell.rect.w * grown,
                    h: cell.rect.h * grown,
                };
                (rect, cell.depth as f64, t)
            }
        }
    }

    /// Fill the panel with `rect` — what a double-click means: go look at
    /// this one, without re-rooting anything.
    fn fit_rect(&mut self, cx: &mut Cx, rect: Rect) {
        let body = self.laid_out;
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 || body.size.x <= 0.0 {
            return;
        }
        let old = self.cam_scale.max(1.0);
        let fit = (body.size.x / rect.size.x).min(body.size.y / rect.size.y) * 0.94;
        let new = (old * fit).clamp(1.0, 512.0);
        let factor = new / old;
        let pos = dvec2(
            (rect.pos.x - body.pos.x + self.cam_off.x) * factor,
            (rect.pos.y - body.pos.y + self.cam_off.y) * factor,
        );
        let size = dvec2(rect.size.x * factor, rect.size.y * factor);
        self.set_camera(
            cx,
            new,
            dvec2(
                pos.x - (body.size.x - size.x) * 0.5,
                pos.y - (body.size.y - size.y) * 0.5,
            ),
        );
    }

    /// Write the finished tree out for next time. Encoding walks the whole
    /// tree so it happens here, where the tree is; the file write is somebody
    /// else's problem, on a thread nobody is waiting for.
    fn save_cache(&self) {
        if crate::vfs::is_demo() {
            return;
        }
        let Some(bytes) = crate::sizecache::encode(&self.root, &self.tree, self.scanned_at) else {
            return;
        };
        let root = self.root.clone();
        thread::spawn(move || crate::sizecache::store(&root, &bytes));
    }

    /// `path` as the chain of names between the mapped folder and it.
    fn names_of(&self, path: &Path) -> Option<Vec<String>> {
        let relative = path.strip_prefix(&self.root).ok()?;
        let names: Vec<String> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        (!names.is_empty()).then_some(names)
    }

    /// Fold a set of finished moves into the map instead of measuring the
    /// disk again. Each pair is where something was and where it went, with
    /// `None` for "it stopped existing".
    ///
    /// This is the whole reason the map is worth caching: the app already
    /// knows exactly how big the thing it just deleted was, so the picture can
    /// be made true again by arithmetic — a delete costs no disk reads at all.
    /// A scan in flight owns the tree and will produce the truth on its own,
    /// so this stays out of its way.
    pub fn absorb_moves(&mut self, cx: &mut Cx, moves: &[(PathBuf, Option<PathBuf>)]) {
        if self.scanning || self.tree.children.is_empty() {
            return;
        }
        let mut changed = false;
        for (from, to) in moves {
            let Some(names) = self.names_of(from) else {
                continue;
            };
            let Some(mut node) = self.tree.detach(&names) else {
                continue;
            };
            changed = true;
            if self.pick.as_ref().is_some_and(|p| &p.path == from) {
                self.pick = None;
            }
            // Moved rather than removed, and landed somewhere still on the
            // map: the bytes did not leave, so neither does the rectangle.
            let Some(to) = to else { continue };
            let Some(name) = to.file_name() else { continue };
            node.name = name.to_string_lossy().into_owned();
            self.graft_at(to, node);
        }
        if changed {
            self.after_change(cx);
        }
    }

    /// Fold finished copies in: the source stays where it is and a second
    /// rectangle of the same size appears at the destination.
    pub fn absorb_copies(&mut self, cx: &mut Cx, copies: &[(PathBuf, PathBuf)]) {
        if self.scanning || self.tree.children.is_empty() {
            return;
        }
        let mut changed = false;
        for (from, to) in copies {
            let Some(names) = self.names_of(from) else {
                continue;
            };
            let Some(indices) = self.names_of(to) else {
                continue;
            };
            let Some(source) = self.tree.at(&names) else {
                continue;
            };
            let mut node = source.clone();
            let Some(name) = to.file_name() else { continue };
            node.name = name.to_string_lossy().into_owned();
            let _ = indices;
            if self.graft_at(to, node) {
                changed = true;
            }
        }
        if changed {
            self.after_change(cx);
        }
    }

    /// Put `node` where `full` says, which is a path *including* the node's
    /// own name — the parent is what actually receives it.
    fn graft_at(&mut self, full: &Path, node: Node) -> bool {
        let Some(parent) = full.parent() else {
            return false;
        };
        if parent == self.root {
            return self.tree.graft(&[], node);
        }
        match self.names_of(parent) {
            Some(names) => self.tree.graft(&names, node),
            None => false,
        }
    }

    /// Drop `path` from the map because the disk says it is not there any
    /// more. Cheap, exact, and the answer to a cached map going stale one
    /// file at a time.
    pub fn forget(&mut self, cx: &mut Cx, path: &Path) {
        if self.scanning {
            return;
        }
        let Some(names) = self.names_of(path) else {
            return;
        };
        if self.tree.detach(&names).is_some() {
            if self.pick.as_ref().is_some_and(|p| p.path == path) {
                self.pick = None;
            }
            self.after_change(cx);
        }
    }

    fn after_change(&mut self, cx: &mut Cx) {
        self.hover = None;
        self.stale = true;
        self.totals_dirty = true;
        self.tree_rev = self.tree_rev.wrapping_add(1);
        self.last_layout = None;
        self.save_cache();
        self.redraw(cx);
    }

    fn relayout(&mut self, rect: Rect, now: f64) {
        let base = self.focus_path();
        // The region the *outgoing* layout covered, before it is replaced —
        // the line between a camera reveal and data actually appearing.
        let old_cull = self.laid_cull;
        let old_remap = self.cam_remap();
        // The map is laid out at the camera's magnification and culled to
        // the panel: zoomed in, the layout does the work of the pixels on
        // screen, not of the whole magnified picture.
        let scale = self.cam_scale.max(1.0);
        let area = MapRect {
            x: rect.pos.x - self.cam_off.x,
            y: rect.pos.y - self.cam_off.y,
            w: rect.size.x * scale,
            h: rect.size.y * scale,
        };
        // A glide in flight knows where it will land. The cull covers the
        // destination's footprint too, so the glide itself can never outrun
        // the layout: a violent out-zoom lays out the whole ride's ground
        // now, in this one relayout, instead of flashing bare background and
        // chasing it frame by frame. The destination offset is the glide's
        // own anchor arithmetic run to its target, under set_camera's
        // clamps. Per axis the destination view maps into this layout's
        // ground frame affinely (same yaw and pitch all glide long).
        let dest = self.zoom_glide.map(|glide| {
            let scale_c = self.cam_scale.max(1.0);
            let scale_t = glide.target.clamp(1.0, 512.0);
            let anchor = glide.anchor - rect.pos;
            let factor = scale_t / scale_c;
            let off_t = dvec2(
                ((self.cam_off.x + anchor.x) * factor - anchor.x)
                    .clamp(0.0, (rect.size.x * (scale_t - 1.0)).max(0.0)),
                ((self.cam_off.y + anchor.y) * factor - anchor.y)
                    .clamp(0.0, (rect.size.y * (scale_t - 1.0)).max(0.0)),
            );
            // ground = rect.pos - off_now + (screen - rect.pos + off_dest)
            //          * (scale_now / scale_dest), per axis.
            let r = scale_c / scale_t;
            (
                dvec2(
                    rect.pos.x - self.cam_off.x + (off_t.x) * r,
                    rect.pos.y - self.cam_off.y + (off_t.y) * r,
                ),
                r,
            )
        });
        // Map a point of the live view into where the glide's destination
        // camera will show that screen spot, in this layout's ground frame.
        let to_dest = |q: DVec2, dest: &(DVec2, f64)| {
            dvec2(
                dest.0.x + (q.x - rect.pos.x) * dest.1,
                dest.0.y + (q.y - rect.pos.y) * dest.1,
            )
        };
        // What the camera can see, on the ground plane: the panel's corners
        // un-projected, boxed, and grown by the tallest possible lean — the
        // cull has to keep whatever could spin or lean into view.
        let viewport = match self.projection {
            MapProjection::Flat => {
                // A margin past the panel, so a pan or an out-zoom rides the
                // remap without exposing unlaid ground before the next
                // refresh. Cells in the margin are laid out but skipped at
                // draw time, so they cost layout, not paint.
                let pad = dvec2(
                    rect.size.x * MOTION_CULL_PAD,
                    rect.size.y * MOTION_CULL_PAD,
                );
                let mut min = dvec2(rect.pos.x - pad.x, rect.pos.y - pad.y);
                let mut max = dvec2(
                    rect.pos.x + rect.size.x + pad.x,
                    rect.pos.y + rect.size.y + pad.y,
                );
                if let Some(dest) = &dest {
                    let a = to_dest(rect.pos, dest);
                    let b = to_dest(
                        dvec2(rect.pos.x + rect.size.x, rect.pos.y + rect.size.y),
                        dest,
                    );
                    min.x = min.x.min(a.x);
                    min.y = min.y.min(a.y);
                    max.x = max.x.max(b.x);
                    max.y = max.y.max(b.y);
                }
                MapRect {
                    x: min.x,
                    y: min.y,
                    w: max.x - min.x,
                    h: max.y - min.y,
                }
            }
            _ => {
                let cam = self.cam_at(rect);
                let corners = [
                    rect.pos,
                    dvec2(rect.pos.x + rect.size.x, rect.pos.y),
                    dvec2(rect.pos.x + rect.size.x, rect.pos.y + rect.size.y),
                    dvec2(rect.pos.x, rect.pos.y + rect.size.y),
                ];
                let mut min = dvec2(f64::MAX, f64::MAX);
                let mut max = dvec2(f64::MIN, f64::MIN);
                for corner in corners {
                    let g = cam.unproject_ground(corner);
                    min.x = min.x.min(g.x);
                    min.y = min.y.min(g.y);
                    max.x = max.x.max(g.x);
                    max.y = max.y.max(g.y);
                    if let Some(dest) = &dest {
                        // The glide's destination sees this screen corner at
                        // a different ground spot; the cull keeps both. Yaw
                        // and pitch hold still during a zoom glide, so the
                        // scale/offset affine is the whole difference.
                        let d = to_dest(g, dest);
                        min.x = min.x.min(d.x);
                        min.y = min.y.min(d.y);
                        max.x = max.x.max(d.x);
                        max.y = max.y.max(d.y);
                    }
                }
                let reach = self.elev(24) + 40.0;
                // Rotation-proof: a mid-drag orbit swings the visible
                // footprint around the pivot without a relayout, so the cull
                // is the square that covers the footprint at any yaw — its
                // centre, sides the footprint's diagonal.
                let half = ((max.x - min.x).powi(2) + (max.y - min.y).powi(2)).sqrt() * 0.5
                    + reach;
                let mid = dvec2((min.x + max.x) * 0.5, (min.y + max.y) * 0.5);
                MapRect {
                    x: mid.x - half,
                    y: mid.y - half,
                    w: half * 2.0,
                    h: half * 2.0,
                }
            }
        };
        // The filter's weights come from a measure tree cached against the
        // tree revision and the query: a camera move re-lays-out every frame
        // and must never pay for re-measuring what did not change.
        match &self.filter {
            None => {
                self.measure = None;
                self.measure_query = None;
                self.filtered = None;
            }
            Some(query) => {
                if self.measure.is_none()
                    || self.measure_rev != self.tree_rev
                    || self.measure_query.as_ref() != Some(query)
                {
                    let focused = self.focused();
                    let measured =
                        treemap::measure(focused, query, query.name_hits(&focused.name));
                    self.measure = Some(measured);
                    self.measure_rev = self.tree_rev;
                    self.measure_query = Some(query.clone());
                }
                self.filtered = self.measure.as_ref().map(|m| (m.bytes, m.files));
            }
        }
        let cells = treemap::layout(
            self.focused(),
            &base,
            area,
            viewport,
            &self.style,
            self.measure.as_ref(),
        );
        self.laid_cull = viewport;
        self.cells = cells;
        self.paint_order = match self.projection {
            MapProjection::Flat => Vec::new(),
            _ => view_order(&self.cells, self.lean()),
        };
        // A filter change captured the map as it looked; aim the tween from
        // there to the layout just built.
        if let Some(snapshot) = self.tween_capture.take() {
            let calm = std::mem::take(&mut self.tween_calm);
            if calm && self.tween_t(now).is_none() {
                // A camera-asked settle with nothing already morphing runs
                // no animation at all. With zoom-invariant packing a
                // survivor's fresh rect IS its remapped old rect, and the
                // detail the refresh brought in was always there on disk —
                // the new layout simply is. Starting the interpolator here
                // would be 200ms of re-presentation per cadence: the "boxes
                // animating" a quiet zoom must not have.
                self.tween_from.clear();
                self.tween_leavers.clear();
                self.tween_start = None;
            } else {
                let now_here: std::collections::HashSet<&Path> =
                    self.cells.iter().map(|c| c.path.as_path()).collect();
                self.tween_from = snapshot
                    .iter()
                    .filter(|(cell, _, _)| now_here.contains(cell.path.as_path()))
                    .map(|(cell, rect, depth)| {
                        (cell.path.clone(), TweenFrom { rect: *rect, depth: *depth })
                    })
                    .collect();
                if calm {
                    // A camera settle that landed while an older morph (a
                    // filter edit moments ago) was still in flight: re-aim
                    // the running morph from the visual truth and let it
                    // finish. Everything the refresh added joins at its own
                    // rect, full alpha — camera-brought detail never fades.
                    for cell in &self.cells {
                        if !self.tween_from.contains_key(&cell.path) {
                            self.tween_from.insert(
                                cell.path.clone(),
                                TweenFrom { rect: cell.rect, depth: cell.depth as f64 },
                            );
                        }
                    }
                    self.tween_leavers = Vec::new();
                } else {
                    // A cell new to the list is either a camera reveal — it
                    // was sitting outside the outgoing layout's cull, always
                    // there on disk, merely unlaid — or detail that genuinely
                    // appeared (a bundle dissolving, a scan step, a filter
                    // edit). Reveals must simply *be there*: styling them
                    // with the arrival fade reads as data popping into
                    // existence at the edge of an orbit or pan. So a reveal
                    // joins the tween at its own rect (no motion, full alpha)
                    // and only true arrivals keep the fade-and-grow. The
                    // snapshot and the fresh layout share a frame — snapshot
                    // rects were remapped to the live camera, which the
                    // fresh layout now rests under — so the old cull is
                    // compared in that frame too, through the remap the
                    // snapshot itself used.
                    if old_cull.w > 0.0 {
                        let (rk, rb) = old_remap;
                        let seen = remap_rect(&old_cull, rk, rb);
                        for cell in &self.cells {
                            if !self.tween_from.contains_key(&cell.path)
                                && !cell.rect.intersects(&seen)
                            {
                                self.tween_from.insert(
                                    cell.path.clone(),
                                    TweenFrom { rect: cell.rect, depth: cell.depth as f64 },
                                );
                            }
                        }
                    }
                    // Symmetric on the way out: a cell the new layout culled
                    // away is just going off-view — it vanishes with the
                    // frame, no goodbye fade. Only a leaver still inside the
                    // laid region (absorbed, filtered out) earns one.
                    self.tween_leavers = snapshot
                        .into_iter()
                        .filter(|(cell, rect, _)| {
                            !now_here.contains(cell.path.as_path())
                                && rect.intersects(&self.laid_cull)
                        })
                        .collect();
                }
                self.tween_start = Some(now);
            }
        }
        self.laid_out = rect;
        self.layout_rev = self.layout_rev.wrapping_add(1);
        // The layout now rests exactly under the live camera: the remap is
        // the identity again until the next gesture departs from here.
        self.layout_scale = self.cam_scale.max(1.0);
        self.layout_off = self.cam_off;
        self.layout_yaw = self.yaw;
        self.layout_pitch = self.pitch;
        self.stale = false;
        self.last_layout = Some(now);
        // The cell list is new, so the hovered index means nothing any more.
        self.hover = None;
        // The selection is a path, not an index, so it survives — but its
        // numbers are refreshed from whatever cell now stands for it.
        if let Some(pick) = self.pick.take() {
            let refreshed = self
                .cells
                .iter()
                .find(|c| c.path == pick.path && !c.is_bundle())
                .map(pick_of);
            self.pick = refreshed.or(Some(pick));
        }
    }

    /// The cell under a window point, if any. In the raised projections the
    /// test happens on the projected faces — top plate first, then the walls
    /// it stands on — front-most first: the reverse of paint order, which is
    /// what "front" means.
    fn hit_cell(&self, pos: DVec2) -> Option<usize> {
        let (rk, rb) = self.cam_remap();
        if self.projection == MapProjection::Flat || self.paint_order.len() != self.cells.len() {
            // The inverse ride: the pointer comes back from the screen into
            // the space the cells were laid out in, so a mid-gesture hover
            // or click lands on what the eye actually sees.
            let p = dvec2((pos.x - rb.x) / rk, (pos.y - rb.y) / rk);
            return treemap::hit(&self.cells, p.x, p.y);
        }
        let cam = self.cam_at(self.laid_out);
        let rise = self.rise();
        for &index in self.paint_order.iter().rev() {
            let cell = &self.cells[index];
            let rect = remap_rect(&cell.rect, rk, rb);
            let z = self.elev(cell.depth);
            if Quad::of_rect(&cam, &rect, z).contains(pos) {
                return Some(index);
            }
            if z > 0.0 {
                for wall in wall_quads(&cam, &rect, z, rise.min(z)).into_iter().flatten() {
                    if wall.quad.contains(pos) {
                        return Some(index);
                    }
                }
            }
        }
        None
    }

    /// The file or folder under a window point — what a right-click there is
    /// about. Never the "N smaller items" bundle, which is not a thing on
    /// disk and must never become the target of an operation.
    pub fn path_at(&self, pos: DVec2) -> Option<PathBuf> {
        self.hit_cell(pos)
            .map(|i| &self.cells[i])
            .filter(|c| !c.is_bundle())
            .map(|c| c.path.clone())
    }

    // ------------------------------------------------------------- painting

    fn tile_colors(&self, cell: &Cell, palette: &Palette) -> (Vec4f, f32) {
        let bg = Palette::vec4(&palette.bg);
        if cell.kind == KIND_BUNDLE {
            // Not a file: the sum of everything too small to see. It reads as
            // a texture rather than as a thing, which is what it is.
            return (blend(Palette::vec4(&palette.muted), bg, 0.45), 0.35);
        }
        let class = kind_class(cell_kind(cell)) as usize;
        let mut hue = palette.kind_color(class);
        if class == OTHER_CLASS && !cell.is_dir {
            // The theme's "other" is a chrome grey — the colour of a border,
            // not of a thing. A 4 GB disk image or a database file painted in
            // it disappears into the background, and on a real disk the
            // unclassifiable blobs are most of what there is to clean up.
            hue = blend(hue, Palette::vec4(&palette.fg), 0.55);
        }
        if cell.is_group {
            // A group is the plate its children sit on: nearly background, but
            // carrying a trace of its own heaviest content's hue so the shape
            // of the disk survives even where nothing inside it fits.
            let plate = blend(hue, bg, 0.86 - 0.02 * cell.depth.min(4) as f32);
            return (plate, 0.22);
        }
        // Leaves darken slightly with depth, which reads as "further in"
        // without ever making two kinds look like each other.
        let depth_shade = 1.0 - 0.05 * cell.depth.min(6) as f32;
        let base = if cell.is_dir {
            // A folder too small to open is still a folder: half way to the
            // plate, so it never reads as one big file.
            blend(hue, bg, 0.45)
        } else {
            hue
        };
        (scale_rgb(base, depth_shade), 0.62)
    }

    fn draw_map(&mut self, cx: &mut Cx2d, palette: &Palette, clip: Rect, now: f64) -> Vec<Label> {
        let border_ink = Palette::vec4(&palette.bg_dark);
        let accent = Palette::vec4(&palette.accent);
        let bright = Palette::vec4(&palette.fg_bright);
        let ink_dark = Palette::vec4(&palette.bg_dark);
        let hovered = self.hover;
        let picked = self.pick.as_ref().map(|p| p.path.clone());
        let mut labels: Vec<Label> = Vec::new();
        // The x-extent and row of every floating name placed so far, for the
        // collision stagger — group names only, so this stays tens long.
        let mut placed_names: Vec<(f64, f64, f64)> = Vec::new();
        let t = self.tween_t(now);
        let raised = self.projection != MapProjection::Flat;
        // The flat map's printed names are frozen per relayout and drawn by
        // draw_frozen_labels, riding the remap untouched. Deriving them here,
        // per frame from the remapped rects, is only for tween windows —
        // where names travel with their morphing tiles — and for the raised
        // hover label.
        let live_labels = raised || t.is_some();
        let rise = self.rise();
        let cam = self.cam_at(self.laid_out);
        // Mid-gesture, every rect rides from the layout's camera to the live
        // one through this one affine map — the whole picture scales as one
        // rigid sheet, which is what "visually constant" means.
        let (rk, rb) = self.cam_remap();

        cx.push_clip_rect(clip);
        self.draw_tile.begin_many_instances(cx);

        // Whatever the filter just dismissed fades out where it stood,
        // under everything that is staying.
        if let Some(t) = t {
            let ghost = (1.0 - t) as f32 * 0.9;
            for index in 0..self.tween_leavers.len() {
                let (rect, depth) = {
                    let (_, r, d) = &self.tween_leavers[index];
                    (remap_rect(r, rk, rb), *d)
                };
                let (fill, _) = {
                    let (cell, _, _) = &self.tween_leavers[index];
                    self.tile_colors(cell, palette)
                };
                let z = if raised { self.elev_f(depth) } else { 0.0 };
                let quad = Quad::of_rect(&cam, &rect, z);
                self.draw_tile.color = fade(fill, ghost);
                self.draw_tile.edge = fade(border_ink, ghost);
                self.draw_tile.cushion = 0.0;
                self.draw_tile.border = 0.5;
                face(&mut self.draw_tile, cx, &quad);
            }
        }

        let order: Vec<usize> = if self.paint_order.len() == self.cells.len() {
            self.paint_order.clone()
        } else {
            (0..self.cells.len()).collect()
        };
        for &index in &order {
            let cell = &self.cells[index];
            let (vrect, vdepth, alpha) = self.tweened(cell, t);
            let vrect = remap_rect(&vrect, rk, rb);
            let alpha = alpha as f32;
            if alpha <= 0.02 {
                continue;
            }
            let z = if raised { self.elev_f(vdepth) } else { 0.0 };
            let quad = Quad::of_rect(&cam, &vrect, z);
            let rect = quad.bounds();
            // The cull margin was laid out to ride the remap, not to be
            // painted: whatever sits wholly off the panel is skipped, with
            // slack for the walls hanging below a plate.
            let slack = z + 4.0;
            if rect.pos.x + rect.size.x < clip.pos.x - slack
                || rect.pos.x > clip.pos.x + clip.size.x + slack
                || rect.pos.y + rect.size.y < clip.pos.y - slack
                || rect.pos.y > clip.pos.y + clip.size.y + slack
            {
                continue;
            }
            let cell = &self.cells[index];
            let (fill, cushion) = self.tile_colors(cell, palette);
            let is_hover = Some(index) == hovered;
            let is_pick = picked.as_deref() == Some(cell.path.as_path()) && !cell.is_bundle();

            // The prism's walls: the faces between this plate and the
            // plateau it stands on, on whichever sides the camera can see.
            // This is where "height means depth" is actually visible — and
            // they paint under their own plate but over everything already
            // painted, which is exactly what one shared instance batch in
            // paint order gives.
            if raised && z > 0.0 {
                for wall in wall_quads(&cam, &vrect, z, rise.min(z)).into_iter().flatten() {
                    self.draw_tile.color = fade(scale_rgb(fill, wall.shade), alpha);
                    self.draw_tile.edge = fade(border_ink, alpha);
                    self.draw_tile.cushion = 0.0;
                    self.draw_tile.border = 0.0;
                    face(&mut self.draw_tile, cx, &wall.quad);
                }
            }

            // Hover is the outline only — a bright border flash, never a
            // relit tile: on a dense map a whole rectangle changing value
            // under the pointer reads as the data changing.
            // The border is what separates siblings, and it can never be
            // allowed to eat the tile it surrounds — a three-point rectangle
            // with a one-point border on every side is all border. So it
            // scales with the tile and simply stops existing on the small
            // ones, where the cushion's own shading does the separating.
            let short = rect.size.x.min(rect.size.y);
            let border = if is_pick || is_hover {
                1.5
            } else {
                (short * 0.14).min(1.0)
            };
            let cushion = cushion * (CUSHION_FULL_AT / short.max(4.0)).clamp(0.30, 1.0) as f32;
            self.draw_tile.color = fade(fill, alpha);
            self.draw_tile.edge = fade(
                if is_pick {
                    accent
                } else if is_hover {
                    bright
                } else {
                    border_ink
                },
                alpha,
            );
            self.draw_tile.cushion = cushion;
            self.draw_tile.border = border as f32;
            face(&mut self.draw_tile, cx, &quad);

            if !live_labels || labels.len() >= LABEL_BUDGET {
                continue;
            }
            if self.projection != MapProjection::Flat && !is_hover {
                // The raised views wear no name tags — a city of prisms all
                // labelled reads as clutter, not a map. A name appears the
                // moment the pointer rests on its tile, and the tooltip
                // carries the numbers as everywhere else. Only the flat map
                // keeps its printed labels.
                continue;
            }
            // A zoomed camera slides tiles half off the panel; a name pinned
            // to a corner nobody can see is a tile nobody can identify, so
            // labels clamp to the visible part of their rectangle — unless
            // almost none of it is visible, where a clamped name would just
            // pile up on the panel edge with its neighbours'.
            let at_x = (rect.pos.x + 4.0).max(clip.pos.x + 4.0);
            let room = rect.pos.x + rect.size.x - at_x - 4.0;
            let clamped = rect.pos.y < clip.pos.y;
            if clamped && rect.pos.y + rect.size.y - clip.pos.y < 40.0 {
                continue;
            }
            if cell.is_group && cell.header > 0.0 {
                // A group's name reserves no layout room any more — a strip
                // that appears at some zoom is a strip that shoves children
                // at that zoom. The name floats over the children on a dim
                // plate of its own, at the top edge in the flat map (where
                // the strip used to sit, so the look barely changes),
                // clamped one line further down per nesting level so a stack
                // of clamped ancestors reads as a breadcrumb instead of
                // garble. In the raised projections the lift exposes the
                // plate's bottom edge; the name goes there.
                let at_y = if raised {
                    (rect.pos.y + rect.size.y - 13.0).min(clip.pos.y + clip.size.y - 13.0)
                } else {
                    // Names share the sheet now, so they must not share a
                    // line: a name whose row collides with an already-placed
                    // ancestor's or neighbour's steps down until it finds
                    // air — nested groups at a common top edge read as a
                    // breadcrumb, exactly like the old clamp stagger but
                    // driven by actual collisions rather than depth.
                    let mut at_y = (rect.pos.y + 1.0).max(clip.pos.y + 1.0);
                    loop {
                        let mut bumped = false;
                        for &(x0, x1, y) in &placed_names {
                            let next = y + 12.0;
                            // Only a bump that actually descends counts —
                            // rows born exactly one line apart re-trigger
                            // the window through float dust, and a "bump"
                            // to the same row would spin here forever.
                            if at_x < x1 && at_x + room > x0 && (at_y - y).abs() < 12.0 && next > at_y
                            {
                                at_y = next;
                                bumped = true;
                            }
                        }
                        if !bumped {
                            break;
                        }
                    }
                    if at_y > rect.pos.y + rect.size.y - 12.0 {
                        // No air left inside its own tile: the name stays on
                        // the tooltip rather than bleeding onto a neighbour.
                        continue;
                    }
                    placed_names.push((at_x, at_x + room, at_y));
                    at_y
                };
                labels.push(Label {
                    at: dvec2(at_x, at_y),
                    room,
                    line: format!("{}  {}", cell.name, treemap::format_bytes(cell.size)),
                    below: None,
                    ink: fade(bright, alpha),
                    scrim: true,
                });
            } else if !cell.is_group
                && rect.size.x >= LABEL_MIN.x
                && rect.size.y >= LABEL_MIN.y
            {
                // Leaves join the same collision ledger as the floating
                // names: a file at the top of a group would otherwise print
                // straight through its ancestors' scrims.
                let mut at_y = (rect.pos.y + 3.0).max(clip.pos.y + 3.0);
                loop {
                    let mut bumped = false;
                    for &(x0, x1, y) in &placed_names {
                        let next = y + 12.0;
                        if at_x < x1 && at_x + room > x0 && (at_y - y).abs() < 12.0 && next > at_y
                        {
                            at_y = next;
                            bumped = true;
                        }
                    }
                    if !bumped {
                        break;
                    }
                }
                if at_y > rect.pos.y + rect.size.y - 12.0 {
                    continue;
                }
                placed_names.push((at_x, at_x + room, at_y));
                let two_lines = rect.size.y + rect.pos.y - at_y >= LABEL_TWO_LINE_H;
                labels.push(Label {
                    at: dvec2(at_x, at_y),
                    room,
                    line: cell.name.clone(),
                    below: two_lines.then(|| treemap::format_bytes(cell.size)),
                    ink: fade(ink_dark, alpha),
                    scrim: false,
                });
            }
        }
        self.draw_tile.end_many_instances(cx);

        // The picked rectangle gets a ring on top of everything, because the
        // thing you are about to delete must be findable even when it is a
        // folder whose children cover it.
        if let Some(path) = &picked {
            let found = self
                .cells
                .iter()
                .position(|c| &c.path == path && !c.is_bundle());
            if let Some(index) = found {
                let cell = &self.cells[index];
                let (vrect, vdepth, _) = self.tweened(cell, t);
                let vrect = remap_rect(&vrect, rk, rb);
                let z = if raised { self.elev_f(vdepth) } else { 0.0 };
                let quad = Quad::of_rect(&cam, &vrect, z);
                self.draw_tile.color = Vec4f { x: 0.0, y: 0.0, z: 0.0, w: 0.0 };
                self.draw_tile.edge = accent;
                self.draw_tile.cushion = 0.0;
                self.draw_tile.border = 2.0;
                face(&mut self.draw_tile, cx, &quad);
            }
        }
        cx.pop_clip_rect();
        labels
    }

    fn draw_labels(&mut self, cx: &mut Cx2d, labels: Vec<Label>, clip: Rect, palette: &Palette) {
        cx.push_clip_rect(clip);
        // Fit everything first: the scrim plates need each line's real width,
        // and every plate must land before any glyph so the text batch reads
        // over all of them.
        let fitted: Vec<String> = labels
            .iter()
            .map(|label| fit_text(&self.draw_text, cx, &label.line, label.room))
            .collect();
        if labels.iter().any(|l| l.scrim) {
            // Floating names bring their own contrast: a dim plate behind
            // the text, in a draw call of its own so it layers above the
            // tile batch and below the glyphs.
            let plate_ink = Palette::vec4(&palette.bg_dark);
            self.draw_tile.new_draw_call(cx);
            self.draw_tile.cushion = 0.0;
            self.draw_tile.border = 0.0;
            for (label, line) in labels.iter().zip(&fitted) {
                if !label.scrim || line.is_empty() {
                    continue;
                }
                let width = text_width(&self.draw_text, cx, line);
                let pos = label.at - dvec2(3.0, 1.0);
                let size = dvec2(width + 6.0, 13.0);
                let plate = fade(plate_ink, 0.78 * label.ink.w);
                self.draw_tile.color = plate;
                self.draw_tile.edge = plate;
                face(&mut self.draw_tile, cx, &Quad {
                    p: [
                        pos,
                        dvec2(pos.x + size.x, pos.y),
                        pos + size,
                        dvec2(pos.x, pos.y + size.y),
                    ],
                });
            }
        }
        for (label, line) in labels.into_iter().zip(fitted) {
            self.draw_text.color = label.ink;
            self.draw_text.draw_abs(cx, label.at, &line);
            if let Some(below) = label.below {
                self.draw_text.color = fade(label.ink, 0.72);
                let below = fit_text(&self.draw_text, cx, &below, label.room);
                self.draw_text
                    .draw_abs(cx, label.at + dvec2(0.0, 11.0), &below);
            }
        }
        cx.pop_clip_rect();
    }

    /// Derive the flat map's labels from the settled layout, once. Content,
    /// truncation and stagger row are all decided here against the layout's
    /// own rects; the draw path then only translates anchors. Keyed on the
    /// layout revision, so this is free until the next relayout.
    fn ensure_frozen_labels(&mut self, cx: &mut Cx2d, palette: &Palette, clip: Rect) {
        if self.frozen_rev == Some(self.layout_rev) {
            return;
        }
        self.frozen_rev = Some(self.layout_rev);
        let bright = Palette::vec4(&palette.fg_bright);
        let ink_dark = Palette::vec4(&palette.bg_dark);
        // Clamping happens in layout space: the panel carried back through
        // the inverse of the live remap — the identity right after a settle,
        // which is when this normally runs.
        let (rk, rb) = self.cam_remap();
        let lclip = MapRect {
            x: (clip.pos.x - rb.x) / rk,
            y: (clip.pos.y - rb.y) / rk,
            w: clip.size.x / rk,
            h: clip.size.y / rk,
        };
        let mut placed_names: Vec<(f64, f64, f64)> = Vec::new();
        let mut out: Vec<FrozenLabel> = Vec::new();
        for cell in &self.cells {
            if out.len() >= LABEL_BUDGET {
                break;
            }
            let rect = cell.rect;
            let at_x = (rect.x + 4.0).max(lclip.x + 4.0);
            let room = rect.x + rect.w - at_x - 4.0;
            let clamped = rect.y < lclip.y;
            if clamped && rect.y + rect.h - lclip.y < 40.0 {
                continue;
            }
            if cell.is_group && cell.header > 0.0 {
                let mut at_y = (rect.y + 1.0).max(lclip.y + 1.0);
                loop {
                    let mut bumped = false;
                    for &(x0, x1, y) in &placed_names {
                        let next = y + 12.0;
                        if at_x < x1 && at_x + room > x0 && (at_y - y).abs() < 12.0 && next > at_y
                        {
                            at_y = next;
                            bumped = true;
                        }
                    }
                    if !bumped {
                        break;
                    }
                }
                if at_y > rect.y + rect.h - 12.0 {
                    continue;
                }
                let full = format!("{}  {}", cell.name, treemap::format_bytes(cell.size));
                let line = fit_text(&self.draw_text, cx, &full, room);
                if line.is_empty() {
                    continue;
                }
                placed_names.push((at_x, at_x + room, at_y));
                let width = text_width(&self.draw_text, cx, &line);
                out.push(FrozenLabel {
                    at: dvec2(at_x, at_y),
                    line,
                    below: None,
                    width,
                    ink: bright,
                    scrim: true,
                });
            } else if !cell.is_group && rect.w >= LABEL_MIN.x && rect.h >= LABEL_MIN.y {
                let mut at_y = (rect.y + 3.0).max(lclip.y + 3.0);
                loop {
                    let mut bumped = false;
                    for &(x0, x1, y) in &placed_names {
                        let next = y + 12.0;
                        if at_x < x1 && at_x + room > x0 && (at_y - y).abs() < 12.0 && next > at_y
                        {
                            at_y = next;
                            bumped = true;
                        }
                    }
                    if !bumped {
                        break;
                    }
                }
                if at_y > rect.y + rect.h - 12.0 {
                    continue;
                }
                let line = fit_text(&self.draw_text, cx, &cell.name, room);
                if line.is_empty() {
                    continue;
                }
                placed_names.push((at_x, at_x + room, at_y));
                let width = text_width(&self.draw_text, cx, &line);
                let below = (rect.h + rect.y - at_y >= LABEL_TWO_LINE_H)
                    .then(|| fit_text(&self.draw_text, cx, &treemap::format_bytes(cell.size), room));
                out.push(FrozenLabel {
                    at: dvec2(at_x, at_y),
                    line,
                    below,
                    width,
                    ink: ink_dark,
                    scrim: false,
                });
            }
        }
        self.frozen_labels = out;
    }

    /// Draw the frozen labels: each anchor rides the same affine remap the
    /// tiles ride, the text itself untouched — so a glide moves names, and
    /// nothing about them flickers.
    fn draw_frozen_labels(&mut self, cx: &mut Cx2d, palette: &Palette, clip: Rect) {
        if self.frozen_labels.is_empty() {
            return;
        }
        let (rk, rb) = self.cam_remap();
        let labels = std::mem::take(&mut self.frozen_labels);
        cx.push_clip_rect(clip);
        if labels.iter().any(|l| l.scrim) {
            let plate_ink = Palette::vec4(&palette.bg_dark);
            self.draw_tile.new_draw_call(cx);
            self.draw_tile.cushion = 0.0;
            self.draw_tile.border = 0.0;
            for label in &labels {
                if !label.scrim {
                    continue;
                }
                let at = dvec2(label.at.x * rk + rb.x, label.at.y * rk + rb.y);
                if off_panel(at, label.width, &clip) {
                    continue;
                }
                let pos = at - dvec2(3.0, 1.0);
                let size = dvec2(label.width + 6.0, 13.0);
                let plate = fade(plate_ink, 0.78 * label.ink.w);
                self.draw_tile.color = plate;
                self.draw_tile.edge = plate;
                face(&mut self.draw_tile, cx, &Quad {
                    p: [
                        pos,
                        dvec2(pos.x + size.x, pos.y),
                        pos + size,
                        dvec2(pos.x, pos.y + size.y),
                    ],
                });
            }
        }
        for label in &labels {
            let at = dvec2(label.at.x * rk + rb.x, label.at.y * rk + rb.y);
            if off_panel(at, label.width, &clip) {
                continue;
            }
            self.draw_text.color = label.ink;
            self.draw_text.draw_abs(cx, at, &label.line);
            if let Some(below) = &label.below {
                self.draw_text.color = fade(label.ink, 0.72);
                self.draw_text.draw_abs(cx, at + dvec2(0.0, 11.0), below);
            }
        }
        cx.pop_clip_rect();
        self.frozen_labels = labels;
    }

    /// The zoom breadcrumb, and on the right whatever the scan is doing.
    fn draw_crumbs(&mut self, cx: &mut Cx2d, strip: Rect, palette: &Palette) {
        self.crumbs.clear();
        self.draw_bg.color = Palette::vec4(&palette.bg_dark);
        self.draw_bg.draw_abs(cx, strip);

        let bright = Palette::vec4(&palette.fg_bright);
        let dim = Palette::vec4(&palette.fg_dim);
        let accent = Palette::vec4(&palette.accent);

        // The right-hand end first, so the crumbs know where to stop. This is
        // where the map admits what it is: how old the numbers are, what it
        // was not allowed to look at, and what it left out on purpose.
        self.rescan_hit = Rect::default();
        let mut right = strip.pos.x + strip.size.x - 8.0;
        if !self.scanning {
            // "Rescan" is not a nicety. A map read back from a file is only
            // honest if making it true again is one click away.
            // A word, not a glyph: the UI font has no reload arrow and a
            // tofu box is worse than no icon at all. Icons in this app are
            // SVGs, and this control does not need one.
            let word = "rescan";
            let width = text_width(&self.draw_bold, cx, word);
            right -= width;
            self.draw_bold.color = accent;
            self.draw_bold.draw_abs(cx, dvec2(right, strip.pos.y + 4.0), word);
            self.rescan_hit = Rect {
                pos: dvec2(right - 4.0, strip.pos.y),
                size: dvec2(width + 8.0, strip.size.y),
            };
            right -= 14.0;
        }
        // The active filter is never invisible: while one is on, the strip
        // says what it matched and offers the way out.
        self.filter_hit = Rect::default();
        if let Some((bytes, _)) = self.filter_matched() {
            let chip = format!(
                "matching {} of {} · clear",
                treemap::format_bytes(bytes),
                treemap::format_bytes(self.focused().size),
            );
            let width = text_width(&self.draw_bold, cx, &chip);
            right -= width;
            self.draw_bold.color = accent;
            self.draw_bold.draw_abs(cx, dvec2(right, strip.pos.y + 4.0), &chip);
            self.filter_hit = Rect {
                pos: dvec2(right - 4.0, strip.pos.y),
                size: dvec2(width + 8.0, strip.size.y),
            };
            right -= 14.0;
        }
        let note = if self.scanning {
            format!(
                "scanning  ·  {} files  ·  {}  ·  {} folders open",
                self.tree.files,
                treemap::format_bytes(self.tree.size),
                self.folders_left,
            )
        } else {
            let mut note = crate::sizecache::age_text(self.scanned_at);
            if let Some(excluded) = crate::model::scan_exclusions() {
                note.push_str("  ·  ");
                note.push_str(&excluded);
            }
            if !self.denied.is_empty() {
                note.push_str("  ·  no access: ");
                note.push_str(&self.denied.join(", "));
            }
            note
        };
        let note_w = text_width(&self.draw_text, cx, &note);
        self.draw_text.color = if self.scanning { accent } else { dim };
        self.draw_text
            .draw_abs(cx, dvec2(right - note_w, strip.pos.y + 5.0), &note);
        let note_w = strip.pos.x + strip.size.x - (right - note_w);

        let limit = strip.pos.x + strip.size.x - note_w - 18.0;
        let mut x = strip.pos.x + 8.0;
        let names: Vec<String> = std::iter::once(crate::model::display_name(&self.root))
            .chain(self.zoom.iter().cloned())
            .collect();
        let last = names.len().saturating_sub(1);
        for (depth, name) in names.iter().enumerate() {
            let text = if depth == 0 {
                name.clone()
            } else {
                format!("› {name}")
            };
            let width = text_width(&self.draw_bold, cx, &text);
            if x + width > limit {
                self.draw_text.color = dim;
                self.draw_text.draw_abs(cx, dvec2(x, strip.pos.y + 5.0), "…");
                break;
            }
            self.draw_bold.color = if depth == last { bright } else { dim };
            self.draw_bold.draw_abs(cx, dvec2(x, strip.pos.y + 4.0), &text);
            self.crumbs.push(CrumbHit {
                rect: Rect {
                    pos: dvec2(x, strip.pos.y),
                    size: dvec2(width, strip.size.y),
                },
                depth,
            });
            x += width + 6.0;
        }
    }

    /// The persistent readout: what the last click landed on, and how big it
    /// is. This is the line a person cleaning up a full disk actually reads,
    /// so it never goes away on its own and never shows anything but the
    /// truth about one real path.
    fn draw_footer(&mut self, cx: &mut Cx2d, strip: Rect, palette: &Palette) {
        self.draw_bg.color = Palette::vec4(&palette.bg_dark);
        self.draw_bg.draw_abs(cx, strip);
        let total = self.focused().size.max(1);
        // Two parts: the numbers, then the path. The numbers are the reason
        // anybody is looking and always get their room first; the path takes
        // whatever is left and is shortened from its *front*, because the end
        // of a path is the half that says which file this is.
        let (head, path, ink) = match &self.pick {
            Some(pick) if pick.bundle > 0 => (
                format!(
                    "{}   {:.1}% of what is on screen",
                    treemap::format_bytes(pick.size),
                    pick.size as f64 * 100.0 / total as f64,
                ),
                format!(
                    "{} items each too small to draw on their own",
                    pick.bundle
                ),
                Palette::vec4(&palette.fg_dim),
            ),
            Some(pick) => (
                format!(
                    "{}   {:.1}% of {}{}",
                    treemap::format_bytes(pick.size),
                    pick.size as f64 * 100.0 / total as f64,
                    treemap::format_bytes(total),
                    if pick.is_dir {
                        format!("   {} files", pick.files)
                    } else {
                        String::new()
                    },
                ),
                pick.path.display().to_string(),
                Palette::vec4(&palette.fg_bright),
            ),
            None => (
                String::new(),
                if self.projection == MapProjection::Flat {
                    "Click picks · scroll zooms · drag pans · double-click fills the view · Esc \
                     backs out · right-click for the file menu"
                } else {
                    "Click picks · left-drag orbits · right-drag pans · scroll zooms · \
                     double-click fills the view · Esc backs out · right-click for the file menu"
                }
                .to_string(),
                Palette::vec4(&palette.fg_dim),
            ),
        };
        let baseline = strip.pos.y + 5.0;
        let mut x = strip.pos.x + 8.0;
        let edge = strip.pos.x + strip.size.x - 8.0;
        if !head.is_empty() {
            self.draw_text.color = ink;
            let head = fit_text(&self.draw_text, cx, &head, edge - x);
            let width = text_width(&self.draw_text, cx, &head);
            self.draw_text.draw_abs(cx, dvec2(x, baseline), &head);
            x += width + 12.0;
        }
        self.draw_text.color = Palette::vec4(&palette.fg_dim);
        let path = fit_tail(&self.draw_text, cx, &path, edge - x);
        self.draw_text.draw_abs(cx, dvec2(x, baseline), &path);
    }

    /// The hovered rectangle's name and size, anchored to the rectangle
    /// itself rather than to the pointer — a tooltip that chases the mouse
    /// forces a full repaint of the whole map on every mouse move, and a map
    /// can be sixty thousand rectangles.
    fn draw_tooltip(&mut self, cx: &mut Cx2d, body: Rect, palette: &Palette) {
        let Some(cell) = self.hover.and_then(|i| self.cells.get(i)) else {
            return;
        };
        let total = self.focused().size.max(1);
        let head = if cell.is_bundle() {
            cell.name.clone()
        } else {
            cell.path
                .strip_prefix(&self.focus_path())
                .unwrap_or(&cell.path)
                .display()
                .to_string()
        };
        let foot = format!(
            "{} · {:.1}%{}{}",
            treemap::format_bytes(cell.size),
            cell.size as f64 * 100.0 / total as f64,
            if cell.is_dir {
                format!(" · {} files", cell.files)
            } else {
                String::new()
            },
            if cell.pending { " · still scanning" } else { "" },
        );
        let cam = self.cam_at(self.laid_out);
        let z = match self.projection {
            MapProjection::Flat => 0.0,
            _ => self.elev(cell.depth),
        };
        let (rk, rb) = self.cam_remap();
        let top = Quad::of_rect(&cam, &remap_rect(&cell.rect, rk, rb), z).bounds();
        let anchor = top.pos;
        let cell_h = top.size.y;

        let width = text_width(&self.draw_bold, cx, &head)
            .max(text_width(&self.draw_text, cx, &foot))
            + 14.0;
        let size = dvec2(width.min(body.size.x - 8.0), 30.0);
        // Below the rectangle when there is room under it, above it when
        // there is not — so the tooltip never covers what it is describing.
        let below = anchor.y + cell_h + 4.0;
        let y = if below + size.y <= body.pos.y + body.size.y {
            below
        } else {
            (anchor.y - size.y - 4.0).max(body.pos.y + 2.0)
        };
        let pos = dvec2(
            anchor
                .x
                .min(body.pos.x + body.size.x - size.x - 4.0)
                .max(body.pos.x + 2.0),
            y,
        );

        // Both the plate and its text open a draw call of their own. Every
        // tile shares one batch and every label shares another, so without
        // this the plate lands in the batch the map was drawn in and the
        // labels underneath read straight through it.
        self.draw_tile.new_draw_call(cx);
        self.draw_tile.color = Palette::vec4(&palette.bg_dark);
        self.draw_tile.edge = Palette::vec4(&palette.accent);
        self.draw_tile.cushion = 0.0;
        self.draw_tile.border = 1.0;
        face(&mut self.draw_tile, cx, &Quad {
            p: [
                pos,
                dvec2(pos.x + size.x, pos.y),
                pos + size,
                dvec2(pos.x, pos.y + size.y),
            ],
        });

        cx.push_clip_rect(Rect { pos, size });
        self.draw_bold.new_draw_call(cx);
        self.draw_bold.color = Palette::vec4(&palette.fg_bright);
        self.draw_bold.draw_abs(cx, pos + dvec2(7.0, 3.0), &head);
        self.draw_text.new_draw_call(cx);
        self.draw_text.color = Palette::vec4(&palette.fg_dim);
        self.draw_text.draw_abs(cx, pos + dvec2(7.0, 16.0), &foot);
        cx.pop_clip_rect();
    }

    fn press(&mut self, cx: &mut Cx, at: DVec2, taps: u32, primary: bool) {
        if self.filter_hit.contains(at) {
            if primary {
                self.set_filter(cx, None);
                cx.widget_action(self.uid, TreemapAction::FilterCleared);
            }
            return;
        }
        if self.rescan_hit.contains(at) {
            if primary {
                self.rescan(cx);
            }
            return;
        }
        if let Some(crumb) = self.crumbs.iter().find(|c| c.rect.contains(at)).copied() {
            if primary {
                self.zoom_to(cx, crumb.depth);
            }
            return;
        }
        let Some(index) = self.hit_cell(at) else {
            return;
        };
        let cell = self.cells[index].clone();
        // One stat, on the one thing that was just pointed at. A map read
        // back from a file can be out of date; this is where that stops being
        // invisible, and it costs nothing because it happens once per click
        // rather than once per rectangle.
        if !cell.is_bundle() && !crate::vfs::is_demo() && !crate::vfs::vfs().exists(&cell.path) {
            let path = cell.path.clone();
            self.forget(cx, &path);
            cx.widget_action(self.uid, TreemapAction::Vanished(path));
            return;
        }
        self.pick = Some(pick_of(&cell));
        self.redraw(cx);
        // Where the cell IS on screen — a double-click straight after a
        // gesture fits what the eye sees, not where the old layout had it.
        let (rk, rb) = self.cam_remap();
        let vrect = remap_rect(&cell.rect, rk, rb);
        let rect = Rect {
            pos: dvec2(vrect.x, vrect.y),
            size: dvec2(vrect.w, vrect.h),
        };
        if cell.is_bundle() {
            // Nothing on disk is under there to act on — but zooming in on
            // it is exactly the right move: at the higher magnification the
            // re-layout dissolves the bundle into the things it stood for.
            if primary && taps >= 2 {
                self.fit_rect(cx, rect);
            }
            return;
        }
        // A secondary press only picks: the context menu that follows it acts
        // on whatever is picked, and zooming out from under a menu that is
        // about to open would be a trap.
        if !primary {
            cx.widget_action(self.uid, TreemapAction::Selected(cell.path));
            return;
        }
        if taps >= 2 {
            // The camera, not a re-root and not a navigation: the breadcrumb,
            // the browser and the scan all stay where they are — the map just
            // goes and looks at this one, and Esc backs straight out again.
            // (Going *to* a file lives in the context menu, on purpose.)
            self.fit_rect(cx, rect);
        }
        cx.widget_action(self.uid, TreemapAction::Selected(cell.path));
    }
}

impl Widget for TreemapView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let palette = Palette::shared();
        self.draw_bg.color = Palette::vec4(&palette.bg);
        self.draw_bg.draw_abs(cx, rect);
        if rect.size.x < 40.0 || rect.size.y < CRUMB_H + FOOT_H + 20.0 {
            cx.add_aligned_rect_area(&mut self.area, rect);
            return DrawStep::done();
        }

        let crumb_strip = Rect {
            pos: rect.pos,
            size: dvec2(rect.size.x, CRUMB_H),
        };
        let foot_strip = Rect {
            pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - FOOT_H),
            size: dvec2(rect.size.x, FOOT_H),
        };
        let body = Rect {
            pos: dvec2(rect.pos.x + 1.0, rect.pos.y + CRUMB_H + 1.0),
            size: dvec2(
                rect.size.x - 2.0,
                rect.size.y - CRUMB_H - FOOT_H - 2.0,
            ),
        };
        let now = cx.seconds_since_app_start();

        // Layout before the chrome: the footer reads the pick's numbers and
        // the relayout is what refreshes them, so a frame that did both in
        // the other order would print a stale size and never come back for
        // the right one.
        if !self.tree.children.is_empty() {
            if self.laid_out != body {
                self.relayout(body, now);
            } else if self.cam_in_motion() {
                // A camera gesture owns the picture: it rides the remap,
                // visually rigid, and the layout underneath only refreshes
                // on a coarse cadence — each refresh a morph that brings in
                // new detail and cull, never a per-frame reshuffle. One
                // thing outranks the cadence: the view escaping the laid
                // cull. A coarse picture can wait 150ms; bare background
                // cannot wait one frame — the relayout runs here, before
                // this same frame paints, so unlaid ground is never shown.
                if self.view_escaped_cull() || self.motion_refresh_due(now) {
                    if self.tween_capture.is_none() {
                        self.tween_calm = !self.stale;
                        self.tween_capture = Some(self.visual_snapshot(now));
                    }
                    self.relayout(body, now);
                } else if self.stale {
                    self.frame = cx.new_next_frame();
                }
            } else if self.stale && self.layout_is_due(now) {
                self.relayout(body, now);
            } else if self.stale {
                // Drawn from a picture the scan has already moved past. The
                // throttle says not yet, so come back for it — a skipped
                // relayout that nothing ever comes back for is a map that
                // stops updating and never says so.
                self.frame = cx.new_next_frame();
            }
        }

        self.draw_crumbs(cx, crumb_strip, palette);
        self.draw_footer(cx, foot_strip, palette);

        if self.tree.children.is_empty() {
            self.draw_text.color = Palette::vec4(&palette.fg_dim);
            let text = self.error.clone().unwrap_or_else(|| {
                if self.scanning {
                    "Reading the folder…".to_string()
                } else {
                    "Nothing to map".to_string()
                }
            });
            self.draw_text
                .draw_abs(cx, body.pos + dvec2(16.0, 16.0), &text);
            cx.add_aligned_rect_area(&mut self.area, rect);
            return DrawStep::done();
        }

        let labels = self.draw_map(cx, palette, body, now);
        if self.projection == MapProjection::Flat && self.tween_t(now).is_none() {
            // Outside a tween the flat map's names are frozen against the
            // layout and merely ride the remap — nothing about them changes
            // frame to frame, which is what keeps a glide quiet.
            self.ensure_frozen_labels(cx, palette, body);
            self.draw_frozen_labels(cx, palette, body);
        } else {
            self.draw_labels(cx, labels, body, palette);
        }
        self.draw_tooltip(cx, body, palette);

        // A running tween owns the frame clock; the frame after it ends
        // draws the exact target state, and only then is it let go of.
        if self.tween_t(now).is_some() {
            self.frame = cx.new_next_frame();
        } else if self.tween_start.is_some() {
            self.tween_start = None;
            self.tween_from.clear();
            self.tween_leavers.clear();
        }

        cx.add_aligned_rect_area(&mut self.area, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let now = cx.seconds_since_app_start();
        if self.frame.is_event(event).is_some() {
            self.step_glides(cx);
            if self.tween_t(now).is_some() {
                self.redraw(cx);
            }
            if self.stale {
                if self.layout_is_due(now) {
                    self.redraw(cx);
                } else {
                    self.frame = cx.new_next_frame();
                }
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                cx.set_cursor(MouseCursor::Arrow);
                let hover = self.hit_cell(e.abs);
                // Only when the rectangle under the pointer actually changes:
                // the tooltip is anchored to the cell, not to the pointer, so
                // moving inside one cell costs nothing.
                if hover != self.hover {
                    self.hover = hover;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(e) => {
                // A hand on the map takes the camera back from any glide.
                self.zoom_glide = None;
                self.yaw_glide = None;
                // Either button: click or that button's drag gesture, decided
                // on release — so a click never nudges the camera and a drag
                // never changes the selection or opens the menu.
                let secondary = !e.device.is_primary_hit() || e.modifiers.control;
                self.drag = Some(Drag {
                    from: e.abs,
                    cam_off: self.cam_off,
                    yaw: self.yaw,
                    pitch: self.pitch,
                    taps: e.tap_count,
                    secondary,
                    moved: false,
                });
            }
            Hit::FingerMove(e) => {
                if let Some(mut drag) = self.drag {
                    let delta = e.abs - drag.from;
                    if !drag.moved && delta.length() > DRAG_THRESHOLD {
                        drag.moved = true;
                    }
                    if drag.moved {
                        let raised = self.projection != MapProjection::Flat;
                        if !drag.secondary && raised {
                            // Left-drag orbits: yaw with the hand, pitch with
                            // the reach, both measured from the press point.
                            self.set_orbit(
                                cx,
                                drag.yaw - delta.x * ORBIT_PER_PT,
                                drag.pitch + delta.y * PITCH_PER_PT,
                            );
                        } else if self.cam_scale > 1.001 {
                            // Right-drag pans (and so does left-drag on the
                            // flat map): the map follows the finger, the
                            // screen delta un-spun into layout space.
                            let shift = if raised {
                                let squash = self.pitch.cos().max(0.25);
                                let dy = delta.y / squash;
                                dvec2(
                                    delta.x * self.yaw.cos() + dy * self.yaw.sin(),
                                    -delta.x * self.yaw.sin() + dy * self.yaw.cos(),
                                )
                            } else {
                                delta
                            };
                            self.set_camera(cx, self.cam_scale, drag.cam_off - shift);
                        }
                    }
                    self.drag = Some(drag);
                }
            }
            Hit::FingerUp(_) => {
                if let Some(drag) = self.drag.take() {
                    if !drag.moved {
                        self.press(cx, drag.from, drag.taps, !drag.secondary);
                        if drag.secondary {
                            // A clean secondary click: the context menu's
                            // moment — the pick above has already landed.
                            cx.widget_action(self.uid, TreemapAction::Context(drag.from));
                        }
                    } else {
                        // The gesture is over: the layout catches up with
                        // wherever the hand left the camera, morphing there.
                        self.settle(cx);
                    }
                }
            }
            Hit::FingerScroll(e) => {
                // Wheel/two fingers zoom about the pointer. The exponent
                // makes equal wheel travel worth equal zoom *ratio*, which
                // is the only way in and out feel like the same control.
                // The step retargets a glide rather than jumping the camera:
                // the ease runs on the frame clock, always about the ground
                // point that was under the cursor.
                let factor = (-e.scroll.y * 0.011).exp();
                let cam = self.cam_at(self.laid_out);
                // Anchor on the surface the cursor actually rests on: in the
                // raised projections that is a tile top, and unprojecting at
                // its elevation keeps THAT point pinned instead of the ground
                // hiding behind a tall tower.
                let z = if self.projection == MapProjection::Flat {
                    0.0
                } else {
                    self.hit_cell(e.abs)
                        .map_or(0.0, |i| self.elev(self.cells[i].depth))
                };
                let anchor = cam.unproject_at(e.abs, z);
                let base = self
                    .zoom_glide
                    .map_or(self.cam_scale.max(1.0), |glide| glide.target);
                self.zoom_glide = Some(ZoomGlide {
                    target: (base * factor).clamp(1.0, 512.0),
                    anchor,
                    last: cx.seconds_since_app_start(),
                });
                self.frame = cx.new_next_frame();
            }
            _ => {}
        }
    }
}

impl TreemapViewRef {
    pub fn set_root(&self, cx: &mut Cx, path: &Path) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_root(cx, path);
        }
    }

    pub fn stop(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.stop(cx);
        }
    }

    pub fn drain(&self, cx: &mut Cx) -> bool {
        self.borrow_mut().map(|mut i| i.drain(cx)).unwrap_or(false)
    }

    pub fn status(&self) -> String {
        self.borrow().map(|i| i.status()).unwrap_or_default()
    }

    pub fn root(&self) -> PathBuf {
        self.borrow().map(|i| i.root().to_path_buf()).unwrap_or_default()
    }

    pub fn set_selected(&self, cx: &mut Cx, path: Option<PathBuf>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, path);
        }
    }

    pub fn selection(&self) -> Option<PathBuf> {
        self.borrow().and_then(|i| i.selection())
    }

    pub fn path_at(&self, pos: DVec2) -> Option<PathBuf> {
        self.borrow().and_then(|i| i.path_at(pos))
    }

    /// Measure the disk again, replacing whatever was cached.
    pub fn rescan(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.rescan(cx);
        }
    }

    /// Re-open the current root under the current scan rules, cache welcome.
    pub fn remap(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.remap(cx);
        }
    }

    /// Choose how the map projects: flat, extruded, or perspective.
    pub fn set_projection(&self, cx: &mut Cx, projection: MapProjection) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_projection(cx, projection);
        }
    }

    /// Nudge the orbit camera — the keyboard's turn keys. A no-op on the
    /// flat map.
    pub fn orbit_by(&self, cx: &mut Cx, dyaw: f64, dpitch: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.orbit_by(cx, dyaw, dpitch);
        }
    }

    /// Apply (or clear, with None) the live filter.
    pub fn set_filter(&self, cx: &mut Cx, filter: Option<Query>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_filter(cx, filter);
        }
    }

    /// Bytes per kind tag under the mapped folder, for the legend.
    pub fn kind_totals(&self, _cx: &mut Cx) -> [u64; 16] {
        self.borrow_mut().map(|mut i| i.kind_totals()).unwrap_or([0; 16])
    }

    /// Fold finished moves and deletes into the map rather than rescanning.
    pub fn absorb_moves(&self, cx: &mut Cx, moves: &[(PathBuf, Option<PathBuf>)]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.absorb_moves(cx, moves);
        }
    }

    /// Fold finished copies into the map rather than rescanning.
    pub fn absorb_copies(&self, cx: &mut Cx, copies: &[(PathBuf, PathBuf)]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.absorb_copies(cx, copies);
        }
    }

    /// Step back out one zoom level. False when there was nowhere to go.
    pub fn zoom_out(&self, cx: &mut Cx) -> bool {
        self.borrow_mut().map(|mut i| i.zoom_out(cx)).unwrap_or(false)
    }

    /// Zoom into whatever the last click picked, when that was a folder.
    pub fn zoom_into_selection(&self, cx: &mut Cx) -> bool {
        let Some(mut inner) = self.borrow_mut() else {
            return false;
        };
        let Some(path) = inner.selection() else {
            return false;
        };
        inner.zoom_into(cx, &path)
    }

}

fn pick_of(cell: &Cell) -> Pick {
    Pick {
        path: cell.path.clone(),
        size: cell.size,
        files: cell.files,
        is_dir: cell.is_dir,
        bundle: cell.extra,
    }
}

fn lerp_rect(a: &MapRect, b: &MapRect, t: f64) -> MapRect {
    MapRect {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        w: a.w + (b.w - a.w) * t,
        h: a.h + (b.h - a.h) * t,
    }
}

/// The affine ride from a layout camera to the live one: `screen = g·k + b`.
/// The layout put a map fraction `u` at `body − layout_off + u·body·scale`;
/// the live camera would put it at the same expression with its own scale
/// and offset, and this is the unique uniform scale-and-shift between the
/// two. Applying it to every laid-out rect moves the whole picture as one
/// rigid sheet — no re-flow, by construction.
fn remap_params(
    body: Rect,
    layout_scale: f64,
    layout_off: DVec2,
    cam_scale: f64,
    cam_off: DVec2,
) -> (f64, DVec2) {
    let k = cam_scale.max(1.0) / layout_scale.max(1.0);
    (
        k,
        dvec2(
            (body.pos.x - cam_off.x) - (body.pos.x - layout_off.x) * k,
            (body.pos.y - cam_off.y) - (body.pos.y - layout_off.y) * k,
        ),
    )
}

/// Whether a translated label anchor has slid far enough off the panel that
/// nothing of its text could show.
fn off_panel(at: DVec2, width: f64, clip: &Rect) -> bool {
    at.x + width < clip.pos.x
        || at.x > clip.pos.x + clip.size.x
        || at.y + 12.0 < clip.pos.y
        || at.y > clip.pos.y + clip.size.y
}

fn remap_rect(r: &MapRect, k: f64, b: DVec2) -> MapRect {
    MapRect {
        x: r.x * k + b.x,
        y: r.y * k + b.y,
        w: r.w * k,
        h: r.h * k,
    }
}

/// Emit one face: the four projected corners into the instance, the
/// bounding box into the rect the area system tracks.
fn face(draw: &mut DrawMapTile, cx: &mut Cx2d, quad: &Quad) {
    draw.c0 = v2f(quad.p[0]);
    draw.c1 = v2f(quad.p[1]);
    draw.c2 = v2f(quad.p[2]);
    draw.c3 = v2f(quad.p[3]);
    draw.draw_abs(cx, quad.bounds());
}

fn v2f(v: DVec2) -> Vec2f {
    Vec2f {
        x: v.x as f32,
        y: v.y as f32,
    }
}

/// `a`, wrapped into (-π, π] so a long orbit never accumulates.
fn wrap_angle(a: f64) -> f64 {
    let mut a = a % std::f64::consts::TAU;
    if a > std::f64::consts::PI {
        a -= std::f64::consts::TAU;
    } else if a <= -std::f64::consts::PI {
        a += std::f64::consts::TAU;
    }
    a
}

/// One visible vertical face of a prism, and how brightly it is lit.
struct Wall {
    quad: Quad,
    shade: f32,
}

/// The walls of the prism standing on `r` between elevations `z - band` and
/// `z` that face the camera — the sides whose outward normal has a
/// screen-downward component under the current yaw. Never more than two.
fn wall_quads(cam: &Cam, r: &MapRect, z: f64, band: f64) -> [Option<Wall>; 2] {
    let edges: [(DVec2, DVec2, DVec2); 4] = [
        // (a, b, outward normal), each edge in layout space.
        (dvec2(r.x, r.y), dvec2(r.x + r.w, r.y), dvec2(0.0, -1.0)),
        (dvec2(r.x + r.w, r.y + r.h), dvec2(r.x, r.y + r.h), dvec2(0.0, 1.0)),
        (dvec2(r.x, r.y + r.h), dvec2(r.x, r.y), dvec2(-1.0, 0.0)),
        (dvec2(r.x + r.w, r.y), dvec2(r.x + r.w, r.y + r.h), dvec2(1.0, 0.0)),
    ];
    let mut out = [None, None];
    let mut slot = 0;
    for (a, b, n) in edges {
        // The normal's screen-down component after the yaw spin.
        let down = n.x * cam.sin_yaw + n.y * cam.cos_yaw;
        if down <= 0.02 || slot >= 2 {
            continue;
        }
        out[slot] = Some(Wall {
            quad: Quad {
                p: [
                    cam.project(a, z),
                    cam.project(b, z),
                    cam.project(b, z - band),
                    cam.project(a, z - band),
                ],
            },
            // Faces turned toward the camera catch more of the light.
            shade: 0.30 + 0.20 * down as f32,
        });
        slot += 1;
    }
    out
}

/// Whether a prism standing on `a` can lean out over `b`: the swept region
/// `a + t·d, t > 0` meets `b`. Exact for axis-aligned rects — the sweep is a
/// t-interval per axis and the intervals either meet in t > 0 or never.
fn leans_over(a: &MapRect, b: &MapRect, d: DVec2) -> bool {
    let mut lo = f64::MIN;
    let mut hi = f64::MAX;
    for (dir, a_min, a_max, b_min, b_max) in [
        (d.x, a.x, a.x + a.w, b.x, b.x + b.w),
        (d.y, a.y, a.y + a.h, b.y, b.y + b.h),
    ] {
        if dir.abs() < 1e-9 {
            if a_max <= b_min || b_max <= a_min {
                return false;
            }
            continue;
        }
        let t0 = (b_min - a_max) / dir;
        let t1 = (b_max - a_min) / dir;
        let (t0, t1) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
        lo = lo.max(t0);
        hi = hi.min(t1);
    }
    lo < hi && hi > 1e-6
}

/// Past this many sibling subtrees the exact pairwise ordering costs more
/// than its correctness is worth; the scalar sort is right for everything
/// but pathologically elongated neighbours.
const EXACT_ORDER_MAX: usize = 400;

/// The paint order for the raised projections under an orbiting camera:
/// towers lean along `lean` (the layout direction that reads as screen-up),
/// so a subtree may only cover subtrees whose footprints lie along that
/// lean from its own. Per sibling level, blocks are ordered so that
/// whatever can be leaned over paints first — an exact pairwise sweep
/// test folded into a cycle-safe depth-first emit. Nesting still means
/// "parent under child", so the ordering happens among siblings and each
/// subtree stays together; the input is the layout's pre-order, which
/// keeps every subtree contiguous.
fn view_order(cells: &[Cell], lean: DVec2) -> Vec<usize> {
    fn emit(cells: &[Cell], start: usize, end: usize, depth: usize, lean: DVec2, out: &mut Vec<usize>) {
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        let mut i = start;
        while i < end {
            let s = i;
            i += 1;
            while i < end && cells[i].depth > depth {
                i += 1;
            }
            blocks.push((s, i));
        }
        // Farthest along the lean first: at yaw 0 that is exactly the old
        // "north before south". This scalar order is the seed (and the
        // whole answer when the level is too wide for the exact pass).
        let mut order: Vec<usize> = (0..blocks.len()).collect();
        let along = |bi: usize| {
            let r = &cells[blocks[bi].0].rect;
            (r.x + r.w * 0.5) * lean.x + (r.y + r.h * 0.5) * lean.y
        };
        order.sort_by(|&a, &b| {
            along(b).partial_cmp(&along(a)).unwrap_or(std::cmp::Ordering::Equal)
        });
        if blocks.len() > 1 && blocks.len() <= EXACT_ORDER_MAX {
            // The exact pass: block `a` must wait for every block it can
            // lean over. Depth-first over "who must paint before me", with
            // an in-progress mark so a (theoretical) cycle degrades to a
            // local misordering instead of a hang.
            let n = blocks.len();
            let mut state = vec![0u8; n]; // 0 fresh, 1 visiting, 2 emitted
            let mut ordered: Vec<usize> = Vec::with_capacity(n);
            // Iterative DFS: (block, next seed-order candidate to examine).
            let mut stack: Vec<(usize, usize)> = Vec::new();
            for seed in 0..n {
                let root = order[seed];
                if state[root] != 0 {
                    continue;
                }
                state[root] = 1;
                stack.push((root, 0));
                while let Some(top) = stack.pop() {
                    let (node, mut cursor) = top;
                    let a = cells[blocks[node].0].rect;
                    let mut descend = None;
                    while cursor < n {
                        let j = order[cursor];
                        cursor += 1;
                        if state[j] == 0 && leans_over(&a, &cells[blocks[j].0].rect, lean) {
                            descend = Some(j);
                            break;
                        }
                    }
                    match descend {
                        Some(j) => {
                            stack.push((node, cursor));
                            state[j] = 1;
                            stack.push((j, 0));
                        }
                        None => {
                            state[node] = 2;
                            ordered.push(node);
                        }
                    }
                }
            }
            order = ordered;
        }
        for bi in order {
            let (s, e) = blocks[bi];
            out.push(s);
            emit(cells, s + 1, e, depth + 1, lean, out);
        }
    }
    let mut out = Vec::with_capacity(cells.len());
    emit(cells, 0, cells.len(), 0, lean, &mut out);
    out
}

/// `a` over `b` at `t`, in premultiplication-free straight colour.
fn blend(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    Vec4f {
        x: a.x * (1.0 - t) + b.x * t,
        y: a.y * (1.0 - t) + b.y * t,
        z: a.z * (1.0 - t) + b.z * t,
        w: 1.0,
    }
}

fn scale_rgb(c: Vec4f, k: f32) -> Vec4f {
    Vec4f {
        x: c.x * k,
        y: c.y * k,
        z: c.z * k,
        w: c.w,
    }
}

fn fade(c: Vec4f, k: f32) -> Vec4f {
    Vec4f { w: c.w * k, ..c }
}

/// The width one line of text would take, in points.
fn text_width(draw: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let laid = draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    laid.rows
        .first()
        .map(|r| r.width_in_lpxs as f64)
        .unwrap_or(0.0)
}

/// `text`, shortened with an ellipsis until it fits in `room` points. The
/// first guess comes from the measured width, so the loop almost never runs
/// more than once — measuring is cached per string, but a treemap draws
/// hundreds of labels a frame and every one of them has to be cheap.
fn fit_text(draw: &DrawText, cx: &mut Cx2d, text: &str, room: f64) -> String {
    if room <= 6.0 {
        return String::new();
    }
    let full = text_width(draw, cx, text);
    if full <= room {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = ((room / full) * chars.len() as f64) as usize;
    for _ in 0..4 {
        keep = keep.min(chars.len().saturating_sub(1));
        if keep == 0 {
            return "…".to_string();
        }
        let candidate: String = chars[..keep].iter().collect::<String>() + "…";
        if text_width(draw, cx, &candidate) <= room {
            return candidate;
        }
        keep = keep * 4 / 5;
    }
    "…".to_string()
}

/// `text`, shortened from its *front* until it fits in `room` points. For a
/// path that is the right end to cut: `…/Sim/Devices/device-a/data.img` still
/// says which file this is, and `/private/tmp/claude-501/-Users-…` says
/// nothing at all.
fn fit_tail(draw: &DrawText, cx: &mut Cx2d, text: &str, room: f64) -> String {
    if room <= 6.0 {
        return String::new();
    }
    let full = text_width(draw, cx, text);
    if full <= room {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = ((room / full) * chars.len() as f64) as usize;
    for _ in 0..4 {
        keep = keep.min(chars.len().saturating_sub(1));
        if keep == 0 {
            return "…".to_string();
        }
        let candidate: String =
            String::from("…") + &chars[chars.len() - keep..].iter().collect::<String>();
        if text_width(draw, cx, &candidate) <= room {
            return candidate;
        }
        keep = keep * 4 / 5;
    }
    "…".to_string()
}

/// [`FileKind`] in discriminant order, so the opaque `u8` the scan carried —
/// which for this app is a `FileKind` discriminant — can be read back. A
/// change to the enum's order breaks this, which is what the test below is
/// for.
const FILE_KINDS: [FileKind; 9] = [
    FileKind::Folder,
    FileKind::Image,
    FileKind::Text,
    FileKind::Code,
    FileKind::Audio,
    FileKind::Video,
    FileKind::Archive,
    FileKind::Pdf,
    FileKind::Generic,
];

/// The kind a cell paints as.
fn cell_kind(cell: &Cell) -> FileKind {
    FILE_KINDS
        .get(cell.kind as usize)
        .copied()
        .unwrap_or(FileKind::Generic)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The visual-constancy contract: remapping a point laid out under one
    // camera must land it exactly where laying out under the live camera
    // would put it. This is the whole reason a wheel zoom no longer
    // re-flows — the picture between relayouts IS this map.
    #[test]
    fn the_remap_rides_exactly_where_a_fresh_layout_would_put_things() {
        let body = Rect { pos: dvec2(10.0, 20.0), size: dvec2(800.0, 600.0) };
        let cases = [
            ((1.0, dvec2(0.0, 0.0)), (3.0, dvec2(140.0, 260.0))),
            ((2.0, dvec2(100.0, 50.0)), (2.0, dvec2(300.0, 120.0))),
            ((4.0, dvec2(900.0, 400.0)), (1.0, dvec2(0.0, 0.0))),
            ((3.0, dvec2(10.0, 700.0)), (7.5, dvec2(0.0, 40.0))),
        ];
        for ((ls, lo), (cs, co)) in cases {
            let (k, b) = remap_params(body, ls, lo, cs, co);
            for u in [dvec2(0.0, 0.0), dvec2(0.25, 0.75), dvec2(1.0, 1.0)] {
                // Where the layout camera puts map fraction u…
                let laid = dvec2(
                    body.pos.x - lo.x + u.x * body.size.x * ls,
                    body.pos.y - lo.y + u.y * body.size.y * ls,
                );
                // …and where the live camera would.
                let live = dvec2(
                    body.pos.x - co.x + u.x * body.size.x * cs,
                    body.pos.y - co.y + u.y * body.size.y * cs,
                );
                assert!((laid.x * k + b.x - live.x).abs() < 1e-9, "x at {u:?}");
                assert!((laid.y * k + b.y - live.y).abs() < 1e-9, "y at {u:?}");
            }
        }
    }

    #[test]
    fn the_remap_is_the_identity_when_the_camera_rests_on_its_layout() {
        let body = Rect { pos: dvec2(0.0, 0.0), size: dvec2(640.0, 480.0) };
        let (k, b) = remap_params(body, 2.5, dvec2(31.0, 7.0), 2.5, dvec2(31.0, 7.0));
        assert!((k - 1.0).abs() < 1e-12);
        assert!(b.x.abs() < 1e-9 && b.y.abs() < 1e-9);
        let r = MapRect { x: 5.0, y: 6.0, w: 7.0, h: 8.0 };
        let m = remap_rect(&r, k, b);
        assert!((m.x - r.x).abs() < 1e-9 && (m.w - r.w).abs() < 1e-9);
    }

    // Hit-testing mid-gesture inverts the remap on the pointer; the two
    // directions must be exact inverses or hover drifts off what is drawn.
    #[test]
    fn a_point_round_trips_through_the_remap_and_its_inverse() {
        let body = Rect { pos: dvec2(1.0, 2.0), size: dvec2(500.0, 300.0) };
        let (k, b) = remap_params(body, 1.0, dvec2(0.0, 0.0), 5.0, dvec2(700.0, 300.0));
        for p in [dvec2(3.0, 4.0), dvec2(250.0, 150.0), dvec2(499.0, 299.0)] {
            let s = dvec2(p.x * k + b.x, p.y * k + b.y);
            let back = dvec2((s.x - b.x) / k, (s.y - b.y) / k);
            assert!((back.x - p.x).abs() < 1e-9 && (back.y - p.y).abs() < 1e-9);
        }
    }

    #[test]
    fn the_kind_tag_survives_the_round_trip_through_a_byte() {
        // The scan stores a FileKind as its discriminant; FILE_KINDS turns it
        // back. If the enum ever gains a variant in the middle, this fails
        // before the map starts painting videos as archives.
        for (index, kind) in FILE_KINDS.iter().enumerate() {
            assert_eq!(*kind as usize, index, "{kind:?}");
        }
    }

    #[test]
    fn every_kind_lands_on_a_palette_class() {
        let palette = Palette::tokyo_night();
        for kind in FILE_KINDS {
            let class = kind_class(kind) as usize;
            assert!(class < palette.kinds.len(), "{kind:?} -> {class}");
        }
        // The classes that carry the picture are all different colors.
        let video = kind_class(FileKind::Video);
        let image = kind_class(FileKind::Image);
        let archive = kind_class(FileKind::Archive);
        assert_ne!(video, image);
        assert_ne!(image, archive);
    }

    fn cell(name: &str, depth: usize, y: f64) -> Cell {
        Cell {
            path: PathBuf::from(format!("/{name}")),
            name: name.to_string(),
            size: 1,
            files: 1,
            is_dir: true,
            kind: 0,
            depth,
            rect: MapRect { x: 0.0, y, w: 10.0, h: 10.0 },
            is_group: true,
            header: 0.0,
            pending: false,
            extra: 0,
        }
    }

    fn cell_at(name: &str, depth: usize, x: f64, y: f64) -> Cell {
        let mut c = cell(name, depth, y);
        c.rect.x = x;
        c
    }

    fn names(cells: &[Cell], order: &[usize]) -> Vec<String> {
        order.iter().map(|&i| cells[i].name.clone()).collect()
    }

    /// The lean direction the camera at `yaw` produces — the same formula
    /// the view uses.
    fn lean_of(yaw: f64) -> DVec2 {
        dvec2(-yaw.sin(), -yaw.cos())
    }

    // The rule that makes the extruded map paint correctly, at every yaw:
    // towers lean along the camera's lean direction, so whatever can be
    // leaned over paints first — among siblings, with each subtree kept
    // together and parents under their children.
    #[test]
    fn the_view_order_follows_the_lean_at_yaw_zero() {
        // Pre-order: P(y=50) with children c1(y=90), c2(y=60); then Q(y=0).
        let cells = vec![
            cell("p", 0, 50.0),
            cell("c1", 1, 90.0),
            cell("c2", 1, 60.0),
            cell("q", 0, 0.0),
        ];
        // Yaw 0 leans north: north paints first.
        let order = view_order(&cells, lean_of(0.0));
        assert_eq!(names(&cells, &order), vec!["q", "p", "c2", "c1"]);
    }

    #[test]
    fn a_half_turn_reverses_the_order_a_quarter_turn_orders_by_x() {
        let cells = vec![
            cell("p", 0, 50.0),
            cell("c1", 1, 90.0),
            cell("c2", 1, 60.0),
            cell("q", 0, 0.0),
        ];
        // 180°: everything leans south now, so south paints first.
        let order = view_order(&cells, lean_of(std::f64::consts::PI));
        assert_eq!(names(&cells, &order), vec!["p", "c1", "c2", "q"]);

        // 90°: the lean is westward — order follows x, subtrees intact.
        let cells = vec![
            cell_at("east", 0, 100.0, 0.0),
            cell_at("kid", 1, 110.0, 0.0),
            cell_at("west", 0, 0.0, 0.0),
        ];
        let order = view_order(&cells, lean_of(std::f64::consts::FRAC_PI_2));
        assert_eq!(names(&cells, &order), vec!["west", "east", "kid"]);
    }

    // The case a scalar sort gets wrong and the sweep test does not: a tall
    // thin tower diagonally behind a long flat neighbour. The sweep of the
    // tower's footprint along the lean reaches the slab, so the slab must
    // paint first — wherever their centres happen to sit.
    #[test]
    fn the_sweep_test_orders_elongated_neighbours_correctly() {
        let tower = MapRect { x: 0.0, y: 0.0, w: 10.0, h: 100.0 };
        let slab = MapRect { x: 10.0, y: -200.0, w: 10.0, h: 245.0 };
        let d = dvec2(std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2);
        assert!(leans_over(&tower, &slab, d));
        // And never both ways along one direction.
        assert!(!leans_over(&slab, &tower, d));

        let mut a = cell_at("tower", 0, 0.0, 0.0);
        a.rect = tower;
        let mut b = cell_at("slab", 0, 10.0, -200.0);
        b.rect = slab;
        let cells = vec![a, b];
        let order = view_order(&cells, d);
        assert_eq!(names(&cells, &order), vec!["slab", "tower"]);
    }

    // The camera's projection and its inverse agree on the ground plane, at
    // any spin and tilt, in both projections.
    #[test]
    fn the_camera_unprojects_its_own_ground() {
        for persp in [false, true] {
            for (yaw, pitch) in [(0.0, 0.0), (0.7, 0.66), (-2.1, 1.1), (3.0, 0.2)] {
                let cam = Cam {
                    pivot: dvec2(500.0, 380.0),
                    sin_yaw: f64::sin(yaw),
                    cos_yaw: f64::cos(yaw),
                    sin_pitch: f64::sin(pitch),
                    cos_pitch: f64::cos(pitch),
                    persp,
                };
                for p in [dvec2(0.0, 0.0), dvec2(731.0, 12.0), dvec2(400.0, 900.0)] {
                    let s = cam.project(p, 0.0);
                    let back = cam.unproject_ground(s);
                    assert!(
                        (back - p).length() < 1e-6,
                        "persp={persp} yaw={yaw} pitch={pitch}: {p:?} -> {s:?} -> {back:?}"
                    );
                }
                // And under the ortho eye, elevation only ever moves things
                // screen-up: the whole meaning of the raised map. (The
                // perspective eye adds a radial swell on top, so the claim
                // is ortho's alone.)
                if !persp && pitch > 0.0 {
                    let flat = cam.project(dvec2(600.0, 500.0), 0.0);
                    let high = cam.project(dvec2(600.0, 500.0), 40.0);
                    assert!(high.y < flat.y);
                }
            }
        }
    }

    // The bundle rectangle carries a kind no palette class answers to, and it
    // must fall through to "other" rather than index off the end.
    #[test]
    fn the_bundle_tag_is_not_a_file_kind() {
        assert!(KIND_BUNDLE as usize >= FILE_KINDS.len());
        let palette = Palette::tokyo_night();
        let class = kind_class(FileKind::Generic) as usize;
        assert!(class < palette.kinds.len());
    }
}
