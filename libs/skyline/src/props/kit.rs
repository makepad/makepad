//! The props kit: the part types, the instance and part records the generators emit, the builders they share (box,
//! gable, beacon, lights, the town kit and site), and the part shader. A prop instance stands in its own upright plane
//! facing the camera `d` m away, or along a straight line in the plan (a road, a power line, a bridge) in true
//! perspective; its parts are simple shapes in metres in that frame (the page's `PT_*`). The page drew them with a
//! per-pixel interpreter walking bucket lists; here each instance is one quad placed for the view (`mod.rs`), whose
//! pixel function is the page's instance frame and `ppDraw` over its own parts, read from a data texture. So the page's
//! per-instance rules hold as they are (parts merge by max, an instance's bodies never dim its own lights, REP / CLIP /
//! WASH act on their neighbouring parts), and the cost follows the covered pixels.
//!
//! Size: at Size k a prop is k times as large and stands where it stood. Its parts, half-width, top and glow are in size
//! units (k m each; at Size 1 metres), and so are the generators' spacings, so k times as large means as many over k, and
//! the eye's height over the plain (`EYE` size units), so what stands on the plain is the base's view k times as large;
//! its bearing, distance and the plan of a line stay in metres, and so do its haze and tone (by distance). A generator
//! turns a size-unit offset beside a prop d m away into an angle over d / k, and a foot of f size units into f k m.
use crate::params::{Params, P};
use crate::rng::{fract, mix, smooth, wd, TAU};
use crate::zones::Zones;
use makepad_widgets::makepad_draw::*;

/// The props' eye height above the ground (m at Size 1; size units: at Size k it is k times as high, `Ctx::eye`).
pub const EYE: f64 = 6.3;

/// Part types (the page's `PT`, documented there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Pt {
    Seg = 0, Trap = 1, Ell = 2, Lat = 3, Ring = 4, Light = 5, Beacon = 6, Win = 7, Arc = 8, Band = 9, LRow = 10, Plume = 20, Gable = 21,
    Wash = 30, Box = 31, GProf = 32, Prof = 40, PBand = 41, Slr = 42, Fan = 43, Rep = 50, Clip = 51, Beam = 60, LBox = 61, CLight = 62,
}

/// One prop instance (the page's `add(o)`); its parts are `parts[p0 .. p0 + np]` of the [`PropSet`].
#[derive(Clone, Debug)]
pub struct Instance {
    /// Slot 0 turbines (on range `range`), 1 behind the city, 2 the props layer; view 0 the scenery (off-centre warp), 1 the city camera.
    pub slot: u8, pub range: u8, pub view: u8,
    /// Hidden by nearer city surfaces / mountain ranges / the industry; a line frame (else a plane).
    pub hide_c: bool, pub hide_m: bool, pub hide_i: bool, pub line: bool,
    /// Plane: bearing, distance (m), foot elevation (rad; NaN: the flat ground's), half-width (m).
    pub uc: f64, pub d: f64, pub base: f64, pub hw: f64,
    /// Line: plan origin (m), direction angle, length (m), margin along it (m).
    pub o: [f64; 2], pub ang: f64, pub len: f64, pub smar: f64,
    /// Top (m); body tone distance (m, 0: per pixel); own seed; extra half-width for glows (m); its parts.
    pub top: f64, pub td: f64, pub seed: u32, pub glow: f64, pub p0: usize, pub np: usize,
}

/// The page's instance defaults.
pub const I0: Instance = Instance { slot: 2, range: 0, view: 0, hide_c: false, hide_m: false, hide_i: false, line: false, uc: 0.0, d: 1.0,
    base: f64::NAN, hw: 0.0, o: [0.0; 2], ang: 0.0, len: 0.0, smar: 0.0, top: 10.0, td: 0.0, seed: 0, glow: 0.0, p0: 0, np: 0 };

/// One part: its type, `c` = (lateral offset of a line-frame part, s0, cv0), and `a`, `b`.
#[derive(Clone, Copy, Debug)]
pub struct Part { pub t: Pt, pub c: [f64; 3], pub a: [f64; 4], pub b: [f64; 4] }

/// Every prop of a parameter set.
#[derive(Default)]
pub struct PropSet { pub inst: Vec<Instance>, pub parts: Vec<Part> }

/// JavaScript's `Math.hypot` of two (V8's scaled Kahan sum), so lengths come out as the page's.
pub fn hypot(a: f64, b: f64) -> f64 {
    let (a, b) = (a.abs(), b.abs());
    let m = a.max(b);
    if m == 0.0 || m.is_infinite() { return m; }
    let (mut sum, mut comp) = (0.0f64, 0.0f64);
    for x in [a, b] { let n = x / m; let s = n * n - comp; let p = sum + s; comp = (p - sum) - s; sum = p; }
    sum.sqrt() * m
}
/// The page's `clamp` (`min(b, max(a, x))`: b wins when the bounds cross).
pub fn clamp(x: f64, a: f64, b: f64) -> f64 { b.min(a.max(x)) }
/// The body tone factor `f` of a prop d m away as the distance whose tone it is (the page's `townTd`).
pub fn town_td(d: f64, f: f64) -> f64 {
    let nk = clamp((d / 400.0).ln() / 12.5f64.ln(), 0.0, 1.0);
    if f == 1.0 { 0.0 } else { 400.0 * 12.5f64.powf(clamp((f * (0.6 + 0.4 * nk) - 0.6) / 0.4, 0.0, 1.0)) }
}

