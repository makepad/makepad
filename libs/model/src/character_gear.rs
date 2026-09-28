//! Clothing and gear. Tight layers are painted onto the body surface (see
//! `body::Wardrobe`); bulky layers are shells built from the body's own
//! rings pushed outward, so they carry the same weights and bend with the
//! body; rigid pieces (pouches, plates, pads, visors) are bound to one joint.
use super::*;
use super::body::{densify, sfx, side_sign, Body, Wardrobe};
use super::head::Face;
use crate::transform::*;
use std::f64::consts::{PI, TAU};

pub const LAYER_KINDS: &[&str] = &[
    "shirt", "tshirt", "tank", "sweater", "hoodie", "jacket", "vest", "plate_carrier", "chest_rig", "armor",
    "pants", "cargo", "shorts", "suit", "gi", "overalls", "belt", "gloves", "wraps", "boots", "shoes", "sneakers",
    "helmet", "cap", "beanie", "balaclava", "headband", "headset", "goggles", "glasses", "scarf",
    "backpack", "holster", "armband", "barefoot", "shin_wraps", "gauntlets", "knee_pads", "elbow_pads", "shoulder_pads", "bracers", "greaves", "cape_collar",
];

/// Default colours for a layer from the character's palette.
pub(crate) fn default_colors(kind: &str, p: [f64; 3], s: [f64; 3], a: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    match kind {
        "pants" | "cargo" | "shorts" | "boots" | "belt" | "gloves" | "holster" | "backpack" | "knee_pads" | "elbow_pads" | "balaclava" | "beanie" => (s, a),
        "shoes" | "sneakers" | "headband" | "scarf" | "bandana" | "goggles" => (a, p),
        "glasses" => (hex("#1c1c20"), hex("#101418")),
        "wraps" => (hex("#e9e2d2"), a),
        _ => (p, a),
    }
}

pub(crate) fn tex_for(l: &Layer, d: Tex) -> Tex { l.tex.unwrap_or(d) }

pub(crate) struct HeadCover { pub balaclava: Option<[f64; 3]>, pub hair: bool, pub ears: bool, pub mouth_visible: bool }
pub(crate) fn head_cover(sp: &CharacterSpec) -> HeadCover {
    let bal = sp.layer("balaclava").map(|l| l.color);
    let full_helmet = sp.layer("helmet").is_some_and(|l| l.style == "racing" || l.style == "full");
    HeadCover {
        balaclava: bal,
        // Short hair stays under a helmet: it shows at the temples and nape.
        hair: bal.is_some() || full_helmet || sp.has("helmet") && !matches!(sp.hair.as_str(), "buzz" | "short" | "long" | "ponytail" | "bob"),
        ears: bal.is_some() || full_helmet,
        mouth_visible: bal.is_none() && !full_helmet,
    }
}

/// Resolve tight layers to materials before the body is built.
pub(crate) fn wardrobe(g: &mut Gen) -> Wardrobe {
    let sp = g.spec;
    let b = &g.b;
    let u = b.u;
    let mut w = Wardrobe { skin: g.mats.id(MatDef::new(sp.skin, Tex::Skin)), ..Default::default() };
    let top = b.nb + 0.012 * u;
    let shirt_low = b.hipj - 0.02 * u;
    let pants_top = b.hipj + 0.1 * u;
    let mut shoes: Option<(u32, u32, u32)> = None;
    for l in &sp.outfit {
        let m = |g: &mut Gen, t: Tex| g.mats.id(MatDef { color2: Some(l.color2), ..MatDef::new(l.color, tex_for(l, t)) });
        match l.kind.as_str() {
            "shirt" | "tshirt" | "sweater" | "hoodie" => {
                let t = if l.kind == "sweater" || l.kind == "hoodie" { Tex::Knit } else { Tex::Fabric };
                let id = m(g, t);
                w.torso.push((shirt_low, top, id, 0.));
                let sleeves = if l.style.is_empty() { if l.kind == "tshirt" { "short" } else { "long" } } else { l.style.as_str() };
                match sleeves { "short" => w.arm.push((-1., 0.48, id)), "none" => {}, "three_quarter" => w.arm.push((-1., 1.45, id)), _ => w.arm.push((-1., 1.96, id)) }
            }
            "tank" => { let id = m(g, Tex::Fabric); w.torso.push((shirt_low, g.b.armpit + 0.02 * u, id, 0.)); }
            "pants" | "cargo" | "overalls" => {
                let id = m(g, if l.kind == "cargo" { Tex::Nylon } else { Tex::Denim });
                w.torso.push((-1., if l.kind == "overalls" { g.b.chest_y + 0.04 * u } else { pants_top }, id, 0.));
                w.leg.push((-1., if l.style == "rolled" { 1.8 } else { 1.99 }, id));
            }
            "shorts" => { let id = m(g, Tex::Fabric); w.torso.push((-1., pants_top, id, 0.)); w.leg.push((-1., if l.style == "long" { 0.95 } else { 0.62 }, id)); }
            "suit" => {
                let id = m(g, Tex::Fabric);
                w.torso.push((-1., top + 0.02 * u, id, 0.)); w.arm.push((-1., 1.97, id)); w.leg.push((-1., 1.99, id)); w.neck = Some(id);
                // Racing livery: contrast side stripes, a chest band, a zip.
                let c2 = g.mats.id(MatDef::new(l.color2, tex_for(l, Tex::Fabric)));
                w.stripe = Some((c2, 0.16));
                w.band = Some((g.b.chest_y - 0.02 * u, g.b.chest_y + 0.035 * u, c2));
                w.zip = Some(g.mats.id(MatDef::new(hex("#202024"), Tex::Metal)));
            }
            "gi" => {
                let id = m(g, Tex::Fabric);
                w.torso.push((-1., top, id, 0.)); w.arm.push((-1., if l.style == "short" { 0.9 } else { 1.55 }, id)); w.leg.push((-1., 1.72, id));
            }
            "gloves" => { let id = m(g, Tex::Leather); w.hand = Some(id); w.arm.push((1.9, 2.2, id)); }
            "wraps" => { let id = m(g, Tex::Fabric); w.hand = Some(id); w.arm.push((1.8, 2.2, id)); }
            "boots" => {
                let id = m(g, Tex::Leather);
                let h = if l.style == "low" { 1.78 } else { 1.52 };
                w.leg.push((h, 2.2, id));
                let sole = g.mats.id(MatDef::new(hex("#1d1b1a"), Tex::Rubber));
                shoes = Some((id, sole, sole));
            }
            "shoes" | "sneakers" => {
                let id = m(g, if l.kind == "shoes" { Tex::Leather } else { Tex::Fabric });
                let sole = g.mats.id(MatDef::new(if l.kind == "sneakers" { hex("#eeeae2") } else { hex("#26201c") }, Tex::Rubber));
                let trim = g.mats.id(MatDef::new(l.color2, Tex::Plastic));
                shoes = Some((id, sole, trim));
            }
            "balaclava" => { let id = m(g, Tex::Knit); w.neck = Some(id); }
            "shin_wraps" => { let id = m(g, Tex::Fabric); w.leg.push((1.3, 1.99, id)); }
            "barefoot" => {
                let skin = w.skin;
                let sole = g.mats.id(MatDef::new(sp.skin.map(|c| c * 0.8), Tex::Skin));
                shoes = Some((skin, sole, skin));
            }
            _ => {}
        }
    }
    let (shoe, sole, trim) = shoes.unwrap_or_else(|| {
        let id = g.mats.id(MatDef::new(sp.secondary, Tex::Fabric));
        let sole = g.mats.id(MatDef::new(hex("#e8e4dc"), Tex::Rubber));
        (id, sole, sole)
    });
    w.shoe = shoe; w.sole = sole; w.shoe_trim = trim;
    w
}

/// Beard coverage for a head direction (0..1).
pub(crate) fn beard_mask(face: &Face, d: [f64; 3]) -> f64 {
    let (fx, fy) = (d[0], d[1]);
    if d[2] > 0.35 { return 0.; }
    let below_cheek = smooth01(face.eye_y - 0.3, face.eye_y - 0.45, fy);
    let lips = 1. - gauss2(fx / (face.mouth_w * 1.35), (fy - face.mouth_y) / 0.085);
    let side = 1. - smooth01(0.82, 0.95, fx.abs() + 0.25 * d[2].max(0.));
    (below_cheek * lips * side).clamp(0., 1.)
}
fn gauss2(a: f64, b: f64) -> f64 { (-(a * a + b * b)).exp() }

