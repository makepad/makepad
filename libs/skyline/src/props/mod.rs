//! Props: every turbine, lamp, pylon, house, bridge ... as instances of simple parts, built in Rust from the zones when a
//! parameter that places them changes (the generators, one file each, in the page's order), and drawn by one part
//! shader (`kit.rs`) in the scene's painter's order, one quad per instance placed for the view.
pub mod kit;

mod bridge; mod churches; mod coast; mod cranes; mod fair; mod far; mod farms; mod industry; mod lamps; mod masts; mod pylons; mod town; mod turbines;

use crate::params::{Params, P};
use crate::render::{bind_texture, SceneCtx, SceneTarget};
use crate::rng::{fract, wd, TAU};
use crate::zones::{mt_dist, Zones};
use kit::{Ctx, DrawSkyPart, DrawSkyPartBlit, Instance, PropSet, EYE};
use makepad_widgets::makepad_draw::*;

/// A prop family's generator.
pub type Generator = fn(&mut Ctx);

/// The generators in the page's order (`pkBuild`).
pub const GENERATORS: [(&str, Generator); 13] = [
    ("turbines", turbines::build), ("lamps", lamps::build), ("pylons", pylons::build), ("industry", industry::build), ("cranes", cranes::build),
    ("churches", churches::build), ("town", town::build), ("bridge", bridge::build), ("fair", fair::build), ("masts", masts::build),
    ("far", far::build), ("farms", farms::build), ("coast", coast::build),
];

/// The parameters that place props (the page rebuilds them on any change but the view's).
pub const PROP_KEYS: &[P] = &[
    P::Land, P::LandC, P::MtH, P::MtW, P::MtC, P::MtRug, P::CtH, P::CtD, P::CtW, P::CtC, P::CtR, P::TrD, P::TrH, P::TrCon, P::TrW,
    P::TrC, P::Treeline, P::PrPyl, P::PrTurb, P::PrCrane, P::PrInd, P::PrChurch, P::PrTown, P::PrLamp, P::PrBridge, P::PrFair,
    P::PrRefl, P::PrRipple, P::PrMast, P::PrFar, P::PrFarm, P::PrCoast, P::Lights, P::Temp, P::WinD, P::Glow, P::Haze, P::Off, P::OffDir,
    P::Size, P::SeaD,
];

/// Where in the scene's painter's order a group of instances is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bucket {
    /// Just before range k: wind turbines on range k, and the props hidden behind range k (`hideM`) at its distance.
    Range(usize),
    /// After every range, before the city: the bridge (the industry stands in front of it), the industry, the far cranes.
    BehindCity,
    /// After the city, before the front treeline and the ground band: the props layer.
    FrontOfCity,
    /// After the ground band: the props layer again where it stands on the water, down to each prop's own waterline.
    OverSea,
}

/// Builds every prop of a parameter set.
pub fn build(z: &Zones, p: &Params) -> PropSet {
    let mut set = PropSet::default();
    let mut c = Ctx::new(z, p, &mut set);
    for (_, generate) in GENERATORS { generate(&mut c); }
    set
}

/// One draw of an instance: its bucket (0-4 the ranges, 5 behind the city, 6 in front, 7 over the sea), its distance
/// window (m: a line is drawn in slices, each in its place in the painter's order) and its sort key.
#[derive(Clone, Copy)]
struct Item { b: u8, i: usize, win: [f64; 2], key: (u8, f64) }

/// The draws in painter's order: by bucket, far to near (the page's farthest-first lists; ties keep the page's order). The
/// page hides props behind nearer ranges per pixel; here they go before the range in front of them, and a line is cut in
/// distance slices (bounds every 25% from 100 m, one from 0 to 100 m, and at the two nearest ranges), each placed by its distance.
fn items(set: &PropSet) -> Vec<Item> {
    let (d3, d4) = (mt_dist(3.0), mt_dist(4.0));
    let mut bounds: Vec<f64> = (0..60).map(|j| 100.0 * 1.25f64.powi(j)).chain([0.0, d3, d4]).collect();
    bounds.sort_by(f64::total_cmp);
    let mut out = vec![];
    for (n, i) in set.inst.iter().enumerate() {
        let (_, _, dmin, dmax, _) = line_range(i);
        let wins: Vec<[f64; 2]> = if i.line { bounds.windows(2).filter(|w| w[1] > dmin && w[0] < dmax).map(|w| [w[0], w[1]]).collect() } else { vec![[0.0, 1e9]] };
        for win in wins {
            let d = if i.line { (win[0] * win[1]).sqrt() } else { i.d };
            let b = match i.slot { 0 => i.range, 1 => 5, _ if i.hide_m && d > d3 => 3, _ if i.hide_m && d > d4 => 4, _ if i.hide_i => 5, _ => 6 };
            out.push(Item { b, i: n, win, key: (u8::from(i.slot == 1), -d) });
            if i.slot == 2 { out.push(Item { b: 7, i: n, win, key: (0, -d) }); }
        }
    }
    out.sort_by(|x, y| (x.b, x.key.0).cmp(&(y.b, y.key.0)).then(x.key.1.total_cmp(&y.key.1)));
    out
}

