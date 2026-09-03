//! Retained per-tile draw lists.
//!
//! Every resident tile owns one platform draw list per global carto pass
//! (fills, road casings, road centres, POI symbols, street-band symbols, and
//! the shadow-mask cast). A list is RECORDED — its draw calls and instance
//! buffers declared through the ordinary `DrawMap*` drawers — once: when the
//! tile's bake arrives, and again only when something that changes its call
//! set flips (a cross-fade ending, an LOD ring, the camera going flat or
//! tilted, the clip rect, a shader compiling). On every other frame the list
//! is merely re-attached to the map's draw list and each of its calls gets
//! fresh uniforms, so a pan, zoom, rotation or tilt is a uniform write per
//! draw call and ZERO instance bytes: the platform re-presents the resident
//! buffers (`CxDrawCall::instance_dirty` never rises for them).
//!
//! Paint order. The flat map orders by the backend's paint-order counter
//! (one `zbias_step` per draw call) and authors its own layering on top:
//! every chunk of one stream shares a depth, road faces sit with the casing
//! they belong to, the AA fringe one step above. A retained list is one unit
//! of that counter (`CxDrawList::zbias_hold`): all its calls resolve to the
//! counter at entry, the tile's internal layering is baked into the
//! instances' `draw_depth` in units of the step, and the counter advances on
//! exit by the number of layers the list used — the same values the
//! per-frame immediate path produced call by call.

use super::geometry::{TileKey, TILE_SIZE};
use super::icons::ICON_MIN_ZOOM;
use super::style::stroke_width_correction;
use super::tile::*;
use super::view::*;
use crate::makepad_draw::*;
use std::collections::HashMap;

/// One retained list per global carto pass. The passes are global (every
/// tile's fills, then every tile's casings, ...) — a tile cannot be a single
/// list, or its casings would stamp over the neighbour's road interiors in
/// the clip-padding overlap at the seams.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TilePhase {
    /// Ground fills, building outlines and, under tilt, the 3D volume.
    Fill,
    /// Road union faces, casings and the AA fringe.
    Casing,
    /// Road centres.
    Stroke,
    /// Vertex-baked road decals and instanced POI symbols.
    Icon,
    /// The street-band symbols (zoom floor > 16), drawn only when revealed.
    IconHigh,
    /// Everything cast into the shadow-mask pass.
    Shadow,
}

pub const TILE_PHASE_COUNT: usize = 6;

impl TilePhase {
    fn index(self) -> usize {
        match self {
            TilePhase::Fill => 0,
            TilePhase::Casing => 1,
            TilePhase::Stroke => 2,
            TilePhase::Icon => 3,
            TilePhase::IconHigh => 4,
            TilePhase::Shadow => 5,
        }
    }

    /// The tilted camera's per-pass depth offset step (`pass_boost + k * 0.02`
    /// keeps casing/centre/icon layering when depth comes from the ground
    /// ladder). The fill pass and the mask pass sit at the ladder itself.
    fn tilt_depth_k(self) -> Option<f32> {
        match self {
            TilePhase::Fill | TilePhase::Shadow => None,
            TilePhase::Casing => Some(0.0),
            TilePhase::Stroke => Some(1.0),
            TilePhase::Icon => Some(2.0),
            TilePhase::IconHigh => Some(3.0),
        }
    }
}

/// Which `MapView` drawer a retained call was recorded through; its
/// `DrawVars` is the staging copy the refreshed uniforms go through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TileDrawer {
    Vector,
    Fill,
    Road,
    Face,
    Roof,
    Icon,
    Prop,
    Wall,
    Shadow,
    ShadowDisc,
}

pub const TILE_DRAWER_COUNT: usize = 10;

impl TileDrawer {
    fn index(self) -> usize {
        match self {
            TileDrawer::Vector => 0,
            TileDrawer::Fill => 1,
            TileDrawer::Road => 2,
            TileDrawer::Face => 3,
            TileDrawer::Roof => 4,
            TileDrawer::Icon => 5,
            TileDrawer::Prop => 6,
            TileDrawer::Wall => 7,
            TileDrawer::Shadow => 8,
            TileDrawer::ShadowDisc => 9,
        }
    }
}

/// Where a call's `tile_fade` uniform comes from each frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FadeSource {
    /// Fully opaque: the outgoing generation under a cross-fade, and road
    /// geometry a mode-only rebake kept resident.
    Full,
    /// The tile's cross-fade progress.
    Alpha,
}

/// Whose zoom bucket the stroke width correction is computed for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WidthSource {
    Resident,
    /// The replaced generation still drawing underneath during a fade.
    Outgoing,
}

/// Where a call's `height_grow` uniform comes from each frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeightSource {
    Full,
    /// The mask pass's footprint cut-out at ground.
    Zero,
    /// The flat->3D reveal: the fade progress while the mode grows heights.
    Grow,
    /// 3D volume: the reveal times the distance LOD ring's sink.
    LodGrow,
}

