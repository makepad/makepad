//! The relief buffer: what one moulded surface does to its neighbours.
//!
//! A surface's own look — bevel, face, inner shadow, the shadow it casts onto
//! whatever lies under it — is analytic (`mod.sdf.Material`) and drawn in its
//! own quad. What it does to OTHER surfaces is not: the light an LED spills
//! onto the bevels on both sides of it, the occlusion where a raised control
//! meets its neighbours. Those come from here.
//!
//! Every surface that takes part registers a *proxy* while it draws: its
//! rect (resolved from its `Area` only when the buffer is built, so a scroll
//! that moves a cached draw list moves its proxies), a rounded-box or disc
//! shape, its absolute height and its emissive colour. When the window
//! finishes a frame, the proxies are drawn — one instanced draw call, painter
//! order — into a small half-float offscreen pass at half density (`a` =
//! height in points, `rgb` = emissive), which the glass stack's Gaussian
//! filters (half-float copies) blur into four levels at a quarter of the
//! window's density. Consumers read the wide levels through a B-spline, so
//! a glow stretched over the window has no texel bands. Material shaders sample those
//! levels in the main pass, with the same four textures and uniforms for
//! every one of them, so binding them never splits a batch.
//!
//! The passes are recorded only when the resolved proxy list changes; a
//! window at rest records and paints nothing here, and a repaint that moved
//! no proxy repaints none of these passes. A window in which no surface ever
//! registered never creates any of it.
//!
//! v1 scope: the window's own body pass (and the supersampled scene that
//! stands in for it). A surface drawn into any other pass — a cached texture
//! view, the exploded view, a glass capture — gets its analytic look and no
//! neighbour term (`relief_on` = 0).
use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::View,
    widget::*,
};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawReliefProxy::script_shader(vm)){
        ..mod.draw.DrawQuad
        // A raw write: alpha is the height, not an opacity. Outside the shape
        // the fragment is discarded, so a child's proxy replaces its parent's
        // where it covers it and leaves the rest alone.
        alpha_blend: false
        color_format: @Rgba16F

        pixel: fn() {
            let p = self.pos * self.rect_size
            let c = self.rect_size * 0.5
            var d = 0.0
            if self.disc > 0.5 {
                d = length(p - c) - min(c.x, c.y)
            } else {
                d = Material.sd_box(p, c, c, min(self.radius, min(c.x, c.y)))
            }
            // The part of the surface its own clip still shows: a proxy
            // scrolled out of its view lights nothing.
            let w = self.rect_pos + p
            if d > 0.0 || w.x < self.clip.x || w.y < self.clip.y || w.x > self.clip.z || w.y > self.clip.w {
                discard()
            }
            return vec4(self.emissive.xyz * self.emissive.w, self.height)
        }
    }

    // The glass stack's two filters, for half-float targets: the relief
    // levels keep their precision all the way down the chain.
    set_type_default() do #(DrawReliefDown::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        color_format: @Rgba16F
        source_texture: texture_2d(float)

        tap: fn(uv: vec2) -> vec4 {
            return self.source_texture.sample(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        pixel: fn() {
            let size = self.source_texture.size()
            let t = vec2(1.0 / max(size.x, 1.0), 1.0 / max(size.y, 1.0))
            let uv = self.pos
            return self.tap(uv) * 0.125
                + (self.tap(uv + t * vec2(-2.0, 2.0)) + self.tap(uv + t * vec2(2.0, 2.0))
                    + self.tap(uv + t * vec2(-2.0, -2.0)) + self.tap(uv + t * vec2(2.0, -2.0))) * 0.03125
                + (self.tap(uv + t * vec2(0.0, 2.0)) + self.tap(uv + t * vec2(-2.0, 0.0))
                    + self.tap(uv + t * vec2(2.0, 0.0)) + self.tap(uv + t * vec2(0.0, -2.0))) * 0.0625
                + (self.tap(uv + t * vec2(-1.0, 1.0)) + self.tap(uv + t * vec2(1.0, 1.0))
                    + self.tap(uv + t * vec2(-1.0, -1.0)) + self.tap(uv + t * vec2(1.0, -1.0))) * 0.125
        }
    }

    set_type_default() do #(DrawReliefUp::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        color_format: @Rgba16F
        source_texture: texture_2d(float)

        tap: fn(uv: vec2) -> vec4 {
            return self.source_texture.sample(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        pixel: fn() {
            let size = self.source_texture.size()
            let t = vec2(0.5 / max(size.x, 1.0), 0.5 / max(size.y, 1.0))
            let uv = self.pos
            return self.tap(uv) * 0.25
                + (self.tap(uv + t * vec2(1.0, 0.0)) + self.tap(uv + t * vec2(-1.0, 0.0))
                    + self.tap(uv + t * vec2(0.0, 1.0)) + self.tap(uv + t * vec2(0.0, -1.0))) * 0.125
                + (self.tap(uv + t * vec2(1.0, 1.0)) + self.tap(uv + t * vec2(-1.0, 1.0))
                    + self.tap(uv + t * vec2(1.0, -1.0)) + self.tap(uv + t * vec2(-1.0, -1.0))) * 0.0625
        }
    }

    mod.widgets.ReliefViewBase = #(ReliefView::register_widget(vm))

    // A moulded surface: a rounded box or a disc with `Material`'s
    // shading, its own cast shadow in its margin, and the relief buffer's
    // neighbour terms on its face. Children draw on top of it, like a View.
    //
    // `depth` is its elevation over whatever it sits on (points, negative
    // sinks it), `inset` the margin between its rect and its shape — the room
    // its own shadow and glow fall into — and `emissive` the light it gives
    // its neighbours while `lit`.
    mod.widgets.ReliefView = set_type_default() do mod.widgets.ReliefViewBase{
        show_bg: true
        depth: 0.0
        inset: 0.0
        radius: 8.0
        press: -1.0
        draw_bg +: {
            relief_l1: texture_2d(float)
            relief_l2: texture_2d(float)
            relief_l3: texture_2d(float)
            relief_l4: texture_2d(float)
            relief_on: uniform(0.0)
            relief_size: uniform(vec2(1.0, 1.0))

            /** key light: direction (x right, y down, z out) and intensity */
            light: uniform(vec4(-0.20, -0.70, 0.68, 1.15))
            /** bevel width, profile curve, raise, specular */
            relief: uniform(vec4(2.5, 0.70, 2.5, 0.35))
            /** occlusion, rim, gloss, roughness */
            finish: uniform(vec4(0.55, 0.62, 0.42, 0.16))
            /** face gradient, hairline, occlusion reach, sink */
            tune: uniform(vec4(0.80, 0.50, 1.10, 3.0))
            /** cast shadow strength, blur, falloff (0 linear 1 expo), contact occlusion */
            shadow: uniform(vec4(0.70, 10.0, 1.0, 0.50))
            /** inner shadow, inner blur, ground lip, glow */
            inner: uniform(vec4(0.72, 9.0, 0.0, 0.55))
            /** neighbour spill strength, neighbour occlusion */
            spill: uniform(vec2(1.6, 0.45))
            /** fine grain on faces and halos, 0 = none 0..0.05 step 0.001 */
            grain: uniform(0.0)
            light_ink: uniform(vec4(1.0, 1.0, 1.0, 1.0))
            shadow_ink: uniform(vec4(0.0, 0.0, 0.0, 1.0))
            glow_ink: uniform(theme.color_primary)
            ground: instance(theme.color_bg_app)

            // Pushed by the widget on every draw.
            depth: instance(0.0)
            convex: instance(0.0)
            lit: instance(0.0)
            inset: instance(0.0)
            radius: instance(8.0)
            disc: instance(0.0)
            height: instance(0.0)

            // One of the blurred relief levels: .xyz emissive, .w height (pt).
            relief_tap: fn(level: float, uv: vec2) -> vec4 {
                let c = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
                if level < 1.5 {
                    return self.relief_l1.sample(c)
                }
                if level < 2.5 {
                    return self.relief_l2.sample(c)
                }
                if level < 3.5 {
                    return self.relief_l3.sample(c)
                }
                return self.relief_l4.sample(c)
            }

            // The same level through a cubic B-spline in four bilinear taps:
            // C1 across texel edges, so a quarter-density level stretched
            // over the window leaves no bands in a smooth glow.
            relief_tap_smooth: fn(level: float, uv: vec2) -> vec4 {
                let size = max(self.relief_l1.size(), vec2(1.0, 1.0))
                let tc = uv * size - vec2(0.5, 0.5)
                let f = fract(tc)
                let tc0 = floor(tc)
                let f2 = f * f
                let f3 = f2 * f
                let omf = vec2(1.0, 1.0) - f
                let w1 = (f3 * 3.0 - f2 * 6.0 + vec2(4.0, 4.0)) / 6.0
                let g0 = omf * omf * omf / 6.0 + w1
                let g1 = vec2(1.0, 1.0) - g0
                let h0 = (tc0 - vec2(0.5, 0.5) + w1 / g0) / size
                let h1 = (tc0 + vec2(1.5, 1.5) + (f3 / 6.0) / g1) / size
                return self.relief_tap(level, vec2(h0.x, h0.y)) * (g0.x * g0.y)
                    + self.relief_tap(level, vec2(h1.x, h0.y)) * (g1.x * g0.y)
                    + self.relief_tap(level, vec2(h0.x, h1.y)) * (g0.x * g1.y)
                    + self.relief_tap(level, vec2(h1.x, h1.y)) * (g1.x * g1.y)
            }

            // Triangular dither of one output LSB (and optional grain) at
            // this screen pixel: static, it never reads time.
            dither: fn() -> vec3 {
                let px = floor(self.world.xy * self.draw_pass.dpi_factor) + vec2(0.5, 0.5)
                let d = Material.dither(px) + Material.grain(px) * self.grain
                return vec3(d, d, d)
            }

            relief_uv: fn() -> vec2 {
                return self.world.xy / max(self.relief_size, vec2(1.0, 1.0))
            }

            // Light the neighbours give this point: the blurred emissive, and
            // more of it on a bevel that faces the emitter (the direction is
            // the screen gradient of the wide level).
            env_spill: fn(n: vec3) -> vec3 {
                if self.relief_on < 0.5 {
                    return vec3(0.0, 0.0, 0.0)
                }
                let uv = self.relief_uv()
                let e2 = self.relief_tap(2.0, uv).xyz
                let e3 = self.relief_tap_smooth(3.0, uv).xyz
                let e4 = self.relief_tap_smooth(4.0, uv).xyz
                // A near wash on every face, and the far light only where a
                // bevel turns toward it: a flat face a long way off gets
                // little, so the glow does not read as leaking.
                let near = e2 * 0.25 + e3 * 0.55
                let far = e3 * 0.5 + e4 * 0.7
                let lum = dot(e3 + e4, vec3(0.333, 0.333, 0.333))
                if lum < 0.0005 {
                    return near * 0.45 * self.spill.x
                }
                // Toward the emitter: central differences of the widest
                // level, 8 points each way. (The screen derivative of a
                // filtered texel flips sign from quad to quad along a bevel.)
                let step = vec2(8.0, 8.0) / max(self.relief_size, vec2(1.0, 1.0))
                let w3 = vec3(0.333, 0.333, 0.333)
                let g = vec2(
                    dot(self.relief_tap(4.0, uv + vec2(step.x, 0.0)).xyz - self.relief_tap(4.0, uv - vec2(step.x, 0.0)).xyz, w3),
                    dot(self.relief_tap(4.0, uv + vec2(0.0, step.y)).xyz - self.relief_tap(4.0, uv - vec2(0.0, step.y)).xyz, w3)
                )
                let gl = length(g)
                var dir = vec2(0.0, 0.0)
                if gl > 0.000001 {
                    dir = g / gl
                }
                // How strongly the field leans, so a flat glow gives no direction.
                let lean = smoothstep(0.0, 1.0, gl / max(lum, 0.0001) * 2.0)
                let facing = clamp(dot(n.xy, dir) * 3.0, 0.0, 1.0) * lean
                return (near * 0.5 + far * (0.3 + 2.2 * facing)) * self.spill.x
            }

            // Occlusion from neighbours standing above this point.
            env_ao: fn(own: float) -> float {
                if self.relief_on < 0.5 {
                    return 0.0
                }
                let m = self.relief_tap_smooth(2.0, self.relief_uv()).w
                return clamp((m - own) / 6.0, 0.0, 1.0) * self.spill.y
            }

            sd: fn(p: vec2) -> float {
                let c = self.rect_size * 0.5
                let h = max(c - vec2(self.inset, self.inset), vec2(0.5, 0.5))
                if self.disc > 0.5 {
                    return length(p - c) - min(h.x, h.y)
                }
                return Material.sd_box(p, c, h, min(self.radius, min(h.x, h.y)))
            }

            // One layer: shadow, lip and glow outside the shape,
            // the lit face inside, as one premultiplied colour.
            layer: fn(base: vec3) -> vec4 {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let h = max(c - vec2(self.inset, self.inset), vec2(0.5, 0.5))
                let d = self.sd(p)
                let e = 0.5
                var g = vec2(
                    self.sd(p + vec2(e, 0.0)) - self.sd(p - vec2(e, 0.0)),
                    self.sd(p + vec2(0.0, e)) - self.sd(p - vec2(0.0, e))
                )
                if length(g) > 0.00001 {
                    g = normalize(g)
                } else {
                    g = vec2(0.0, 1.0)
                }
                let l = self.light
                let sdir = Material.shadow_dir(l)
                let tanel = max(l.z, 0.05) / max(length(l.xy), 0.05)
                let blur = max(self.shadow.y, 0.001)
                let fall = self.shadow.z
                let outside = smoothstep(-3.0 * px, 0.0, d)
                var under = vec4(0.0, 0.0, 0.0, 0.0)
                let rel = clamp(self.depth / max(self.relief.z, 0.001), 0.0, 1.0)
                let off = max(self.depth, 0.0) / tanel
                let facing = clamp(-dot(g, sdir), 0.0, 1.0)
                let dark = Material.tail(self.sd(p - sdir * off), blur, fall) * outside * rel
                let lite = Material.tail(self.sd(p + sdir * off), blur, fall) * outside * rel * smoothstep(0.0, 0.7, facing)
                let contact = exp(-max(d, 0.0) / (blur * 0.30)) * outside * step(0.0, self.depth)
                let a1 = clamp(dark * self.shadow.x + contact * self.shadow.w, 0.0, 1.0)
                under = vec4(self.shadow_ink.xyz * a1, a1)
                let a2 = clamp(lite * self.inner.z, 0.0, 1.0)
                under = vec4(self.light_ink.xyz * a2, a2) + under * (1.0 - a2)
                if self.lit > 0.001 && self.inner.w > 0.001 {
                    let a3 = clamp(Material.tail(d, self.inner.w * 26.0, fall) * self.inner.w, 0.0, 1.0) * 0.85 * self.lit
                    under = vec4(self.glow_ink.xyz * a3, a3) + under * (1.0 - a3)
                }
                // The inner shadow: the surround's shadow over a face that has
                // dropped below it — the outline shifted down-light, blurred.
                var insh = 0.0
                if self.inner.x > 0.001 && self.depth < 0.0 {
                    let ioff = sdir * abs(self.depth) * 1.6
                    if self.disc > 0.5 {
                        insh = 1.0 - smoothstep(0.0, max(self.inner.y, 0.001), -self.sd(p - ioff))
                    } else {
                        insh = 1.0 - Material.box_cov(c - h + ioff, c + h + ioff, p, max(self.inner.y * 0.5, 0.35), min(self.radius, min(h.x, h.y)))
                    }
                }
                let uv = (p - c) / (2.0 * h) + vec2(0.5, 0.5)
                var face = Material.face(base, d, g, uv, self.depth, self.convex, 0.0, insh, 0.0, l, self.relief, self.finish, self.tune, self.inner.x, self.light_ink.xyz, self.shadow_ink.xyz, 1.0)
                let n = Material.normal(d, g, self.relief.x, self.relief.y, self.convex, 0.0)
                face = face * (1.0 - self.env_ao(self.height)) + self.env_spill(n)
                if self.lit > 0.001 {
                    face = mix(face, self.glow_ink.xyz, min(self.inner.w * 1.6, 1.0) * 0.72 * self.lit)
                }
                // Whatever falls outside the shape fades out before the quad
                // edge, so a shadow or a halo never ends on a straight line.
                var edge = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
                if self.disc > 0.5 {
                    edge = min(c.x, c.y) - length(p - c)
                }
                under = under * smoothstep(0.0, max(self.inset, 1.0), edge)
                let cov = 1.0 - smoothstep(-px, px, d)
                let dith = self.dither()
                face = face + dith
                under = vec4(under.xyz + dith * step(0.002, under.w), under.w)
                return vec4(face * cov, cov) + under * (1.0 - cov)
            }

            pixel: fn() {
                return self.layer(self.ground.xyz)
            }
        }
    }
}

/// Relief pass density relative to layout points; the blurred levels run at
/// half of it.
const RELIEF_DPI: f64 = 0.5;
const RELIEF_DOWNS: usize = 4;
/// The consumer's texture slots, by name.
const RELIEF_LEVELS: [LiveId; 4] = [
    live_id!(relief_l1),
    live_id!(relief_l2),
    live_id!(relief_l3),
    live_id!(relief_l4),
];

/// One surface as the relief buffer sees it: the shape inside its area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReliefProxy {
    /// Shrinks the area's rect on every side.
    pub inset: f64,
    pub radius: f64,
    pub disc: bool,
    /// rgb and intensity.
    pub emissive: Vec4f,
}