/// A line's direction, its foot (s nearest the eye), its distance range along it (m) and the distance of its infinite
/// line; a plane's distance.
fn line_range(i: &Instance) -> ([f64; 2], f64, f64, f64, f64) {
    if !i.line { return ([0.0; 2], 0.0, i.d, i.d, i.d); }
    let (t, s0, s1) = ([i.ang.cos(), i.ang.sin()], -i.smar, i.len + i.smar);
    let (sf, p, dist) = (-(i.o[0] * t[0] + i.o[1] * t[1]), (i.o[0] * t[1] - i.o[1] * t[0]).abs(), |s: f64| (i.o[0] + s * t[0]).hypot(i.o[1] + s * t[1]));
    (t, sf, if sf > s0 && sf < s1 { p } else { dist(s0).min(dist(s1)) }, dist(s0).max(dist(s1)), p)
}

/// The highest raw elevation (rad) any prop reaches over the off-centre camera's nearest (1 - Off). 0 without props.
pub fn reach(set: &PropSet, p: &Params) -> f64 {
    let (k, near) = (p[P::Size], (1.0 - p[P::Off]).max(0.1));
    let top = set.inst.iter().map(|i| {
        let (_, _, dmin, _, _) = line_range(i);
        // as `place` puts its top row: (top - eye) / d + the foot's elevation over the plain, its glow above
        let base = if i.line { 0.0 } else { i.base + EYE * k / i.d };
        ((i.top + i.glow) * k - EYE * k) / dmin.max(1.0) + base
    }).fold(0.0f64, f64::max);
    top / near
}

/// Whether the reflection run sees instance `i` from the mirrored eye (the eye as far under the water as it is over it):
/// the props layer (slot 2) as the sea and the lakes mirror it, standing on the water. Under a wet ground the strip draws
/// itself (`SkyLight::own_wet`), a prop on land is seen from the eye itself: the wet ground lies in front of the near rise,
/// behind which the props on land stand, and mirrors them at the mirrored angle with the rise and the treeline (`draw_scene`).
fn mirrored_eye(sc: &SceneCtx, i: &Instance) -> bool {
    if !sc.mirror || i.slot != 2 { return false; }
    // the land share where the prop stands (a plane's bearing, a line's middle); from 0.35 up no sea (`water_at`)
    let u = if i.line { let m = 0.5 * i.len; fract((i.o[1] + m * i.ang.sin()).atan2(i.o[0] + m * i.ang.cos()) / TAU) } else { i.uc };
    !(sc.light.own_wet() > 0.0) || sc.z.land(u) < 0.35
}