/// What a generator gets (the page's `ctx`): the zones, the parameters and the emitters, plus the town kit's state
/// while one of its instances is open (foot, lateral offset, the extent so far); `k` the Size (m a size unit).
pub struct Ctx<'a> { pub z: &'a Zones, pub p: &'a Params, pub set: &'a mut PropSet, kit: Option<[f64; 4]>, pub k: f64 }

impl<'a> Ctx<'a> {
    pub fn new(z: &'a Zones, p: &'a Params, set: &'a mut PropSet) -> Self { Self { z, p, set, kit: None, k: p[P::Size] } }
    pub fn rnd(&self, id: f64, st: f64, ly: f64) -> f64 { self.z.rnd(id, st, ly) }
    /// Starts an instance; the parts that follow are its own.
    pub fn add(&mut self, mut i: Instance) {
        if !i.line && i.base.is_nan() { i.base = -EYE * self.k / i.d; }
        (i.p0, i.np, self.kit) = (self.set.parts.len(), 0, None);
        self.set.inst.push(i);
    }
    pub fn last(&mut self) -> &mut Instance { self.set.inst.last_mut().unwrap() }
    /// The eye's height above the plain (m): Size times the base's.
    pub fn eye(&self) -> f64 { EYE * self.k }
    pub fn part(&mut self, t: Pt, a: [f64; 4], b: [f64; 4]) { self.part_c(t, a, b, [0.0, 0.0, 1.0]); }
    /// A part with its line-frame placement `c`; in a town-kit instance y is m above the plain and x in the caller's
    /// frame, and the part grows the instance's extent (the page's `townKit().p`).
    pub fn part_c(&mut self, t: Pt, mut a: [f64; 4], mut b: [f64; 4], c: [f64; 3]) {
        use Pt::*;
        if let Some([y0, x0, ..]) = self.kit {
            (a[0], a[1]) = (a[0] - x0, a[1] - y0);
            if matches!(t, Seg | Lat | GProf) { (a[2], a[3]) = (a[2] - x0, a[3] - y0); }
            if matches!(t, GProf | Wash) { b[0] -= y0; }
            let k = self.kit.as_mut().unwrap();
            let mut g = |x: f64, y: f64| (k[2], k[3]) = (k[2].max(x.abs()), k[3].max(y));
            match t {
                Seg | Lat => { let w = b[0].max(b[1]); g(a[0].abs() + w, a[1] + w); g(a[2].abs() + w, a[3] + w); }
                Trap => g(a[0].abs() + a[2].max(a[3]), a[1] + b[0]),
                Ell | Box => g(a[0].abs() + a[2], a[1] + a[3]),
                Light => g(a[0].abs() + 3.0 * a[2] + 1.0, a[1] + 3.0 * a[2] + 1.0),
                Beacon => g(a[0].abs() + 1.0, a[1] + 1.0),
                GProf => { g(a[0].abs(), a[1].max(a[3])); g(a[2].abs(), 0.0); }
                _ => {}
            }
        }
        self.set.parts.push(Part { t, c, a, b });
        self.last().np += 1;
    }
    /// The town kit (`townKit`): an instance on a foot (size units above the plain), centred xo size units to the side of
    /// its bearing.
    pub fn begin(&mut self, mut i: Instance, foot: f64, xo: f64) {
        if foot != 0.0 { i.base = (foot - EYE) * self.k / i.d; }
        if xo != 0.0 { i.uc = fract(i.uc + xo * self.k / (TAU * i.d)); }
        self.add(i);
        self.kit = Some([foot, xo, 0.0, 0.0]);
    }
    /// Ends a kit instance: its half-width and top are its parts' extent plus mx, my.
    pub fn end(&mut self, mx: f64, my: f64) { let [_, _, x, y] = self.kit.unwrap(); let i = self.last(); (i.hw, i.top) = (x + mx, y + my); }
    /// Builders: an upright trapezoid (foot centre x, y, half-widths w0 at the foot and w1 at the top, height h, opacity o);
    /// a box from y0 up to y1 (`exBox`); a tapered pole; an ellipse; a point light (core r, squash 1, kelvin factor kw).
    pub fn trap(&mut self, x: f64, y: f64, w0: f64, w1: f64, h: f64, o: f64) { self.part(Pt::Trap, [x, y, w0, w1], [h, 0.0, o, 0.0]); }
    pub fn bx(&mut self, x: f64, y0: f64, y1: f64, w: f64) { self.trap(x, y0, w, w, y1 - y0, 1.0); }
    pub fn seg(&mut self, a: [f64; 4], w0: f64, w1: f64, o: f64) { self.part(Pt::Seg, a, [w0, w1, o, 0.0]); }
    pub fn ell(&mut self, x: f64, y: f64, rx: f64, ry: f64) { self.part(Pt::Ell, [x, y, rx, ry], [0.0, 0.0, 1.0, 0.0]); }
    pub fn light(&mut self, x: f64, y: f64, r: f64, amp: f64, kw: f64) { self.part(Pt::Light, [x, y, r, 1.0], [0.0, amp, 0.0, kw]); }
    /// A building seen side-on: walls up to hw, a gable roof up to hr, run .3 m into the walls (`exGable`).
    pub fn gable(&mut self, x: f64, w: f64, hw: f64, hr: f64) { self.bx(x, 0.0, hw, 0.5 * w); self.trap(x, hw - 0.3, 0.5 * w * (1.0 + 0.3 / (hr - hw)), 0.0, hr - hw + 0.3, 1.0); }
    /// A red aviation light s times the city's beacons, its haze dimming at d m folded in (`exBeacon`).
    pub fn beacon(&mut self, x: f64, y: f64, s: f64, d: f64) {
        self.part(Pt::CLight, [x, y, 0.0, s * 0.07 * (1.0 - 0.6 * (1.0 - (-d / 70.0 * (0.004 + 0.0185 * self.p[P::Haze])).exp()))], [1.0, 0.08, 0.03, 0.0]);
    }
    /// The town's site (`townSite`): front row distance, row spacing, rise toward the back, rise inland (m).
    pub fn town_site(&self) -> [f64; 4] {
        let r = |k: f64| self.rnd(k, 1.0, 1600.0);
        let lev = self.z.town_x[1] != 0.0 && r(3.0) <= 0.35;
        [mix(1250.0, 1800.0, r(1.0)), mix(46.0, 64.0, r(2.0)), if lev { mix(2.0, 6.0, r(4.0)) } else { mix(18.0, 50.0, r(4.0)) },
            if lev { mix(5.0, 9.0, r(3.0)) } else { mix(10.0, 20.0, r(3.0)) }]
    }
    /// The town site's ground (m) at azimuth u, kf rows back (`townGround`).
    pub fn town_ground(&self, u: f64, kf: f64, s: &[f64; 4]) -> f64 {
        let (z, u) = (self.z, fract(u));
        let (adu, hw) = (wd(u, z.town[0]).abs(), z.town[1].max(0.006));
        let (te, li) = (smooth(hw, (hw - 0.022).max(hw * 0.45), adu), if z.town_x[1] != 0.0 { smooth(0.0, 0.012, -wd(u, z.town_x[0]) * z.town_x[1]) } else { 1.0 });
        let wob = (z.rng.pn(u * 120.0, 120.0, 1601.0) - 0.5) * 0.5 + (z.rng.pn(u * 420.0, 420.0, 1602.0) - 0.5) * 0.25;
        1.8 * smooth(hw, hw - 0.004, adu) + te * (s[3] * li * (1.0 + 0.4 * wob) + s[2] * clamp(kf / 3.0, 0.0, 1.4).powf(1.25) * mix(0.45, 1.0, li) * (1.0 + wob))
    }
}

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // One prop instance per quad: its frame, then the page's ppDraw over its parts (three texels each: c = (type, lateral
    // offset, s0, cv0), a, b), merged by max into its body coverage m and its light em; the body takes the night tone of
    // its distance. Instance fields (Rust): h0 plane (bearing, distance, foot above the flat ground rad, flags) or line
    // (0, margin, 0, flags), flags slot + 4 view + 8 hideC + 16 line + 32 hideM; h1 (first texel, part count, top m, tone
    // distance m); h2 plane (half-width) or line (origin xy, direction angle, length); h3 (distance window m, seed). The
    // instance's frame is in metres (its half-width and top come in metres too); its parts are in size units (`size` m each:
    // their frame's lengths over `size`), its haze and tone by its distance in metres.
    mod.draw.DrawSkyPart = mod.std.set_type_default() do #(DrawSkyPart::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        ..mod.draw.SkyCityDepth,
        parts: texture_2d(float)
        // parts texture width and height; 1 when the city targets are bound; 1 in the props layer's own pass
        parts_geom: uniform(vec4(1024.0, 1.0, 0.0, 0.0))
        lights: uniform(3.0)
        temp: uniform(3200.0)
        size: uniform(1.0)
        texel: fn(i: float) -> vec4 {
            let y = floor(i / self.parts_geom.x)
            return self.parts.sample_nearest(vec2((i - y * self.parts_geom.x + 0.5) / self.parts_geom.x, (y + 0.5) / self.parts_geom.y))
        }
        // body tone of an unlit prop d m away (prTone): as black as the front treeline near, the range tops' tone from 5 km;
        // hz the haze at its azimuth, day its daylight (added before the haze)
        tone: fn(d: float, hz: vec3, day: vec3) -> vec3 {
            let t = clamp(log(d / 5000.0) / log(6.0), 0.0, 1.0); let nk = clamp(log(d / 400.0) / log(12.5), 0.0, 1.0); let hz2 = self.haze * self.haze * 0.35
            let fogc = mix(hz * (0.7 + 0.25 * t), hz * (1.6 + 1.6 * t) + vec3(0.06, 0.07, 0.09) * (t * self.design_gain), hz2)
            let c = mix(self.sil * mix(0.6, 1.0, nk) + day, fogc, clamp(mix(0.012 + 0.01 * t, 0.30 * t * t + 0.06 * t + 0.02, hz2), 0.0, 1.0) * nk)
            // by day the air between, as the ranges' (`aerial`)
            return c + (hz - c) * self.aerial(d)
        }
        // range k's coverage at pass column x, elevation ang (DrawSkyRange's: rock, then its forest band and single trees)
        range_cover: fn(x: float, k: float, ang: float, px: float, tr: float) -> float {
            let r = self.col_row(x, k); var tm = r.w * max(self.cover(r.z - ang, 1.5 * px), tr)
            if ang < r.y - px { tm = r.w }
            return tm + self.cover(r.x - ang, px) * (1.0 - tm)
        }
        // a small point light's profile (prGlow): core r widened to .7 px and dimmed, a 6% halo of 3 core radii
        glow_pt: fn(q: vec2, r: float, pxm: float) -> float {
            let rr = max(r, 0.7 * pxm); let hr = max(3.0 * r, 2.5 * pxm); let q2 = dot(q, q)
            return min(1.0, r / rr) * (exp(0.0 - 2.0 * q2 / (rr * rr)) + 0.06 * exp(0.0 - q2 / (hr * hr)))
        }
        fragment: fn(){
            let vr = self.view_raw(); let h0 = self.h0; let h1 = self.h1; let h2 = self.h2; let h3 = self.h3; let fl = u32(h0.w); let line = (fl & 16u) != 0u
            var w = vr; var eye = 6.3 * self.size
            if (fl & 4u) == 0u { w = self.pix_view(vr) }
            if (fl & 3u) == 2u && self.mirror > 0.5 { eye = -6.3 * self.size }
            let e2 = vec2(cos(w.x * 6.2831853), sin(w.x * 6.2831853)); let dl = vec2(cos(h2.z), sin(h2.z)); let den = dl.x * e2.y - dl.y * e2.x
            // the haze at the pixel's azimuth; the gate on the instance's lights (`lit`: the city's lights, by day off), its beacons' (on)
            let hzc = self.haze_at(vr.x); let gate = vec2(self.lit * self.design_gain, self.design_gain)
            // the instance's own frame: s, height y, distance, the line's distance p, d(dist)/ds
            var fs = 0.0; var fy = 0.0; var fd = h0.y; var fp = h0.y; var fsf = 0.0; var cut = self.mirror > 0.5 && vr.y <= 0.0
            if line {
                fs = (e2.x * h2.y - e2.y * h2.x) / den; let x = h2.xy + fs * dl; fd = length(x); fp = abs(h2.x * dl.y - h2.y * dl.x); fsf = dot(x, dl) / fd
                fy = w.y * fd + eye; cut = cut || abs(den) < 0.000001 || dot(x, e2) <= 0.0 || fs < 0.0 - h0.y || fs > h2.w + h0.y
            } else { fs = (fract(w.x - h0.x + 0.5) - 0.5) * 6.2831853 * fd; fy = (w.y - h0.z) * fd + eye; cut = cut || abs(fs) > h2.x + 3.0 * w.z * fd }
            let px0 = w.z * fd; var m = 0.0; var em = vec3(0.0); let sz = self.size; let eyk = eye / sz
            if !(cut || fy < -2.0 * px0 || fy > h1.z + 3.0 * px0 || fd <= h3.x || fd > h3.y) {
                let fog = (1.0 - 0.6 * (1.0 - exp(0.0 - fd / 70.0 * (0.0015 + 0.0075 * self.haze)))) * self.cover(fy, px0); let hs = u32(h3.z) * 2654435761u
                var rn = 0.0; var ro = vec2(0.0); var rm = vec2(1.0, 0.0); var cn = 0.0; var ck = 0.0
                for i in 0..160 {
                    if f32(i) >= h1.y { break }
                    let pi = h1.x + 3.0 * f32(i); let c = self.texel(pi); let a = self.texel(pi + 1.0); let b = self.texel(pi + 2.0); let t = c.x
                    // the part's frame: a part on its own parallel line c.y m to the left
                    var gs = fs; var gy = fy; var gd = fd; var gp = fp; var gsf = fsf
                    if line && c.y != 0.0 {
                        let o = h2.xy + (c.y * sz) * vec2(0.0 - dl.y, dl.x); gs = (e2.x * o.y - e2.y * o.x) / den; let x = o + gs * dl
                        gd = length(x); gp = abs(o.x * dl.y - o.y * dl.x); gsf = dot(x, dl) / gd; gy = w.y * gd + eye
                    }
                    // into the parts' size units (the beacons' haze keeps the distance in metres, gr)
                    let gr = gd; gs = gs / sz; gy = gy / sz; gd = gd / sz; gp = gp / sz
                    let pxm = w.z * gd; var q = vec2((gs - c.z) * c.w, gy); let clp = cn > 0.0; let m0 = m
                    if rn > 0.0 { rn = rn - 1.0; q = q - ro; q = vec2(dot(q, rm), q.y * rm.x - q.x * rm.y) }
                    if clp { cn = cn - 1.0 }
                    if !clp || ck > 0.0 {
                        if clp { m = 0.0 }
                        // shared inputs: r from the part's origin, sc screen m per m of s, u the share of the span, y0 its parabola
                        let rp = t == 8.0 && b.w > 0.0; var r = q - a.xy; var ll = max(a.z - a.x, 0.001)
                        if t > 40.0 && t < 44.0 { r.x = gs - a.x }
                        if rp { ll = b.w }
                        var u = r.x / ll
                        if rp { u = fract(u) }
                        var lq = r; let sc = gp / gd; let g4 = 4.0 * u * (1.0 - u); let y0 = mix(a.y, a.w, u) - g4 * b.x; let inx = step(a.x, q.x) * step(q.x, a.z)
                        // the one evaluator's inputs: covers dd, factor k, lines A and B, a veil s faded by f, a point light la (see ppDraw)
                        var dd = vec3(1.0e9); var k = 0.0; var da = 0.0; var wa = 1.0e9; var db = 0.0; var wb = 1.0; var kb = 0.0; var s = 0.0; var f = 0.0; var e = 0.0
                        var la = 0.0; var lr = 0.0; var lfog = fog; var li = 0.0; var kw = b.w; var lf = 0.0; var pl = 1.0; var cc = 0.0; var hw = 0.0; var lc = 0.0
                        var lat = false; var wr = false; var col = vec3(-1.0); var o = 0.0; var bcn = false
                        if t == 0.0 || t == 3.0 {
                            let ab = a.zw - a.xy; let lg = max(length(ab), 0.000001); let d = ab / lg; let ss = dot(r, d); cc = dot(r, vec2(0.0 - d.y, d.x));
                            hw = mix(b.x, b.y, clamp(ss / lg, 0.0, 1.0))
                            if t == 0.0 { da = length(vec2(ss - clamp(ss, 0.0, lg), cc)); wa = hw; k = b.z } else {
                                lf = ss / b.z; pl = b.z; lc = b.w; lat = true; dd.y = min(ss, lg - ss) + b.w; k = 1.0
                            }
                        } else if t == 1.0 {
                            let hb = max(b.x, 0.000001); let v = clamp(r.y / hb, 0.0, 1.0); let w0 = max(mix(a.z, a.w, v) - b.y * 4.0 * v * (1.0 - v), 0.0); let ww = max(w0, 0.5 * pxm)
                            let sl = (a.w - a.z) / hb; dd.x = min((ww - abs(r.x)) / sqrt(1.0 + sl * sl), min(r.y, b.x - r.y)); k = w0 / ww * b.z
                        } else if t == 2.0 {
                            let r0 = min(a.z, a.w); let kk = max(r0, 0.5 * pxm) / max(r0, 0.000001); dd.x = (1.0 - length(r / (a.zw * kk))) * r0 * kk; k = b.z / (kk * kk)
                            if b.y > 0.5 { dd.y = r.y }
                        } else if t == 4.0 { da = length(r) - a.z; wa = b.x; k = 1.0 }
                        else if t == 5.0 { lq.y = lq.y * a.w; lr = a.z; la = b.y }
                        else if t == 6.0 { lr = -1.0; la = if b.x > 0.0 { b.x } else { 1.0 } }
                        else if t == 7.0 {
                            let rr = r + a.zw; let cl = floor(rr / b.xy); let fw = rr / b.xy - cl; let fz = vec2(pxm) / b.xy; let bw = max(min(fz, vec2(0.45)), vec2(0.004))
                            let sm = smoothstep(vec3(0.7, 0.7, 0.6), vec3(1.6, 1.6, 1.2), vec3(bw.x, bw.y, max(fz.x, fz.y)))
                            let g = mix(clamp((fw - vec2(0.25, 0.3)) / bw + 0.5, vec2(0.0), vec2(1.0)) * clamp((vec2(0.75) - fw) / bw + 0.5, vec2(0.0), vec2(1.0)),
                                vec2(0.5, 0.45), sm.xy) * step(abs(r), a.zw)
                            let hv = self.hu(hs ^ ((u32(i32(cl.x)) + 512u) * 2654435761u) ^ ((u32(i32(cl.y)) + 512u) * 2246822507u))
                            li = g.x * g.y * mix(step(f32(hv & 255u) / 255.0, b.z), b.z * 0.5, sm.z) * (0.4 + 0.6 * f32((hv >> 8u) & 255u) / 255.0) * 0.5
                        } else if t == 8.0 {
                            let sl = ((a.w - a.y - 4.0 * b.x * (1.0 - 2.0 * u)) / ll * gd - (y0 - eyk) * gsf) / gp; da = (q.y - y0) / sqrt(1.0 + sl * sl); wa = b.y; k = inx * b.z
                        } else if t == 9.0 {
                            dd.x = min(a.w - (a.w - a.y) * (1.0 - clamp(r.x / max(b.x, 0.001), 0.0, 1.0) * clamp((a.z - q.x) / max(b.y, 0.001), 0.0, 1.0)) - q.y, r.y); k = inx * b.z
                        } else if t == 10.0 {
                            let kr = floor(r.x / b.x + 0.5); lq = vec2((r.x - kr * b.x) * sc, r.y); lr = a.w; la = b.z * step(-0.5, kr) * step(kr * b.x, a.z - a.x + 0.5)
                        } else if t == 20.0 {
                            // smoke: the distance to the centreline (aw s, R (1 - exp(-s/Lp)) + .06 s), refined by two Newton steps; two octaves of puffs
                            let aw = abs(b.w); let rad = a.w; let lp = b.x; let rw = vec2(if b.w < 0.0 { 0.0 - r.x } else { r.x }, r.y); let sx = max(rw.x, 0.0) / aw
                            let s0 = 0.0 - lp * log(max(1.0 - max(rw.y, 0.0) / (rad + 0.001), 0.02)); let dx = rw.x - aw * s0; let dv = rw.y - rad * (1.0 - exp(0.0 - sx / lp)) - 0.06 * sx
                            var sp = (dx * dx * sx + dv * dv * s0) / (dv * dv + dx * dx + 0.0001); var d = vec2(0.0); var tg = vec2(0.0)
                            for it in 0..3 {
                                let ex = exp(0.0 - sp / lp); d = rw - vec2(aw * sp, rad * (1.0 - ex) + 0.06 * sp); tg = vec2(aw, rad / lp * ex + 0.06)
                                if f32(it) < 1.5 { sp = max(sp + dot(d, tg) / max(dot(tg, tg) + d.y * rad / (lp * lp) * ex, 0.0001), 0.0) }
                            }
                            let dn = length(d); let wp = a.z + 0.16 * sp
                            if dn < 1.6 * wp && sp < 600.0 {
                                let nc = vec2(sp, if tg.x * d.y - tg.y * d.x > 0.0 { dn } else { 0.0 - dn }) / (9.0 + 0.08 * sp); var n = 0.0
                                for kn in 0..8 {
                                    let kf = f32(kn); let j = floor(kf / 4.0); let ck2 = vec2(kf - 2.0 * floor(kf / 2.0), floor(kf / 2.0) - 2.0 * j)
                                    let pp = if j < 0.5 { nc + vec2(0.0, 7.0) } else { nc * 2.3 + vec2(13.0, 3.0) }; let c0 = floor(pp); var fq = pp - c0; fq = fq * fq * (3.0 - 2.0 * fq)
                                    let hv = self.hu(hs ^ ((u32(i32(c0.x + ck2.x)) + 4096u) * 2654435761u) ^ ((u32(i32(c0.y + ck2.y)) + 4096u + 8192u * u32(j)) * 2246822507u))
                                    let wq = mix(vec2(1.0) - fq, fq, ck2); n = n + f32(hv & 255u) / 255.0 * wq.x * wq.y * (if j < 0.5 { 0.62 } else { 0.38 })
                                }
                                let sm = smoothstep(vec3(380.0, 0.0, 0.0 - a.z), vec3(600.0, 1.0, a.z * 0.3),
                                    vec3(sp, exp(0.0 - dn * dn / (wp * wp)) * (0.5 + 1.1 * n) - 0.12, r.y + a.z * 0.5))
                                k = clamp(b.y * exp(0.0 - sp / abs(b.z)) * (1.0 - sm.x) * sm.y * sm.z, 0.0, 1.0)
                                // the haze it stands in, the city's glow on it (fading with the lights) and the sky's ambient by day
                                em = em + max(hzc * c.y + vec3(0.02, 0.013, 0.007) * self.glow * (0.5 + 0.53 * c.y) * (self.lit * self.design_gain) + self.ambient_rgb * 0.5 - 0.8 * self.sil, vec3(0.0)) * k
                                if b.z < 0.0 { li = k * exp(0.0 - length(r) / 22.0) * 0.2; kw = 0.45 }
                            }
                        } else if t == 21.0 {
                            let ww = a.z - a.x; let j0 = floor(r.x / ww - 0.5); let lr2 = vec2(b.x - a.x, a.z - b.x); let sl = (b.y - a.w) / max(lr2, vec2(0.001))
                            let ks = vec2(1.0) / sqrt(vec2(1.0) + sl * sl); dd.x = -1.0e9; k = b.z
                            for gi in 0..2 {
                                let x = r.x - ww * clamp(j0 + f32(gi), 0.0, max(b.w, 1.0) - 1.0); let rf = (vec2(a.w) + sl * vec2(x, ww - x) - q.y) * ks
                                dd.x = max(dd.x, min(min(min(x, ww - x), r.y), min(if lr2.x > 0.001 { rf.x } else { 1.0e9 }, if lr2.y > 0.001 { rf.y } else { 1.0e9 })))
                            }
                        } else if t == 30.0 {
                            dd.x = q.y - b.x; k = m * exp(0.0 - max(r.y, 0.0) / a.w) * (if a.z > 0.0 { exp(0.0 - r.x * r.x / (a.z * a.z)) } else { 1.0 }) * b.y; o = 1.0
                        } else if t == 31.0 {
                            let hh = max(a.zw, vec2(0.5 * pxm)); let rc = min(abs(b.x) * hh.x / a.z, min(hh.x, hh.y)) * (if b.x < 0.0 && r.y < 0.0 { 0.0 } else { 1.0 });
                            let v = abs(r) - hh + rc
                            dd.x = rc - length(max(v, vec2(0.0))) - min(max(v.x, v.y), 0.0); k = a.z * a.w / (hh.x * hh.y) * (if b.y > 0.0 { b.y } else { b.z });
                            o = if b.y > 0.0 { 1.0 } else { 0.0 }
                        } else if t == 32.0 {
                            let v = clamp(u, 0.0, 1.0); let sl = (a.w - a.y) / ll * mix(1.0, 6.0 * v * (1.0 - v), b.y)
                            dd.x = min((mix(a.y, a.w, mix(v, v * v * (3.0 - 2.0 * v), b.y)) - q.y) / sqrt(1.0 + sl * sl), q.y - b.x); k = inx * b.z
                        } else if t == 40.0 {
                            let kp = (gs - c.z) / c.w; let d0 = vec4(0.0, 1.0, 2.0, 3.0) - kp; let d1 = d0 + 4.0
                            let sl = (dot(a, sign(d0) * step(abs(d0), vec4(1.0))) + dot(b, sign(d1) * step(abs(d1), vec4(1.0)))) / (c.w * sc)
                            dd.x = (dot(a, max(vec4(1.0) - abs(d0), vec4(0.0))) + dot(b, max(vec4(1.0) - abs(d1), vec4(0.0))) - gy) / sqrt(1.0 + sl * sl); dd.y = gy;
                            k = step(0.0, kp) * step(kp, 7.0)
                        } else if t == 41.0 || t == 42.0 {
                            let pp = max(abs(c.z), 0.001); var kr = floor(r.x / pp + 0.5)
                            if t == 42.0 {
                                kr = clamp(kr, 0.0, floor(ll / pp + 0.01)); let hl = self.hu(hs ^ ((u32(i32(kr)) + 4096u) * 2654435761u) ^ asuint(c.y))
                                lq = vec2((r.x - (kr + (if c.w < 1.0 { 0.6 * f32(hl & 255u) / 255.0 - 0.3 } else { 0.0 })) * pp) * sc, q.y - y0)
                                la = step(f32(hl >> 24u) / 255.0, c.w) * (if b.w < 0.0 { 1.0 } else { b.z }); lr = if b.w < 0.0 { -1.0 } else { b.y }
                                if b.w == 0.0 { col = vec3(1.0, 0.1, 0.04) }
                            } else {
                                let y1 = mix(b.y, b.z, u) - g4 * b.w; let ps = pp * sc; k = 1.0
                                dd = vec3(min(y1 - q.y, q.y - y0), (min(r.x, a.z - gs) + (if c.z > 0.0 { 0.5 * c.z } else { c.w })) * sc, q.y)
                                if c.z > 0.0 { da = (r.x - kr * pp) * sc; wa = c.w; s = 2.0 * c.w / ps; f = smoothstep(3.0, 1.5, ps / pxm) }
                                else if c.z < 0.0 {
                                    let sl = 0.5 * (a.w - a.y + b.z - b.y - 4.0 * (1.0 - 2.0 * u) * (b.x + b.w)) / (ll * sc); let cs = 1.0 / sqrt(1.0 + sl * sl)
                                    cc = (q.y - 0.5 * (y0 + y1)) * cs; hw = 0.5 * (y1 - y0) * cs; lf = r.x / pp; pl = ps; lc = c.w; lat = true; wr = true; dd.x = 1.0e9
                                }
                            }
                        } else if t == 43.0 {
                            // a family of c.z straight lines: the three nearest found directly, an even veil where they crowd
                            let nn = c.z; let s2 = vec2(sc, 1.0); let rs = r * s2; let dz = (a.zw - a.xy) * s2; let ds = b.xy * s2; let dq = (b.zw - b.xy) * s2
                            let c0 = dz.x * rs.y - dz.y * rs.x; let c1 = dq.x * rs.y - dq.y * rs.x - dz.x * ds.y + dz.y * ds.x; let c2 = dq.y * ds.x - dq.x * ds.y
                            let qq = -0.5 * (c1 + (if c1 < 0.0 { -1.0 } else { 1.0 }) * sqrt(max(c1 * c1 - 4.0 * c2 * c0, 0.0)))
                            let ia = c0 / (if abs(qq) > 1.0e-12 { qq } else { 1.0e-12 }); let ib = if abs(c2) > 1.0e-12 { qq / c2 } else { 1.0e9 }
                            let ie = if abs(ib - 0.5 * nn + 0.5) < abs(ia - 0.5 * nn + 0.5) { ib } else { ia }; da = 1.0e9; wa = c.w; k = 1.0
                            for j in 0..3 {
                                let ii = clamp(floor(ie + 0.5) + f32(j) - 1.0, 0.0, nn - 1.0); let ri = rs - ii * ds; let di = dz + ii * dq
                                da = min(da, length(ri - di * clamp(dot(ri, di) / max(dot(di, di), 0.000001), 0.0, 1.0)))
                            }
                            let de = dz + ie * dq; let ue = dot(rs - ie * ds, de) / max(dot(de, de), 0.000001); let gap = abs(c1 + 2.0 * c2 * ie) / sqrt(max(dot(de, de), 0.000001))
                            let st = step(vec2(-0.5, 0.0), vec2(ie, ue)) * step(vec2(ie, ue), vec2(nn - 0.5, 1.0)); s = min(2.0 * c.w / max(gap, 0.0001), 1.0) * st.x * st.y; e = 1.0;
                            f = smoothstep(2.5, 1.2, gap / pxm)
                        } else if t == 50.0 {
                            // repeat the next b.w parts in the frame of the nearest of b.x cells (a row, rays, or points on an ellipse)
                            var kr = 6.2831853 / b.x; ro = a.xy; rm = vec2(1.0, 0.0); rn = b.w
                            if b.y < 0.5 { ro = ro + clamp(floor(dot(r, a.zw) / dot(a.zw, a.zw) + 0.5), 0.0, b.x - 1.0) * a.zw }
                            else {
                                kr = b.z + kr * floor((atan2(r.y, r.x / a.w) - b.z) / kr + 0.5); let cs = vec2(cos(kr), sin(kr)); ro = ro + a.z * vec2(a.w * cs.x, cs.y);
                                rm = mix(cs, rm, b.y - 1.0)
                            }
                        } else if t == 51.0 { dd.x = y0 - q.y; k = inx; o = 2.0 }
                        else if t == 60.0 { let sb = dot(r, a.zw); let cr = (r.y * a.z - r.x * a.w) / (b.x + b.y * max(sb, 0.0)); if sb > 0.0 { li = exp(0.0 - sb - cr * cr) * b.z } }
                        else if t == 61.0 { let v = a.zw - abs(r); dd.x = min(v.x, v.y); k = b.y; o = 1.0 }
                        else if t == 62.0 { lr = a.z; la = a.w; col = b.xyz; if a.z <= 0.0 { lfog = 1.0 } }
                        // lattice: chords at cc = +-hw, a web every panel pl (X bracing, or a Warren truss); a 55% strip under a few pixels
                        if lat {
                            let fr = fract(lf); let zig = hw * (2.0 * fr - 1.0) * (if wr && fract(lf * 0.5) >= 0.5 { -1.0 } else { 1.0 });
                            let kk = 1.0 / sqrt(1.0 + 4.0 * hw * hw / (pl * pl))
                            da = abs(cc) - hw; wa = lc; db = if wr { min(min(fr, 1.0 - fr) * pl, abs(cc - zig) * kk) } else { (abs(cc) - abs(zig)) * kk };
                            wb = lc * (if wr { 0.55 } else { 0.8 })
                            kb = step(abs(cc), hw + lc); s = self.cover(hw + (if wr { lc } else { 0.0 }) - abs(cc), pxm) * 0.55; f = smoothstep(0.25, 0.9, pxm / pl)
                        }
                        let lw = vec2(wa, wb); let lw2 = max(lw, vec2(0.5 * pxm)); let ln = clamp((lw2 - abs(vec2(da, db))) / pxm + 0.5, vec2(0.0), vec2(1.0)) * min(lw / lw2, vec2(1.0))
                        let cv = clamp(dd / pxm + 0.5, vec3(0.0), vec3(1.0)); let lm = max(ln.x, ln.y * kb); let gg = cv.x * cv.y * cv.z * k * mix(lm, max(lm * e, s), f)
                        if o == 0.0 { m = max(m, gg) } else if o == 1.0 { li = gg } else { ck = gg; cn = b.w }
                        if la > 0.0 { // a red aviation light: the city's tower beacons' size and strength
                            // a beacon (`Ctx::beacon`'s core radius 0, or the aviation light below) burns day and night
                            bcn = lr <= 0.0
                            if lr < 0.0 { la = la * 0.07 * (1.0 - 0.6 * (1.0 - exp(0.0 - gr / 70.0 * (0.004 + 0.0185 * self.haze)))); col = vec3(1.0, 0.08, 0.03); lfog = 1.0 }
                            // (a host's beacon a point, `beacon_core`; the example's 1.22 px)
                            if lr > 0.0 { li = la * self.glow_pt(lq, lr, pxm) }
                            else if self.point_core.x > 0.0 { li = la * self.beacon_core(lq / pxm, w.z) }
                            else { li = la * exp(0.0 - dot(lq, lq) / (3.0 * pxm * pxm)) }
                        }
                        if clp { m = max(m0, m * ck); li = li * ck }
                        if li > 0.0 { em = em + (if col.x < 0.0 { self.kelvin(self.temp * kw) } else { col }) * (li * self.lights * lfog * (if bcn { gate.y } else { gate.x })) }
                    }
                }
                // in the props layer's own pass: behind the nearer ranges (the page's prMtHide; in the scene the draw order does it)
                if (fl & 32u) != 0u && self.parts_geom.w > 0.5 && fd > 5000.0 {
                    let pp = self.frag(); let g = self.col_row(pp.x, 6.0); let ang = vr.y * g.z; let px = self.px0() * g.z; let tr = self.tree_cov.sample_nearest(pp / self.view_size)
                    var hd = 1.0 - self.range_cover(pp.x, 4.0, ang, px, tr.z)
                    if fd > 7825.42 { hd = hd * (1.0 - self.range_cover(pp.x, 3.0, ang, px, tr.y)) }
                    m = m * hd; em = em * hd
                }
                if (fl & 8u) != 0u && self.parts_geom.z > 0.5 { let hd = 1.0 - self.city_hide(fd); m = m * hd; em = em * hd } // behind a nearer city surface
            }
            // by day the body is lit by the ambient and the sun (no normal: by its height)
            let day = self.day_lit(vec3(0.18, 0.18, 0.18), 1.0, 0.35 + 0.65 * max(sin(self.sun_el), 0.0))
            // under a host's wet floor, in the scene list (not the props layer's own pass, over the sea), only its share
            // over the horizon (`under_floor`: what reaches under it stands behind the floor's water, as it stood behind
            // the ground)
            if self.host_wet > 0.0 && self.parts_geom.w < 0.5 { let k = 1.0 - self.under_floor(); m = m * k; em = em * k }
            // at the instance's distance (m) there: a plane's, or the line's point the pixel sees; its lights there too
            self.fb0 = vec4(self.tone(if h1.w > 0.0 { h1.w } else { fd }, hzc, day) * m, m); self.fb1 = vec4(em, m); self.fb2 = self.at_lit(m, fd, em, fd)
        }
    }

    // The props layer, composited in a pass of its own, pulled through over the sea band (the page's PL x wp): in front of
    // the water down to each prop's own waterline, behind the near ground rise on land.
    mod.draw.DrawSkyPartBlit = mod.std.set_type_default() do #(DrawSkyPartBlit::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        pl_c: texture_2d(float)
        pl_e: texture_2d(float)
        // the layer's distance (`SceneTarget::dist_in`)
        pl_d: texture_2d(float)
        fragment: fn(){
            let pp = self.frag(); let c = self.pl_c.sample_nearest(pp / self.view_size); let e = self.pl_e.sample_nearest(pp / self.view_size)
            let g = self.col_row(pp.x, 6.0); let ang = self.ang_raw(pp.y) * g.z; let px = self.px0() * g.z
            let wp = self.cover(g.y - ang, px) * (1.0 - self.cover(g.y - min(0.03, self.sea_edge) * (1.0 - smoothstep(0.0, 0.35, g.x)) - ang, px))
            self.fb0 = c * wp; self.fb1 = vec4(e.xyz * wp, c.w * wp); self.fb2 = self.pl_d.sample_nearest(pp / self.view_size) * wp
        }
    }
}

#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyPartBlit { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyPart { #[deref] pub draw_super: DrawQuad, #[live] pub h0: Vec4f, #[live] pub h1: Vec4f, #[live] pub h2: Vec4f, #[live] pub h3: Vec4f }