/// One retained draw call and how to rebuild its uniforms each frame.
#[derive(Clone, Copy, Debug)]
pub struct TileCallSpec {
    pub area: Area,
    pub drawer: TileDrawer,
    pub fade: FadeSource,
    pub width: WidthSource,
    pub height: HeightSource,
    /// Mask-pass stage (0 wall silhouettes, 1 lifted projections, 2 ground
    /// cut-outs, 3 contact discs); 0 in the main pass.
    pub shadow_cast: f32,
}

/// What a recorded list's call set depended on. A different signature next
/// frame means the list is recorded again; equal means it is re-attached.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TilePhaseSig {
    /// Flat-camera paint order (instances carry the layering); tilted or
    /// exploded depth ignores the counter and bakes zero.
    pub flat_fix: bool,
    /// The clip the aligned instances were stamped with at `end_turtle`.
    pub clip: Rect,
    /// Distance LOD ring: 0 none, 1 mid (crossed-quad trees), 2 near.
    pub lod_band: u8,
    /// AA fringes drawn (tilt under 25 degrees).
    pub fringe: bool,
    /// Symbol passes drawn at this zoom for this tile's bucket.
    pub icons: bool,
    /// Bit per drawer whose shader had compiled when the list was recorded.
    pub shaders: u16,
}

#[derive(Debug)]
pub struct TilePhaseList {
    pub list: DrawList2d,
    pub calls: Vec<TileCallSpec>,
    pub sig: Option<TilePhaseSig>,
}

impl TilePhaseList {
    fn new(cx: &mut Cx) -> Self {
        Self {
            list: DrawList2d::new(cx),
            calls: Vec::new(),
            sig: None,
        }
    }
}

/// A tile's retained lists, one per phase, allocated the first frame the
/// phase draws the tile and freed with the entry.
#[derive(Default, Debug)]
pub struct TileDrawLists {
    phases: [Option<TilePhaseList>; TILE_PHASE_COUNT],
}

impl TileDrawLists {
    /// The next frame records every phase again. Called whenever the entry's
    /// drawable content changes under it (a cross-fade ending).
    pub fn invalidate(&mut self) {
        for phase in self.phases.iter_mut().flatten() {
            phase.sig = None;
        }
    }

    /// Every retained call of this tile, over all its phases.
    pub fn calls(&self) -> impl Iterator<Item = &TileCallSpec> {
        self.phases.iter().flatten().flat_map(|phase| phase.calls.iter())
    }
}

/// The frame's view state shared by every tile draw, built once per
/// `draw_walk`.
pub(crate) struct TileFrame {
    pub now: f64,
    pub view_zoom: f64,
    pub map_offset: Vec2d,
    pub rect: Rect,
    pub view_rot: [f32; 2],
    pub rot_pivot: [f32; 2],
    pub tilt_params: [f32; 4],
    /// Tilted camera: depth from the ground ladder, `pass_depth` per pass.
    pub tilted: bool,
    /// Flat paint order authored in `draw_depth`; off when tilted or exploded.
    pub flat_fix: bool,
    pub zbias_step: f32,
    pub pass_boost: f32,
    pub fringe: bool,
    pub high_band: bool,
    /// Near-ring radius base of the distance LOD (`frustum` extent before
    /// the per-tile diagonal margin).
    pub frustum: f64,
    pub terrain_org: [f32; 2],
    pub terrain_span: [f32; 2],
    pub terrain_uvfit: [f32; 4],
    pub terrain_fill_lift: f32,
    pub terrain_tex: Texture,
    pub shadow_mask: Option<Texture>,
    pub shadow_mask_on: f32,
    pub shadow_mask_size: [f32; 2],
    pub shadow_mask_flip: f32,
    /// Sun direction on the ground times shadow length per metre; the mask
    /// pass scales it into each tile's units.
    pub shadow_sun: [f32; 2],
    pub space_warp: [f32; 4],
    pub space_warp2: [f32; 4],
    pub shiny_time: f32,
    pub shiny: ShinyConfig,
}

impl TileFrame {
    fn pass_depth(&self, phase: TilePhase) -> f32 {
        match (self.tilted, phase.tilt_depth_k()) {
            (true, Some(k)) => self.pass_boost + k * 0.02,
            _ => 0.0,
        }
    }

    fn env(&self, phase: TilePhase) -> MapDrawEnv<'_> {
        MapDrawEnv {
            shiny: &self.shiny,
            terrain_tex: &self.terrain_tex,
            // Nothing drawn into the mask may sample it: binding the target
            // texture as a sampler is a feedback loop (a WebGL error flood
            // and a blank map; undefined on Metal).
            shadow_mask: if phase == TilePhase::Shadow {
                None
            } else {
                self.shadow_mask.as_ref()
            },
        }
    }
}

