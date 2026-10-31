//! The fractal viewport: zoom and pan input, the tile layers, and the shader
//! that colours tiles through the palette. In `picker` mode it is the small
//! Mandelbrot map in the panel where you drag the Julia constant.

use crate::deep::{DeepRef, Dd};
use crate::kernels::{Params, Precision, TileRect};
use std::sync::Arc;
use crate::remote::{NetworkPool, NodeStatus};
use crate::tiles::{spiral_tiles, Done, Job, TilePool, TILE_PX};
use std::sync::mpsc::{channel, Receiver};
use makepad_widgets::*;
use std::time::Instant;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawTile::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex: texture_2d(float)
        palette: texture_2d(float)

        pal: fn(t: float) -> vec3 {
            let u = fract(t) * 256.0
            let i0 = floor(u)
            let f = u - i0
            var i1 = i0 + 1.0
            if i1 > 255.5 {
                i1 = 0.0
            }
            let v = (self.palette_row + 0.5) / self.palette_rows
            let c0 = self.palette.sample_nearest(vec2((i0 + 0.5) / 256.0, v)).xyz
            let c1 = self.palette.sample_nearest(vec2((i1 + 0.5) / 256.0, v)).xyz
            return mix(c0, c1, f)
        }

        shade: fn(uv: vec2) -> vec3 {
            let s = self.tex.sample_nearest(uv)
            let top = floor(s.w * 255.0 + 0.5)
            let n = (floor(s.x * 255.0 + 0.5) * 65536.0 + floor(s.y * 255.0 + 0.5) * 256.0 + floor(s.z * 255.0 + 0.5)) / 64.0
            if self.is_newton > 0.5 {
                if top > 254.5 {
                    return self.inside.xyz
                }
                let base = self.pal((top - 1.0) / max(self.roots, 1.0) + self.offset)
                let glow = exp(-n * 0.05 * self.density)
                return base * (0.3 + 0.7 * glow)
            }
            // the colouring: the original's (escape count over the cap, 18
            // turns), a steady smooth one, or a logarithmic one for deep views
            var t = n / max(self.max_iter, 1.0) * 18.0 * self.density
            if self.mapping > 0.5 {
                t = n * 0.02 * self.density
            }
            if self.mapping > 1.5 {
                t = log(n + 1.0) * 0.25 * self.density
            }
            if self.trap_on > 0.5 {
                t = n * 0.025 * self.density
            }
            if top > 254.5 {
                if self.trap_on > 0.5 {
                    return self.pal(t + self.offset) * 0.55
                }
                return self.inside.xyz
            }
            var col = self.pal(t + self.offset)
            if top > 0.5 {
                // distance estimation: the distance to the set in screen
                // pixels; filaments get a crisp dark edge and a soft glow
                let px = exp2((top - 100.0) / 20.0) * self.px_per_texel
                let edge = clamp(pow(px * 0.6, 0.4), 0.0, 1.0)
                col = col * mix(1.0, edge, self.de_strength)
                col = col + self.glow * col * exp(-px * 0.5)
            }
            return col
        }

        hue_rotate: fn(c: vec3, a: float) -> vec3 {
            let k = vec3(0.57735, 0.57735, 0.57735)
            let ca = cos(a)
            return c * ca + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - ca)
        }

        pixel: fn() {
            var col = self.shade(self.pos)
            if self.aa > 0.5 {
                let d = 0.5 / self.tex_size
                col = (col
                    + self.shade(self.pos + vec2(d, 0.0))
                    + self.shade(self.pos + vec2(0.0, d))
                    + self.shade(self.pos + vec2(d, d))) * 0.25
            }
            col = self.hue_rotate(col, self.hue)
            col = pow(max(col, vec3(0.0)), vec3(self.gamma))
            if self.tint.w > 0.0 {
                let p = self.pos * self.rect_size
                let edge = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
                let line = 1.0 - smoothstep(0.5, 1.5, edge)
                col = mix(col, self.tint.xyz, max(self.tint.w, line))
            }
            return vec4(col * self.fade, self.fade)
        }
    }

    set_type_default() do #(DrawMarker::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let c = self.rect_size * 0.5
            sdf.circle(c.x, c.y, c.x - 2.0)
            sdf.stroke(vec4(0.0, 0.0, 0.0, 0.6), 3.0)
            sdf.circle(c.x, c.y, c.x - 2.0)
            sdf.stroke(vec4(1.0, 1.0, 1.0, 1.0), 1.5)
            sdf.circle(c.x, c.y, 1.5)
            sdf.fill(vec4(1.0, 1.0, 1.0, 1.0))
            return sdf.result
        }
    }

    mod.widgets.FractalViewBase = #(FractalView::register_widget(vm))
    mod.widgets.FractalView = set_type_default() do mod.widgets.FractalViewBase{
        width: Fill
        height: Fill
        draw_bg +: { color: #x07080c }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTile {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    tex_size: f32,
    #[live]
    aa: f32,
    #[live]
    density: f32,
    #[live]
    offset: f32,
    #[live]
    hue: f32,
    /// 0: the original's colouring, 1: smooth, 2: logarithmic.
    #[live]
    mapping: f32,
    #[live]
    max_iter: f32,
    #[live]
    is_newton: f32,
    #[live]
    trap_on: f32,
    #[live]
    de_strength: f32,
    #[live]
    glow: f32,
    #[live]
    gamma: f32,
    /// Screen pixels per texel (the distance codes are in texels).
    #[live]
    px_per_texel: f32,
    /// Tile fade-in, 0..1.
    #[live]
    fade: f32,
    #[live]
    roots: f32,
    #[live]
    palette_row: f32,
    #[live]
    palette_rows: f32,
    #[live]
    inside: Vec4f,
    /// The overlay colour of the tile's node and worker (alpha 0: off).
    #[live]
    tint: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMarker {
    #[deref]
    draw_super: DrawQuad,
}

/// How the colours are laid onto the iteration counts; the shader applies it
/// to every tile, so none of this re-renders.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub palette: usize,
    pub palette_count: usize,
    pub density: f64,
    pub offset: f64,
    /// Palette turns per second.
    pub cycle: f64,
    pub hue: f64,
    /// 0: the original's colouring (count over the cap), 1: smooth, 2: logarithmic.
    pub mapping: u32,
    pub inside: [f32; 3],
    /// How dark the distance-estimated edges get, 0..1.
    pub de_strength: f64,
    /// Light along the filaments, 0..1.
    pub glow: f64,
    pub gamma: f64,
    /// The iteration count the original colouring divides by (the slider's,
    /// not the depth-raised one, so colours stay put while zooming).
    pub norm_iter: u32,
}

impl Default for Look {
    fn default() -> Self {
        // the original zoomer's look: its palette, its colouring, its slow cycle
        Look {
            palette: 0,
            palette_count: 1,
            density: 1.0,
            offset: 0.0,
            cycle: 0.05,
            hue: 0.0,
            mapping: 0,
            inside: [0.0, 0.0, 0.0],
            de_strength: 0.6,
            glow: 0.15,
            gamma: 1.0,
            norm_iter: 500,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Quality {
    /// Preview resolution only (half-res texels).
    Draft,
    /// Full resolution at rest.
    Full,
    /// Full resolution, then 2x2 supersampling at rest.
    Supersampled,
}

#[derive(Clone, Debug, Default)]
pub enum FractalViewAction {
    /// The picker's point (the Julia constant) moved.
    Picked(f64, f64),
    /// A press on the view ended the tour.
    TourStopped,
    #[default]
    None,
}

struct LayerTile {
    rect: TileRect,
    texture: Option<usize>,
    /// (node, worker) that rendered it, for the overlay.
    by: (u16, u16),
    /// Jobs for this tile still out (a hedged tile has two).
    out: u8,
    hedged: bool,
    /// When its texture arrived (the fade-in).
    shown_at: f64,
}

struct Layer {
    id: u64,
    size: f64,
    res: usize,
    params: Params,
    /// The coarse whole-view pass under a layer that has nothing to cover.
    preview: bool,
    tiles: Vec<LayerTile>,
    /// Tiles without a texture yet.
    missing: usize,
    /// Jobs not answered yet.
    out: usize,
    /// What a re-issued job of this layer needs.
    origin: (f64, f64),
    precision: Precision,
    deep: Option<Arc<DeepRef>>,
    /// The fractal-space box the tiles cover.
    bounds: (f64, f64, f64, f64),
    started: Instant,
    /// When the last tile so far was ready.
    last_done: Instant,
    /// Tiles per node (0: this machine).
    by_node: Vec<(u16, u32)>,
    /// Sum of the workers' tile times.
    micros: u64,
}

impl Layer {
    fn complete(&self) -> bool {
        self.missing == 0
    }

    /// Nothing in flight, yet tiles are missing (cancelled ones): holes.
    fn has_holes(&self) -> bool {
        self.out == 0 && self.missing > 0
    }

    fn job(&self, index: usize, buffer: Vec<u32>, local_only: bool) -> Job {
        Job {
            layer: self.id,
            index,
            origin: self.origin,
            deep: self.deep.clone(),
            rect: self.tiles[index].rect,
            res: self.res,
            precision: self.precision,
            params: self.params,
            local_only,
            buffer,
        }
    }
}

/// Numbers for the panel.
#[derive(Clone, Debug, Default)]
pub struct ViewStats {
    /// Tiles a node dropped (failed or cancelled) that the local workers rendered instead.
    pub dropped: usize,
    /// Tiles re-rendered here because a node was slower than the idle local workers.
    pub hedged: usize,
    /// Network rendering state for the panel.
    pub network: String,
    /// Per node tried: address and what happened (connected, auth refused, unreachable, ...).
    pub node_status: Vec<(String, String)>,
    /// Remote node ids and names.
    pub node_names: Vec<(u16, String)>,
    /// Per node (0 is this machine) over the last complete layer: tiles, tiles per second.
    pub node_rates: Vec<(u16, u32, f64)>,
    pub fps: f64,
    pub frame_ms: f64,
    /// Wall time of the last completed full-view layer.
    pub layer_ms: f64,
    pub layer_res: usize,
    pub layer_tiles: usize,
    /// Megapixels per second over that layer.
    pub mpix_s: f64,
    pub threads: usize,
    pub precision: &'static str,
    pub width: f64,
    pub pending: usize,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FractalView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_tile: DrawTile,
    #[live]
    draw_marker: DrawMarker,
    /// The small Julia-constant map instead of the main viewport.
    #[live]
    picker: bool,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    pool: Option<TilePool>,
    #[rust]
    pub params: Params,
    #[rust]
    pub look: Look,
    #[rust(Quality::Supersampled)]
    pub quality: Quality,
    #[rust]
    palette_atlas: Option<Texture>,
    /// The view's origin in double-double: the center and every tile are
    /// f64 offsets from it, so deep views keep their digits.
    #[rust]
    origin: (Dd, Dd),
    /// View center in fractal space, as an offset from `origin`.
    #[rust]
    center: (f64, f64),
    /// Fractal units per logical pixel; 0 until the first draw frames the home view.
    #[rust]
    scale: f64,
    #[rust]
    rect: Rect,
    #[rust(1.0)]
    dpi: f64,
    #[rust]
    layers: Vec<Layer>,
    #[rust]
    next_layer_id: u64,
    #[rust]
    textures: Vec<Texture>,
    #[rust]
    free_textures: Vec<usize>,
    #[rust]
    buffers: Vec<Vec<u32>>,
    /// Smooth zoom: the scale the view eases to, about `zoom_anchor`.
    #[rust]
    target_scale: f64,
    #[rust]
    zoom_anchor: Vec2d,
    #[rust]
    drag: Option<(Vec2d, (f64, f64))>,
    /// Press-and-hold zoom (the original's gesture): where, since when, in or out.
    #[rust]
    hold: Option<(Vec2d, f64, bool)>,
    #[rust]
    moved_at: f64,
    #[rust]
    now: f64,
    #[rust]
    last_frame: f64,
    #[rust]
    frame_ms_avg: f64,
    #[rust]
    pub stats: ViewStats,
    #[rust]
    pick: (f64, f64),
    #[rust]
    bench: Option<Bench>,
    /// Tint each tile by the node and worker that rendered it.
    #[rust]
    pub show_workers: bool,
    /// A tile drawn last frame was still fading in.
    #[rust]
    fading: bool,
    #[rust]
    flight: Option<Flight>,
    /// When the last layer completed (flights pace themselves by it).
    #[rust]
    last_layer_at: f64,
    /// The guided tour: the places to visit, the next one, and the time to leave the current one.
    #[rust]
    tour: Option<(Vec<(f64, f64, f64)>, usize, f64)>,
    /// Where a press started and when (a short press without a drag is a click).
    #[rust]
    press: Option<(Vec2d, f64)>,
    /// `--view x y width`: the first view instead of the fractal's home.
    #[rust]
    pub start_view: Option<(f64, f64, f64)>,
    /// `--threads N`: tile workers (default: the cores less one).
    #[rust]
    pub threads: Option<usize>,
    /// Render on the AI hub's kernel nodes as well (when any answer).
    #[rust(true)]
    network_enabled: bool,
    #[rust]
    network: Option<NetworkPool>,
    /// A network start in progress (it waits for the fleet beacon off the UI thread).
    #[rust]
    network_start: Option<Receiver<(Option<NetworkPool>, Vec<NodeStatus>)>>,
}

/// Seconds a new tile takes to fade in over the layer under it.
const FADE: f64 = 0.16;

/// Seconds the view counts as moving after the last input.
const SETTLE: f64 = 0.18;

/// A preview tile is 2x2 normal tiles at 1/PREVIEW of the device resolution
/// (64 texels: many small jobs, so every worker gets one).
const PREVIEW: f64 = 8.0;

impl FractalView {
    pub fn set_palette_atlas(&mut self, cx: &mut Cx, atlas: Texture) {
        self.palette_atlas = Some(atlas);
        self.area.redraw(cx);
    }

    pub fn set_params(&mut self, cx: &mut Cx, params: Params) {
        if params != self.params {
            let kind_changed = params.kind != self.params.kind;
            self.params = params;
            if kind_changed && !self.picker {
                self.reset_view();
            }
            self.kick(cx);
        }
    }

    pub fn set_look(&mut self, cx: &mut Cx, look: Look) {
        self.look = look;
        self.kick(cx);
    }

    pub fn set_quality(&mut self, cx: &mut Cx, quality: Quality) {
        self.quality = quality;
        self.kick(cx);
    }

    /// The picker's marker.
    pub fn set_pick(&mut self, cx: &mut Cx, c: (f64, f64)) {
        self.pick = c;
        self.area.redraw(cx);
    }

    /// Back to the fractal's home view (framed at the next draw).
    pub fn reset_view(&mut self) {
        self.stop_tour();
        self.hold = None;
        self.drag = None;
        self.press = None;
        self.scale = 0.0;
        self.target_scale = 0.0;
        self.center = (0.0, 0.0);
        self.origin = (Dd::ZERO, Dd::ZERO);
    }

    /// The keyboard, from the app (so keys work whatever has focus):
    /// arrows pan, + and - zoom, R or Home resets. True when it used the key.
    pub fn key(&mut self, cx: &mut Cx, key: KeyCode) -> bool {
        if self.picker || self.scale <= 0.0 {
            return false;
        }
        let c = self.rect.pos + self.rect.size * 0.5;
        let pan = 0.15 * self.rect.size.x * self.scale;
        match key {
            KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ArrowUp | KeyCode::ArrowDown => {
                self.stop_tour();
                match key {
                    KeyCode::ArrowLeft => self.center.0 -= pan,
                    KeyCode::ArrowRight => self.center.0 += pan,
                    KeyCode::ArrowUp => self.center.1 -= pan,
                    _ => self.center.1 += pan,
                }
            }
            KeyCode::Equals | KeyCode::NumpadAdd => {
                self.stop_tour();
                self.zoom_anchor = c;
                self.target_scale = self.clamp_scale(self.target_scale * 0.5);
            }
            KeyCode::Minus | KeyCode::NumpadSubtract => {
                self.stop_tour();
                self.zoom_anchor = c;
                self.target_scale = self.clamp_scale(self.target_scale * 2.0);
            }
            KeyCode::KeyR | KeyCode::Home => self.reset_view(),
            _ => return false,
        }
        self.touch();
        self.kick(cx);
        true
    }

    /// The largest absolute coordinate of the view (at least 1).
    fn magnitude(&self) -> f64 {
        (self.origin.0.hi + self.center.0).abs().max((self.origin.1.hi + self.center.1).abs()).max(1.0)
    }

    /// Moves the origin to the center once the center is far from it in
    /// pixels, so offsets stay small; tiles shift with it.
    fn rebase(&mut self) {
        if self.picker || self.scale <= 0.0 || self.flight.is_some() {
            return;
        }
        let (cx, cy) = self.center;
        if cx.abs().max(cy.abs()) < 1.0e6 * self.scale {
            return;
        }
        self.origin = (self.origin.0.add_f64(cx), self.origin.1.add_f64(cy));
        self.center = (0.0, 0.0);
        for l in &mut self.layers {
            for t in &mut l.tiles {
                t.rect.x -= cx;
                t.rect.y -= cy;
            }
            l.bounds = (l.bounds.0 - cx, l.bounds.1 - cy, l.bounds.2 - cx, l.bounds.3 - cy);
        }
    }

    pub fn view(&self) -> (f64, f64, f64) {
        (self.center.0, self.center.1, self.scale * self.rect.size.x)
    }

    fn kick(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn moving(&self) -> bool {
        self.drag.is_some() || self.hold.is_some() || self.now - self.moved_at < SETTLE || (self.target_scale / self.scale - 1.0).abs() > 1e-3
    }

    fn touch(&mut self) {
        self.moved_at = self.now;
    }

    /// An event position in the view's own draw coordinates: events come in
    /// window coordinates, `rect` is the hit area there (the picker draws in
    /// the panel's scrolled list).
    fn local(&self, abs: Vec2d, rect: Rect) -> Vec2d {
        abs - rect.pos + self.rect.pos
    }

    fn screen_to_fractal(&self, p: Vec2d) -> (f64, f64) {
        let c = self.rect.pos + self.rect.size * 0.5;
        (self.center.0 + (p.x - c.x) * self.scale, self.center.1 + (p.y - c.y) * self.scale)
    }

    fn fractal_to_screen(&self, x: f64, y: f64) -> Vec2d {
        let c = self.rect.pos + self.rect.size * 0.5;
        dvec2(c.x + (x - self.center.0) / self.scale, c.y + (y - self.center.1) / self.scale)
    }

    /// Scales about a screen point, keeping the fractal point under it fixed.
    fn zoom_about(&mut self, factor: f64, about: Vec2d) {
        let before = self.screen_to_fractal(about);
        self.scale = self.clamp_scale(self.scale * factor);
        let after = self.screen_to_fractal(about);
        self.center.0 += before.0 - after.0;
        self.center.1 += before.1 - after.1;
    }

    fn clamp_scale(&self, s: f64) -> f64 {
        // f64 runs out of mantissa near 1e-15 of the coordinate size; the
        // perturbation path goes on to the double-double origin's ~1e-31
        let mag = self.magnitude();
        let limit = if !self.picker && DeepRef::supports(self.params.kind) { 2.0e-31 } else { 4.0e-15 };
        let min = limit * mag * self.dpi;
        let max = 8.0 / self.rect.size.x.max(1.0);
        s.clamp(min, max)
    }

    fn view_box(&self, margin: f64) -> (f64, f64, f64, f64) {
        let a = self.screen_to_fractal(self.rect.pos);
        let b = self.screen_to_fractal(self.rect.pos + self.rect.size);
        (a.0 - margin, a.1 - margin, b.0 + margin, b.1 + margin)
    }

    fn take_texture(&mut self, cx: &mut Cx) -> usize {
        if let Some(i) = self.free_textures.pop() {
            return i;
        }
        let t = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 { width: 1, height: 1, data: Some(vec![0]), updated: TextureUpdated::Full },
        );
        self.textures.push(t);
        self.textures.len() - 1
    }

    fn recycle_layer(&mut self, cx: &mut Cx, layer: Layer) {
        for t in layer.tiles {
            if let Some(i) = t.texture {
                let buf = self.textures[i].take_vec_u32(cx);
                self.buffers.push(buf);
                // keep a placeholder so the slot never holds taken data
                self.textures[i].set_data_u32(cx, 1, 1, vec![0]);
                self.free_textures.push(i);
            }
        }
    }

    /// Turns network rendering on or off; on, it looks for nodes in the background.
    pub fn set_network(&mut self, cx: &mut Cx, on: bool) {
        self.network_enabled = on;
        if !on {
            self.network_start = None;
            if let Some(net) = self.network.take() {
                // its drop waits for the in-flight polls: not on the UI thread
                std::thread::spawn(move || drop(net));
            }
            self.stats.network = "off".into();
        } else {
            self.start_network();
        }
        self.kick(cx);
    }

    fn start_network(&mut self) {
        if self.picker || !self.network_enabled || self.network.is_some() || self.network_start.is_some() {
            return;
        }
        let q = self.pool().queue_handle();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut report = Vec::new();
            let net = NetworkPool::start(q, &mut report);
            let _ = tx.send((net, report));
        });
        self.network_start = Some(rx);
        self.stats.network = "looking for nodes".into();
    }

    fn poll_network(&mut self) {
        if let Some(rx) = &self.network_start {
            if let Ok((net, report)) = rx.try_recv() {
                self.network_start = None;
                self.network = net;
                self.stats.node_status = report.into_iter().map(|s| (s.addr, s.state)).collect();
                if self.network.is_none() {
                    self.stats.network = "no nodes found".into();
                }
            }
        }
        if let Some(net) = &self.network {
            let nodes = net.nodes();
            let cores: u32 = nodes.iter().map(|n| n.cores).sum();
            self.stats.network = format!("{} nodes {} cores", nodes.len(), cores);
            self.stats.node_names = nodes.into_iter().map(|n| (n.id, n.name)).collect();
        }
    }

    fn pool(&mut self) -> &TilePool {
        let threads = if self.picker {
            2
        } else if let Some(n) = self.threads {
            n
        } else {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(1).max(1)
        };
        self.pool.get_or_insert_with(|| TilePool::new(threads))
    }

    /// Puts finished tiles on their layers (a bounded amount per frame, as
    /// the original did, so uploads never hitch a frame).
    fn receive_tiles(&mut self, cx: &mut Cx) -> bool {
        // at most ~8 MB of texture uploads per frame, so a burst of finished
        // tiles never stretches a frame
        let mut got = false;
        let mut bytes = 0;
        while bytes < 8 << 20 {
            let Some(done) = self.pool.as_ref().and_then(|p| p.try_done()) else { break };
            got = true;
            bytes += done.res * done.res * 4 + 4096;
            self.place_tile(cx, done);
        }
        got
    }

    fn place_tile(&mut self, cx: &mut Cx, done: Done) {
        let Some(li) = self.layers.iter().position(|l| l.id == done.layer) else {
            self.buffers.push(done.buffer);
            return;
        };
        {
            let newest = li + 1 == self.layers.len();
            let layer = &mut self.layers[li];
            layer.out -= 1;
            layer.tiles[done.index].out -= 1;
            // a hedged twin that lost the race
            if layer.tiles[done.index].texture.is_some() {
                self.buffers.push(done.buffer);
                return;
            }
            if done.res == 0 {
                // a node dropped it: stale layers let it go, the newest renders it here
                if newest && layer.tiles[done.index].out == 0 {
                    layer.tiles[done.index].out = 1;
                    layer.out += 1;
                    let job = layer.job(done.index, done.buffer, true);
                    self.stats.dropped += 1;
                    self.pool().push([job]);
                } else {
                    self.buffers.push(done.buffer);
                }
                return;
            }
        }
        let tex = self.take_texture(cx);
        self.textures[tex].set_data_u32(cx, done.res, done.res, done.buffer);
        let layer = &mut self.layers[li];
        layer.tiles[done.index].texture = Some(tex);
        layer.tiles[done.index].by = (done.node, done.worker);
        layer.tiles[done.index].shown_at = self.now;
        match layer.by_node.iter_mut().find(|c| c.0 == done.node) {
            Some(c) => c.1 += 1,
            None => layer.by_node.push((done.node, 1)),
        }
        layer.missing -= 1;
        layer.micros += done.micros;
        layer.last_done = layer.last_done.max(done.at);
        if layer.complete() && layer.preview {
            let ms = layer.last_done.duration_since(layer.started).as_secs_f64() * 1000.0;
            log!("fractal preview: {} tiles @{} in {:.1} ms", layer.tiles.len(), layer.res, ms);
        }
        if layer.complete() && !layer.preview {
            self.last_layer_at = self.now;
            // the render's wall time: from queueing to the last tile, not to the frame that showed it
            let ms = layer.last_done.duration_since(layer.started).as_secs_f64() * 1000.0;
            let texels = (layer.tiles.len() * layer.res * layer.res) as f64;
            self.stats.layer_ms = ms;
            self.stats.layer_res = layer.res;
            self.stats.layer_tiles = layer.tiles.len();
            self.stats.mpix_s = texels / (ms * 1000.0).max(1.0);
            let mut by = layer.by_node.clone();
            by.sort();
            if self.network.is_some() && layer.res >= 256 {
                log!("fractal network: {} tiles @{} in {:.0} ms by node (0 = here): {:?}", layer.tiles.len(), layer.res, ms, by);
            }
            self.stats.node_rates = by.into_iter().map(|(n, c)| (n, c, c as f64 * 1000.0 / ms.max(0.001))).collect();
            if let Some(b) = &mut self.bench {
                b.layer_done(ms, layer.res);
            }
        }
    }

    /// A complete layer that covers the view hides everything under it.
    fn retire_covered_layers(&mut self, cx: &mut Cx) {
        let view = self.view_box(0.0);
        let now = self.now;
        // a layer hides what is under it once it is complete, covers the view and has faded in
        let covers = |l: &Layer| {
            l.complete()
                && l.tiles.iter().all(|t| now - t.shown_at >= FADE)
                && l.bounds.0 <= view.0
                && l.bounds.1 <= view.1
                && l.bounds.2 >= view.2
                && l.bounds.3 >= view.3
        };
        if let Some(top) = self.layers.iter().rposition(|l| covers(l) && l.params == self.params) {
            let old: Vec<Layer> = self.layers.drain(..top).collect();
            for l in old {
                self.recycle_layer(cx, l);
            }
        }
    }

    /// The layer the view wants now: tile size from the scale (a tile is
    /// TILE_PX device pixels at that scale), texel density from the motion.
    fn wanted(&self) -> (f64, usize) {
        let moving = self.moving();
        let mut size = TILE_PX * self.scale / self.dpi;
        // render ahead of a zoom so the layer is still sharp when it lands
        if moving {
            let ratio = self.target_scale / self.scale;
            if ratio < 0.999 || self.hold.is_some_and(|h| h.2) {
                size *= 0.8;
            } else if ratio > 1.001 || self.hold.is_some_and(|h| !h.2) {
                size *= 1.25;
            }
        }
        let res = match (moving, self.quality) {
            (true, _) | (false, Quality::Draft) => 128,
            (false, Quality::Full) => 256,
            (false, Quality::Supersampled) => {
                // supersample once a full-resolution layer of this view is up
                let full_done = self
                    .layers
                    .iter()
                    .any(|l| l.complete() && l.res >= 256 && l.params == self.params && (l.size / size).ln().abs() <= 0.05);
                if full_done { 512 } else { 256 }
            }
        };
        (size, res)
    }

    fn schedule(&mut self, cx: &mut Cx) {
        if self.scale <= 0.0 || self.rect.size.x < 1.0 {
            return;
        }
        self.rebase();
        let (size, res) = self.wanted();
        let params = self.params;
        let view = self.view_box(0.0);
        let start = match self.layers.last() {
            None => true,
            Some(l) => {
                let off = (l.size / size).ln().abs();
                let covers = l.bounds.0 <= view.0 && l.bounds.1 <= view.1 && l.bounds.2 >= view.2 && l.bounds.3 >= view.3;
                if l.params != params {
                    true
                } else if l.complete() {
                    off > 0.05 || res != l.res || !covers
                } else {
                    // abandon a layer the view has left behind
                    off > 0.7 || (res < l.res) || !overlaps(l.bounds, view) || l.has_holes()
                }
            }
        };
        if !start {
            return;
        }
        // jobs not started yet belong to a layer we no longer want
        let new_id = self.next_layer_id;
        let cancelled = self.pool().cancel_below(new_id);
        for job in cancelled {
            if let Some(l) = self.layers.iter_mut().find(|l| l.id == job.layer) {
                l.out -= 1;
                l.tiles[job.index].out -= 1;
            }
            self.buffers.push(job.buffer);
        }
        // keep the newest complete layer and at most two partial ones over it
        while self.layers.len() > 3 {
            let drop_at = self.layers.iter().position(|l| !l.complete()).unwrap_or(0);
            let l = self.layers.remove(drop_at);
            self.recycle_layer(cx, l);
        }
        let focus = if self.hold.is_some() || (self.target_scale / self.scale - 1.0).abs() > 1e-3 {
            let a = self.hold.map(|h| h.0).unwrap_or(self.zoom_anchor);
            self.screen_to_fractal(a)
        } else {
            self.center
        };
        let step = size / res as f64;
        let mag = self.magnitude() + 2.0;
        // past f64's reach: one double-double reference orbit at the focus
        // f64 holds to a pixel of ~10 ulps of the coordinates; past that, perturbation
        let deep = if !self.picker && DeepRef::supports(params.kind) && step < 2.0e-15 * mag {
            Some(Arc::new(DeepRef::new(&params, self.origin, focus)))
        } else {
            None
        };
        // nothing of this fractal on screen (a jump, a new type, a pan into the
        // unknown): a coarse preview of the whole view first, here, at 1/8 of
        // the device resolution, so the view never stays black while the
        // full layer streams in over it
        let view = self.view_box(0.0);
        let shown = self.layers.iter().any(|l| {
            l.params == params && l.complete() && l.bounds.0 <= view.0 && l.bounds.1 <= view.1 && l.bounds.2 >= view.2 && l.bounds.3 >= view.3
        });
        if !shown && !self.picker && self.bench.is_none() {
            self.push_layer(focus, 2.0 * TILE_PX * self.scale / self.dpi, (2.0 * TILE_PX / PREVIEW) as usize, params, deep.clone(), true);
        }
        let precision = self.push_layer(focus, size, res, params, deep.clone(), false);
        self.stats.precision = match (&deep, precision) {
            (Some(_), _) => "deep f64x4",
            (None, Precision::F64) => "f64x4",
            (None, Precision::F32) => "f32x8",
        };
        self.start_network();
    }

    /// Queues a layer of tiles `size` wide at `res` texels, spiralling out
    /// from `focus`. A preview layer renders here only (no network round
    /// trip) and stays out of the stats.
    fn push_layer(&mut self, focus: (f64, f64), size: f64, res: usize, params: Params, deep: Option<Arc<DeepRef>>, preview: bool) -> Precision {
        let margin = size * 0.25;
        let rects = spiral_tiles(focus, size, self.view_box(margin));
        let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for r in &rects {
            bounds = (bounds.0.min(r.x), bounds.1.min(r.y), bounds.2.max(r.x + r.size), bounds.3.max(r.y + r.size));
        }
        let id = self.next_layer_id;
        self.next_layer_id += 1;
        let step = size / res as f64;
        let mag = self.magnitude() + 2.0;
        // with nodes, every shared layer is f64: node tiles are f64 and the
        // local f64 path matches them bit for bit (src/parity.rs), so a layer
        // shows no seam where a node's tile meets a local one
        let networked = self.network.is_some() || self.network_start.is_some();
        let precision = if networked && !preview { Precision::F64 } else { Precision::for_step(step, mag, mag) };
        let origin = (self.origin.0.hi, self.origin.1.hi);
        let count = rects.len();
        let layer = Layer {
            id,
            size,
            res,
            params,
            preview,
            missing: count,
            out: count,
            origin,
            precision,
            deep,
            tiles: rects.into_iter().map(|rect| LayerTile { rect, texture: None, by: (0, 0), out: 1, hedged: false, shown_at: 0.0 }).collect(),
            bounds,
            started: Instant::now(),
            last_done: Instant::now(),
            by_node: Vec::new(),
            micros: 0,
        };
        let jobs: Vec<Job> = (0..count).map(|i| layer.job(i, self.buffers.pop().unwrap_or_default(), preview)).collect();
        self.layers.push(layer);
        self.pool().push(jobs);
        precision
    }

    /// Flies to a place (absolute center, width across the view) along the
    /// smooth zoom-and-pan path: out, across, in.
    pub fn fly_to(&mut self, cx: &mut Cx, x: f64, y: f64, width: f64) {
        if self.scale <= 0.0 || self.rect.size.x < 1.0 {
            self.start_view = Some((x, y, width));
            return;
        }
        self.fly_to_dd(cx, (Dd::new(x), Dd::new(y)), width);
    }

    /// `fly_to` with a double-double target (a click deep in keeps its digits).
    fn fly_to_dd(&mut self, cx: &mut Cx, target: (Dd, Dd), width: f64) {
        let w0 = self.scale * self.rect.size.x;
        let w1 = width.max(self.clamp_scale(0.0) * self.rect.size.x);
        // the deeper end becomes the origin, so its view keeps its digits:
        // the target when zooming in, the start when zooming out
        let here = (self.origin.0.add_f64(self.center.0), self.origin.1.add_f64(self.center.1));
        let origin = if w1 <= w0 { target } else { here };
        let from = (here.0.sub(origin.0).hi, here.1.sub(origin.1).hi);
        let to = (target.0.sub(origin.0).hi, target.1.sub(origin.1).hi);
        // tiles live in offsets from the origin: move them with it
        let shift = (self.center.0 - from.0, self.center.1 - from.1);
        self.origin = origin;
        for l in &mut self.layers {
            for t in &mut l.tiles {
                t.rect.x -= shift.0;
                t.rect.y -= shift.1;
            }
            l.bounds = (l.bounds.0 - shift.0, l.bounds.1 - shift.1, l.bounds.2 - shift.0, l.bounds.3 - shift.1);
        }
        self.center = from;
        self.flight = Some(Flight::new(from, to, w0, w1));
        self.hold = None;
        self.drag = None;
        self.kick(cx);
    }

    /// Visits `places` one after another, lingering at each.
    pub fn start_tour(&mut self, cx: &mut Cx, places: Vec<(f64, f64, f64)>) {
        self.tour = Some((places, 0, self.now));
        self.kick(cx);
    }

    pub fn stop_tour(&mut self) {
        self.tour = None;
        self.flight = None;
    }

    pub fn touring(&self) -> bool {
        self.tour.is_some()
    }

    fn animate(&mut self, cx: &mut Cx, dt: f64) {
        // not framed yet (start, or just reset): the next draw frames it
        if self.scale <= 0.0 {
            return;
        }
        if let Some((places, next, leave_at)) = &mut self.tour {
            if self.flight.is_none() && self.now >= *leave_at && !places.is_empty() {
                let (x, y, w) = places[*next % places.len()];
                *next += 1;
                self.fly_to(cx, x, y, w);
            }
        }
        // fly no faster than the tiles keep up: slow down while no layer has
        // landed for a while (deep, iteration-heavy places)
        let behind = self.now - self.last_layer_at;
        let rate = if behind > 0.25 { (0.25 / behind).max(0.15) } else { 1.0 };
        if let Some(f) = &mut self.flight {
            f.clock += dt * rate;
        }
        if let Some(f) = &self.flight {
            let (u, w, done) = f.at();
            let (from, to, rx) = (f.from, f.to, self.rect.size.x.max(1.0));
            // u runs 0..1 from `from` to `to`
            self.center = (from.0 + (to.0 - from.0) * u, from.1 + (to.1 - from.1) * u);
            self.scale = w / rx;
            self.target_scale = self.scale;
            self.touch();
            if done {
                self.flight = None;
                self.center = to;
                if let Some(t) = &mut self.tour {
                    t.2 = self.now + 3.5;
                }
            }
            return;
        }
        if let Some((abs, since, zoom_in)) = self.hold {
            // the original's press-and-hold: ease into a steady zoom
            let ramp = ((self.now - since) * 3.0).min(1.0);
            let rate = 1.0 + 1.6 * ramp;
            let f = if zoom_in { 1.0 / rate } else { rate };
            self.zoom_about(f.powf(dt), abs);
            self.target_scale = self.scale;
            self.touch();
        } else if (self.target_scale / self.scale - 1.0).abs() > 1e-4 {
            let k = 1.0 - (-dt * 14.0).exp();
            let f = (self.target_scale / self.scale).powf(k);
            let anchor = self.zoom_anchor;
            self.zoom_about(f, anchor);
            self.touch();
        }
    }

    /// Nothing queued while tiles of the newest layer are still out: queue a
    /// twin of each, so whoever is free (an idle local worker or a node)
    /// races the straggler, first answer wins. One slow tile, here or on a
    /// node, never holds the view back.
    fn hedge(&mut self) {
        let Some(pool) = &self.pool else { return };
        if pool.queued() > 0 {
            return;
        }
        let Some(layer) = self.layers.last_mut() else { return };
        let mut jobs = Vec::new();
        for i in 0..layer.tiles.len() {
            let t = &layer.tiles[i];
            if t.texture.is_none() && t.out > 0 && !t.hedged {
                layer.tiles[i].hedged = true;
                layer.tiles[i].out += 1;
                layer.out += 1;
                jobs.push(layer.job(i, self.buffers.pop().unwrap_or_default(), false));
            }
        }
        if !jobs.is_empty() {
            self.stats.hedged += jobs.len();
            self.pool().push(jobs);
        }
    }

    fn busy(&self) -> bool {
        self.flight.is_some()
            || self.tour.is_some()
            || self.network_start.is_some()
            || self.fading
            || self.moving() || self.layers.iter().any(|l| !l.complete()) || self.look.cycle != 0.0 || self.bench.is_some()
    }

    pub fn start_bench(&mut self, cx: &mut Cx, bench: Bench) {
        self.bench = Some(bench);
        self.kick(cx);
    }

    pub fn bench_report(&mut self) -> Option<String> {
        match &self.bench {
            Some(b) if b.finished() => {
                let r = b.report(self.stats.threads, &self.rect, self.dpi);
                self.bench = None;
                Some(r)
            }
            _ => None,
        }
    }

    fn bench_step(&mut self, cx: &mut Cx) {
        let Some(b) = &mut self.bench else { return };
        if !b.wants_next() {
            return;
        }
        let Some((x, y, w)) = b.next_view() else { return };
        // a fixed 1600x900 device-pixel view, whatever the window: runs compare
        self.rect.size = dvec2(BENCH_PX.0, BENCH_PX.1) / self.dpi;
        self.origin = (Dd::ZERO, Dd::ZERO);
        self.center = (x, y);
        self.scale = w / self.rect.size.x.max(1.0);
        self.target_scale = self.scale;
        self.moved_at = -1.0e9;
        // a fresh layer every run: drop what is on screen
        let old: Vec<Layer> = self.layers.drain(..).collect();
        for l in old {
            self.recycle_layer(cx, l);
        }
        self.schedule(cx);
    }
}

/// A distinct colour per node (hue) and worker (lightness).
fn worker_color(by: (u16, u16)) -> Vec4f {
    let h = (by.0 as f32 * 0.618_034 + by.1 as f32 * 0.071).fract() * 6.0;
    let x = 1.0 - ((h % 2.0) - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    vec4(r, g, b, 0.22)
}

fn overlaps(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

impl Widget for FractalView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::GpuReset = event {
            // the tile textures lived on the old device: render everything again
            let old: Vec<Layer> = self.layers.drain(..).collect();
            for l in old {
                self.recycle_layer(cx, l);
            }
            self.kick(cx);
        }
        if matches!(event, Event::Startup) {
            self.kick(cx);
        }
        if let Some(ne) = self.next_frame.is_event(event) {
            let dt = if self.last_frame > 0.0 { (ne.time - self.last_frame).clamp(0.0, 0.1) } else { 1.0 / 60.0 };
            if self.last_frame > 0.0 {
                let ms = (ne.time - self.last_frame) * 1000.0;
                self.frame_ms_avg = if self.frame_ms_avg == 0.0 { ms } else { self.frame_ms_avg * 0.9 + ms * 0.1 };
                self.stats.frame_ms = self.frame_ms_avg;
                self.stats.fps = 1000.0 / self.frame_ms_avg.max(0.001);
            }
            self.last_frame = ne.time;
            self.now = ne.time;
            self.animate(cx, dt);
            self.receive_tiles(cx);
            self.retire_covered_layers(cx);
            self.poll_network();
            if self.network.is_some() {
                self.hedge();
            }
            self.bench_step(cx);
            if self.bench.is_none() {
                self.schedule(cx);
            }
            if self.look.cycle != 0.0 {
                self.look.offset = (self.look.offset + self.look.cycle * dt).rem_euclid(1.0);
            }
            self.stats.pending = self.layers.iter().map(|l| l.missing).sum();
            self.stats.threads = self.pool.as_ref().map_or(0, |p| p.thread_count());
            self.stats.width = self.scale * self.rect.size.x;
            self.area.redraw(cx);
            if self.busy() {
                self.next_frame = cx.new_next_frame();
            } else {
                self.last_frame = 0.0;
            }
        }

        if self.picker {
            match event.hits(cx, self.area) {
                Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Crosshair),
                Hit::FingerDown(fe) => {
                    self.drag = Some((self.local(fe.abs, fe.rect), self.center));
                    self.pick = self.screen_to_fractal(self.local(fe.abs, fe.rect));
                    cx.widget_action(self.uid, FractalViewAction::Picked(self.pick.0, self.pick.1));
                    self.area.redraw(cx);
                }
                Hit::FingerMove(fe) => {
                    if self.drag.is_some() {
                        self.pick = self.screen_to_fractal(self.local(fe.abs, fe.rect));
                        cx.widget_action(self.uid, FractalViewAction::Picked(self.pick.0, self.pick.1));
                        self.area.redraw(cx);
                    }
                }
                Hit::FingerUp(_) => self.drag = None,
                _ => {}
            }
            return;
        }

        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(fe) => {
                cx.set_key_focus(self.area);
                if self.tour.is_some() || self.flight.is_some() {
                    self.stop_tour();
                    cx.widget_action(self.uid, FractalViewAction::TourStopped);
                }
                self.drag = Some((self.local(fe.abs, fe.rect), self.center));
                self.press = Some((self.local(fe.abs, fe.rect), self.now));
                // right button or shift: hold zooms out right away
                let secondary = fe.mouse_button().is_some_and(|b| b != MouseButton::PRIMARY);
                if secondary || fe.modifiers.shift {
                    self.hold = Some((self.local(fe.abs, fe.rect), self.now, false));
                    self.drag = None;
                    self.press = None;
                }
                self.touch();
                self.kick(cx);
            }
            Hit::FingerLongPress(lp) => {
                // press and hold without moving: zoom in about the finger
                if self.drag.is_some() {
                    self.hold = Some((self.local(lp.abs, lp.rect), self.now, true));
                    self.drag = None;
                    self.press = None;
                    self.kick(cx);
                }
            }
            Hit::FingerMove(fe) => {
                let at = self.local(fe.abs, fe.rect);
                if let Some(h) = &mut self.hold {
                    h.0 = at;
                } else if let Some((start, c0)) = self.drag {
                    let d = self.local(fe.abs, fe.rect) - start;
                    if d.x.abs() + d.y.abs() > 4.0 {
                        self.press = None;
                    }
                    self.center = (c0.0 - d.x * self.scale, c0.1 - d.y * self.scale);
                    self.touch();
                    self.kick(cx);
                }
            }
            Hit::FingerUp(fe) => {
                self.drag = None;
                self.hold = None;
                // a click (no drag, no hold): fly in 3x toward that point
                if let Some((at, since)) = self.press.take() {
                    if self.now - since < 0.35 && (self.local(fe.abs, fe.rect) - at).x.abs() + (self.local(fe.abs, fe.rect) - at).y.abs() <= 4.0 {
                        let (x, y) = self.screen_to_fractal(self.local(fe.abs, fe.rect));
                        let w = self.scale * self.rect.size.x / 3.0;
                        let target = (self.origin.0.add_f64(x), self.origin.1.add_f64(y));
                        self.fly_to_dd(cx, target, w);
                    }
                }
                self.touch();
                self.kick(cx);
            }
            Hit::FingerScroll(fe) => {
                self.zoom_anchor = self.local(fe.abs, fe.rect);
                self.target_scale = self.clamp_scale(self.target_scale * (fe.scroll.y * 0.0025).exp());
                self.touch();
                self.kick(cx);
            }
            Hit::FingerPinch(pe) => {
                self.zoom_about(1.0 / pe.scale.max(0.01), self.local(pe.abs, pe.rect));
                self.target_scale = self.scale;
                self.touch();
                self.kick(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.dpi = cx.current_dpi_factor() as f64;
        let resized = rect.size != self.rect.size;
        if self.bench.is_none() {
            self.rect = rect;
        }
        if self.scale <= 0.0 && rect.size.x > 1.0 {
            let (x, y, w) = if self.picker { (-0.6, 0.0, 3.0) } else { self.start_view.take().unwrap_or(self.params.kind.home()) };
            self.center = (x, y);
            // fit the home width or its 3:2 height, whichever is tighter
            self.scale = (w / rect.size.x).max(w / 1.5 / rect.size.y);
            self.target_scale = self.scale;
            self.next_frame = cx.new_next_frame();
        } else if resized {
            self.next_frame = cx.new_next_frame();
        }
        self.draw_bg.draw_abs(cx, rect);

        self.fading = false;
        if let Some(atlas) = self.palette_atlas.clone() {
            let look = self.look;
            let dt = &mut self.draw_tile;
            dt.draw_vars.set_texture(1, &atlas);
            dt.density = look.density as f32;
            dt.offset = look.offset as f32;
            dt.hue = look.hue as f32;
            dt.mapping = look.mapping as f32;
            dt.max_iter = look.norm_iter.max(1) as f32;
            dt.is_newton = if self.params.kind == crate::kernels::FractalKind::Newton { 1.0 } else { 0.0 };
            dt.trap_on = if self.params.trap { 1.0 } else { 0.0 };
            dt.de_strength = look.de_strength as f32;
            dt.glow = look.glow as f32;
            dt.gamma = look.gamma as f32;
            dt.roots = self.params.power.max(2) as f32;
            dt.palette_row = look.palette as f32;
            dt.palette_rows = look.palette_count.max(1) as f32;
            dt.inside = vec4(look.inside[0], look.inside[1], look.inside[2], 1.0);
            let view = self.view_box(0.0);
            for li in 0..self.layers.len() {
                let res = self.layers[li].res;
                for ti in 0..self.layers[li].tiles.len() {
                    let t = &self.layers[li].tiles[ti];
                    let Some(tex) = t.texture else { continue };
                    let r = t.rect;
                    if !overlaps((r.x, r.y, r.x + r.size, r.y + r.size), view) {
                        continue;
                    }
                    let p0 = self.fractal_to_screen(r.x, r.y);
                    let p1 = self.fractal_to_screen(r.x + r.size, r.y + r.size);
                    self.draw_tile.draw_vars.set_texture(0, &self.textures[tex]);
                    self.draw_tile.tex_size = res as f32;
                    self.draw_tile.aa = if res >= 512 { 1.0 } else { 0.0 };
                    self.draw_tile.px_per_texel = ((p1.x - p0.x) * self.dpi / res as f64) as f32;
                    let age = if li == 0 || self.bench.is_some() { FADE } else { self.now - t.shown_at };
                    self.draw_tile.fade = (age / FADE).clamp(0.0, 1.0) as f32;
                    if age < FADE {
                        self.fading = true;
                    }
                    self.draw_tile.tint = if self.show_workers { worker_color(t.by) } else { vec4(0.0, 0.0, 0.0, 0.0) };
                    self.draw_tile.draw_abs(cx, Rect { pos: p0, size: p1 - p0 });
                }
            }
        }
        if self.picker {
            let p = self.fractal_to_screen(self.pick.0, self.pick.1);
            self.draw_marker.draw_abs(cx, Rect { pos: p - dvec2(9.0, 9.0), size: dvec2(18.0, 18.0) });
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

/// A flight between two views along the optimal zoom-and-pan path (wide
/// enough to see both ends at the top), at a steady perceived speed.
struct Flight {
    /// The start and the end, as offsets from the origin.
    from: (f64, f64),
    to: (f64, f64),
    w0: f64,
    w1: f64,
    u1: f64,
    r0: f64,
    s_total: f64,
    /// Seconds of flight so far (the view advances it, slower while tiles lag).
    clock: f64,
    duration: f64,
}

const RHO: f64 = 1.42;

impl Flight {
    fn new(from: (f64, f64), to: (f64, f64), w0: f64, w1: f64) -> Flight {
        let d = (to.0 - from.0, to.1 - from.1);
        let u1 = (d.0 * d.0 + d.1 * d.1).sqrt();
        let (r0, s_total) = if u1 < 1e-9 * w0.min(w1) {
            (0.0, (w1 / w0).ln().abs() / RHO)
        } else {
            let b = |w: f64, sign: f64| (w1 * w1 - w0 * w0 + sign * RHO.powi(4) * u1 * u1) / (2.0 * w * RHO * RHO * u1);
            // r = ln(-b + sqrt(b^2 + 1)) = -asinh(b): the log form cancels to
            // ln(0) for a large b (every zoom out of a deep place)
            let r0 = -b(w0, 1.0).asinh();
            let r1 = -b(w1, -1.0).asinh();
            (r0, (r1 - r0) / RHO)
        };
        let duration = (s_total * 0.7).clamp(0.9, 16.0);
        Flight { from, to, w0, w1, u1, r0, s_total, clock: 0.0, duration }
    }

    /// (fraction of the way from `from` to `to`, width, done).
    fn at(&self) -> (f64, f64, bool) {
        let t = (self.clock / self.duration).clamp(0.0, 1.0);
        if t >= 1.0 {
            return (1.0, self.w1, true);
        }
        let e = t * t * (3.0 - 2.0 * t);
        let s = self.s_total * e;
        if self.u1 < 1e-9 * self.w0.min(self.w1) {
            let w = self.w0 * (self.w1 / self.w0).powf(e);
            return (e, w, false);
        }
        // u = w0/rho^2 (cosh r0 tanh(rho s + r0) - sinh r0), written without
        // the cancellation of two huge terms
        let x = RHO * s + self.r0;
        let u = self.w0 / (RHO * RHO) * (RHO * s).sinh() / x.cosh();
        let w = self.w0 * self.r0.cosh() / x.cosh();
        ((u / self.u1).clamp(0.0, 1.0), w, false)
    }
}

/// `--bench`: renders fixed views as full layers and reports the wall time
/// of each (the frame time of a full refresh of the view).
const BENCH_PX: (f64, f64) = (1600.0, 900.0);

pub struct Bench {
    views: Vec<(&'static str, f64, f64, f64)>,
    runs: usize,
    at: usize,
    waiting: bool,
    pub results: Vec<(&'static str, Vec<f64>)>,
}

impl Bench {
    pub fn new(runs: usize) -> Self {
        let views = vec![
            ("home f32", -0.6, 0.0, 3.4),
            ("seahorse f64", -0.743_643_887_037_151, 0.131_825_904_205_330, 2.0e-9),
            ("c=i width 1e-25 perturbation", 0.0, 1.0, 1.0e-25),
        ];
        let results = views.iter().map(|v| (v.0, Vec::new())).collect();
        Bench { views, runs, at: 0, waiting: false, results }
    }

    fn total(&self) -> usize {
        self.views.len() * (self.runs + 1)
    }

    fn finished(&self) -> bool {
        self.at >= self.total() && !self.waiting
    }

    fn wants_next(&self) -> bool {
        !self.waiting && self.at < self.total()
    }

    fn next_view(&mut self) -> Option<(f64, f64, f64)> {
        let v = self.views.get(self.at / (self.runs + 1))?;
        self.waiting = true;
        Some((v.1, v.2, v.3))
    }

    fn layer_done(&mut self, ms: f64, _res: usize) {
        if !self.waiting {
            return;
        }
        let vi = self.at / (self.runs + 1);
        // run 0 of every view warms up (and under the JIT, compiles)
        if self.at % (self.runs + 1) != 0 {
            self.results[vi].1.push(ms);
        }
        self.at += 1;
        self.waiting = false;
    }

    fn report(&self, threads: usize, rect: &Rect, dpi: f64) -> String {
        let mut s = format!(
            "BENCH view {:.0}x{:.0} px @{dpi}x, {threads} tile threads, 256 texel tiles, max_iter 500\n",
            rect.size.x * dpi,
            rect.size.y * dpi
        );
        for (name, ms) in &self.results {
            let mut v = ms.clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = v.get(v.len() / 2).copied().unwrap_or(0.0);
            let min = v.first().copied().unwrap_or(0.0);
            s += &format!("BENCH {name}: median {med:.2} ms, min {min:.2} ms over {} runs\n", v.len());
            let runs: Vec<String> = ms.iter().map(|m| format!("{m:.1}")).collect();
            s += &format!("BENCH {name} runs: {}\n", runs.join(" "));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::Flight;

    /// Every tour leg, both ways: finite widths and positions, ending exactly at the target.
    #[test]
    fn flights_stay_finite() {
        let views = [(-0.6, 0.0, 3.4), (-0.75, 0.1, 0.05), (-0.7756, 0.1365, 6e-4), (-0.7436, 0.1318, 4e-11), (-1.7549, 0.0, 0.045), (0.0, 1.0, 1e-20)];
        for a in &views {
            for b in &views {
                // the deeper end is the origin
                let o = if b.2 <= a.2 { b } else { a };
                for i in 0..=200 {
                    let mut f = Flight::new((a.0 - o.0, a.1 - o.1), (b.0 - o.0, b.1 - o.1), a.2, b.2);
                    f.clock = if i == 200 { f.duration } else { f.duration * i as f64 / 200.0 };
                    let (u, w, done) = f.at();
                    assert!(u.is_finite() && w.is_finite() && w > 0.0, "{a:?} -> {b:?} at {i}: u {u} w {w}");
                    assert!((0.0..=1.0).contains(&u));
                    if i == 0 {
                        assert!(u < 1e-9 && (w / a.2 - 1.0).abs() < 1e-9, "{a:?} -> {b:?} start: u {u} w {w}");
                    }
                    if i == 200 {
                        assert!(done && u == 1.0 && w == b.2);
                    }
                }
            }
        }
    }
}