pub(crate) fn facial_hair(g: &mut Gen, face: &Face) {
    let sp = g.spec;
    let kind = sp.facial_hair.as_str();
    if !matches!(kind, "beard" | "full" | "goatee" | "mustache" | "moustache") { return; }
    let m = g.mats.id(MatDef { color2: sp.hair_color2, ..MatDef::new(sp.hair_color, Tex::Hair) });
    let u = g.b.u;
    let (jaw, headw) = (w1(j("jaw")), w1(j("head")));
    let mouth_world_y = face.point(face.dir(0., face.mouth_y))[1];
    let mut part = Part::new("beard");
    let a = face.r[0];
    if kind != "mustache" && kind != "moustache" {
        let (nth, nph) = (48usize, 22usize);
        let mask = |d: [f64; 3]| { let b = beard_mask(face, d); if kind == "goatee" { b * gauss2(d[0] / 0.3, 0.) } else { b } };
        let mut grid: Vec<Vec<(u32, f64)>> = Vec::new();
        for i in 0..=nph {
            let phi = mix(PI * 0.03, PI * 0.58, i as f64 / nph as f64);
            let mut row = Vec::new();
            for k in 0..=nth {
                let theta = mix(-PI * 0.62, PI * 0.62, k as f64 / nth as f64);
                let d = [phi.sin() * theta.sin(), -phi.cos(), -phi.sin() * theta.cos()];
                let c = mask(d);
                let p = face.point(d);
                let out = face.outward(d);
                let t = a * 0.045 * sp.hair_volume * c.powf(0.7) * (1. + 0.6 * smooth01(-0.6, -0.95, d[1])) + 0.0008 * u;
                let q = add(p, mul(out, t));
                let w = wmix(&headw, &jaw, smooth01(mouth_world_y, mouth_world_y - 0.03 * u, q[1]));
                row.push((part.vertex(q, w), c));
            }
            grid.push(row);
        }
        for i in 0..nph { for k in 0..nth {
            let cs = [grid[i][k].1, grid[i][k + 1].1, grid[i + 1][k + 1].1, grid[i + 1][k].1];
            if cs.iter().sum::<f64>() / 4. < 0.3 { continue; }
            let v = vec![grid[i][k].0, grid[i][k + 1].0, grid[i + 1][k + 1].0, grid[i + 1][k].0];
            let p: Vec<[f64; 3]> = v.iter().map(|&x| part.positions[x as usize]).collect();
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let mut v = v;
            if dot(n, sub(p[0], face.c)) < 0. { v.reverse(); }
            part.poly(v, vec![[k as f64 * 0.01, i as f64 * 0.01]; 4], m);
        } }
    }
    // Moustache: a tapered lock along the upper lip.
    let n = 9;
    let pts: Vec<[f64; 3]> = (0..n).map(|k| {
        let t = k as f64 / (n - 1) as f64;
        let fx = face.mouth_w * 1.25 * (2. * t - 1.);
        let fy = face.mouth_y + 0.075 - 0.05 * (2. * t - 1.).powi(2);
        let d = face.dir(fx, fy);
        add(face.point(d), mul(face.outward(d), 0.004 * u))
    }).collect();
    let rings: Vec<Ring> = (0..n).map(|k| {
        let t = k as f64 / (n - 1) as f64;
        let tan = norm(sub(pts[(k + 1).min(n - 1)], pts[k.saturating_sub(1)]));
        let r = a * 0.06 * (1. - (2. * t - 1.).powi(4)).max(0.25);
        let w = if t < 0.2 { wmix(&headw, &w1(j("mouth_l")), 0.5) } else if t > 0.8 { wmix(&headw, &w1(j("mouth_r")), 0.5) } else { headw.clone() };
        Ring::around(pts[k], tan, norm(sub(pts[k], face.c)), r * 0.55, r, w)
    }).collect();
    tube(&mut part, &rings, 8, Cap::Point(pts[0]), Cap::Point(pts[n - 1]), &|_, _| m, 1., 0., None);
    g.parts.push(part);
}

// ── shells ──────────────────────────────────────────────────────────────

/// Body torso rings (as in `body::torso`) between two heights, pushed out.
fn torso_shell_rings(b: &Body, y0: f64, y1: f64, grow: f64, boxy: f64) -> Vec<Ring> {
    let u = b.u;
    let (hw, ww, cw) = (b.hip_w, b.waist_w, b.chest_w);
    let rows: Vec<(f64, f64, f64, f64)> = vec![
        (b.crotch + 0.01 * u, hw[0] * 0.86, hw[1] * 0.85, hw[2] * 0.92),
        (b.hipj - 0.01 * u, hw[0], hw[1], hw[2]),
        (b.hipj + 0.05 * u, hw[0] * 0.97, hw[1] * 0.98, hw[2] * 0.9),
        (mix(b.hipj, b.waist_y, 0.7), mix(hw[0], ww[0], 0.75), mix(hw[1], ww[1], 0.7), mix(hw[2], ww[2], 0.8)),
        (b.waist_y, ww[0], ww[1], ww[2]),
        (mix(b.waist_y, b.chest_y, 0.5), mix(ww[0], cw[0], 0.6), mix(ww[1], cw[1], 0.55), mix(ww[2], cw[2], 0.6)),
        (b.chest_y, cw[0], cw[1], cw[2]),
        (b.armpit, cw[0] * 1.03, cw[1] * 0.96, cw[2] * 1.02),
        (b.sh - 0.06 * u, b.sh_half - 0.03 * u, cw[1] * 0.8, cw[2] * 0.86),
        (b.sh - 0.03 * u, b.sh_half - 0.045 * u, cw[1] * 0.66, cw[2] * 0.74),
        (b.sh - 0.005 * u, mix(b.sh_half - 0.05 * u, b.neck_r * 1.6, 0.45), cw[1] * 0.55, cw[2] * 0.62),
        (b.nb, b.neck_r * 1.32, b.neck_r * 1.12, b.neck_r * 1.18),
    ];
    let key: Vec<Ring> = rows.iter().map(|&(y, x, f, bk)| { let mut r = Ring::new([0., y, 0.], [1., 0., 0.], [0., 0., 1.], x + grow, 1., Vec::new()); r.rz = [bk + grow, f + grow]; r.exp = 2.3 + boxy; r }).collect();
    let dense = densify(&key, 4);
    // Clip to [y0, y1], interpolating the end rings.
    let mut out: Vec<Ring> = Vec::new();
    for i in 0..dense.len() {
        let y = dense[i].c[1];
        if y >= y0 && y <= y1 { out.push(dense[i].clone()); }
    }
    let at = |y: f64| -> Option<Ring> {
        for i in 0..dense.len() - 1 { let (a, bb) = (&dense[i], &dense[i + 1]); if y >= a.c[1] && y <= bb.c[1] { let t = (y - a.c[1]) / (bb.c[1] - a.c[1]).max(1e-9); let mut r = a.clone(); r.c = lerp3(a.c, bb.c, t); for q in 0..2 { r.rx[q] = mix(a.rx[q], bb.rx[q], t); r.rz[q] = mix(a.rz[q], bb.rz[q], t); } return Some(r); } }
        None
    };
    if let Some(r) = at(y0) { if out.first().is_none_or(|f| f.c[1] - y0 > 1e-4) { out.insert(0, r); } }
    if let Some(r) = at(y1) { if out.last().is_none_or(|l| y1 - l.c[1] > 1e-4) { out.push(r); } }
    out
}