/// One tile's view-dependent values this frame.
struct TileView {
    map_scale: Vec2f,
    screen_offset: Vec2f,
    fade_alpha: f32,
    /// `fade_alpha` while the flat->3D reveal grows heights, else 1.
    grow: f32,
    /// The incoming road core is the unchanged resident one: it stays
    /// opaque and at full height while only the overlay cross-fades.
    reused_road: bool,
    width_resident: [f32; 4],
    width_outgoing: [f32; 4],
    lod: f32,
    lod_band: u8,
    icons: bool,
    shadow_dir: [f32; 2],
}

fn tile_view(frame: &TileFrame, key: TileKey, entry: &TileEntry) -> TileView {
    let scale = 2.0_f64.powf(frame.view_zoom - key.z as f64);
    let tile_offset = frame.map_offset
        + dvec2(
            key.x as f64 * TILE_SIZE * scale,
            key.y as f64 * TILE_SIZE * scale,
        );
    let map_scale = Vec2f {
        x: scale as f32,
        y: scale as f32,
    };
    let screen_offset = Vec2f {
        x: tile_offset.x as f32,
        y: tile_offset.y as f32,
    };
    let (fade_alpha, grow, reused_road, width_outgoing) = match &entry.fade {
        Some(fade) => {
            let fade_alpha = (((frame.now - fade.started).max(0.0) / TILE_FADE_SECONDS) as f32)
                .clamp(0.0, 1.0);
            (
                fade_alpha,
                if fade.grow_heights { fade_alpha } else { 1.0 },
                fade.reuse_road_core,
                stroke_width_correction(fade.bucket, frame.view_zoom),
            )
        }
        None => (1.0, 1.0, false, [1.0; 4]),
    };
    // 3D volume rides the fill pass with a ground-circle distance fade from
    // the view focus: the far field under tilt (the blurred zone) skips
    // walls/trees/roofs — the bulk of the fill vertex mass. Flat views sit
    // inside the near radius everywhere, so nothing changes at top-down.
    let tile_center_px = dvec2(
        tile_offset.x + TILE_SIZE * scale * 0.5,
        tile_offset.y + TILE_SIZE * scale * 0.5,
    );
    let focus = frame.rect.pos + frame.rect.size * 0.5;
    let dist =
        ((tile_center_px.x - focus.x).powi(2) + (tile_center_px.y - focus.y).powi(2)).sqrt();
    // Ring radii from the actual frustum extent: the near ring must contain
    // every visible tile CENTER with margin (a tile diagonal), under any
    // rotation and the full tilt stretch — visible geometry never drops
    // below full detail (perf-never-breaks-the-picture).
    let near = frame.frustum * 1.35 + TILE_SIZE * scale;
    let far = near * 1.7;
    let lod = (1.0 - ((dist - near) / (far - near)).clamp(0.0, 1.0)) as f32;
    // LOD rings: near = full detail; mid = roofs + crossed-quad trees
    // ("roofs only"); far = heights sink to 0.
    let lod_band = if lod > 0.55 {
        2
    } else if lod > 0.003 {
        1
    } else {
        0
    };
    // Stale higher-bucket tiles keep their baked symbols until the rebuild
    // lands. Charger pins bake from z9 so the pass itself stays on when
    // zoomed out — but a stale POI-carpet tile (baked at z16+) hides its
    // symbols the moment the view drops below icon level, instead of
    // splattering hundreds of full-size shop icons across the region.
    let icons = !(frame.view_zoom < 7.75
        || (entry.bucket >= ICON_MIN_ZOOM && frame.view_zoom < ICON_MIN_ZOOM as f64 - 0.25));
    let units_per_m = MapView::tile_units_per_m(key);
    let shadow_dir = [
        frame.shadow_sun[0] * units_per_m,
        frame.shadow_sun[1] * units_per_m,
    ];
    TileView {
        map_scale,
        screen_offset,
        fade_alpha,
        grow,
        reused_road,
        width_resident: stroke_width_correction(entry.bucket, frame.view_zoom),
        width_outgoing,
        lod,
        lod_band,
        icons,
        shadow_dir,
    }
}

/// The one place a draw's uniforms come from: recording and refreshing go
/// through it with the same inputs.
fn call_uniforms(
    frame: &TileFrame,
    phase: TilePhase,
    view: &TileView,
    fade: FadeSource,
    width: WidthSource,
    height: HeightSource,
    shadow_cast: f32,
) -> MapDrawUniforms {
    let mask_pass = phase == TilePhase::Shadow;
    MapDrawUniforms {
        shiny_time: frame.shiny_time,
        map_scale: view.map_scale,
        map_offset: view.screen_offset,
        fade: match fade {
            FadeSource::Full => 1.0,
            FadeSource::Alpha => view.fade_alpha,
        },
        width_correction: match width {
            WidthSource::Resident => view.width_resident,
            WidthSource::Outgoing => view.width_outgoing,
        },
        view_rot: frame.view_rot,
        rot_pivot: frame.rot_pivot,
        tilt_params: frame.tilt_params,
        icon_zoom: frame.view_zoom as f32,
        height_grow: match height {
            HeightSource::Full => 1.0,
            HeightSource::Zero => 0.0,
            HeightSource::Grow => view.grow,
            // Distance LOD sinks heights, never alpha: translucent
            // buildings read as broken.
            HeightSource::LodGrow => view.lod * view.grow,
        },
        terrain_org: frame.terrain_org,
        terrain_span: frame.terrain_span,
        terrain_uvfit: frame.terrain_uvfit,
        terrain_fill_lift: frame.terrain_fill_lift,
        shadow_dir: if mask_pass { view.shadow_dir } else { [0.0, 0.0] },
        shadow_cast,
        shadow_mask_on: if mask_pass { 0.0 } else { frame.shadow_mask_on },
        shadow_mask_size: frame.shadow_mask_size,
        shadow_mask_flip: if mask_pass { 0.0 } else { frame.shadow_mask_flip },
        space_warp: frame.space_warp,
        space_warp2: frame.space_warp2,
        pass_depth: frame.pass_depth(phase),
    }
}

