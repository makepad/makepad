//! `blobs`: the liquid glass sheet's ground, a still composition of soft
//! coloured metaballs on a white backdrop, laid out anew from a seed.
//!
//! The layout is the metaball composer of a blob studio, ported here line
//! for line (its random stream, mulberry32, is bit-identical to the one it
//! was written against, so a seed gives the same layout), with one look baked
//! in: the "blobs" settings, two masses of three lobes, three edge blobs, two
//! corner blobs in front, two floating spheres, the first five of eight
//! colours, a white backdrop, matte shading with a soft glow inside the rim.
//! No headline, no motion: a still picture with a still grain.
//!
//! The sheet calls `mod.blobs.compose(seed)` when it is applied and hands the
//! answer to the ground's shader as uniforms: every ball's place, radius and
//! two colours, every depth layer's shadow and fade, a box round each layer
//! so the shader skips a layer where it cannot reach. Nothing is a texture:
//! a sheet cannot bind one, and the shader reads no texture at all.
//!
//! The composition is made for a 16:9 frame; the shader stretches it to the
//! window the way the composer stretches a layout to its frame (places with
//! the width, sizes with the square root of the aspect).
use makepad_math::Vec4f;
use makepad_widgets_core::makepad_platform::makepad_script::numeric::NumericValue;
use makepad_widgets_core::makepad_platform::*;

/// The frame the composition is made for (width over height).
pub const DESIGN_ASPECT: f64 = 16.0 / 9.0;
/// Ball slots the shader reads, and the layers they belong to: the back
/// layer's edge blobs (four), the middle layer's (two), the second mass's
/// three lobes, the first mass's three lobes with the floating spheres (five),
/// the two corner blobs in front.
pub const SLOTS: usize = 16;
const LAYER_SLOTS: [(f64, usize, usize); 5] = [(0.14, 0, 4), (0.38, 4, 6), (0.55, 6, 9), (0.62, 9, 14), (0.9, 14, 16)];

/// The look: the "blobs" settings. Only the seed changes.
struct Params {
    seed: u32,
    clusters: f64,
    lobes: f64,
    size: f64,
    stretch: f64,
    angle: f64,
    pos_x: f64,
    pos_y: f64,
    spacing: f64,
    arrange: f64,
    gooey: f64,
    edge_count: f64,
    edge_size: f64,
    front_count: f64,
    open_center: f64,
    spheres: f64,
    sphere_min: f64,
    sphere_max: f64,
    clearance: f64,
    spread: f64,
    depth_spread: f64,
    /// the palette, of which the first `color_count` are used
    colors: [&'static str; 8],
    color_count: usize,
    dof: f64,
    swirl: f64,
    shadow: f64,
    refl: f64,
    refl_inset: f64,
    refl_soft: f64,
    refl_tint: f64,
    grain: f64,
}

const BLOBS: Params = Params {
    seed: 785530,
    clusters: 2.0,
    lobes: 3.0,
    size: 0.185,
    stretch: 0.0,
    angle: 26.0,
    pos_x: 0.06,
    pos_y: 0.06,
    spacing: 0.34,
    arrange: 0.0,
    gooey: 1.82,
    edge_count: 3.0,
    edge_size: 0.145,
    front_count: 2.0,
    open_center: 0.0,
    spheres: 2.0,
    sphere_min: 0.059,
    sphere_max: 0.15,
    clearance: 0.105,
    spread: 1.0,
    depth_spread: 0.0,
    colors: ["#4337b4", "#b98efb", "#25c4bf", "#f6d3a6", "#f25f58", "#9a6cc4", "#5e1394", "#009dd8"],
    color_count: 5,
    dof: 0.0,
    swirl: 0.01,
    shadow: 0.23,
    refl: 1.0,
    refl_inset: 1.0,
    refl_soft: 0.28,
    refl_tint: 0.45,
    grain: 0.42,
};

/// mulberry32, bit-identical to the JavaScript one.
struct Mulberry(u32);
impl Mulberry {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6D2B79F5);
        let a = self.0;
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        ((t ^ (t >> 14)) as f64) / 4294967296.0
    }
}

fn hex_rgb(h: &str) -> [f64; 3] {
    let n = u32::from_str_radix(h.trim_start_matches('#'), 16).unwrap_or(0x808080);
    [((n >> 16) & 255) as f64 / 255.0, ((n >> 8) & 255) as f64 / 255.0, (n & 255) as f64 / 255.0]
}