/// A registration: its parent is the surface that was open when it opened,
/// and its height is relative to that parent, so a cached child list follows
/// a parent that moved.
struct ReliefEntry {
    /// Position in the list's draw items when it was opened: its place in
    /// painter order against the list's sub lists.
    pos: usize,
    parent: Option<(DrawListId, usize)>,
    depth: f64,
    area: Area,
    proxy: Option<ReliefProxy>,
}

struct ReliefList {
    redraw_id: u64,
    entries: Vec<ReliefEntry>,
}

#[derive(Clone, Copy)]
struct Resolved {
    rect: Rect,
    clip: Rect,
    radius: f64,
    disc: bool,
    height: f64,
    emissive: Vec4f,
}

#[derive(Default)]
struct ReliefWindow {
    lists: HashMap<DrawListId, ReliefList>,
    body_pass: Option<DrawPassId>,
    began: bool,
    wanted: bool,
    open: Vec<(DrawListId, usize)>,
    stack: Option<ReliefStack>,
}

/// Counters a host can read to see the buffer's work.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReliefStats {
    pub proxies: usize,
    pub rebuilds: u64,
    pub source_painted: u64,
    pub source_dirty: bool,
    pub output_painted: u64,
    pub output_dirty: bool,
}

/// Per-window proxy registries and relief stacks. Created by the first
/// surface that binds or registers; never by the window itself.
#[derive(Default)]
pub struct ReliefGlobal {
    windows: Vec<(WindowId, ReliefWindow)>,
    disabled: bool,
}