/// The recording of one phase list: collects the call specs and the flat
/// paint-order layering (see the module doc).
struct PhaseRecorder<'a> {
    calls: &'a mut Vec<TileCallSpec>,
    /// Layers used so far: the `zbias_hold` step count on exit.
    steps: u32,
    flat_fix: bool,
    step: f32,
}

impl PhaseRecorder<'_> {
    /// The instance `draw_depth` of a call at the current layer.
    fn depth(&self) -> f32 {
        if self.flat_fix {
            self.steps as f32 * self.step
        } else {
            0.0
        }
    }

    /// A stream group ends: one layer for the whole group, if it drew.
    fn close_group(&mut self, emitted: usize) {
        if emitted > 0 {
            self.steps += 1;
        }
    }
}

/// The map's drawers and the frame, borrowed field-wise out of `MapView` so
/// a tile entry can be borrowed mutably beside them.
pub(crate) struct TileDrawCtx<'a> {
    pub frame: &'a TileFrame,
    pub draw_map: &'a mut DrawMapVector,
    pub draw_fill: &'a mut DrawMapFill,
    pub draw_road: &'a mut DrawMapRoad,
    pub draw_face: &'a mut DrawMapFace,
    pub draw_roof: &'a mut DrawMapRoof,
    pub draw_icon: &'a mut DrawMapIcon,
    pub draw_prop: &'a mut DrawMapProp,
    pub draw_wall: &'a mut DrawMapWall,
    pub draw_shadow: &'a mut DrawMapShadow,
    pub draw_shadow_disc: &'a mut DrawMapShadowDisc,
    pub icon_mesh_geometries: &'a mut HashMap<u16, Geometry>,
    pub slots: [MapUniformSlots; TILE_DRAWER_COUNT],
}

impl<'a> TileDrawCtx<'a> {
    fn draw_vars(&mut self, drawer: TileDrawer) -> &mut DrawVars {
        match drawer {
            TileDrawer::Vector => &mut self.draw_map.draw_super.draw_vars,
            TileDrawer::Fill => &mut self.draw_fill.draw_vars,
            TileDrawer::Road => &mut self.draw_road.draw_vars,
            TileDrawer::Face => &mut self.draw_face.road.draw_vars,
            TileDrawer::Roof => &mut self.draw_roof.draw_vars,
            TileDrawer::Icon => &mut self.draw_icon.draw_vars,
            TileDrawer::Prop => &mut self.draw_prop.draw_vars,
            TileDrawer::Wall => &mut self.draw_wall.draw_vars,
            TileDrawer::Shadow => &mut self.draw_shadow.draw_vars,
            TileDrawer::ShadowDisc => &mut self.draw_shadow_disc.draw_vars,
        }
    }

    /// Uniform slot tables for every drawer, resolved once per frame.
    pub(crate) fn resolve_slots(
        cx: &Cx,
        draw_map: &DrawMapVector,
        draw_fill: &DrawMapFill,
        draw_road: &DrawMapRoad,
        draw_face: &DrawMapFace,
        draw_roof: &DrawMapRoof,
        draw_icon: &DrawMapIcon,
        draw_prop: &DrawMapProp,
        draw_wall: &DrawMapWall,
        draw_shadow: &DrawMapShadow,
        draw_shadow_disc: &DrawMapShadowDisc,
    ) -> [MapUniformSlots; TILE_DRAWER_COUNT] {
        [
            MapUniformSlots::resolve(cx, &draw_map.draw_super.draw_vars),
            MapUniformSlots::resolve(cx, &draw_fill.draw_vars),
            MapUniformSlots::resolve(cx, &draw_road.draw_vars),
            MapUniformSlots::resolve(cx, &draw_face.road.draw_vars),
            MapUniformSlots::resolve(cx, &draw_roof.draw_vars),
            MapUniformSlots::resolve(cx, &draw_icon.draw_vars),
            MapUniformSlots::resolve(cx, &draw_prop.draw_vars),
            MapUniformSlots::resolve(cx, &draw_wall.draw_vars),
            MapUniformSlots::resolve(cx, &draw_shadow.draw_vars),
            MapUniformSlots::resolve(cx, &draw_shadow_disc.draw_vars),
        ]
    }