fn rgb_lab(c: [f64; 3]) -> [f64; 3] {
    let (r, g, b) = (c[0].powf(2.2), c[1].powf(2.2), c[2].powf(2.2));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn lab_rgb(c: [f64; 3]) -> [f64; 3] {
    let l = c[0] + 0.3963377774 * c[1] + 0.2158037573 * c[2];
    let m = c[0] - 0.1055613458 * c[1] - 0.0638541728 * c[2];
    let s = c[0] - 0.0894841775 * c[1] - 1.2914855480 * c[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    let lin = [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ];
    [lin[0].max(0.0).powf(1.0 / 2.2), lin[1].max(0.0).powf(1.0 / 2.2), lin[2].max(0.0).powf(1.0 / 2.2)]
}

fn mixc(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

#[derive(Clone, Debug)]
struct Ball {
    x: f64,
    y: f64,
    r: f64,
    rgb: [f64; 3],
    lab: [f64; 3],
    lab2: [f64; 3],
    ang: f64,
    amp: f64,
    p1: f64,
    p2: f64,
}

#[derive(Clone, Debug, Default)]
struct Layer {
    z: f64,
    balls: Vec<Ball>,
    avg: [f64; 3],
}

fn comp_scale(aspect: f64) -> f64 {
    aspect.sqrt().clamp(0.6, 1.6)
}

struct Gen {
    r: Mulberry,
    c: Mulberry,
    pal: Vec<[f64; 3]>,
    last: Option<usize>,
    layers: Vec<Layer>,
}

impl Gen {
    fn rnd(&mut self) -> f64 {
        self.r.next()
    }

    fn add(&mut self, li: usize, x: f64, y: f64, r: f64, float: bool) {
        let n = self.pal.len();
        let mut i = (self.c.next() * n as f64).floor() as usize;
        if Some(i) == self.last && n > 1 {
            i = (i + 1) % n;
        }
        self.last = Some(i);
        let mut j = (self.c.next() * (n.max(2) - 1) as f64).floor() as usize;
        if n > 1 && j >= i {
            j += 1;
        }
        if n == 1 {
            j = 0;
        }
        let (rgb, rgb2) = (self.pal[i.min(n - 1)], self.pal[j.min(n - 1)]);
        let mix_t = 0.55 + self.c.next() * 0.45;
        let ang = self.c.next() * 6.283;
        let base = if float { 0.15 } else { 0.25 };
        // The motion's two frequencies are drawn as the composer draws them,
        // so the stream after them stays the same; the picture is still.
        let _f1 = base + self.c.next() * 0.35;
        let _f2 = base + self.c.next() * 0.35;
        let p1 = self.c.next() * 6.283;
        let p2 = self.c.next() * 6.283;
        self.layers[li].balls.push(Ball {
            x,
            y,
            r,
            rgb,
            lab: rgb_lab(rgb),
            lab2: rgb_lab(mixc(rgb, rgb2, mix_t)),
            ang,
            amp: if float { r * 0.45 + 0.008 } else { (r * 0.12).min(0.025) },
            p1,
            p2,
        });
    }
}

/// The depth layers for a frame of aspect `a`: y runs -0.5..0.5 (up), x
/// runs -a/2..a/2.
fn build(p: &Params, a: f64) -> Vec<Layer> {
    let k = comp_scale(a);
    let pal: Vec<[f64; 3]> = p.colors.iter().take(p.color_count.clamp(1, 8)).map(|c| hex_rgb(c)).collect();
    let mut g = Gen {
        r: Mulberry(p.seed),
        c: Mulberry(((p.seed as u64).wrapping_mul(2654435761) as u32) ^ 0x5bd1e995),
        pal,
        last: None,
        layers: vec![Layer { z: 0.14, ..Default::default() }, Layer { z: 0.38, ..Default::default() }],
    };
    const BACK: usize = 0;
    const MID: usize = 1;

    let clusters = p.clusters.round().clamp(0.0, 3.0) as usize;
    let lobes = p.lobes.round().clamp(1.0, 10.0) as usize;
    let ang = p.angle.to_radians();
    let dir = [ang.cos(), ang.sin()];
    let perp = [-dir[1], dir[0]];
    let base = [p.pos_x * a / 2.0, p.pos_y * 0.5];
    let anchored = p.pos_x.abs() > 0.8 || p.pos_y.abs() > 0.8;

    // masses
    for c in 0..clusters {
        let li = g.layers.len();
        g.layers.push(Layer { z: 0.62 - c as f64 * 0.07, ..Default::default() });
        let aa = p.arrange.to_radians();
        let arr = [aa.cos(), aa.sin()];
        let ext = arr[0].abs() * a + arr[1].abs();
        let tight = p.size * k * 2.4;
        let cf = clusters as f64;
        let gap = tight
            + (ext * 0.95 / (cf - 1.0).max(1.0) - tight) * p.spacing.min(1.0)
            + ext * 0.5 * (p.spacing - 1.0).max(0.0);
        let off = (c as f64 - (cf - 1.0) / 2.0) * gap;
        let cx = base[0] + arr[0] * off + (g.rnd() - 0.5) * 0.03;
        let cy = base[1] + arr[1] * off + (g.rnd() - 0.5) * 0.03;
        let r0 = p.size * k * (1.0 - c as f64 * 0.12);
        let mut pts: Vec<[f64; 3]> = vec![];
        let (mut px, mut py, mut r) = (cx, cy, r0);
        for i in 0..lobes {
            let comp = if i == 0 {
                [cx, cy, r0]
            } else {
                let a1 = g.rnd() * 6.283;
                let d = r0 * (0.45 + g.rnd() * 0.5);
                let rr = r0 * (0.42 + g.rnd() * 0.38);
                [cx + a1.cos() * d * 1.15, cy + a1.sin() * d, rr]
            };
            let t = p.stretch;
            pts.push([comp[0] + (px - comp[0]) * t, comp[1] + (py - comp[1]) * t, comp[2] + (r - comp[2]) * t]);
            let step = r * (0.6 + g.rnd() * 0.35);
            let wob = (g.rnd() - 0.5) * r * 1.3;
            px += dir[0] * step + perp[0] * wob;
            py += dir[1] * step + perp[1] * wob;
            r = (0.025 * k).max(r * (0.8 + g.rnd() * 0.14));
        }
        if !anchored && !pts.is_empty() {
            let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
            for q in &pts {
                sx += q[0] * q[2];
                sy += q[1] * q[2];
                sw += q[2];
            }
            let (dx, dy) = (cx - sx / sw, cy - sy / sw);
            for q in pts.iter_mut() {
                q[0] += dx;
                q[1] += dy;
            }
        }
        for q in &pts {
            g.add(li, q[0], q[1], q[2], false);
        }
    }
    let mass_layers: Vec<usize> = (2..g.layers.len()).collect();

    // edge blobs
    let edge_n = p.edge_count.round().max(0.0) as usize;
    for i in 0..edge_n {
        let li = if i % 2 == 1 { MID } else { BACK };
        let a1 = i as f64 / edge_n as f64 * 6.283 + g.rnd() * 0.9;
        let r = p.edge_size * k * (0.7 + g.rnd() * 0.5) * if li == MID { 0.6 } else { 1.0 };
        let mut x = a1.cos() * a * (0.36 + g.rnd() * 0.14);
        let mut y = a1.sin() * (0.36 + g.rnd() * 0.14);
        let dd = x.hypot(y);
        let need = p.open_center * k + r;
        if dd < need && dd > 1e-6 {
            x *= need / dd;
            y *= need / dd;
        }
        g.add(li, x, y, r, false);
        if g.rnd() < 0.5 {
            let a2 = g.rnd() * 6.283;
            let (x2, y2) = (x + a2.cos() * r * 0.9, y + a2.sin() * r * 0.9);
            let r2 = r * (0.5 + g.rnd() * 0.25);
            g.add(li, x2, y2, r2, false);
        }
    }

    // foreground blobs in the corners
    let front = g.layers.len();
    g.layers.push(Layer { z: 0.9, ..Default::default() });
    let mut corners = [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]];
    for i in (1..4).rev() {
        let j = (g.rnd() * (i + 1) as f64).floor() as usize;
        corners.swap(i, j);
    }
    let front_n = (p.front_count.round().max(0.0) as usize).min(4);
    for corner in corners.iter().take(front_n) {
        let x = corner[0] * (a / 2.0 + g.rnd() * 0.04);
        let y = corner[1] * (0.5 + g.rnd() * 0.04);
        let r = p.edge_size * k * (0.8 + g.rnd() * 0.5);
        g.add(front, x, y, r, false);
    }

    // floating spheres
    let solid: Vec<(f64, f64, f64)> = mass_layers
        .iter()
        .flat_map(|&li| g.layers[li].balls.iter().map(|b| (b.x, b.y, b.r)).collect::<Vec<_>>())
        .collect();
    let main_l = mass_layers.first().copied().unwrap_or(MID);
    let (smin, smax) = (p.sphere_min.min(p.sphere_max), p.sphere_min.max(p.sphere_max));
    let want = p.spheres.round().max(0.0) as usize;
    let mut placed: Vec<(f64, f64, f64)> = vec![];
    let mut tries = 0;
    while placed.len() < want && tries < want * 40 + 50 {
        tries += 1;
        let rr = (smin + g.rnd().powi(2) * (smax - smin)) * k;
        let x = (g.rnd() - 0.5) * a * p.spread;
        let y = (g.rnd() - 0.5) * p.spread;
        if solid.iter().any(|b| (b.0 - x).hypot(b.1 - y) < b.2 + rr * 0.35 + p.clearance * k) {
            continue;
        }
        if placed.iter().any(|b| (b.0 - x).hypot(b.1 - y) < (b.2 + rr) * 1.2 + 0.008) {
            continue;
        }
        if x.hypot(y) < p.open_center * k + rr {
            continue;
        }
        let roll = g.rnd();
        let li = if roll < p.depth_spread * 0.5 {
            BACK
        } else if roll < p.depth_spread {
            front
        } else {
            main_l
        };
        g.add(li, x, y, rr, true);
        placed.push((x, y, rr));
    }

    let mut layers: Vec<Layer> = g.layers.into_iter().filter(|l| !l.balls.is_empty()).collect();
    layers.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap());
    for l in layers.iter_mut() {
        let n = l.balls.len().max(1) as f64;
        for j in 0..3 {
            l.avg[j] = l.balls.iter().map(|b| b.rgb[j]).sum::<f64>() / n;
        }
    }
    layers
}

/// One ball as the shader reads it.
#[derive(Clone, Copy, Debug, Default)]
pub struct BallSlot {
    /// place x, y and radius (design frame), 0 radius for an empty slot
    pub pos: [f32; 4],
    /// colour A in OKLab, and the cosine of its gradient's angle
    pub a: [f32; 4],
    /// colour B in OKLab, and the sine of its gradient's angle
    pub b: [f32; 4],
    /// the two colours' sRGB mean (for a softened view of the ball) and its layer's fade
    pub soft: [f32; 4],
}

/// One depth layer as the shader reads it.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayerSlot {
    /// edge blur, fade toward the backdrop, shadow strength, shadow distance
    pub a: [f32; 4],
    /// the shadow's tint, and the reach of its largest ball (0 for an empty layer)
    pub b: [f32; 4],
    /// the box round its balls' centres: x min, x max, y min, y max
    pub bound: [f32; 4],
}