/// Arm rings for a sleeve between chain parameters t0..t1 (0 shoulder,
/// 1 elbow, 2 wrist), grown outward.
fn arm_shell_rings(b: &Body, side: usize, t0: f64, t1: f64, grow: f64, puff: f64) -> Vec<(Ring, f64)> {
    let u = b.u; let l = b.limb;
    let [p0, p1, p2, _] = b.arm[side];
    let d1 = norm(sub(p1, p0)); let d2 = norm(sub(p2, p1));
    let (l1, l2) = (length(sub(p1, p0)), length(sub(p2, p1)));
    let keys: [(f64, f64); 11] = [(-0.16, 0.05), (-0.02, 0.062), (0.18, 0.061), (0.45, 0.054), (0.7, 0.048), (0.9, 0.041), (1.0, 0.039), (1.12, 0.045), (1.3, 0.046), (1.55, 0.041), (1.8, 0.033)];
    let keys: Vec<(f64, f64)> = keys.iter().copied().chain([(1.97, 0.029), (2.04, 0.029)]).collect();
    let radius = |t: f64| { for i in 0..keys.len() - 1 { if t <= keys[i + 1].0 { let k = ((t - keys[i].0) / (keys[i + 1].0 - keys[i].0)).clamp(0., 1.); return mix(keys[i].1, keys[i + 1].1, k); } } keys[keys.len() - 1].1 };
    let n = (((t1 - t0) / 0.1).ceil() as usize).max(2);
    (0..=n).map(|i| {
        let t = mix(t0, t1, i as f64 / n as f64);
        let (c, axis) = if t <= 1. { (add(p0, mul(d1, t * l1)), d1) } else { (add(p1, mul(d2, (t - 1.) * l2)), d2) };
        // Sleeves stand off the thin forearm: loose cloth.
        let r = radius(t) * u * l + grow + puff * u * smooth01(1.0, 1.9, t) * 0.012;
        (Ring::around(c, axis, [0., 0., 1.], r * 0.97, r, arm_weight(b, side, t)), t)
    }).collect()
}
pub(crate) fn arm_weight(b: &Body, side: usize, t: f64) -> W {
    let u = b.u;
    let x = sfx(side);
    let [p0, p1, p2, _] = b.arm[side];
    let (l1, l2) = (length(sub(p1, p0)), length(sub(p2, p1)));
    let (sh, ua, la, hd) = (w1(j(&format!("shoulder_{x}"))), w1(j(&format!("upper_arm_{x}"))), w1(j(&format!("lower_arm_{x}"))), w1(j(&format!("hand_{x}"))));
    let d_el = if t <= 1. { (t - 1.) * l1 } else { (t - 1.) * l2 };
    let mut w = wmix(&ua, &la, smooth01(-0.06 * u, 0.054 * u, d_el));
    w = wmix(&w, &hd, smooth01(-0.02 * u, 0.02 * u, (t - 2.) * l2));
    wmix(&wmix(&sh, &ua, 0.55), &w, smooth01(-0.04 * u, 0.1 * u, t * l1))
}
fn leg_shell_rings(b: &Body, side: usize, t0: f64, t1: f64, grow: f64, flare: f64) -> Vec<Ring> {
    let u = b.u; let l = b.limb;
    let x = sfx(side);
    let [hip, kn, an, _, _, _] = b.leg[side];
    let d1 = norm(sub(kn, hip)); let d2 = norm(sub(an, kn));
    let (l1, l2) = (length(sub(kn, hip)), length(sub(an, kn)));
    let keys: [(f64, f64); 12] = [(-0.2, 0.085), (-0.05, 0.098), (0.15, 0.095), (0.45, 0.083), (0.75, 0.068), (0.93, 0.059), (1.02, 0.056), (1.15, 0.062), (1.35, 0.066), (1.6, 0.054), (1.86, 0.043), (2.02, 0.04)];
    let radius = |t: f64| { for i in 0..keys.len() - 1 { if t <= keys[i + 1].0 { let k = ((t - keys[i].0) / (keys[i + 1].0 - keys[i].0)).clamp(0., 1.); return mix(keys[i].1, keys[i + 1].1, k); } } keys[keys.len() - 1].1 };
    let (hips, ul, ll, ft) = (w1(j("hips")), w1(j(&format!("upper_leg_{x}"))), w1(j(&format!("lower_leg_{x}"))), w1(j(&format!("foot_{x}"))));
    let n = (((t1 - t0) / 0.1).ceil() as usize).max(2);
    (0..=n).map(|i| {
        let t = mix(t0, t1, i as f64 / n as f64);
        let (c, axis) = if t <= 1. { (add(hip, mul(d1, t * l1)), d1) } else { (add(kn, mul(d2, (t - 1.) * l2)), d2) };
        let fold = if grow > 0.004 * u { 0.004 * u * (-((t - 1.02) / 0.05).powi(2)).exp() } else { 0. };
        let r = radius(t) * u * l * if t > 1. { 1.1 } else { 1. } + grow + flare * smooth01(1.2, 2., t) + fold;
        let d_kn = if t <= 1. { (t - 1.) * l1 } else { (t - 1.) * l2 };
        let mut w = wmix(&ul, &ll, smooth01(-0.05 * u, 0.05 * u, d_kn));
        w = wmix(&w, &ft, smooth01(-0.025 * u, 0.03 * u, (t - 2.) * l2));
        w = wmix(&wmix(&hips, &ul, 0.5), &w, smooth01(-0.06 * u, 0.1 * u, t * l1));
        Ring::around(c, axis, [1., 0., 0.], r, r, w)
    }).collect()
}

/// A shell tube with a turned-in hem at each open end so edges read thick.
fn shell(part: &mut Part, rings: &[Ring], segs: usize, m: u32, hem: u32, hem_depth: f64, wf: Option<&dyn Fn([f64; 3], usize) -> W>, skip: Option<&dyn Fn(f64) -> bool>) {
    if rings.len() < 2 { return; }
    let n = rings.len();
    let mut all = Vec::new();
    // Bottom hem: a smaller ring just inside.
    let inset = |r: &Ring, toward: [f64; 3]| { let mut q = r.scaled(1., -hem_depth * 0.6); q.c = add(q.c, mul(toward, hem_depth)); q };
    let d0 = norm(sub(rings[1].c, rings[0].c)); let d1 = norm(sub(rings[n - 1].c, rings[n - 2].c));
    all.push(inset(&rings[0], d0));
    all.extend_from_slice(rings);
    all.push(inset(&rings[n - 1], mul(d1, -1.)));
    let last = all.len() - 2;
    let mat = |i: usize, theta: f64| { if skip.is_some_and(|s| s(theta)) { return u32::MAX; } if i == 0 || i >= last { hem } else { m } };
    let before = part.polys.len();
    tube(part, &all, segs, Cap::Open, Cap::Open, &mat, 1., 0., wf);
    // Faces marked u32::MAX are openings (a jacket's open front).
    let mut k = before;
    while k < part.polys.len() { if part.polys[k].material == u32::MAX { part.polys.swap_remove(k); } else { k += 1; } }
}

fn front_angle(theta: f64) -> f64 { let mut d = (theta - 1.5 * PI).rem_euclid(TAU); if d > PI { d -= TAU; } d }