    fn shader_mask(&mut self) -> u16 {
        let drawers = [
            TileDrawer::Vector,
            TileDrawer::Fill,
            TileDrawer::Road,
            TileDrawer::Face,
            TileDrawer::Roof,
            TileDrawer::Icon,
            TileDrawer::Prop,
            TileDrawer::Wall,
            TileDrawer::Shadow,
            TileDrawer::ShadowDisc,
        ];
        let mut mask = 0u16;
        for drawer in drawers {
            if self.draw_vars(drawer).draw_shader_id.is_some() {
                mask |= 1 << drawer.index();
            }
        }
        mask
    }

    /// Draw one tile in one phase: attach its retained list, recording it
    /// first when its signature moved, else refreshing its calls' uniforms.
    pub(crate) fn draw_tile(
        &mut self,
        cx: &mut Cx2d,
        key: TileKey,
        entry: &mut TileEntry,
        phase: TilePhase,
    ) {
        let frame = self.frame;
        let view = tile_view(frame, key, entry);
        let icons = view.icons && (phase != TilePhase::IconHigh || frame.high_band);
        if matches!(phase, TilePhase::Icon | TilePhase::IconHigh) && !icons {
            return;
        }
        // Only what this phase's call set depends on, so a ring or gate
        // flip re-records the one list it changes.
        let sig = TilePhaseSig {
            flat_fix: frame.flat_fix,
            clip: frame.rect,
            lod_band: if phase == TilePhase::Fill { view.lod_band } else { 0 },
            fringe: phase == TilePhase::Casing && frame.fringe,
            icons: matches!(phase, TilePhase::Icon | TilePhase::IconHigh) && icons,
            shaders: self.shader_mask(),
        };
        let TileEntry {
            state, fade, draw, ..
        } = entry;
        let TileLoadState::Ready { .. } = state else {
            return;
        };
        let slot = draw.phases[phase.index()].get_or_insert_with(|| TilePhaseList::new(cx));
        let record = slot.sig != Some(sig);
        if slot.list.begin_maybe(cx, record).is_redrawing() {
            slot.calls.clear();
            let mut rec = PhaseRecorder {
                calls: &mut slot.calls,
                steps: 0,
                flat_fix: frame.flat_fix && phase != TilePhase::Shadow,
                step: frame.zbias_step,
            };
            self.record(cx, &mut rec, phase, &view, state, fade.as_ref());
            let steps = rec.steps;
            cx.draw_lists[slot.list.id()].zbias_hold = Some(steps);
            slot.list.end(cx);
            slot.sig = Some(sig);
        } else {
            self.refresh(cx, phase, &view, &slot.calls);
        }
    }

    /// A pan frame: every retained call gets this frame's uniforms and
    /// textures; its instances stay where they are.
    fn refresh(&mut self, cx: &mut Cx2d, phase: TilePhase, view: &TileView, calls: &[TileCallSpec]) {
        let frame = self.frame;
        let env = frame.env(phase);
        for spec in calls {
            let uniforms = call_uniforms(
                frame,
                phase,
                view,
                spec.fade,
                spec.width,
                spec.height,
                spec.shadow_cast,
            );
            let slots = self.slots[spec.drawer.index()];
            let draw_vars = self.draw_vars(spec.drawer);
            stamp_map_uniforms(draw_vars, &slots, &uniforms, &env);
            draw_vars.update_uniforms_on_area(cx, spec.area);
        }
    }

    fn push(
        rec: &mut PhaseRecorder,
        area: Area,
        drawer: TileDrawer,
        fade: FadeSource,
        width: WidthSource,
        height: HeightSource,
        shadow_cast: f32,
    ) -> usize {
        if !matches!(area, Area::Instance(_)) {
            return 0;
        }
        rec.calls.push(TileCallSpec {
            area,
            drawer,
            fade,
            width,
            height,
            shadow_cast,
        });
        1
    }

    /// One whole-geometry draw; returns how many draw calls it made (0 or 1).
    #[allow(clippy::too_many_arguments)]
    fn geometry(
        &mut self,
        cx: &mut Cx2d,
        rec: &mut PhaseRecorder,
        phase: TilePhase,
        view: &TileView,
        drawer: TileDrawer,
        geometry_id: GeometryId,
        fade: FadeSource,
        width: WidthSource,
        height: HeightSource,
        shadow_cast: f32,
    ) -> usize {
        let frame = self.frame;
        let env = frame.env(phase);
        let uniforms = call_uniforms(frame, phase, view, fade, width, height, shadow_cast);
        let slots = self.slots[drawer.index()];
        let depth = rec.depth();
        let area = match drawer {
            TileDrawer::Vector => {
                self.draw_map
                    .draw_geometry(cx, geometry_id, &slots, &uniforms, &env, depth)
            }
            TileDrawer::Fill => {
                self.draw_fill
                    .draw_geometry(cx, geometry_id, &slots, &uniforms, &env, depth)
            }
            TileDrawer::Road => {
                self.draw_road
                    .draw_geometry(cx, geometry_id, &slots, &uniforms, &env, depth)
            }
            TileDrawer::Face => {
                self.draw_face
                    .draw_geometry(cx, geometry_id, &slots, &uniforms, &env, depth)
            }
            TileDrawer::Roof => {
                self.draw_roof
                    .draw_geometry(cx, geometry_id, &slots, &uniforms, &env, depth)
            }
            TileDrawer::Icon
            | TileDrawer::Prop
            | TileDrawer::Wall
            | TileDrawer::Shadow
            | TileDrawer::ShadowDisc => Area::Empty,
        };
        Self::push(rec, area, drawer, fade, width, height, shadow_cast)
    }