/// A composition, packed for the shader.
#[derive(Clone, Debug)]
pub struct Composition {
    pub seed: u32,
    pub balls: [BallSlot; SLOTS],
    pub layers: [LayerSlot; 5],
    pub reach: f32,
    pub iso: f32,
    pub swirl: f32,
    pub grain: f32,
    pub seed_s: f32,
    /// glow strength, inset, softness, tint
    pub refl: [f32; 4],
    pub bg_top: [f32; 4],
    pub bg_bottom: [f32; 4],
    /// balls the slots had no room for (never, with this preset)
    pub dropped: usize,
}

/// The composition for `seed`, made for the design frame.
pub fn compose(seed: u32) -> Composition {
    let p = Params { seed, ..BLOBS };
    let a = DESIGN_ASPECT;
    let sc = comp_scale(a);
    let layers = build(&p, a);
    let kk = p.gooey.clamp(1.3, 3.0);
    let mut out = Composition {
        seed,
        balls: [BallSlot::default(); SLOTS],
        layers: [LayerSlot::default(); 5],
        reach: kk as f32,
        iso: (1.0 - 1.0 / (kk * kk)).powi(3) as f32,
        swirl: p.swirl as f32,
        grain: (p.grain * 0.06) as f32,
        seed_s: ((seed % 997) as f64 * 0.37) as f32,
        refl: [p.refl as f32, (0.02 + p.refl_inset * 0.3) as f32, (0.015 + p.refl_soft * 0.2) as f32, p.refl_tint as f32],
        bg_top: [0.992, 0.992, 0.996, 1.0],
        bg_bottom: [0.925, 0.925, 0.945, 1.0],
        dropped: 0,
    };
    for l in &layers {
        let Some(li) = LAYER_SLOTS.iter().position(|(z, _, _)| (z - l.z).abs() < 1e-6) else {
            out.dropped += l.balls.len();
            continue;
        };
        let (_, from, to) = LAYER_SLOTS[li];
        let blur = p.dof * (l.z - 0.62).abs() * 0.06 * sc;
        let fade = (0.62 - l.z).max(0.0) * 0.55;
        let tint = mixc(l.avg, [0.42, 0.40, 0.53], 0.55);
        let (mut x0, mut x1, mut y0, mut y1, mut rmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN, 0.0f64);
        for (slot, b) in (from..to).zip(l.balls.iter()) {
            // The picture is the composer's at time zero.
            let x = b.x + b.amp * b.p1.sin();
            let y = b.y + b.amp * b.p2.cos();
            let r = b.r * (1.0 + 0.03 * (b.p1 * 1.3).sin());
            let ang = b.ang + 0.25 * b.p2.sin();
            let s = mixc(lab_rgb(b.lab), lab_rgb(b.lab2), 0.5);
            out.balls[slot] = BallSlot {
                pos: [x as f32, y as f32, r as f32, 0.0],
                a: [b.lab[0] as f32, b.lab[1] as f32, b.lab[2] as f32, ang.cos() as f32],
                b: [b.lab2[0] as f32, b.lab2[1] as f32, b.lab2[2] as f32, ang.sin() as f32],
                soft: [s[0] as f32, s[1] as f32, s[2] as f32, fade as f32],
            };
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            rmax = rmax.max(r * kk);
        }
        out.dropped += l.balls.len().saturating_sub(to - from);
        out.layers[li] = LayerSlot {
            a: [blur as f32, fade as f32, (p.shadow * 0.55) as f32, ((0.5 + l.z) * sc) as f32],
            b: [tint[0] as f32, tint[1] as f32, tint[2] as f32, rmax as f32],
            bound: [x0 as f32, x1 as f32, y0 as f32, y1 as f32],
        };
    }
    out
}