impl ReliefGlobal {
    fn window(&mut self, id: WindowId) -> &mut ReliefWindow {
        if let Some(i) = self.windows.iter().position(|(w, _)| *w == id) {
            return &mut self.windows[i].1;
        }
        self.windows.push((id, ReliefWindow::default()));
        &mut self.windows.last_mut().unwrap().1
    }

    fn find(&self, id: WindowId) -> Option<&ReliefWindow> {
        self.windows.iter().find(|(w, _)| *w == id).map(|(_, rw)| rw)
    }
}

/// Switch the neighbour terms off (or back on) for every window: surfaces
/// keep their own look and bind no relief levels.
pub fn set_relief_enabled(cx: &mut Cx, on: bool) {
    cx.global::<ReliefGlobal>().disabled = !on;
}

/// The window's relief counters, when it has a relief buffer.
pub fn relief_stats(cx: &Cx, window: WindowId) -> Option<ReliefStats> {
    let rw = cx.get_global_ref::<ReliefGlobal>()?.find(window)?;
    let stack = rw.stack.as_ref()?;
    let source = &cx.passes[stack.source.pass.draw_pass_id()];
    let output = &cx.passes[stack.output_pass()];
    Some(ReliefStats {
        proxies: stack.proxies,
        rebuilds: stack.rebuilds,
        source_painted: source.painted_serial,
        source_dirty: source.paint_dirty,
        output_painted: output.painted_serial,
        output_dirty: output.paint_dirty,
    })
}