/// The pass rects of instance i's draw in window `win`: its azimuth extent (the page's bucket extent) and its height band
/// from row ymin down; a rect per side of a line's nearest point, and per turn where it wraps round the strip. Its top,
/// half-width and glow are in size units (`kit`): Size m each.
fn place(sc: &SceneCtx, i: &Instance, win: [f64; 2], ymin: f64, mut out: impl FnMut(Rect)) {
    let (v, p, warp) = (sc.view, sc.p, i.view == 0);
    let (top, hw) = (i.top * p[P::Size], (i.hw + i.glow) * p[P::Size]);
    let oc = [p[P::Off] * (p[P::OffDir] * TAU).cos(), p[P::Off] * (p[P::OffDir] * TAU).sin()];
    // the scenery's azimuth u as the raw view sees it, and the off-centre camera's scale there
    let raw = |u: f64| if warp { ((u * TAU).sin() - oc[1]).atan2((u * TAU).cos() - oc[0]) / TAU } else { u };
    let scale = |u: f64| { let cd = oc[0] * (u * TAU).cos() + oc[1] * (u * TAU).sin(); -cd + (cd * cd - oc[0] * oc[0] - oc[1] * oc[1] + 1.0).max(1e-4).sqrt() };
    let (eye, base) = ((if mirrored_eye(sc, i) { -EYE } else { EYE }) * p[P::Size], if i.line { 0.0 } else { i.base + EYE * p[P::Size] / i.d });
    let mut piece = |ua: f64, ub: f64, d0: f64, d1: f64| {
        let (mut a, b) = (raw(ua), raw(ub));
        let mut w = fract(b - a);
        if w > 0.5 { (a, w) = (b, 1.0 - w); }
        let (a, w, pad) = (a - 0.001, w + 0.002, 3.0 * v.px0());
        let (tn, tx) = (0..5).map(|k| if warp { scale(a + w * k as f64 / 4.0) } else { 1.0 }).fold((f64::MAX, 0.0f64), |(n, x), t| (n.min(t), x.max(t)));
        let (lo, hi) = [-eye / d0, -eye / d1, (top - eye) / d0, (top - eye) / d1].map(|e| e + base).iter().fold((f64::MAX, f64::MIN), |(n, x), &e| (n.min(e), x.max(e)));
        let (lo, hi) = (lo / if lo < 0.0 { tn } else { tx } - pad, hi / if hi > 0.0 { tn } else { tx } + pad);
        let (ya, yb) = if sc.mirror { (v.row_of(-lo).max(v.row_of(0.0)), v.row_of(-hi)) } else { (v.row_of(hi), v.row_of(lo)) }; // the mirror only looks up
        let (y0, y1) = (ya.floor().max(ymin), yb.ceil().min(v.h as f64));
        for k in [-1.0, 0.0, 1.0].into_iter().filter(|_| y1 > y0) {
            let x = |u: f64| (u + k - v.u0) / v.span * v.w as f64;
            let (x0, x1) = (x(a).floor().max(0.0), x(a + w).ceil().min(v.w as f64));
            if x1 > x0 { out(Rect { pos: dvec2(x0, y0), size: dvec2(x1 - x0, y1 - y0) }); }
        }
    };
    if !i.line { let h = hw / (TAU * i.d); return piece(i.uc - h, i.uc + h, i.d, i.d); }
    let (t, sf, dmin, dmax, pd) = line_range(i);
    let at = |s: f64| [i.o[0] + s * t[0], i.o[1] + s * t[1]];
    let (az, dist) = (|s: f64| { let q = at(s); q[1].atan2(q[0]) / TAU }, |s: f64| { let q = at(s); q[0].hypot(q[1]) });
    let (ra, rb) = ((win[0] * win[0] - pd * pd).max(0.0).sqrt(), (win[1].min(dmax) * win[1].min(dmax) - pd * pd).max(0.0).sqrt());
    // the two sides of the nearest point; one rect when theirs would overlap (a pixel must be drawn once)
    let sides = [(sf - rb, sf - ra), (sf + ra, sf + rb)].map(|(a, b)| (a.max(-i.smar), b.min(i.len + i.smar)));
    let one = sides.iter().all(|q| q.1 > q.0) && wd(az(sides[1].0), az(sides[0].1)).abs() < 0.003 + 4.0 * v.span / v.w as f64;
    for (a, b) in (if one { vec![(sides[0].0, sides[1].1)] } else { sides.to_vec() }).into_iter().filter(|q| q.1 > q.0) {
        piece(az(a), az(b), (if sf > a && sf < b { pd } else { dist(a).min(dist(b)) }).max(win[0]).max(dmin), dist(a).max(dist(b)));
    }
}

/// Texels per row of the parts texture.
const TW: usize = 1024;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.SkyPropsBase = #(Props::script_component(vm))
    mod.draw.SkyProps = mod.std.set_type_default() do mod.draw.SkyPropsBase{
        draw_part: mod.draw.DrawSkyPart{}
        draw_blit: mod.draw.DrawSkyPartBlit{}
    }
}

#[derive(Script, ScriptHook)]
pub struct Props {
    #[live] pub draw_part: DrawSkyPart, #[live] pub draw_blit: DrawSkyPartBlit,
    #[rust] pub set: PropSet, #[rust] pub key: Option<u64>, #[rust] items: Vec<Item>,
    /// The parts as texels (type and c, a, b) and their rows; whether the parts changed; the props layer's own pass.
    #[rust] tex: Option<(Texture, usize)>, #[rust] dirty: bool, #[rust] pub pl: Option<SceneTarget>,
}

impl Props {
    /// Rebuilds the instances when a parameter that places them changed.
    pub fn update(&mut self, z: &Zones, p: &Params) {
        let key = p.hash(PROP_KEYS, &[]);
        if self.key != Some(key) { (self.set, self.key, self.dirty) = (build(z, p), Some(key), true); self.items = items(&self.set); }
    }