pub(crate) fn build(g: &mut Gen) {
    let sp = g.spec;
    let u = g.b.u;
    let face = Face::new(sp, &g.b);
    let mut part = Part::new("gear");
    let mut hard = Part::new("gear_hard").hard(40.);
    let outfit = sp.outfit.clone();
    // The shells read the body while materials and sockets are added to `g`.
    let body = g.b.clone_layout();
    let b: &Body = &body;
    for l in &outfit {
        let mat = |g: &mut Gen, t: Tex| g.mats.id(MatDef { color2: Some(l.color2), ..MatDef::new(l.color, tex_for(l, t)) });
        match l.kind.as_str() {
            "jacket" => {
                let m = mat(g, if l.style == "leather" { Tex::Leather } else { Tex::Fabric });
                let lining = g.mats.id(MatDef::new(l.color2, Tex::Fabric));
                let grow = 0.014 * u;
                let rings = torso_shell_rings(b, b.hipj - 0.04 * u, b.nb + 0.005 * u, grow, 0.);
                let bb = b;
                let wf = move |p: [f64; 3], _: usize| torso_weight_pub(bb, p);
                let open = l.open;
                let skip = move |theta: f64| open && front_angle(theta).abs() < 0.28;
                shell(&mut part, &rings, 32, m, lining, 0.012 * u, Some(&wf), Some(&skip));
                // Collar: a standing band that flares out.
                let top = rings[rings.len() - 1].clone();
                let collar: Vec<Ring> = (0..4).map(|i| { let t = i as f64 / 3.; let mut r = top.scaled(1. + 0.12 * t, 0.004 * u); r.c = add(r.c, [0., 0.03 * u * t, 0.006 * u * t]); r.w = wmix(&w1(j("chest")), &w1(j("neck")), t * 0.7); r }).collect();
                shell(&mut part, &collar, 32, m, lining, 0.006 * u, None, Some(&skip));
                for side in 0..2 {
                    let rs: Vec<Ring> = arm_shell_rings(b, side, -0.16, 1.93, 0.011 * u, 1.).into_iter().map(|(r, _)| r).collect();
                    shell(&mut part, &rs, 20, m, lining, 0.01 * u, None, None);
                    // Cuff.
                    let cuff: Vec<Ring> = arm_shell_rings(b, side, 1.84, 1.95, 0.016 * u, 1.).into_iter().map(|(r, _)| r).collect();
                    shell(&mut part, &cuff, 20, lining, lining, 0.004 * u, None, None);
                }
            }
            "hoodie" => {
                // A bunched hood behind the neck.
                let m = mat(g, Tex::Knit);
                let c = [0., b.nb + 0.02 * u, 0.05 * u];
                let rings: Vec<Ring> = (0..7).map(|i| {
                    let t = i as f64 / 6.;
                    let a = mix(-1.1, 1.1, t);
                    let cc = add(c, [a.sin() * b.neck_r * 1.7, 0.01 * u * (1. - (a * 1.2).cos()), a.cos() * b.neck_r * 1.4]);
                    let tan = [a.cos(), 0., -a.sin()];
                    let r = 0.035 * u * (1. - 0.6 * (2. * t - 1.).powi(2));
                    Ring::around(cc, tan, [0., 1., 0.], r * 1.3, r, wmix(&w1(j("chest")), &w1(j("neck")), 0.4))
                }).collect();
                let (s0, e) = (rings[0].c, rings[6].c);
                tube(&mut part, &rings, 14, Cap::Point(s0), Cap::Point(e), &|_, _| m, 1., 0., None);
            }
            "vest" | "plate_carrier" | "armor" => {
                let (m, t) = if l.kind == "armor" { (mat(g, Tex::Metal), 0.03) } else { (mat(g, Tex::Nylon), if l.kind == "vest" { 0.018 } else { 0.034 }) };
                let dark = g.mats.id(MatDef::new(l.color.map(|c| c * 0.55), Tex::Nylon));
                let y0 = if l.kind == "vest" { b.hipj + 0.02 * u } else { b.waist_y - 0.03 * u };
                let rings = torso_shell_rings(b, y0, b.armpit + 0.03 * u, t * u, if l.kind == "vest" { 0.3 } else { 1.2 });
                let bb = b;
                let wf = move |p: [f64; 3], _: usize| torso_weight_pub(bb, p);
                shell(&mut part, &rings, 32, m, dark, 0.012 * u, Some(&wf), None);
                // Shoulder straps over the trapezius.
                for side in 0..2 {
                    let sg = side_sign(side);
                    let x = sg * (b.neck_r * 1.35 + 0.035 * u);
                    let top = b.sh + 0.012 * u;
                    let pts = [[x, b.armpit + 0.02 * u, -b.chest_w[1] - t * u * 0.7], [x, top - 0.02 * u, -b.chest_w[1] * 0.55], [x, top + 0.006 * u, 0.], [x, top - 0.02 * u, b.chest_w[2] * 0.6], [x, b.armpit + 0.02 * u, b.chest_w[2] + t * u * 0.7]];
                    let rings: Vec<Ring> = pts.iter().enumerate().map(|(i, &p)| { let tan = norm(sub(pts[(i + 1).min(4)], pts[i.saturating_sub(1)])); Ring::around(p, tan, [1., 0., 0.], 0.03 * u, 0.008 * u, wmix(&w1(j("chest")), &w1(j(&format!("shoulder_{}", sfx(side)))), 0.3 * (i == 2) as i32 as f64)) }).collect();
                    tube(&mut hard, &rings, 8, Cap::Point(pts[0]), Cap::Point(pts[4]), &|_, _| dark, 1., 0., None);
                }
                if l.kind == "plate_carrier" {
                    // Three rifle-magazine pouches across the front, and a radio pouch.
                    let front_z = -(b.chest_w[1] + t * u) - 0.012 * u;
                    for k in 0..3 {
                        let x = (k as f64 - 1.) * 0.07 * u;
                        rounded_box(&mut hard, [x, b.waist_y + 0.07 * u, front_z - 0.012 * u], [0.03 * u, 0.055 * u, 0.02 * u], ID3, 4., w1(j("spine")), m, 16);
                        rounded_box(&mut hard, [x, b.waist_y + 0.125 * u, front_z - 0.014 * u], [0.032 * u, 0.012 * u, 0.022 * u], ID3, 5., w1(j("spine")), dark, 12);
                    }
                    // Magazine bodies standing out of the open pouch tops.
                    let mag = g.mats.id(MatDef::new(hex("#2b2a27"), Tex::Plastic));
                    for k in 0..3 {
                        let x = (k as f64 - 1.) * 0.07 * u;
                        rounded_box(&mut hard, [x, b.waist_y + 0.145 * u, front_z - 0.014 * u], [0.022 * u, 0.02 * u, 0.012 * u], rot_x(8.), 6., w1(j("spine")), mag, 10);
                    }
                    // Radio with a stubby antenna on the chest, flashlight on the shoulder strap.
                    let radio = [0.13 * u, b.chest_y + 0.02 * u, front_z - 0.004 * u];
                    rounded_box(&mut hard, radio, [0.022 * u, 0.045 * u, 0.018 * u], ID3, 4., w1(j("chest")), dark, 12);
                    let ant: Vec<Ring> = (0..4).map(|i| Ring::around(add(radio, [0.008 * u, 0.045 * u + 0.03 * u * i as f64, 0.]), [0., 1., 0.], [1., 0., 0.], 0.004 * u * (1. - 0.2 * i as f64), 0.004 * u * (1. - 0.2 * i as f64), w1(j("chest")))).collect();
                    let top = ant[3].c;
                    tube(&mut hard, &ant, 6, Cap::Open, Cap::Point(add(top, [0., 0.004 * u, 0.])), &|_, _| mag, 1., 0., None);
                    let lamp = [-0.13 * u, b.chest_y + 0.07 * u, front_z + 0.006 * u];
                    let lamp_rings: Vec<Ring> = (0..3).map(|i| Ring::around(add(lamp, [0., 0., -0.02 * u * i as f64]), [0., 0., -1.], [1., 0., 0.], 0.012 * u, 0.012 * u, w1(j("chest")))).collect();
                    let lens_c = lamp_rings[2].c;
                    tube(&mut hard, &lamp_rings, 10, Cap::Point(lamp), Cap::Point(lens_c), &|_, _| mag, 1., 0., None);
                    // Name tape / patch in the accent colour.
                    let patch = g.mats.id(MatDef::new(l.color2, Tex::Fabric));
                    rounded_box(&mut hard, [-0.08 * u, b.chest_y + 0.035 * u, front_z + 0.006 * u], [0.035 * u, 0.018 * u, 0.004 * u], ID3, 6., w1(j("chest")), patch, 10);
                }
                if l.kind == "armor" {
                    let trim = g.mats.id(MatDef::new(l.color2, Tex::Metal));
                    rounded_box(&mut hard, [0., b.chest_y + 0.02 * u, -(b.chest_w[1] + t * u) - 0.004 * u], [b.chest_w[0] * 0.75, 0.1 * u, 0.02 * u], ID3, 3., w1(j("chest")), trim, 18);
                }
            }
            "chest_rig" => {
                let m = mat(g, Tex::Nylon);
                let dark = g.mats.id(MatDef::new(l.color.map(|c| c * 0.6), Tex::Nylon));
                let fz = -b.chest_w[1] - 0.01 * u;
                // Panel with four pouches, straps over the shoulders.
                rounded_box(&mut hard, [0., b.chest_y - 0.02 * u, fz - 0.01 * u], [b.chest_w[0] * 0.95, 0.06 * u, 0.018 * u], ID3, 4., w1(j("chest")), m, 16);
                for k in 0..4 {
                    let x = (k as f64 - 1.5) * 0.058 * u;
                    rounded_box(&mut hard, [x, b.chest_y - 0.035 * u, fz - 0.035 * u], [0.025 * u, 0.05 * u, 0.017 * u], ID3, 4., w1(j("chest")), m, 12);
                    rounded_box(&mut hard, [x, b.chest_y + 0.012 * u, fz - 0.037 * u], [0.027 * u, 0.01 * u, 0.019 * u], ID3, 5., w1(j("chest")), dark, 10);
                }
                for side in 0..2 {
                    let sg = side_sign(side);
                    let x = sg * (b.neck_r * 1.5 + 0.03 * u);
                    let top = b.sh + 0.014 * u;
                    let pts = [[sg * b.chest_w[0] * 0.75, b.chest_y + 0.03 * u, fz - 0.005 * u], [x, top - 0.02 * u, -b.chest_w[1] * 0.6], [x, top, 0.], [x, top - 0.03 * u, b.chest_w[2] * 0.7], [-sg * 0.02 * u, b.chest_y, b.chest_w[2] + 0.012 * u]];
                    let rings: Vec<Ring> = pts.iter().enumerate().map(|(i, &p)| { let tan = norm(sub(pts[(i + 1).min(4)], pts[i.saturating_sub(1)])); Ring::around(p, tan, [1., 0., 0.], 0.022 * u, 0.006 * u, w1(j("chest"))) }).collect();
                    tube(&mut hard, &rings, 8, Cap::Point(pts[0]), Cap::Point(pts[4]), &|_, _| dark, 1., 0., None);
                }
            }
            "gi" => {
                // Loose jacket body, wide sleeves ending mid-forearm, loose trousers.
                let cloth = mat(g, Tex::Fabric);
                let rings = torso_shell_rings(b, b.hipj - 0.06 * u, b.nb + 0.004 * u, 0.018 * u, 0.);
                let bb = b;
                let wf = move |p: [f64; 3], _: usize| torso_weight_pub(bb, p);
                shell(&mut part, &rings, 32, cloth, cloth, 0.008 * u, Some(&wf), None);
                for side in 0..2 {
                    // Wide sleeves: the opening flares to about twice the forearm.
                    let end = if l.style == "short" { 0.9 } else { 1.5 };
                    let sleeve: Vec<Ring> = arm_shell_rings(b, side, -0.16, end, 0.016 * u, 0.).into_iter().map(|(r, t)| r.scaled(1. + 0.9 * smooth01(0.6, end, t), 0.)).collect();
                    shell(&mut part, &sleeve, 18, cloth, cloth, 0.008 * u, None, None);
                    let legs = leg_shell_rings(b, side, -0.02, 1.72, 0.02 * u, 0.028 * u);
                    shell(&mut part, &legs, 20, cloth, cloth, 0.008 * u, None, None);
                }
                // Crossed lapels over a white inner gi, and a knotted belt.
                let belt = g.mats.id(MatDef::new(l.color2, Tex::Fabric));
                let lapel = mat(g, Tex::Fabric);
                let inner = g.mats.id(MatDef::new(hex("#f2efe6"), Tex::Fabric));
                let v_top = b.nb + 0.005 * u; let v_bot = mix(b.chest_y, b.waist_y, 0.3);
                let vz = |y: f64| -mix(b.neck_r * 1.05, b.chest_w[1] + 0.022 * u, smooth01(v_top, v_top - 0.12 * u, y)) - 0.002 * u;
                let vrings: Vec<Ring> = (0..6).map(|i| { let t = i as f64 / 5.; let y = mix(v_top, v_bot, t); let c = [0., y, vz(y)]; let bb = b; Ring::around(c, [0., -1., 0.], [1., 0., 0.], (b.neck_r * 1.1) * (1. - t) + 0.004 * u, 0.004 * u, torso_weight_pub(bb, c)) }).collect();
                let (vt, vb) = (vrings[0].c, vrings[5].c);
                tube(&mut part, &vrings, 8, Cap::Point(vt), Cap::Point(vb), &|_, _| inner, 1., 0., None);
                for side in 0..2 {
                    let sg = side_sign(side);
                    let pts: Vec<[f64; 3]> = (0..6).map(|i| { let t = i as f64 / 5.; [sg * mix(b.neck_r * 1.1, -0.02 * u, t) , mix(b.nb + 0.01 * u, b.waist_y, t), -mix(b.neck_r * 1.1, b.chest_w[1] + 0.024 * u, (t * 2.).min(1.)) - 0.004 * u] }).collect();
                    let rings: Vec<Ring> = pts.iter().enumerate().map(|(i, &p)| { let tan = norm(sub(pts[(i + 1).min(5)], pts[i.saturating_sub(1)])); let bb = b; Ring::around(p, tan, [0., 0., -1.], 0.042 * u, 0.008 * u, torso_weight_pub(bb, p)) }).collect();
                    tube(&mut part, &rings, 8, Cap::Point(pts[0]), Cap::Point(pts[5]), &|_, _| lapel, 1., 0., None);
                }
                belt_ring(&mut part, b, b.waist_y - 0.01 * u, 0.04 * u, 0.03 * u, belt);
                for side in 0..2 {
                    let sg = side_sign(side);
                    let p = [sg * 0.03 * u, b.waist_y - 0.06 * u, -b.waist_w[1] - 0.02 * u];
                    let tail = if l.style == "long" { 0.17 } else { 0.07 } * u;
                    rounded_box(&mut part, add(p, [sg * 0.01 * u, 0.07 * u - tail, 0.]), [0.018 * u, tail, 0.006 * u], rot_mul(rot_y(sg * 10.), ID3), 5., w1(j("hips")), belt, 10);
                }
                rounded_box(&mut part, [0., b.waist_y - 0.01 * u, -b.waist_w[1] - 0.022 * u], [0.028 * u, 0.024 * u, 0.012 * u], ID3, 3., w1(j("hips")), belt, 10);
            }
            "belt" => {
                let m = mat(g, Tex::Leather);
                let buckle = g.mats.id(MatDef::new(hex("#b9b3a6"), Tex::Metal));
                let y = b.hipj + 0.085 * u;
                belt_ring(&mut part, b, y, 0.042 * u, 0.008 * u, m);
                let z = -torso_depth_front(b, y) - 0.014 * u;
                rounded_box(&mut hard, [0., y, z], [0.03 * u, 0.024 * u, 0.006 * u], ID3, 4., w1(j("hips")), buckle, 12);
            }
            "boots" => {
                let m = mat(g, Tex::Leather);
                let top = if l.style == "low" { 1.78 } else { 1.52 };
                for side in 0..2 {
                    let rings = leg_shell_rings(b, side, top, 2.0, 0.008 * u, 0.006 * u);
                    shell(&mut part, &rings, 22, m, m, 0.006 * u, None, None);
                    let rim = leg_shell_rings(b, side, top, top + 0.05, 0.013 * u, 0.);
                    shell(&mut part, &rim, 22, m, m, 0.004 * u, None, None);
                    // Laces: crossed bars up the boot front, over a tongue strip.
                    let lace = g.mats.id(MatDef::new(hex("#1a1714"), Tex::Fabric));
                    let x = sfx(side);
                    let lr = leg_shell_rings(b, side, top + 0.02, 1.98, 0.016 * u, 0.006 * u);
                    for (k, r) in lr.iter().enumerate().skip(1) {
                        let front = r.point(1.5 * std::f64::consts::PI);
                        let wv = r.w.clone();
                        let _ = k;
                        rounded_box(&mut hard, front, [0.018 * u, 0.0035 * u, 0.004 * u], rot_y(if k % 2 == 0 { 12. } else { -12. }), 5., if wv.is_empty() { w1(j(&format!("lower_leg_{x}"))) } else { wv }, lace, 8);
                    }
                    // A chunkier boot over the shoe.
                    let fr = body::foot_rings(b, side, 0.007 * u);
                    let dir = norm(sub(fr[fr.len() - 1].c, fr[0].c));
                    let sole = g.mats.id(MatDef::new(hex("#1d1b1a"), Tex::Rubber));
                    let n = fr.len();
                    let matf = move |i: usize, theta: f64| { let _ = i; if theta.sin() > 0.55 { sole } else { m } };
                    let (s0, e) = (fr[0].c, fr[n - 1].c);
                    tube(&mut part, &fr, 28, Cap::Point(sub(s0, mul(dir, 0.014 * u))), Cap::Point(add(e, mul(dir, 0.012 * u))), &matf, 1., 0., None);
                }
            }
            "gloves" | "wraps" => {
                let m = mat(g, if l.kind == "wraps" { Tex::Fabric } else { Tex::Leather });
                for side in 0..2 {
                    let rs: Vec<Ring> = arm_shell_rings(b, side, 1.9, 2.02, if l.kind == "wraps" { 0.004 * u } else { 0.007 * u }, 0.).into_iter().map(|(r, _)| r).collect();
                    shell(&mut part, &rs, 16, m, m, 0.004 * u, None, None);
                }
            }
            "suit" => {
                // Firesuit: standing collar and shoulder epaulettes.
                let c2 = g.mats.id(MatDef::new(l.color2, tex_for(l, Tex::Fabric)));
                let m = mat(g, Tex::Fabric);
                let y = b.nb + 0.02 * u;
                let rings: Vec<Ring> = (0..3).map(|i| Ring::new([0., y + 0.018 * u * i as f64, 0.006 * u], [1., 0., 0.], [0., 0., 1.], b.neck_r * 1.32 + 0.004 * u, b.neck_r * 1.3 + 0.004 * u, wmix(&w1(j("chest")), &w1(j("neck")), 0.6))).collect();
                shell(&mut part, &rings, 28, m, c2, 0.006 * u, None, None);
                for side in 0..2 {
                    let sg = side_sign(side);
                    let x = sfx(side);
                    let p = [sg * (b.sh_half - 0.07 * u), b.sh + 0.004 * u, 0.004 * u];
                    rounded_box(&mut hard, p, [0.045 * u, 0.006 * u, 0.028 * u], rot_mul(ID3, [[1., 0., 0.], [0., 0.97, sg * -0.24], [0., sg * 0.24, 0.97]]), 5., wmix(&w1(j("chest")), &w1(j(&format!("shoulder_{x}"))), 0.6), c2, 10);
                }
            }
            "shirt" | "sweater" if l.style != "short" && l.style != "none" && !sp.has("suit") => {
                // Loose sleeves and a body that stands off the torso.
                let m = mat(g, if l.kind == "shirt" { Tex::Fabric } else { Tex::Knit });
                let grow = if l.kind == "shirt" { 0.007 } else { 0.012 } * u;
                let rings = torso_shell_rings(b, b.hipj + 0.02 * u, b.nb + 0.004 * u, grow, 0.);
                let bb = b;
                let wf = move |p: [f64; 3], _: usize| torso_weight_pub(bb, p);
                shell(&mut part, &rings, 32, m, m, 0.006 * u, Some(&wf), None);
                for side in 0..2 {
                    let rs: Vec<Ring> = arm_shell_rings(b, side, -0.16, 1.93, grow * 0.9, 0.6).into_iter().map(|(r, _)| r).collect();
                    shell(&mut part, &rs, 16, m, m, 0.005 * u, None, None);
                }
            }
            "pants" | "cargo" => {
                let m = mat(g, if l.kind == "cargo" { Tex::Nylon } else { Tex::Denim });
                let hem = if l.style == "rolled" { 1.8 } else { 1.97 };
                let bottom = if sp.has("boots") { sp.layer("boots").map_or(1.5, |b| if b.style == "low" { 1.76 } else { 1.52 }) + 0.04 } else { hem };
                for side in 0..2 {
                    let x = sfx(side);
                    let rs = leg_shell_rings(b, side, -0.02, bottom, if l.kind == "cargo" { 0.013 } else { 0.007 } * u, if l.kind == "cargo" { 0.008 } else { 0.004 } * u);
                    shell(&mut part, &rs, 20, m, m, 0.006 * u, None, None);
                    // A thigh cargo pocket.
                    let [hip, kn, _, _, _, _] = b.leg[side];
                    let sg = side_sign(side);
                    if l.kind == "cargo" {
                        let c = add(lerp3(hip, kn, 0.55), [sg * 0.085 * u * b.limb, 0., -0.01 * u]);
                        rounded_box(&mut hard, c, [0.016 * u, 0.06 * u, 0.05 * u], ID3, 4., w1(j(&format!("upper_leg_{x}"))), m, 12);
                    }
                }
            }
            "armband" => {
                // High-value team band round both upper arms (reads at 40 m).
                let m = g.mats.id(MatDef::new(l.color, Tex::Plain).rough(0.7));
                for side in 0..2 {
                    let rs: Vec<Ring> = arm_shell_rings(b, side, 0.28, 0.48, 0.016 * u, 0.).into_iter().map(|(r, _)| r).collect();
                    shell(&mut part, &rs, 16, m, m, 0.004 * u, None, None);
                }
            }
            "shin_wraps" => {
                let m = mat(g, Tex::Fabric);
                for side in 0..2 {
                    let rs = leg_shell_rings(b, side, 1.3, 1.99, 0.004 * u, 0.);
                    shell(&mut part, &rs, 18, m, m, 0.003 * u, None, None);
                }
            }
            "gauntlets" => {
                let m = mat(g, tex_for(l, Tex::Metal));
                let trim = g.mats.id(MatDef::new(l.color2, Tex::Metal));
                let glow = g.mats.id(MatDef::new(l.color2, Tex::Emissive));
                for side in 0..2 {
                    let x = sfx(side);
                    let [_, el, wr, kn] = b.arm[side];
                    let [d, n, t] = b.hand_frame[side];
                    let frame = [t, mul(n, -1.), d];
                    let len = length(sub(wr, el));
                    let fc = add(lerp3(el, wr, 0.6), mul(n, 0.004 * u));
                    let fr = [norm(cross(d, n)), n, d];
                    let _ = frame;
                    // An oversized forearm housing with a cuff ring, a glowing
                    // core on the back of the forearm and a knuckle block.
                    let la = w1(j(&format!("lower_arm_{x}")));
                    rounded_box(&mut hard, fc, [0.085 * u * b.limb, 0.08 * u * b.limb, len * 0.5], fr, 3.4, la.clone(), m, 20);
                    rounded_box(&mut hard, add(fc, mul(d, len * 0.47)), [0.095 * u * b.limb, 0.09 * u * b.limb, 0.02 * u], fr, 4., la.clone(), trim, 16);
                    rounded_box(&mut hard, add(fc, mul(n, -0.075 * u * b.limb)), [0.028 * u, 0.012 * u, len * 0.3], fr, 4., la, glow, 10);
                    rounded_box(&mut hard, lerp3(wr, kn, 0.9), [0.07 * u, 0.05 * u, 0.06 * u], fr, 3.5, w1(j(&format!("hand_{x}"))), m, 16);
                    rounded_box(&mut hard, add(lerp3(wr, kn, 1.25), mul(n, -0.02 * u)), [0.066 * u, 0.02 * u, 0.02 * u], fr, 4., w1(j(&format!("hand_{x}"))), trim, 10);
                }
            }
            "knee_pads" => {
                let m = mat(g, Tex::Plastic);
                for side in 0..2 {
                    let x = sfx(side);
                    let [_, kn, _, _, _, _] = b.leg[side];
                    rounded_box(&mut hard, add(kn, [0., -0.012 * u, -0.06 * u * b.limb]), [0.05 * u, 0.058 * u, 0.022 * u], rot_x(-6.), 3., w1(j(&format!("lower_leg_{x}"))), m, 16);
                }
            }
            "elbow_pads" => {
                let m = mat(g, Tex::Plastic);
                for side in 0..2 {
                    let x = sfx(side);
                    let [_, el, _, _] = b.arm[side];
                    rounded_box(&mut hard, add(el, [0., 0., 0.045 * u * b.limb]), [0.04 * u, 0.045 * u, 0.016 * u], ID3, 3., w1(j(&format!("lower_arm_{x}"))), m, 12);
                }
            }
            "shoulder_pads" => {
                let m = mat(g, tex_for(l, Tex::Metal));
                for side in 0..2 {
                    let x = sfx(side);
                    let sg = side_sign(side);
                    let [p0, _, _, _] = b.arm[side];
                    let c = add(p0, [sg * 0.02 * u, 0.035 * u, 0.]);
                    let r = 0.085 * u * b.limb;
                    let f = move |q: [f64; 3]| add(c, [q[0] * r * 1.05, q[1].max(-0.2) * r * 0.7, q[2] * r * 1.1]);
                    blob(&mut hard, c, [0., 1., 0.], [1., 0., 0.], 18, 10, &f, &|_| wmix(&w1(j(&format!("shoulder_{x}"))), &w1(j(&format!("upper_arm_{x}"))), 0.6), &|_, _| m, 1.);
                }
            }
            "bracers" | "greaves" => {
                let m = mat(g, tex_for(l, Tex::Leather));
                for side in 0..2 {
                    let top = if sp.has("boots") { 1.48 } else { 1.82 };
                    let rs: Vec<Ring> = if l.kind == "bracers" { arm_shell_rings(b, side, 1.35, 1.9, 0.01 * u, 0.).into_iter().map(|(r, _)| r).collect() } else { leg_shell_rings(b, side, 1.1, top, 0.012 * u, 0.) };
                    shell(&mut part, &rs, 18, m, m, 0.006 * u, None, None);
                }
            }
            "holster" => {
                let m = mat(g, Tex::Nylon);
                let dark = g.mats.id(MatDef::new(hex("#18181a"), Tex::Plastic));
                let [hip, kn, _, _, _, _] = b.leg[1];
                let c = add(lerp3(hip, kn, 0.3), [0.075 * u * b.limb, 0., 0.01 * u]);
                rounded_box(&mut hard, c, [0.018 * u, 0.075 * u, 0.04 * u], ID3, 4., w1(j("upper_leg_r")), m, 14);
                rounded_box(&mut hard, add(c, [0.004 * u, 0.09 * u, -0.012 * u]), [0.014 * u, 0.03 * u, 0.018 * u], rot_x(-15.), 4., w1(j("upper_leg_r")), dark, 10);
                g.sockets.push(("hip.r".into(), j("upper_leg_r"), c, [0., 0., 0., 1.]));
            }
            "backpack" => {
                let m = mat(g, tex_for(l, Tex::Nylon));
                let trim = g.mats.id(MatDef::new(l.color2, Tex::Nylon));
                let c = [0., mix(b.waist_y, b.armpit, 0.55), b.chest_w[2] + 0.075 * u];
                rounded_box(&mut hard, c, [0.12 * u, 0.15 * u, 0.065 * u], ID3, 3.2, w1(j("chest")), m, 20);
                rounded_box(&mut hard, add(c, [0., -0.07 * u, 0.06 * u]), [0.09 * u, 0.06 * u, 0.025 * u], ID3, 3.5, w1(j("chest")), trim, 14);
                g.sockets.push(("back".into(), j("chest"), add(c, [0., 0., 0.07 * u]), [0., 0., 0., 1.]));
            }
            "scarf" => {
                let m = mat(g, tex_for(l, Tex::Knit));
                let y = b.nb + 0.012 * u;
                let rings: Vec<Ring> = (0..3).map(|i| Ring::new([0., y + 0.02 * u * i as f64 - 0.02 * u, 0.004 * u], [1., 0., 0.], [0., 0., 1.], b.neck_r * 1.55 - 0.004 * u * i as f64, b.neck_r * 1.5, wmix(&w1(j("chest")), &w1(j("neck")), 0.5))).collect();
                shell(&mut part, &rings, 24, m, m, 0.01 * u, None, None);
                let tail: Vec<Ring> = (0..5).map(|i| { let t = i as f64 / 4.; Ring::around([0.05 * u + 0.02 * u * t, y - 0.01 * u - 0.16 * u * t, -b.neck_r * 1.4 - 0.02 * u - 0.02 * u * t], [0.15, -1., -0.1], [1., 0., 0.], 0.035 * u, 0.008 * u, w1(j("chest"))) }).collect();
                let (s0, e) = (tail[0].c, tail[4].c);
                tube(&mut part, &tail, 8, Cap::Point(s0), Cap::Point(e), &|_, _| m, 1., 0., None);
            }
            "helmet" | "cap" | "beanie" | "headband" | "headset" | "goggles" | "glasses" => headgear(g, &mut part, &mut hard, &face, l),
            _ => {}
        }
    }
    // Weapon grip on the right hand: palm centre, rotation set by the clips
    // module so the barrel points forward in the aim pose.
    let rig = clips::RigInfo::new(&g.b);
    for side in 0..2 {
        let (pos, rot) = clips::grip_socket(&rig, side);
        let x = sfx(side);
        g.sockets.push((format!("hand.{x}"), j(&format!("hand_{x}")), pos, rot));
    }
    let hp = g.b.jp("head");
    g.sockets.push(("head".into(), j("head"), add(hp, [0., g.b.hh * 0.45, 0.]), [0., 0., 0., 1.]));
    if !part.is_empty() { g.parts.push(part); }
    if !hard.is_empty() { g.parts.push(hard); }
}