fn relief_disabled(cx: &Cx) -> bool {
    cx.get_global_ref::<ReliefGlobal>().is_some_and(|g| g.disabled)
}

/// Absolute height of a registration: its depth plus its parents'.
fn entry_height(rw: &ReliefWindow, mut at: Option<(DrawListId, usize)>) -> f64 {
    let mut height = 0.0;
    let mut guard = 0;
    while let Some((list, index)) = at {
        let Some(entry) = rw.lists.get(&list).and_then(|rl| rl.entries.get(index)) else {
            break;
        };
        height += entry.depth;
        at = entry.parent;
        guard += 1;
        if guard > 256 {
            break;
        }
    }
    height
}

/// An open registration.
pub struct ReliefSlot {
    window: WindowId,
    list: DrawListId,
    index: usize,
    redraw_id: u64,
    /// The surface's absolute height, points.
    pub height: f64,
}

/// Open a surface `depth` points above the one it is drawn on. Call before
/// drawing its children; close it with [`relief_close`] once its area is
/// known. Its children's proxies land after it in painter order.
pub fn relief_open(cx: &mut Cx2d, depth: f64) -> Option<ReliefSlot> {
    if relief_disabled(cx) {
        return None;
    }
    let window = cx.get_current_window_id()?;
    let list = *cx.draw_list_stack.last()?;
    let redraw_id = cx.draw_lists[list].redraw_id;
    let pos = cx.draw_lists[list].draw_items.len();
    let rw = cx.global::<ReliefGlobal>().window(window);
    let parent = rw.open.last().copied();
    let rl = rw.lists.entry(list).or_insert_with(|| ReliefList {
        redraw_id,
        entries: Vec::new(),
    });
    if rl.redraw_id != redraw_id {
        rl.entries.clear();
        rl.redraw_id = redraw_id;
    }
    rl.entries.push(ReliefEntry {
        pos,
        parent,
        depth,
        area: Area::Empty,
        proxy: None,
    });
    let index = rl.entries.len() - 1;
    rw.open.push((list, index));
    let height = entry_height(rw, Some((list, index)));
    Some(ReliefSlot {
        window,
        list,
        index,
        redraw_id,
        height,
    })
}