    /// Draws the instances of one bucket into the scene (`sc.mirror`: the props layer seen from the mirrored eye). Over the
    /// sea band the props layer is first composited on its own, in a pass of its own (only the rows the ground band covers),
    /// then blitted with the waterline weight, as the page does: drawing each instance again with that weight would let
    /// stacked bodies dim the lights in front of them many times over. The trees' and the city's passes paint first.
    pub fn draw(&mut self, cx: &mut Cx2d, sc: &SceneCtx, bucket: Bucket) {
        let b = match bucket { Bucket::Range(k) => k as u8, Bucket::BehindCity => 5, Bucket::FrontOfCity => 6, Bucket::OverSea => 7 };
        let (lo, hi, v) = (self.items.partition_point(|it| it.b < b), self.items.partition_point(|it| it.b <= b), sc.view);
        if lo == hi { return; }
        if self.dirty || self.tex.is_none() { // the parts texture
            let rows = (3 * self.set.parts.len()).div_ceil(TW).max(1);
            let mut data = vec![0f32; TW * rows * 4];
            for (o, q) in data.chunks_mut(12).zip(&self.set.parts) {
                o.copy_from_slice(&[q.t as u8 as f64, q.c[0], q.c[1], q.c[2], q.a[0], q.a[1], q.a[2], q.a[3], q.b[0], q.b[1], q.b[2], q.b[3]].map(|x| x as f32));
            }
            let format = TextureFormat::VecRGBAf32 { width: TW, height: rows, data: Some(data), updated: TextureUpdated::Full };
            self.tex = Some(match self.tex.take() { Some((t, _)) => { *t.get_format(cx) = format; (t, rows) } None => (Texture::new_with_format(cx, format), rows) });
            self.dirty = false;
        }
        if b < 7 { return self.instances(cx, sc, lo..hi, 0.0, false); }
        let y0 = v.row_of(sc.rise() + 2.0 * v.px0()).floor().max(0.0);
        if y0 >= v.h as f64 { return; }
        let mut pl = self.pl.take().unwrap_or_else(|| SceneTarget::with_dist(cx, "skyline_props", true, sc.dist));
        pl.record(cx, v.w, v.h, |cx| self.instances(cx, sc, lo..hi, y0, true));
        cx.make_child_pass(&pl.pass);
        for i in sc.inputs.into_iter().flatten() { cx.attach_child_pass(i, pl.pass.draw_pass_id(), None); } // it reads the trees and the city
        let dv = &mut self.draw_blit.draw_vars;
        sc.set_uniforms(cx, dv);
        for (name, tex) in [(live_id!(pl_c), &pl.color), (live_id!(pl_e), &pl.light), (live_id!(pl_d), pl.dist_in())] { bind_texture(cx, dv, name, tex); }
        self.draw_blit.draw_abs(cx, Rect { pos: dvec2(0.0, y0), size: dvec2(v.w as f64, v.h as f64 - y0) });
        self.pl = Some(pl);
    }

    /// Draws `self.items[r]` from pass row `y0` down, hidden by nearer city surfaces (`pl`: into the props layer's pass).
    /// The shader gets an instance's top and half-width in metres (`kit`'s size units times Size).
    fn instances(&mut self, cx: &mut Cx2d, sc: &SceneCtx, r: std::ops::Range<usize>, y0: f64, pl: bool) {
        let ((tex, rows), dp, k) = (self.tex.as_ref().unwrap(), &mut self.draw_part, sc.p[P::Size]);
        sc.set_uniforms(cx, &mut dp.draw_vars);
        bind_texture(cx, &mut dp.draw_vars, live_id!(parts), tex);
        crate::city::bind_city(cx, &mut dp.draw_vars, sc);
        dp.draw_vars.set_uniform(cx, live_id!(parts_geom), &[TW as f32, *rows as f32, if sc.city.is_some() { 1.0 } else { 0.0 }, if pl { 1.0 } else { 0.0 }]);
        for it in &self.items[r] {
            let i = &self.set.inst[it.i];
            // the shader takes the mirrored eye for slot 2 in the reflection run: a prop seen from the eye itself goes as slot 0
            let slot = if sc.mirror && i.slot == 2 && !mirrored_eye(sc, i) { 0 } else { i.slot };
            let fl = (slot as u32 + 4 * i.view as u32 + 8 * i.hide_c as u32 + 16 * i.line as u32 + 32 * i.hide_m as u32) as f32;
            (dp.h0, dp.h2) = if i.line { (vec4(0.0, i.smar as f32, 0.0, fl), vec4(i.o[0] as f32, i.o[1] as f32, i.ang as f32, i.len as f32)) }
                else { (vec4(i.uc as f32, i.d as f32, (i.base + EYE * k / i.d) as f32, fl), vec4((i.hw * k) as f32, 0.0, 0.0, 0.0)) };
            (dp.h1, dp.h3) = (vec4((3 * i.p0) as f32, i.np as f32, (i.top * k) as f32, i.td as f32), vec4(it.win[0] as f32, it.win[1] as f32, (i.seed & 8191) as f32, 0.0));
            place(sc, i, it.win, y0, |r| dp.draw_abs(cx, r));
        }
    }
}