    /// Every chunk of one typed stream: one layer for all of them.
    #[allow(clippy::too_many_arguments)]
    fn stream(
        &mut self,
        cx: &mut Cx2d,
        rec: &mut PhaseRecorder,
        phase: TilePhase,
        view: &TileView,
        drawer: TileDrawer,
        chunks: &[Geometry],
        fade: FadeSource,
        width: WidthSource,
        height: HeightSource,
    ) -> usize {
        let mut emitted = 0;
        for chunk in chunks {
            emitted += self.geometry(
                cx,
                rec,
                phase,
                view,
                drawer,
                chunk.geometry_id(),
                fade,
                width,
                height,
                0.0,
            );
        }
        emitted
    }

    /// The instanced prop draws (stalks, stoplights): one layer per call.
    #[allow(clippy::too_many_arguments)]
    fn props(
        &mut self,
        cx: &mut Cx2d,
        rec: &mut PhaseRecorder,
        phase: TilePhase,
        view: &TileView,
        template: &Option<Geometry>,
        records: &[MapPropInstance],
        height: HeightSource,
        shadow_cast: f32,
    ) {
        let Some(template) = template else {
            return;
        };
        let frame = self.frame;
        let env = frame.env(phase);
        let (fade, width) = (FadeSource::Alpha, WidthSource::Resident);
        let fade = if phase == TilePhase::Shadow { FadeSource::Full } else { fade };
        let uniforms = call_uniforms(frame, phase, view, fade, width, height, shadow_cast);
        let slots = self.slots[TileDrawer::Prop.index()];
        let area = self.draw_prop.draw_instances(
            cx,
            template.geometry_id(),
            records,
            &slots,
            &uniforms,
            &env,
            rec.depth(),
        );
        let n = Self::push(rec, area, TileDrawer::Prop, fade, width, height, shadow_cast);
        rec.close_group(n);
    }

    fn icon_groups(
        &mut self,
        cx: &mut Cx2d,
        rec: &mut PhaseRecorder,
        phase: TilePhase,
        view: &TileView,
        groups: &[IconInstances],
        fade: FadeSource,
        width: WidthSource,
        height: HeightSource,
    ) {
        let frame = self.frame;
        let env = frame.env(phase);
        let uniforms = call_uniforms(frame, phase, view, fade, width, height, 0.0);
        let slots = self.slots[TileDrawer::Icon.index()];
        for group in groups {
            let area = self.draw_icon.draw_group(
                cx,
                group,
                self.icon_mesh_geometries,
                &slots,
                &uniforms,
                &env,
                rec.depth(),
            );
            let n = Self::push(rec, area, TileDrawer::Icon, fade, width, height, 0.0);
            rec.close_group(n);
        }
    }