/// Close a slot. `None` registers nothing but still closes its scope.
pub fn relief_close(cx: &mut Cx2d, slot: Option<ReliefSlot>, area: Area, proxy: Option<ReliefProxy>) {
    let Some(slot) = slot else { return };
    let rw = cx.global::<ReliefGlobal>().window(slot.window);
    rw.open.pop();
    if let Some(rl) = rw.lists.get_mut(&slot.list) {
        if rl.redraw_id == slot.redraw_id {
            if let Some(entry) = rl.entries.get_mut(slot.index) {
                entry.area = area;
                entry.proxy = proxy;
            }
        }
    }
}

/// The slot a shader declares a texture in, by name.
fn texture_slot(cx: &Cx, vars: &DrawVars, id: LiveId) -> Option<usize> {
    let shader = vars.draw_shader_id?;
    cx.draw_shaders[shader.index]
        .mapping
        .textures
        .iter()
        .position(|t| t.id == id)
}

/// Bind the window's relief levels to a material shader that declares
/// `relief_l1..relief_l4`, `relief_on` and `relief_size`. Every surface in a
/// window binds the same textures and values, so batching is unchanged.
pub fn bind_relief(cx: &mut Cx2d, vars: &mut DrawVars) {
    let mut levels = None;
    if !relief_disabled(cx) {
        if let Some(window) = cx.get_current_window_id() {
            let pass = cx
                .draw_list_stack
                .last()
                .and_then(|list| cx.draw_lists[*list].draw_pass_id);
            let need_stack = {
                let rw = cx.global::<ReliefGlobal>().window(window);
                rw.wanted = true;
                rw.body_pass.is_some() && rw.body_pass == pass && rw.stack.is_none()
            };
            if need_stack {
                let stack = ReliefStack::new(cx);
                cx.global::<ReliefGlobal>().window(window).stack = Some(stack);
            }
            let rw = cx.global::<ReliefGlobal>().window(window);
            if rw.body_pass.is_some() && rw.body_pass == pass {
                levels = rw.stack.as_ref().map(|stack| stack.levels());
            }
        }
    }
    let slots: Vec<Option<usize>> = RELIEF_LEVELS
        .iter()
        .map(|id| texture_slot(cx, vars, *id))
        .collect();
    let mut on = 0.0;
    match levels {
        Some(levels) if slots.iter().all(|s| s.is_some()) => {
            for (slot, texture) in slots.iter().zip(levels.iter()) {
                vars.set_texture(slot.unwrap(), texture);
            }
            on = 1.0;
        }
        // Off: let go of whatever a previous stack left bound.
        _ => {
            for slot in slots.into_iter().flatten() {
                vars.empty_texture(slot);
            }
        }
    }
    let size = cx.current_pass_size();
    vars.set_uniform(cx, live_id!(relief_on), &[on]);
    vars.set_uniform(cx, live_id!(relief_size), &[size.x as f32, size.y as f32]);
}

/// The window names the pass its body draws into this frame, or `None` when
/// that pass is not window-shaped (the exploded view, a glass capture).
pub(crate) fn begin_window_relief_frame(cx: &mut Cx, window: WindowId, body: Option<DrawPassId>) {
    if !cx.has_global::<ReliefGlobal>() {
        return;
    }
    // Entries of closed windows go; a recycled id starts fresh.
    let valid: Vec<bool> = cx
        .get_global_ref::<ReliefGlobal>()
        .unwrap()
        .windows
        .iter()
        .map(|(w, _)| cx.windows.is_valid(*w))
        .collect();
    let global = cx.global::<ReliefGlobal>();
    let mut valid = valid.into_iter();
    global.windows.retain(|_| valid.next().unwrap_or(false));
    let rw = global.window(window);
    rw.body_pass = body;
    rw.began = true;
    rw.wanted = false;
    rw.open.clear();
}