pub(crate) fn torso_weight_pub(b: &Body, p: [f64; 3]) -> W {
    let u = b.u;
    let (hips, spine, chest, neck) = (w1(j("hips")), w1(j("spine")), w1(j("chest")), w1(j("neck")));
    let y = p[1];
    let mut w = wmix(&hips, &spine, smooth01(b.hipj + 0.03 * u, b.waist_y + 0.04 * u, y));
    w = wmix(&w, &chest, smooth01(b.waist_y + 0.02 * u, b.chest_y, y));
    w = wmix(&w, &neck, smooth01(b.sh + 0.005 * u, b.nb + 0.03 * u, y) * 0.6);
    let side = if p[0] < 0. { 0 } else { 1 };
    let x = sfx(side);
    let shoulder_zone = smooth01(b.chest_w[0] * 0.55, b.sh_half * 0.85, p[0].abs()) * smooth01(b.armpit - 0.08 * u, b.sh - 0.02 * u, y);
    w = wmix(&w, &w1(j(&format!("shoulder_{x}"))), shoulder_zone * 0.5);
    let low = 1. - smooth01(b.hipj - 0.02 * u, b.hipj + 0.08 * u, y);
    w = wmix(&w, &w1(j(&format!("upper_leg_{x}"))), low * smooth01(0.02 * u, 0.09 * u, p[0].abs()) * 0.4);
    prune(w)
}