    fn record(
        &mut self,
        cx: &mut Cx2d,
        rec: &mut PhaseRecorder,
        phase: TilePhase,
        view: &TileView,
        state: &TileLoadState,
        fade: Option<&TileFade>,
    ) {
        let TileLoadState::Ready {
            fill_geometry,
            fill_misc_geometry,
            face_geometry,
            casing_geometry,
            stroke_geometry,
            icon_geometry,
            icon_high_geometry,
            icon_instances,
            icon_high_instances,
            shadow_disc_instances,
            fringe_geometry,
            fill_3d_geometry,
            fill_3d_misc_geometry,
            wall_geometry,
            wall_instances,
            tree_geometry,
            tree_cross_geometry,
            tree_template_geometry,
            tree_cross_template_geometry,
            tree_instances,
            stalk_template_geometry,
            stalk_instances,
            stoplight_template_geometry,
            stoplight_instances,
            ..
        } = state
        else {
            return;
        };
        use FadeSource::{Alpha, Full};
        use HeightSource::{Grow, LodGrow, Zero};
        use WidthSource::{Outgoing, Resident};
        let frame = self.frame;
        // The incoming road core of a mode-only rebake is the unchanged
        // resident one: opaque and at full height while the overlay fades.
        let road_fade = if view.reused_road { Full } else { Alpha };
        let road_height = if view.reused_road { HeightSource::Full } else { Grow };
        match phase {
            TilePhase::Fill => {
                // Cross-fade: the replaced generation's geometry stays
                // drawable underneath while the new one fades in.
                if let Some(fade) = fade {
                    let n = self.stream(cx, rec, phase, view, TileDrawer::Fill, &fade.fill_geometry, Full, Outgoing, HeightSource::Full);
                    rec.close_group(n);
                    if let Some(outgoing) = &fade.fill_misc_geometry {
                        let n = self.geometry(cx, rec, phase, view, TileDrawer::Vector, outgoing.geometry_id(), Full, Outgoing, HeightSource::Full, 0.0);
                        rec.close_group(n);
                    }
                }
                let n = self.stream(cx, rec, phase, view, TileDrawer::Fill, fill_geometry, Alpha, Resident, Grow);
                rec.close_group(n);
                if let Some(geometry) = fill_misc_geometry {
                    let n = self.geometry(cx, rec, phase, view, TileDrawer::Vector, geometry.geometry_id(), Alpha, Resident, Grow, 0.0);
                    rec.close_group(n);
                }
                if view.lod_band >= 1 {
                    let n = self.stream(cx, rec, phase, view, TileDrawer::Roof, fill_3d_geometry, Alpha, Resident, LodGrow);
                    rec.close_group(n);
                    // Marker stalks and complete stoplights occupy the same
                    // 3D-volume phase and LOD/grow gate as the generic misc
                    // mesh records they replace.
                    self.props(cx, rec, phase, view, stalk_template_geometry, stalk_instances, LodGrow, 0.0);
                    self.props(cx, rec, phase, view, stoplight_template_geometry, stoplight_instances, LodGrow, 0.0);
                }
                let bands: [(&Option<Geometry>, u8, u8); 4] = [
                    (fill_3d_misc_geometry, 1, 2),
                    (wall_geometry, 2, 2),
                    (tree_geometry, 2, 2),
                    (tree_cross_geometry, 1, 1),
                ];
                for (band, min_band, max_band) in bands {
                    if view.lod_band < min_band || view.lod_band > max_band {
                        continue;
                    }
                    let Some(volume) = band else { continue };
                    let n = self.geometry(cx, rec, phase, view, TileDrawer::Vector, volume.geometry_id(), Alpha, Resident, LodGrow, 0.0);
                    rec.close_group(n);
                }
                // Instanced street trees: the near ring draws the canopy
                // template, the mid ring the crossed stand-in, both from
                // the same records — the LOD gates of the tree bands.
                if !tree_instances.is_empty() {
                    let template = match view.lod_band {
                        2 => tree_template_geometry.as_ref(),
                        1 => tree_cross_template_geometry.as_ref(),
                        _ => None,
                    };
                    if let Some(template) = template {
                        let env = frame.env(phase);
                        let uniforms = call_uniforms(frame, phase, view, Alpha, Resident, LodGrow, 0.0);
                        let slots = self.slots[TileDrawer::Vector.index()];
                        let area = self.draw_map.draw_instanced(cx, template.geometry_id(), tree_instances, &slots, &uniforms, &env, rec.depth());
                        let n = Self::push(rec, area, TileDrawer::Vector, Alpha, Resident, LodGrow, 0.0);
                        rec.close_group(n);
                    }
                }
                // Instanced walls follow the wall band's LOD gate.
                if view.lod_band == 2 && !wall_instances.is_empty() {
                    let env = frame.env(phase);
                    let uniforms = call_uniforms(frame, phase, view, Alpha, Resident, LodGrow, 0.0);
                    let slots = self.slots[TileDrawer::Wall.index()];
                    let area = self.draw_wall.draw_edges(cx, wall_instances, &slots, &uniforms, &env, rec.depth());
                    let n = Self::push(rec, area, TileDrawer::Wall, Alpha, Resident, LodGrow, 0.0);
                    rec.close_group(n);
                }
            }
            TilePhase::Casing => {
                // Faces lead the tile's casing pass and share its layer, so
                // the baked per-feature ticks keep ordering faces against
                // the strokes they were baked with.
                if let Some(fade) = fade {
                    self.stream(cx, rec, phase, view, TileDrawer::Face, &fade.face_geometry, Full, Outgoing, HeightSource::Full);
                    let n = self.stream(cx, rec, phase, view, TileDrawer::Road, &fade.casing_geometry, Full, Outgoing, HeightSource::Full);
                    rec.close_group(n);
                }
                self.stream(cx, rec, phase, view, TileDrawer::Face, face_geometry, road_fade, Resident, road_height);
                let n = self.stream(cx, rec, phase, view, TileDrawer::Road, casing_geometry, road_fade, Resident, road_height);
                rec.close_group(n);
                // AA fringes ride the casing pass, but only where 1px edge
                // AA is visible: at strong tilt the tilt-shift blur and 3D
                // density hide it, and the fringes are ~2/3 of the casing
                // vertex mass on street tiles.
                if frame.fringe {
                    let n = self.stream(cx, rec, phase, view, TileDrawer::Road, fringe_geometry, road_fade, Resident, HeightSource::Full);
                    rec.close_group(n);
                }
            }
            TilePhase::Stroke => {
                if let Some(fade) = fade {
                    let n = self.stream(cx, rec, phase, view, TileDrawer::Road, &fade.stroke_geometry, Full, Outgoing, HeightSource::Full);
                    rec.close_group(n);
                }
                let n = self.stream(cx, rec, phase, view, TileDrawer::Road, stroke_geometry, road_fade, Resident, road_height);
                rec.close_group(n);
            }
            TilePhase::Icon => {
                // The outgoing generation first (cross-fade), then the
                // resident groups, then the vertex-baked decals.
                if let Some(fade) = fade {
                    if let Some(outgoing) = &fade.icon_geometry {
                        let n = self.geometry(cx, rec, phase, view, TileDrawer::Vector, outgoing.geometry_id(), Full, Outgoing, HeightSource::Full, 0.0);
                        rec.close_group(n);
                    }
                    self.icon_groups(cx, rec, phase, view, &fade.icon_instances, Full, Outgoing, HeightSource::Full);
                }
                self.icon_groups(cx, rec, phase, view, icon_instances, Alpha, Resident, Grow);
                if let Some(geometry) = icon_geometry {
                    let n = self.geometry(cx, rec, phase, view, TileDrawer::Vector, geometry.geometry_id(), Alpha, Resident, Grow, 0.0);
                    rec.close_group(n);
                }
            }
            TilePhase::IconHigh => {
                self.icon_groups(cx, rec, phase, view, icon_high_instances, Alpha, Resident, Grow);
                if let Some(geometry) = icon_high_geometry {
                    let n = self.geometry(cx, rec, phase, view, TileDrawer::Vector, geometry.geometry_id(), Alpha, Resident, Grow, 0.0);
                    rec.close_group(n);
                }
            }
            TilePhase::Shadow => {
                let env = frame.env(phase);
                // a. Wall silhouette quads along the sun.
                if !wall_instances.is_empty() {
                    let uniforms = call_uniforms(frame, phase, view, Full, Resident, HeightSource::Full, 0.0);
                    let slots = self.slots[TileDrawer::Shadow.index()];
                    let area = self.draw_shadow.draw_edges(cx, wall_instances, &slots, &uniforms, &env, rec.depth());
                    let n = Self::push(rec, area, TileDrawer::Shadow, Full, Resident, HeightSource::Full, 0.0);
                    rec.close_group(n);
                }
                // b. Roof / deck projections of lifted geometry. The face
                // stream is not cast: every face record is grounded, so
                // each of its fragments would discard here.
                for roof in fill_3d_geometry {
                    let n = self.geometry(cx, rec, phase, view, TileDrawer::Roof, roof.geometry_id(), Full, Resident, HeightSource::Full, 1.0);
                    rec.close_group(n);
                }
                if let Some(geometry) = fill_3d_misc_geometry {
                    let n = self.geometry(cx, rec, phase, view, TileDrawer::Vector, geometry.geometry_id(), Full, Resident, HeightSource::Full, 1.0);
                    rec.close_group(n);
                }
                for chunk in casing_geometry.iter().chain(stroke_geometry.iter()) {
                    let n = self.geometry(cx, rec, phase, view, TileDrawer::Road, chunk.geometry_id(), Full, Resident, HeightSource::Full, 1.0);
                    rec.close_group(n);
                }
                self.props(cx, rec, phase, view, stalk_template_geometry, stalk_instances, HeightSource::Full, 1.0);
                self.props(cx, rec, phase, view, stoplight_template_geometry, stoplight_instances, HeightSource::Full, 1.0);
                // c. Footprint cut-out at ground.
                for roof in fill_3d_geometry {
                    let n = self.geometry(cx, rec, phase, view, TileDrawer::Roof, roof.geometry_id(), Full, Resident, Zero, 2.0);
                    rec.close_group(n);
                }
                if let Some(geometry) = fill_3d_misc_geometry {
                    let n = self.geometry(cx, rec, phase, view, TileDrawer::Vector, geometry.geometry_id(), Full, Resident, Zero, 2.0);
                    rec.close_group(n);
                }
                self.props(cx, rec, phase, view, stalk_template_geometry, stalk_instances, Zero, 2.0);
                self.props(cx, rec, phase, view, stoplight_template_geometry, stoplight_instances, Zero, 2.0);
                // d. Tree / signal contact discs.
                if !shadow_disc_instances.is_empty() {
                    let uniforms = call_uniforms(frame, phase, view, Full, Resident, Zero, 3.0);
                    let slots = self.slots[TileDrawer::ShadowDisc.index()];
                    let area = self.draw_shadow_disc.draw_discs(cx, shadow_disc_instances, &slots, &uniforms, &env);
                    let n = Self::push(rec, area, TileDrawer::ShadowDisc, Full, Resident, Zero, 3.0);
                    rec.close_group(n);
                }
            }
        }
    }
}