fn vec4_value(vm: &mut ScriptVm, v: [f32; 4]) -> ScriptValue {
    let parts = vm.bx.parts_mut();
    NumericValue::Vec4(Vec4f { x: v[0], y: v[1], z: v[2], w: v[3] }).to_script_value_heap(parts.heap, parts.code)
}

/// `mod.blobs.compose(seed)`: an object with the composition's uniforms,
/// `p0`..`p15`, `a0`.., `b0`.., `s0`.. (the balls), `la0`..`la4`, `lb0`..,
/// `bd0`.. (the layers), and `glob` (reach, iso, swirl, grain), `look`
/// (swirl's seed, and spare), `refl`, `bg_top`, `bg_bottom`, each a vec4.
pub fn script_mod(vm: &mut ScriptVm) {
    let blobs = vm.new_module(id!(blobs));
    vm.add_method(blobs, id!(compose), script_args!(seed = NIL), move |vm, args| {
        let seed = script_value!(vm, args.seed).as_number().unwrap_or(BLOBS.seed as f64);
        let c = compose((seed.abs() as u64 % 4_294_967_296) as u32);
        if c.dropped > 0 {
            log!("blobs: {} balls had no slot for seed {}", c.dropped, c.seed);
        }
        let mut fields: Vec<(LiveId, [f32; 4])> = Vec::new();
        for (i, b) in c.balls.iter().enumerate() {
            fields.push((LiveId::from_str(&format!("p{i}")), b.pos));
            fields.push((LiveId::from_str(&format!("a{i}")), b.a));
            fields.push((LiveId::from_str(&format!("b{i}")), b.b));
            fields.push((LiveId::from_str(&format!("s{i}")), b.soft));
        }
        for (i, l) in c.layers.iter().enumerate() {
            fields.push((LiveId::from_str(&format!("la{i}")), l.a));
            fields.push((LiveId::from_str(&format!("lb{i}")), l.b));
            fields.push((LiveId::from_str(&format!("bd{i}")), l.bound));
        }
        fields.push((id!(glob), [c.reach, c.iso, c.swirl, c.grain]));
        fields.push((id!(look), [c.seed_s, 0.0, 0.0, 0.0]));
        fields.push((id!(refl), c.refl));
        fields.push((id!(bg_top), c.bg_top));
        fields.push((id!(bg_bottom), c.bg_bottom));
        let obj = vm.heap_mut().new_object();
        for (key, v) in fields {
            let value = vec4_value(vm, v);
            let parts = vm.bx.parts_mut();
            parts.heap.set_value(obj, key.into(), value, ScriptTrap::Inner(parts.trap));
        }
        let parts = vm.bx.parts_mut();
        parts.heap.set_value(obj, id!(seed).into(), (c.seed as f64).into(), ScriptTrap::Inner(parts.trap));
        obj.into()
    });
}