/// Build (only if it changed) and attach the window's relief buffer.
pub(crate) fn end_window_relief_frame(cx: &mut Cx2d, window: WindowId) {
    if !cx.has_global::<ReliefGlobal>() {
        return;
    }
    let (began, wanted, body) = {
        let rw = cx.global::<ReliefGlobal>().window(window);
        let began = std::mem::replace(&mut rw.began, false);
        (began, rw.wanted, rw.body_pass)
    };
    if !began {
        // The first surface bound this frame, before the window knew to look:
        // one more frame of this window, and from then on it names its body.
        if wanted {
            if let Some(list) = cx.windows[window]
                .main_pass_id
                .and_then(|pass| cx.passes[pass].main_draw_list_id)
            {
                cx.redraw_list(list);
            }
        }
        return;
    }
    let Some(body) = body else { return };
    let Some(root) = cx.passes[body].main_draw_list_id else {
        return;
    };
    let Some(mut stack) = cx.global::<ReliefGlobal>().window(window).stack.take() else {
        return;
    };
    let proxies = {
        let cx_ref: &Cx = cx;
        let rw = cx_ref
            .get_global_ref::<ReliefGlobal>()
            .unwrap()
            .find(window)
            .unwrap();
        collect_proxies(cx_ref, rw, root)
    };
    // Registrations of lists that were recorded again without registering,
    // or freed, are gone.
    {
        let stale: Vec<DrawListId> = {
            let cx_ref: &Cx = cx;
            let rw = cx_ref
                .get_global_ref::<ReliefGlobal>()
                .unwrap()
                .find(window)
                .unwrap();
            rw.lists
                .iter()
                .filter(|(id, rl)| {
                    cx_ref.draw_lists.is_id_freed(**id)
                        || cx_ref.draw_lists[**id].redraw_id != rl.redraw_id
                })
                .map(|(id, _)| *id)
                .collect()
        };
        let rw = cx.global::<ReliefGlobal>().window(window);
        for id in stale {
            rw.lists.remove(&id);
        }
    }
    let size = cx
        .get_pass_rect(body, 1.0)
        .map(|r| r.size)
        .unwrap_or(dvec2(0.0, 0.0));
    let hash = hash_proxies(&proxies, size);
    if size.x >= 1.0 && size.y >= 1.0 && hash != stack.hash {
        stack.record(cx, &proxies, size);
        stack.hash = hash;
        stack.rebuilds += 1;
        stack.proxies = proxies.len();
    }
    // Declared a dependency of the body on every frame the body records,
    // whether or not the chain was recorded again.
    cx.attach_child_pass(stack.output_pass(), body, Some(root));
    cx.global::<ReliefGlobal>().window(window).stack = Some(stack);
}