fn torso_depth_front(b: &Body, y: f64) -> f64 {
    let r = torso_shell_rings(b, y - 0.001, y + 0.001, 0., 0.);
    r.first().map_or(b.waist_w[1], |r| r.rz[1])
}

fn belt_ring(part: &mut Part, b: &Body, y: f64, h: f64, grow: f64, m: u32) {
    let rings = torso_shell_rings(b, y - h * 0.5, y + h * 0.5, grow, 0.4);
    let bb = b;
    let wf = move |p: [f64; 3], _: usize| torso_weight_pub(bb, p);
    shell(part, &rings, 36, m, m, grow * 0.8, Some(&wf), None);
}

/// Shell over the head following the face shape, limited by a polar edge
/// (degrees from the crown at front / side / back).
fn head_shell(part: &mut Part, face: &Face, edge: [f64; 3], thick: f64, rim: f64, m: u32, rim_m: u32, w: W, skip: Option<&dyn Fn([f64; 3]) -> bool>) {
    let (nth, nph) = (56usize, 16usize);
    let az = |theta: f64| { let f = theta.cos(); if f >= 0. { mix(edge[1], edge[0], f.powf(1.3)) } else { mix(edge[1], edge[2], (-f).powf(1.1)) } };
    let dir = |phi: f64, theta: f64| [phi.sin() * theta.sin(), phi.cos(), -phi.sin() * theta.cos()];
    let mut outer = Vec::new(); let mut inner = Vec::new(); let mut dirs = Vec::new();
    for i in 0..=nph {
        let t = i as f64 / nph as f64;
        let mut ro = Vec::new(); let mut ri = Vec::new(); let mut rd = Vec::new();
        for k in 0..nth {
            let theta = TAU * k as f64 / nth as f64;
            let phi = t * az(theta).to_radians();
            let d = dir(phi, theta);
            let s = face.point_opt(d, false);
            let out = norm(sub(s, face.c));
            let tt = thick + rim * smooth01(0.8, 1., t);
            ro.push(part.vertex(add(s, mul(out, tt)), w.clone()));
            ri.push(part.vertex(add(s, mul(out, thick * 0.3)), w.clone()));
            rd.push(d);
        }
        outer.push(ro); inner.push(ri); dirs.push(rd);
    }
    let c = face.c;
    let put = |part: &mut Part, v: Vec<u32>, m: u32, outward: bool| {
        let p: Vec<[f64; 3]> = v.iter().map(|&x| part.positions[x as usize]).collect();
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let mid = mul(p.iter().fold([0.; 3], |a, b| add(a, *b)), 1. / p.len() as f64);
        let mut v = v;
        if (dot(n, sub(mid, c)) > 0.) != outward { v.reverse(); }
        part.poly(v, vec![[0., 0.]; p.len()], m);
    };
    for i in 0..nph { for k in 0..nth {
        let k1 = (k + 1) % nth;
        if skip.is_some_and(|s| s(dirs[i][k])) { continue; }
        let m_ = if i >= nph - 2 { rim_m } else { m };
        if i == 0 { put(part, vec![outer[0][k], outer[1][k1], outer[1][k]], m_, true); continue; }
        put(part, vec![outer[i][k], outer[i][k1], outer[i + 1][k1], outer[i + 1][k]], m_, true);
    } }
    // Rim underside back to the skull.
    let l = nph;
    for k in 0..nth { let k1 = (k + 1) % nth; if skip.is_some_and(|s| s(dirs[l][k])) { continue; } put(part, vec![outer[l][k], outer[l][k1], inner[l][k1], inner[l][k]], rim_m, true); }
}