fn collect_proxies(cx: &Cx, rw: &ReliefWindow, root: DrawListId) -> Vec<Resolved> {
    fn emit(
        cx: &Cx,
        rw: &ReliefWindow,
        list: DrawListId,
        index: usize,
        entry: &ReliefEntry,
        out: &mut Vec<Resolved>,
    ) {
        let Some(proxy) = entry.proxy else { return };
        let Some(area_list) = entry.area.draw_list_id() else {
            return;
        };
        if cx.draw_lists.is_id_freed(area_list) {
            return;
        }
        let mut rect = entry.area.rect(cx);
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }
        let dl = &cx.draw_lists[area_list];
        // What its view's clip (a scroll view) still shows of it, in window
        // coordinates, and its own draw clip where the shader exposes one.
        let mut clip = Rect {
            pos: dvec2(-1.0e6, -1.0e6),
            size: dvec2(2.0e6, 2.0e6),
        };
        if dl.draw_list_has_clip {
            let s = dl.draw_list_uniforms.view_shift;
            rect.pos += dvec2(s.x as f64, s.y as f64);
            let c = dl.draw_list_uniforms.view_clip;
            clip = Rect {
                pos: dvec2(c.x as f64, c.y as f64),
                size: dvec2((c.z - c.x) as f64, (c.w - c.y) as f64),
            };
        }
        let own = entry.area.clipped_rect(cx);
        if own.size.x > 0.0 && own.size.y > 0.0 {
            clip = clip.clip((own.pos, own.pos + own.size));
        }
        let inset = proxy.inset.min(rect.size.x * 0.5).min(rect.size.y * 0.5);
        rect.pos += dvec2(inset, inset);
        rect.size -= dvec2(inset * 2.0, inset * 2.0);
        let x0 = rect.pos.x.max(clip.pos.x);
        let y0 = rect.pos.y.max(clip.pos.y);
        let x1 = (rect.pos.x + rect.size.x).min(clip.pos.x + clip.size.x);
        let y1 = (rect.pos.y + rect.size.y).min(clip.pos.y + clip.size.y);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        out.push(Resolved {
            rect,
            clip,
            radius: proxy.radius,
            disc: proxy.disc,
            height: entry_height(rw, Some((list, index))),
            emissive: proxy.emissive,
        });
    }
    fn walk(
        cx: &Cx,
        rw: &ReliefWindow,
        list: DrawListId,
        seen: &mut HashSet<DrawListId>,
        out: &mut Vec<Resolved>,
    ) {
        if cx.draw_lists.is_id_freed(list) || !seen.insert(list) {
            return;
        }
        let draw_list = &cx.draw_lists[list];
        let entries: &[ReliefEntry] = match rw.lists.get(&list) {
            Some(rl) if rl.redraw_id == draw_list.redraw_id => &rl.entries,
            _ => &[],
        };
        let mut next = 0;
        for item in 0..draw_list.draw_items.len() {
            while next < entries.len() && entries[next].pos <= item {
                emit(cx, rw, list, next, &entries[next], out);
                next += 1;
            }
            if let Some(sub) = draw_list.draw_items[item].kind.sub_list() {
                walk(cx, rw, sub, seen, out);
            }
        }
        while next < entries.len() {
            emit(cx, rw, list, next, &entries[next], out);
            next += 1;
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    walk(cx, rw, root, &mut seen, &mut out);
    out
}

fn hash_proxies(proxies: &[Resolved], size: Vec2d) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    size.x.to_bits().hash(&mut h);
    size.y.to_bits().hash(&mut h);
    for p in proxies {
        for v in [
            p.rect.pos.x,
            p.rect.pos.y,
            p.rect.size.x,
            p.rect.size.y,
            p.clip.pos.x,
            p.clip.pos.y,
            p.clip.size.x,
            p.clip.size.y,
            p.radius,
            p.height,
        ] {
            v.to_bits().hash(&mut h);
        }
        p.disc.hash(&mut h);
        for v in [p.emissive.x, p.emissive.y, p.emissive.z, p.emissive.w] {
            v.to_bits().hash(&mut h);
        }
    }
    // Never equal to the empty stack's initial value.
    h.finish() | 1
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawReliefProxy {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    emissive: Vec4f,
    /// The visible part, window coordinates: x0, y0, x1, y1.
    #[live]
    clip: Vec4f,
    #[live]
    radius: f32,
    #[live]
    height: f32,
    #[live]
    disc: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawReliefDown {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawReliefUp {
    #[deref]
    draw_super: DrawQuad,
}

struct ReliefStage {
    pass: DrawPass,
    list: DrawList2d,
    texture: Texture,
    dpi: f64,
}

impl ReliefStage {
    fn new(cx: &mut Cx, name: &str, dpi: f64) -> Self {
        let pass = DrawPass::new_with_name(cx, name);
        let list = DrawList2d::new(cx);
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderRGBAf16 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        pass.set_color_texture(
            cx,
            &texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        Self {
            pass,
            list,
            texture,
            dpi,
        }
    }

    fn begin(&mut self, cx: &mut Cx2d, size: Vec2d) -> Vec2d {
        self.pass.set_size(cx, size);
        cx.begin_pass(&self.pass, Some(self.dpi));
        self.list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        pass_size
    }

    fn end(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
    }
}

/// The relief pass and its blur chain, half-float throughout: source at
/// half density, four downsamples, and tent upsamples that re-home every
/// deep level at the first level's quarter density.
pub(crate) struct ReliefStack {
    source: ReliefStage,
    downs: Vec<ReliefStage>,
    ups: Vec<Vec<ReliefStage>>,
    proxy: DrawReliefProxy,
    downsample: DrawReliefDown,
    upsample: DrawReliefUp,
    hash: u64,
    rebuilds: u64,
    proxies: usize,
}

impl ReliefStack {
    fn new(cx: &mut Cx) -> Self {
        let source = ReliefStage::new(cx, "relief_source", RELIEF_DPI);
        let downs = (0..RELIEF_DOWNS)
            .map(|i| {
                let dpi = RELIEF_DPI / (1u32 << (i + 1)) as f64;
                ReliefStage::new(cx, &format!("relief_down_{i}"), dpi)
            })
            .collect();
        // Level i (1..) is re-homed at downs[0]'s density one doubling at a time.
        let ups = (1..RELIEF_DOWNS)
            .map(|i| {
                (0..i)
                    .map(|step| {
                        let dpi = RELIEF_DPI / (1u32 << (i - step)) as f64;
                        ReliefStage::new(cx, &format!("relief_up_{i}_{step}"), dpi)
                    })
                    .collect()
            })
            .collect();
        let (proxy, downsample, upsample) = cx.with_vm(|vm| {
            (
                DrawReliefProxy::script_new_with_default(vm),
                DrawReliefDown::script_new_with_default(vm),
                DrawReliefUp::script_new_with_default(vm),
            )
        });
        Self {
            source,
            downs,
            ups,
            proxy,
            downsample,
            upsample,
            hash: 0,
            rebuilds: 0,
            proxies: 0,
        }
    }

    /// The four levels a consumer binds, blur growing, all at one density.
    fn levels(&self) -> [Texture; 4] {
        [
            self.downs[0].texture.clone(),
            self.ups[0].last().unwrap().texture.clone(),
            self.ups[1].last().unwrap().texture.clone(),
            self.ups[2].last().unwrap().texture.clone(),
        ]
    }

    /// Producer before consumer: source, downsamples, then each re-homing.
    fn chain(&self) -> Vec<DrawPassId> {
        let mut out = vec![self.source.pass.draw_pass_id()];
        out.extend(self.downs.iter().map(|s| s.pass.draw_pass_id()));
        for chain in &self.ups {
            out.extend(chain.iter().map(|s| s.pass.draw_pass_id()));
        }
        out
    }

    fn output_pass(&self) -> DrawPassId {
        *self.chain().last().unwrap()
    }

    fn record(&mut self, cx: &mut Cx2d, proxies: &[Resolved], size: Vec2d) {
        self.source.begin(cx, size);
        for p in proxies {
            self.proxy.radius = p.radius as f32;
            self.proxy.height = p.height as f32;
            self.proxy.disc = if p.disc { 1.0 } else { 0.0 };
            self.proxy.emissive = p.emissive;
            self.proxy.clip = vec4(
                p.clip.pos.x as f32,
                p.clip.pos.y as f32,
                (p.clip.pos.x + p.clip.size.x) as f32,
                (p.clip.pos.y + p.clip.size.y) as f32,
            );
            self.proxy.draw_abs(cx, p.rect);
        }
        self.source.end(cx);

        let full = |size: Vec2d| Rect {
            pos: dvec2(0.0, 0.0),
            size,
        };
        let mut source = self.source.texture.clone();
        for stage in self.downs.iter_mut() {
            let pass_size = stage.begin(cx, size);
            self.downsample.draw_vars.set_texture(0, &source);
            self.downsample.draw_abs(cx, full(pass_size));
            stage.end(cx);
            source = stage.texture.clone();
        }
        for (i, chain) in self.ups.iter_mut().enumerate() {
            let mut source = self.downs[i + 1].texture.clone();
            for stage in chain.iter_mut() {
                let pass_size = stage.begin(cx, size);
                self.upsample.draw_vars.set_texture(0, &source);
                self.upsample.draw_abs(cx, full(pass_size));
                stage.end(cx);
                source = stage.texture.clone();
            }
        }
        // Recorded draw calls own their inputs.
        self.downsample.draw_vars.empty_texture(0);
        self.upsample.draw_vars.empty_texture(0);

        let chain = self.chain();
        for pair in chain.windows(2) {
            cx.attach_child_pass(pair[0], pair[1], None);
        }
    }
}

/// A moulded surface: see the module docs and the `ReliefView` script.
#[derive(Script, ScriptHook, Widget)]
pub struct ReliefView {
    #[deref]
    pub view: View,
    /// Elevation over the surface below, points; negative sinks it.
    #[live(0.0)]
    pub depth: f64,
    /// How the face itself bulges: `None` follows `depth` at rest.
    #[live]
    pub convex: Option<f64>,
    /// Signed change of elevation while `held`.
    #[live(-1.0)]
    pub press: f64,
    /// Pressed in (0..1).
    #[live(0.0)]
    pub held: f64,
    /// Margin between the rect and the shape: room for its own shadow.
    #[live(0.0)]
    pub inset: f64,
    #[live(8.0)]
    pub radius: f64,
    #[live(false)]
    pub disc: bool,
    /// The light it gives its neighbours while lit: rgb, intensity.
    #[live]
    pub emissive: Vec4f,
    /// Lit (0..1): scales the emissive and the face's own glow.
    #[live(0.0)]
    pub lit: f64,
    /// Whether it stands in the relief buffer at all.
    #[live(true)]
    pub proxy: bool,
    /// Extra textures the host gives its shader, bound by name each draw.
    #[rust]
    textures: Vec<(LiveId, Texture)>,
    #[rust]
    slot: Option<ReliefSlot>,
    #[rust]
    drawing: bool,
}

impl ReliefView {
    pub fn current_depth(&self) -> f64 {
        self.depth + self.held * self.press
    }

    pub fn set_lit(&mut self, cx: &mut Cx, lit: f64) {
        if self.lit != lit {
            self.lit = lit;
            self.view.redraw(cx);
        }
    }

    pub fn set_held(&mut self, cx: &mut Cx, held: f64) {
        if self.held != held {
            self.held = held;
            self.view.redraw(cx);
        }
    }

    fn push_instances(&mut self, cx: &mut Cx2d, height: f64) {
        let depth = self.current_depth();
        let convex = self.convex.unwrap_or(self.depth);
        let vars = &mut self.view.draw_bg.draw_vars;
        vars.set_dyn_instance(cx, live_id!(depth), &[depth as f32]);
        vars.set_dyn_instance(cx, live_id!(convex), &[convex as f32]);
        vars.set_dyn_instance(cx, live_id!(lit), &[self.lit as f32]);
        vars.set_dyn_instance(cx, live_id!(inset), &[self.inset as f32]);
        vars.set_dyn_instance(cx, live_id!(radius), &[self.radius as f32]);
        vars.set_dyn_instance(cx, live_id!(disc), &[if self.disc { 1.0 } else { 0.0 }]);
        vars.set_dyn_instance(cx, live_id!(height), &[height as f32]);
    }

    fn own_proxy(&self) -> Option<ReliefProxy> {
        self.proxy.then(|| ReliefProxy {
            inset: self.inset,
            radius: self.radius,
            disc: self.disc,
            emissive: vec4(
                self.emissive.x,
                self.emissive.y,
                self.emissive.z,
                self.emissive.w * self.lit as f32,
            ),
        })
    }

    fn begin_relief(&mut self, cx: &mut Cx2d) {
        if self.drawing {
            return;
        }
        self.drawing = true;
        bind_relief(cx, &mut self.view.draw_bg.draw_vars);
        for (id, texture) in &self.textures {
            if let Some(slot) = texture_slot(cx, &self.view.draw_bg.draw_vars, *id) {
                self.view.draw_bg.draw_vars.set_texture(slot, texture);
            }
        }
        self.slot = relief_open(cx, self.current_depth());
        let height = self.slot.as_ref().map(|s| s.height).unwrap_or(0.0);
        self.push_instances(cx, height);
    }

    fn end_relief(&mut self, cx: &mut Cx2d) {
        if !self.drawing {
            return;
        }
        self.drawing = false;
        let proxy = self.own_proxy();
        relief_close(cx, self.slot.take(), self.view.area(), proxy);
    }
}

impl Widget for ReliefView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.begin_relief(cx);
        self.view.draw_walk(cx, scope, walk)?;
        self.end_relief(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl ReliefViewRef {
    pub fn set_lit(&self, cx: &mut Cx, lit: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_lit(cx, lit);
        }
    }

    pub fn set_held(&self, cx: &mut Cx, held: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_held(cx, held);
        }
    }

    pub fn lit(&self) -> f64 {
        self.borrow().map(|inner| inner.lit).unwrap_or(0.0)
    }

    /// A uniform on the surface's shader (a knob's value), with a redraw.
    pub fn set_bg_uniform(&self, cx: &mut Cx, uniform: LiveId, value: &[f32]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.view.draw_bg.set_uniform(cx, uniform, value);
            inner.view.redraw(cx);
        }
    }

    /// A texture the surface's shader declares under `id`, bound on every
    /// draw from now on.
    pub fn set_bg_texture(&self, cx: &mut Cx, id: LiveId, texture: &Texture) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.textures.retain(|(t, _)| *t != id);
            inner.textures.push((id, texture.clone()));
            inner.view.redraw(cx);
        }
    }
}