fn headgear(g: &mut Gen, part: &mut Part, hard: &mut Part, face: &Face, l: &Layer) {
    let u = g.b.u;
    let a = face.r[0];
    let headw = w1(j("head"));
    let mat = |g: &mut Gen, t: Tex| g.mats.id(MatDef { color2: Some(l.color2), ..MatDef::new(l.color, tex_for(l, t)) });
    match l.kind.as_str() {
        "helmet" => match l.style.as_str() {
            "racing" | "full" => {
                let m = mat(g, Tex::Plastic);
                let visor = g.mats.id(MatDef::new(l.color2.map(|c| c * 0.25), Tex::Glossy).metal(0.5));
                let stripe = g.mats.id(MatDef::new(g.spec.accent, Tex::Plastic));
                let fy = face.eye_y; let re = face.re / face.r[1];
                let opening = move |d: [f64; 3]| d[2] < -0.45 && (d[1] - fy).abs() < re * 2.4 + 0.06 && d[0].abs() < 0.72;
                let _ = stripe;
                head_shell(part, face, [150., 150., 150.], a * 0.2, a * 0.02, m, m, headw.clone(), Some(&opening));
                // Visor: a glossy curved plate across the opening.
                let rings: Vec<Ring> = (0..9).map(|i| {
                    let t = i as f64 / 8.;
                    let fx = mix(-0.78, 0.78, t);
                    let d = face.dir(fx, fy);
                    let p = add(face.point_opt(d, false), mul(norm(sub(face.point_opt(d, false), face.c)), a * 0.2));
                    Ring::around(p, [1., 0., 0.], [0., 0., -1.], a * 0.03, (re * 2.4 + 0.075) * face.r[1] * 1.05 * (1. - 0.25 * (2. * t - 1.).powi(4)), headw.clone())
                }).collect();
                let (s0, e) = (rings[0].c, rings[8].c);
                tube(hard, &rings, 10, Cap::Point(s0), Cap::Point(e), &|_, _| visor, 1., 0., None);
            }
            "cap" => {}
            _ => {
                // Tactical high-cut helmet with rails, NVG shroud and strap.
                let m = mat(g, if l.tex == Some(Tex::Camo) { Tex::Camo } else { Tex::Plastic });
                let dark = g.mats.id(MatDef::new(l.color.map(|c| c * 0.45), Tex::Plastic));
                let tactical = l.style != "round";
                let edge = if tactical { [70., 92., 104.] } else { [74., 98., 108.] };
                head_shell(part, face, edge, a * 0.3, a * 0.06, m, dark, headw.clone(), None);
                if tactical {
                    for side in 0..2 {
                        let sg = side_sign(side);
                        let d = norm([sg, 0.35, 0.05]);
                        let p = add(face.point_opt(d, false), mul(norm(sub(face.point_opt(d, false), face.c)), a * 0.17));
                        rounded_box(hard, p, [a * 0.035, a * 0.07, a * 0.3], rot_mul(rot_y(0.), ID3), 5., headw.clone(), dark, 12);
                    }
                    let d = face.dir(0., 0.62);
                    let p = add(face.point_opt(d, false), mul(norm(sub(face.point_opt(d, false), face.c)), a * 0.18));
                    rounded_box(hard, p, [a * 0.14, a * 0.07, a * 0.05], rot_x(20.), 4., headw.clone(), dark, 12);
                }
                // Chin strap.
                let strap = g.mats.id(MatDef::new(hex("#222320"), Tex::Nylon));
                let pts: Vec<[f64; 3]> = (0..9).map(|i| { let t = i as f64 / 8.; let th = mix(-1.05, 1.05, t); let d = norm([th.sin(), -0.25 - 0.62 * th.cos().powi(2), -0.1 * th.cos()]); let s = face.point(d); add(s, mul(norm(sub(s, face.c)), 0.004 * u)) }).collect();
                let rings: Vec<Ring> = pts.iter().enumerate().map(|(i, &p)| { let tan = norm(sub(pts[(i + 1).min(8)], pts[i.saturating_sub(1)])); Ring::around(p, tan, norm(sub(p, face.c)), 0.002 * u, 0.008 * u, wmix(&headw, &w1(j("jaw")), 0.5 * (1. - (2. * i as f64 / 8. - 1.).abs()))) }).collect();
                tube(hard, &rings, 6, Cap::Point(pts[0]), Cap::Point(pts[8]), &|_, _| strap, 1., 0., None);
                g.sockets.push(("helmet".into(), j("head"), face.point(face.dir(0., 0.62)), [0., 0., 0., 1.]));
            }
        },
        "cap" => {
            let m = mat(g, Tex::Fabric);
            head_shell(part, face, [60., 84., 100.], a * 0.07, a * 0.015, m, m, headw.clone(), None);
            // Brim.
            let d = face.dir(0., 0.52);
            let s = face.point_opt(d, false);
            let c = add(add(s, mul(norm(sub(s, face.c)), a * 0.08)), [0., -a * 0.02, -a * 0.28]);
            rounded_box(hard, c, [a * 0.62, a * 0.025, a * 0.36], rot_x(-8.), 5., headw.clone(), m, 16);
            let top = face.point_opt([0., 1., 0.], false);
            rounded_box(hard, add(top, [0., a * 0.08, 0.]), [a * 0.06, a * 0.03, a * 0.06], ID3, 2., headw.clone(), m, 8);
        }
        "beanie" => {
            let m = mat(g, Tex::Knit);
            head_shell(part, face, [58., 86., 104.], a * 0.12, a * 0.06, m, m, headw.clone(), None);
        }
        "headband" => {
            let m = mat(g, Tex::Fabric);
            let fy = face.eye_y + 0.42;
            let rings: Vec<Ring> = (0..3).map(|i| { let y = face.c[1] + (fy + 0.05 * (i as f64 - 1.)) * face.r[1]; Ring::new([0., y, face.c[2] + 0.02 * a], [1., 0., 0.], [0., 0., 1.], face.r[0] * 1.02 + 0.006 * u, face.r[2] * 0.98 + 0.006 * u, headw.clone()) }).collect();
            shell(part, &rings, 40, m, m, 0.004 * u, None, None);
            let back = [0., face.c[1] + fy * face.r[1], face.c[2] + face.r[2] * 1.02];
            for side in 0..2 { let sg = side_sign(side); rounded_box(part, add(back, [sg * 0.02 * u, -0.06 * u, 0.012 * u]), [0.012 * u, 0.06 * u, 0.003 * u], rot_mul(rot_y(sg * 20.), ID3), 5., headw.clone(), m, 8); }
        }
        "headset" => {
            let m = mat(g, Tex::Plastic);
            let pad = g.mats.id(MatDef::new(hex("#1a1a1c"), Tex::Leather));
            let covered = g.spec.has("helmet");
            let lift = if covered { a * 0.16 } else { a * 0.03 };
            let pts: Vec<[f64; 3]> = (0..11).map(|i| { let t = i as f64 / 10.; let th = mix(-1.45, 1.45, t); let d = norm([th.sin(), th.cos(), 0.08]); let s = face.point_opt(d, false); add(s, mul(norm(sub(s, face.c)), lift)) }).collect();
            if !covered {
                let rings: Vec<Ring> = pts.iter().enumerate().map(|(i, &p)| { let tan = norm(sub(pts[(i + 1).min(10)], pts[i.saturating_sub(1)])); Ring::around(p, tan, norm(sub(p, face.c)), 0.006 * u, 0.014 * u, headw.clone()) }).collect();
                tube(hard, &rings, 8, Cap::Point(pts[0]), Cap::Point(pts[10]), &|_, _| m, 1., 0., None);
            }
            for side in 0..2 {
                let sg = side_sign(side);
                let d = norm([sg, (face.eye_y + face.nose_y) * 0.5 - 0.04, 0.1]);
                let s = face.point_opt(d, false);
                let c = add(s, [sg * (0.02 * u + lift * 0.5), 0., 0.]);
                rounded_box(hard, c, [0.02 * u, 0.05 * u, 0.045 * u], ID3, 2.6, headw.clone(), m, 14);
                rounded_box(hard, add(c, [-sg * 0.016 * u, 0., 0.]), [0.01 * u, 0.042 * u, 0.038 * u], ID3, 2.4, headw.clone(), pad, 12);
                if side == 0 {
                    // Boom mic to the mouth corner.
                    let mc = face.mouth_corner(0);
                    let tip = add(mc, [-0.012 * u, 0., -0.03 * u]);
                    let rings: Vec<Ring> = (0..5).map(|i| { let t = i as f64 / 4.; let p = add(lerp3(c, tip, t), [0., -0.02 * u * (t * (1. - t)) * 4., -0.01 * u * t]); Ring::around(p, norm(sub(tip, c)), [0., 1., 0.], 0.004 * u, 0.004 * u, headw.clone()) }).collect();
                    tube(hard, &rings, 6, Cap::Point(c), Cap::Point(tip), &|_, _| pad, 1., 0., None);
                    rounded_box(hard, tip, [0.008 * u, 0.008 * u, 0.012 * u], ID3, 2., headw.clone(), pad, 8);
                }
            }
        }
        "goggles" | "glasses" => {
            let frame = mat(g, Tex::Plastic);
            let lens = g.mats.id(MatDef::new(l.color2.map(|c| c * 0.5), Tex::Glossy).metal(0.6));
            let on_helmet = l.kind == "goggles" && g.spec.has("helmet");
            for side in 0..2 {
                let e = face.eye_centre(side);
                let c = if on_helmet { let d = face.dir(side_sign(side) * 0.22, 0.55); let s = face.point_opt(d, false); add(s, mul(norm(sub(s, face.c)), a * 0.22)) } else { add(e, [0., 0., -face.re * if l.kind == "glasses" { 1.35 } else { 1.6 }]) };
                let r = face.re * if l.kind == "glasses" { 1.35 } else { 1.55 };
                let rot = if on_helmet { rot_x(-55.) } else { ID3 };
                rounded_box(hard, c, [r, r * 0.8, face.re * 0.3], rot, if l.kind == "glasses" { 3. } else { 2.4 }, headw.clone(), frame, 14);
                rounded_box(hard, add(c, if on_helmet { [0., r * 0.25, -r * 0.2] } else { [0., 0., -face.re * 0.2] }), [r * 0.82, r * 0.64, face.re * 0.2], rot, 2.6, headw.clone(), lens, 14);
            }
            if l.kind == "goggles" && !on_helmet {
                let strap = g.mats.id(MatDef::new(l.color, Tex::Nylon));
                let y = face.eye_centre(0)[1];
                let rings: Vec<Ring> = (0..2).map(|i| Ring::new([0., y + (i as f64 - 0.5) * 0.02 * u, face.c[2]], [1., 0., 0.], [0., 0., 1.], face.r[0] * 1.03, face.r[2] * 1.02, headw.clone())).collect();
                shell(part, &rings, 40, strap, strap, 0.003 * u, None, None);
            }
        }
        _ => {}
    }
}
