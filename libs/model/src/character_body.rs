//! Body proportions, skeleton and the skinned body surface: torso, neck,
//! arms, hands, legs and feet as ring tubes with analytic blend weights.
use super::*;
use crate::transform::*;
use crate::{Joint, Skeleton};
use std::f64::consts::{PI, TAU};

/// Resolved proportions in metres. `u` is the body unit: 1.0 for a
/// 1.8 m adult of ordinary build; every width and length scales with it.
pub(crate) struct Body {
    pub u: f64, pub s: f64,
    pub hh: f64,
    /// Head ellipsoid centre and half extents (x width, y height, z depth).
    pub head_c: [f64; 3], pub head_r: [f64; 3],
    pub chin_y: f64, pub nb: f64, pub sh: f64, pub hipj: f64, pub crotch: f64,
    pub waist_y: f64, pub chest_y: f64, pub armpit: f64,
    pub sh_half: f64, pub neck_r: f64,
    /// (half width, front depth, back depth) at hip, waist, chest.
    pub hip_w: [f64; 3], pub waist_w: [f64; 3], pub chest_w: [f64; 3],
    pub limb: f64,
    pub hand_len: f64, pub palm_w: f64,
    pub joints: Vec<[f64; 3]>,
    /// Per side (0 = left, 1 = right): arm chain points shoulder, elbow,
    /// wrist, knuckles; hand frame (dir, palm normal, thumb side).
    pub arm: [[[f64; 3]; 4]; 2],
    pub hand_frame: [[[f64; 3]; 3]; 2],
    /// Leg chain: hip, knee, ankle, ball, toe tip, heel.
    pub leg: [[[f64; 3]; 6]; 2],
    pub foot_w: f64,
}
pub(crate) fn side_sign(side: usize) -> f64 { if side == 0 { -1. } else { 1. } }
pub(crate) fn sfx(side: usize) -> &'static str { if side == 0 { "l" } else { "r" } }

impl Body {
    pub fn new(sp: &CharacterSpec) -> Body {
        let s = sp.stylize;
        let h = sp.height;
        let hh = h / sp.heads;
        let fem = sp.fem;
        let head_w = hh * mix(0.74, 0.86, s) * sp.head_width * mix(1., 0.95, fem);
        let head_d = hh * mix(0.80, 0.86, s);
        let chin_y = h - hh;
        let neck_len = hh * mix(0.14, 0.05, s) / sp.neck.sqrt() * mix(1., 1.15, fem);
        let nb = chin_y - neck_len;
        let u = nb / 1.52;
        let sh = 1.45 * u;
        let wide = mix(1., 1.3, s);
        let limb = mix(1.14, 1.3, s) * sp.muscle.sqrt() * mix(1., 0.84, fem);
        let lf = sp.legs * mix(1., 0.74, s);
        let ankle = 0.08 * u;
        let hipj = (0.08 + 0.84 * lf) * u;
        let knee = (0.08 + 0.42 * lf) * u;
        let crotch = hipj - 0.075 * u;
        let t = sh - hipj;
        let waist_y = hipj + 0.40 * t;
        let chest_y = hipj + 0.72 * t;
        let armpit = sh - 0.10 * u;
        let sh_half = 0.225 * u * sp.shoulders * wide * mix(1., 0.86, fem) * mix(1., 1.05, sp.muscle - 1.);
        let hip_w = [0.165 * u * sp.hips * wide * mix(0.88, 1.0, fem) / sp.muscle.powf(0.25), 0.085 * u * wide, 0.095 * u * wide * mix(1., 1.04, fem)];
        let waist_w = [0.125 * u * mix(1.08, 1., fem) * sp.waist * wide * mix(1., 0.86, fem) * mix(1., 1.1, sp.belly - 1.), 0.085 * u * sp.belly * wide, 0.075 * u * wide];
        let chest_w = [0.178 * u * sp.chest * wide * mix(1., 0.84, fem) * sp.muscle.powf(0.2), 0.108 * u * sp.chest * wide * mix(1., 1.08, fem), 0.095 * u * wide];
        let neck_r = mix(0.066 * u, 0.13 * hh, s) * sp.neck.powf(0.4) * mix(1., 0.8, fem) * mix(1., 1.2, s) * (sp.muscle).powf(0.3);
        let head_c = [0., chin_y + hh * 0.5, -0.012 * u];
        let head_r = [head_w * 0.5, hh * 0.5, head_d * 0.5];
        let apose = mix(50., 56., s).to_radians();
        let hands = sp.hands * mix(1., 1.3, s);
        let hand_len = 0.19 * u * hands;
        let palm_w = 0.085 * u * hands * mix(1., 0.9, fem);
        let fwd = [0., 0., -1.];
        let mut arm = [[[0.; 3]; 4]; 2];
        let mut hand_frame = [[[0.; 3]; 3]; 2];
        let mut leg = [[[0.; 3]; 6]; 2];
        let arms = sp.arms * mix(1., 0.82, s);
        let feet = sp.feet * mix(1., 1.2, s);
        for side in 0..2 {
            let sg = side_sign(side);
            let p0 = [sg * (sh_half - 0.058 * u * limb), sh - 0.045 * u, 0.005 * u];
            let d1 = norm([sg * apose.sin(), -apose.cos(), -0.06]);
            let p1 = add(p0, mul(d1, 0.30 * u * arms));
            let bend = 12f64.to_radians();
            let d2 = norm(add(mul(d1, bend.cos()), mul(fwd, bend.sin())));
            let p2 = add(p1, mul(d2, 0.26 * u * arms));
            let p3 = add(p2, mul(d2, hand_len * 0.5));
            arm[side] = [p0, p1, p2, p3];
            let n = norm(mul(cross(d2, fwd), -sg));
            let tside = norm(sub(sub(fwd, mul(d2, dot(fwd, d2))), mul(n, dot(fwd, n))));
            hand_frame[side] = [d2, n, tside];
            let hip = [sg * 0.088 * u * sp.hips.sqrt() * wide, hipj, 0.01 * u];
            let kn = [sg * 0.083 * u * wide.sqrt(), knee, -0.004 * u];
            let an = [sg * 0.08 * u * wide.sqrt(), ankle, 0.018 * u];
            let ball = [sg * 0.086 * u * wide.sqrt(), 0.028 * u, an[2] - 0.155 * u * feet];
            let tip = [sg * 0.088 * u * wide.sqrt(), 0.03 * u, an[2] - 0.215 * u * feet];
            let heel = [sg * 0.08 * u * wide.sqrt(), 0.035 * u, an[2] + 0.045 * u * feet];
            leg[side] = [hip, kn, an, ball, tip, heel];
        }
        let head_joint = [0., chin_y + 0.12 * hh, head_c[2] + head_r[2] * 0.15];
        let mut b = Body {
            u, s, hh, head_c, head_r, chin_y, nb, sh, hipj, crotch, waist_y, chest_y, armpit, sh_half, neck_r,
            hip_w, waist_w, chest_w, limb, hand_len, palm_w, joints: vec![[0.; 3]; CHARACTER_JOINTS.len()], arm, hand_frame, leg,
            foot_w: 0.05 * u * feet * mix(1., 0.9, fem),
        };
        let mut set = |name: &str, p: [f64; 3]| b.joints[j(name) as usize] = p;
        set("hips", [0., hipj + 0.05 * u, 0.]);
        set("spine", [0., waist_y, 0.008 * u]);
        set("chest", [0., chest_y, 0.004 * u]);
        set("neck", [0., nb - 0.012 * u, 0.006 * u]);
        set("head", head_joint);
        for side in 0..2 {
            let sg = side_sign(side);
            let x = sfx(side);
            let [p0, p1, p2, _] = arm[side];
            set(&format!("shoulder_{x}"), [sg * 0.022 * u, nb - 0.045 * u, -0.012 * u]);
            set(&format!("upper_arm_{x}"), p0);
            set(&format!("lower_arm_{x}"), p1);
            set(&format!("hand_{x}"), p2);
            let [hip, kn, an, ball, _, _] = leg[side];
            set(&format!("upper_leg_{x}"), hip);
            set(&format!("lower_leg_{x}"), kn);
            set(&format!("foot_{x}"), an);
            set(&format!("toe_{x}"), ball);
            let f = hand_frame[side];
            let fingers = hand_fingers(hand_len, palm_w, p2, f, s);
            set(&format!("index_1_{x}"), fingers[0].0);
            set(&format!("index_2_{x}"), add(fingers[0].0, mul(fingers[0].1, fingers[0].2 * 0.45)));
            set(&format!("fingers_1_{x}"), fingers[1].0);
            set(&format!("fingers_2_{x}"), add(fingers[1].0, mul(fingers[1].1, fingers[1].2 * 0.45)));
            set(&format!("thumb_1_{x}"), fingers[4].0);
            set(&format!("thumb_2_{x}"), add(fingers[4].0, mul(fingers[4].1, fingers[4].2 * 0.45)));
        }
        let hc = head_c; let hr = head_r;
        set("jaw", [0., hc[1] - 0.2 * hr[1], hc[2] + 0.2 * hr[2]]);
        set("hair_1", [0., hc[1] + 0.1 * hr[1], hc[2] + 0.9 * hr[2]]);
        set("hair_2", [0., hc[1] - 0.45 * hr[1], hc[2] + 1.15 * hr[2]]);
        // Face joints are placed by the head builder once the face is laid out.
        b
    }
    pub fn skeleton(&self) -> Skeleton {
        Skeleton { joints: CHARACTER_JOINTS.iter().enumerate().map(|(i, (name, parent))| {
            let base = parent.map_or([0.; 3], |p| self.joints[p]);
            Joint { name: (*name).into(), parent: parent.map(|p| p as u32), translation: sub(self.joints[i], base) }
        }).collect() }
    }
    /// A copy of the proportions for builders that also mutate the generator.
    pub fn clone_layout(&self) -> Body {
        Body { joints: self.joints.clone(), ..*self }
    }
    pub fn jp(&self, name: &str) -> [f64; 3] { self.joints[j(name) as usize] }
    /// Arm girth factor: realistic builds get lean arms (a 1.25-muscle
    /// operator's upper arm is ~11 cm across, not a padded tube); stylised
    /// ones keep the chunky limb.
    /// Where the arm sockets into the torso (x of the upper_arm joint): the
    /// torso's shoulder rows hug it so the deltoid belongs to the arm and
    /// the torso never shows a boxy corner above it.
    pub fn shoulder_x(&self) -> f64 { self.arm[1][0][0].abs() }
    /// Leg girth factor, leaner for realistic builds the same way.
    pub fn leg_limb(&self) -> f64 { self.limb * mix(0.86, 1., smooth01(0., 0.45, self.s)) }
    pub fn arm_limb(&self) -> f64 { self.limb * mix(0.8, 1., smooth01(0., 0.45, self.s)) }
}

/// Knuckle position, direction, length and radius of the fingers
/// (index, middle, ring, pinky) and the thumb.
pub(crate) fn hand_fingers(hand_len: f64, palm_w: f64, wrist: [f64; 3], f: [[f64; 3]; 3], s: f64) -> [([f64; 3], [f64; 3], f64, f64); 5] {
    let [d, n, t] = f;
    let knuck = add(wrist, mul(d, hand_len * 0.5));
    let fl = hand_len * 0.5 * mix(1., 0.72, s);
    // Realistic fingers are ~1.8 cm thick; stylised ones chunkier still.
    let r = palm_w * 0.12 * mix(1., 1.1, s);
    let spread = |k: f64| norm(add(d, mul(t, k)));
    let finger = |off: f64, back: f64, len: f64, rr: f64, sp: f64| (add(add(knuck, mul(t, palm_w * off)), mul(d, -back)), spread(sp), fl * len, r * rr);
    let thumb_base = add(add(add(wrist, mul(d, hand_len * 0.12)), mul(t, palm_w * 0.3)), mul(n, palm_w * 0.12));
    let tdir = norm(add(add(mul(d, 0.55), mul(t, 0.75)), mul(n, 0.35)));
    [finger(0.38, 0.0, 0.94, 0.92, 0.09), finger(0.13, -0.004, 1.0, 0.94, 0.02), finger(-0.12, 0.002, 0.95, 0.9, -0.05), finger(-0.36, 0.012, 0.78, 0.82, -0.13),
     (thumb_base, tdir, fl * 0.95, r * 1.18)]
}

/// Materials of the tight layers painted straight onto the body, resolved
/// before geometry so face closures are plain data.
#[derive(Clone, Default)]
pub(crate) struct Wardrobe {
    pub skin: u32,
    /// (y0, y1, material, open-front half angle) — later entries win.
    pub torso: Vec<(f64, f64, u32, f64)>,
    /// Arm/leg chain parameter ranges: 0 root, 1 elbow/knee, 2 wrist/ankle.
    pub arm: Vec<(f64, f64, u32)>,
    pub leg: Vec<(f64, f64, u32)>,
    pub neck: Option<u32>,
    pub hand: Option<u32>,
    pub shoe: u32,
    pub sole: u32,
    pub shoe_trim: u32,
    /// A contrast stripe down the outside of the legs and the torso sides
    /// (material, half width in radians), plus a front zip line.
    pub stripe: Option<(u32, f64)>,
    pub zip: Option<u32>,
    /// Torso band across the chest (y0, y1, material).
    pub band: Option<(f64, f64, u32)>,
}
impl Wardrobe {
    pub fn torso_mat(&self, y: f64, theta_front: f64) -> u32 {
        let mut m = self.skin;
        for &(y0, y1, mat, open) in &self.torso { if y >= y0 && y <= y1 && !(open > 0. && theta_front.abs() < open) { m = mat; } }
        if m != self.skin {
            if let Some((y0, y1, band)) = self.band { if y >= y0 && y <= y1 { m = band; } }
            if let Some((s, w)) = self.stripe { if (theta_front.abs() - std::f64::consts::FRAC_PI_2).abs() < w * 1.4 { m = s; } }
            if let Some(z) = self.zip { if theta_front.abs() < 0.035 { m = z; } }
        }
        m
    }
    pub fn range_mat(list: &[(f64, f64, u32)], t: f64, skin: u32) -> u32 { let mut m = skin; for &(a, b, mat) in list { if t >= a && t <= b { m = mat; } } m }
}

/// Angle measured from the front (-Z) around a vertical ring whose x axis
/// is +X and z axis is +Z: front is theta = 3PI/2.
fn front_angle(theta: f64) -> f64 { let mut d = (theta - 1.5 * PI).rem_euclid(TAU); if d > PI { d -= TAU; } d }

pub(crate) fn build(g: &mut Gen) {
    let wr = gear::wardrobe(g);
    let b = &g.b;
    let u = b.u;
    let mut part = Part::new("body");
    torso(&mut part, b, &wr);
    neck(&mut part, b, &wr);
    for side in 0..2 {
        arm(&mut part, b, &wr, side);
        hand(&mut part, b, &wr, side, g.spec.hand_style == "mitten");
        leg(&mut part, b, &wr, side);
        foot(&mut part, b, &wr, side);
    }
    let _ = u;
    g.parts.push(part);
}

/// Torso weights: spine chain by height, plus shoulder and hip influence
/// at the flanks so arms and legs pull the surface around them.
fn torso_weight(b: &Body, p: [f64; 3]) -> W {
    let u = b.u;
    let (hips, spine, chest, neck) = (w1(j("hips")), w1(j("spine")), w1(j("chest")), w1(j("neck")));
    let y = p[1];
    let mut w = wmix(&hips, &spine, smooth01(b.hipj + 0.03 * u, b.waist_y + 0.04 * u, y));
    w = wmix(&w, &chest, smooth01(b.waist_y + 0.02 * u, b.chest_y, y));
    let centre = (p[0].abs() / (b.neck_r * 2.2)).min(1.);
    w = wmix(&w, &neck, smooth01(b.sh - 0.01 * u, b.nb + 0.02 * u, y) * (1. - centre * 0.6));
    let side = if p[0] < 0. { 0 } else { 1 };
    let x = sfx(side);
    // Shoulder girdle: the upper flank follows the clavicle, the deltoid
    // area near the joint follows the arm a little.
    let shoulder_zone = smooth01(b.chest_w[0] * 0.55, b.sh_half * 0.85, p[0].abs()) * smooth01(b.armpit - 0.08 * u, b.sh - 0.02 * u, y);
    w = wmix(&w, &w1(j(&format!("shoulder_{x}"))), shoulder_zone * 0.55);
    let ua = b.jp(&format!("upper_arm_{x}"));
    let near_arm = 1. - smooth01(0.05 * u, 0.11 * u, length(sub(p, ua)));
    w = wmix(&w, &w1(j(&format!("upper_arm_{x}"))), near_arm * 0.35);
    // Hip flanks and seat follow the thigh.
    let hip = b.leg[side][0];
    let low = 1. - smooth01(b.hipj - 0.02 * u, b.hipj + 0.08 * u, y);
    let lateral = smooth01(0.02 * u, 0.09 * u, p[0].abs());
    w = wmix(&w, &w1(j(&format!("upper_leg_{x}"))), low * lateral * 0.5 * (1. - smooth01(0.1 * u, 0.2 * u, length(sub(p, hip)))).max(0.3));
    prune(w)
}

fn torso(part: &mut Part, b: &Body, wr: &Wardrobe) {
    let u = b.u;
    let (hw, ww, cw) = (b.hip_w, b.waist_w, b.chest_w);
    // (y, half width, front, back, squareness)
    let rows: Vec<(f64, f64, f64, f64, f64)> = vec![
        (b.crotch - 0.02 * u, hw[0] * 0.5, hw[1] * 0.55, hw[2] * 0.6, 2.),
        (b.crotch + 0.01 * u, hw[0] * 0.86, hw[1] * 0.85, hw[2] * 0.92, 2.2),
        (b.hipj - 0.01 * u, hw[0], hw[1], hw[2], 2.3),
        (b.hipj + 0.05 * u, hw[0] * 0.97, hw[1] * 0.98, hw[2] * 0.9, 2.3),
        (mix(b.hipj, b.waist_y, 0.7), mix(hw[0], ww[0], 0.75), mix(hw[1], ww[1], 0.7), mix(hw[2], ww[2], 0.8), 2.2),
        (b.waist_y, ww[0], ww[1], ww[2], 2.2),
        (mix(b.waist_y, b.chest_y, 0.5), mix(ww[0], cw[0], 0.6), mix(ww[1], cw[1], 0.55), mix(ww[2], cw[2], 0.6), 2.3),
        (b.chest_y, cw[0], cw[1], cw[2], 2.4),
        (b.armpit, cw[0] * 1.03, cw[1] * 0.96, cw[2] * 1.02, 2.5),
        (b.sh - 0.06 * u, b.shoulder_x() + 0.012 * u, cw[1] * 0.8, cw[2] * 0.86, 2.4),
        (b.sh - 0.03 * u, b.shoulder_x() + 0.004 * u, cw[1] * 0.66, cw[2] * 0.74, 2.3),
        (b.sh - 0.005 * u, mix(b.shoulder_x(), b.neck_r * 1.6, 0.35), cw[1] * 0.55, cw[2] * 0.62, 2.3),
        // Trapezius: a gentle slope from the neck to the shoulder.
        (mix(b.sh, b.nb, 0.5), mix(b.shoulder_x(), b.neck_r * 1.32, 0.65), cw[1] * 0.5, cw[2] * 0.56, 2.2),
        (b.nb, b.neck_r * 1.32, b.neck_r * 1.12, b.neck_r * 1.18, 2.),
        (b.nb + 0.035 * u, b.neck_r * 0.95, b.neck_r * 0.9, b.neck_r * 0.95, 2.),
    ];
    let rings: Vec<Ring> = rows.iter().map(|&(y, hw, f, bk, e)| {
        let mut r = Ring::new([0., y, 0.], [1., 0., 0.], [0., 0., 1.], hw, 1., Vec::new());
        r.rz = [bk, f]; r.exp = e;
        r
    }).collect();
    let rings = densify(&rings, 2);
    let ys: Vec<f64> = rings.iter().map(|r| r.c[1]).collect();
    let mat = |i: usize, theta: f64| wr.torso_mat((ys[i] + ys[(i + 1).min(ys.len() - 1)]) * 0.5, front_angle(theta));
    let wf = |p: [f64; 3], _: usize| torso_weight(b, p);
    let first = rings[0].c; let last = rings[rings.len() - 1].c;
    tube(part, &rings, 32, Cap::Point(add(first, [0., -0.012 * u, 0.])), Cap::Point(add(last, [0., 0.01 * u, 0.])), &mat, 1., 0., Some(&wf));
}

/// Insert `n - 1` interpolated rings between each pair (Catmull-Rom on
/// centres and radii) so the surface stays smooth between key sections.
pub(crate) fn densify(rings: &[Ring], n: usize) -> Vec<Ring> {
    if rings.len() < 2 || n < 2 { return rings.to_vec(); }
    let cr = |a: f64, b: f64, c: f64, d: f64, t: f64| { let t2 = t * t; let t3 = t2 * t; 0.5 * (2. * b + (-a + c) * t + (2. * a - 5. * b + 4. * c - d) * t2 + (-a + 3. * b - 3. * c + d) * t3) };
    let mut out = Vec::new();
    for i in 0..rings.len() - 1 {
        let (a, b0, c, d) = (&rings[i.saturating_sub(1)], &rings[i], &rings[i + 1], &rings[(i + 2).min(rings.len() - 1)]);
        for k in 0..n {
            let t = k as f64 / n as f64;
            let mut r = b0.clone();
            r.c = std::array::from_fn(|q| cr(a.c[q], b0.c[q], c.c[q], d.c[q], t));
            for q in 0..2 { r.rx[q] = cr(a.rx[q], b0.rx[q], c.rx[q], d.rx[q], t).max(1e-4); r.rz[q] = cr(a.rz[q], b0.rz[q], c.rz[q], d.rz[q], t).max(1e-4); }
            r.exp = mix(b0.exp, c.exp, t);
            r.x = norm(lerp3(b0.x, c.x, t)); r.z = norm(lerp3(b0.z, c.z, t));
            r.w = wmix(&b0.w, &c.w, t);
            r.bumps = if t < 0.5 { b0.bumps.clone() } else { c.bumps.clone() };
            out.push(r);
        }
    }
    out.push(rings[rings.len() - 1].clone());
    out
}

fn neck(part: &mut Part, b: &Body, wr: &Wardrobe) {
    let u = b.u;
    let base = [0., b.nb - 0.03 * u, 0.008 * u];
    let top = [0., b.chin_y + 0.28 * b.hh, b.head_c[2] + b.head_r[2] * 0.12];
    let (neck, head, chest) = (w1(j("neck")), w1(j("head")), w1(j("chest")));
    let n = 7;
    let rings: Vec<Ring> = (0..n).map(|i| {
        let t = i as f64 / (n - 1) as f64;
        let c = lerp3(base, top, t);
        let r = b.neck_r * mix(1.06, 0.96, t);
        let w = if t < 0.3 { wmix(&chest, &neck, smooth01(0., 0.3, t)) } else { wmix(&neck, &head, smooth01(0.45, 0.85, t)) };
        let mut ring = Ring::around(c, sub(top, base), [1., 0., 0.], r, r * 1.05, w);
        // Adam's apple / throat front is flatter, the nape rounder.
        ring.rz = [r * 0.98, r * 1.08];
        ring
    }).collect();
    let skin = wr.skin;
    let neck_mat = wr.neck.unwrap_or(skin);
    let mat = |i: usize, _: f64| if i as f64 / (n - 1) as f64 <= 0.7 { neck_mat } else { skin };
    tube(part, &rings, 24, Cap::Open, Cap::Open, &mat, 1., 0., None);
}

/// Arm chain parameter of a point along the arm: 0 shoulder … 1 elbow … 2 wrist.
fn arm(part: &mut Part, b: &Body, wr: &Wardrobe, side: usize) {
    let u = b.u; let l = b.arm_limb();
    let x = sfx(side);
    let [p0, p1, p2, _] = b.arm[side];
    let (sh, ua, la, hd) = (w1(j(&format!("shoulder_{x}"))), w1(j(&format!("upper_arm_{x}"))), w1(j(&format!("lower_arm_{x}"))), w1(j(&format!("hand_{x}"))));
    let d1 = norm(sub(p1, p0)); let d2 = norm(sub(p2, p1));
    let l1 = length(sub(p1, p0)); let l2 = length(sub(p2, p1));
    // (chain t, radius, flatten)
    let keys: [(f64, f64, f64); 13] = [(-0.16, 0.04, 1.0), (-0.02, 0.055, 1.0), (0.18, 0.058, 0.96), (0.45, 0.054, 0.94), (0.7, 0.048, 0.95), (0.9, 0.041, 0.95),
        (1.0, 0.039, 0.93), (1.12, 0.045, 0.9), (1.3, 0.046, 0.86), (1.55, 0.041, 0.8), (1.8, 0.033, 0.74), (1.97, 0.029, 0.7), (2.04, 0.029, 0.72)];
    let pos = |t: f64| if t <= 1. { add(p0, mul(d1, t * l1)) } else { add(p1, mul(d2, (t - 1.) * l2)) };
    let side_hint = [0., 0., 1.];
    let blend = 0.06 * u;
    let weight = |t: f64| {
        let d_el = if t <= 1. { (t - 1.) * l1 } else { (t - 1.) * l2 };
        let mut w = wmix(&ua, &la, smooth01(-blend, blend * 0.9, d_el));
        let d_wr = (t - 2.) * l2;
        w = wmix(&w, &hd, smooth01(-0.02 * u, 0.02 * u, d_wr));
        let d_sh = t * l1;
        // The cap inside the shoulder follows the clavicle more than the arm,
        // so lowering the arm from the A-pose doesn't raise a hump.
        w = wmix(&wmix(&sh, &ua, 0.3), &w, smooth01(-0.04 * u, 0.1 * u, d_sh));
        w
    };
    let rings: Vec<Ring> = keys.iter().map(|&(t, r, fl)| {
        let axis = if t <= 1. { d1 } else { d2 };
        let r = r * u * l;
        let mut ring = Ring::around(pos(t), axis, side_hint, r * fl, r, weight(t));
        // The elbow point sits behind; the biceps bulges in front.
        if (t - 1.).abs() < 0.08 { ring.bumps.push((0., 0.5, 0.008 * u * l)); }
        if (0.3..0.7).contains(&t) { ring.bumps.push((PI, 0.7, 0.006 * u * l * l)); }
        ring
    }).collect();
    let rings = densify(&rings, 2);
    let ts: Vec<f64> = { let mut v = Vec::new(); for i in 0..keys.len() - 1 { for k in 0..2 { v.push(mix(keys[i].0, keys[i + 1].0, k as f64 / 2.)); } } v.push(keys[keys.len() - 1].0); v };
    let wf = |p: [f64; 3], i: usize| { let _ = p; weight(ts[i]) };
    let mat = |i: usize, _: f64| Wardrobe::range_mat(&wr.arm, (ts[i] + ts[(i + 1).min(ts.len() - 1)]) * 0.5, wr.skin);
    let start = rings[0].c; let end = rings[rings.len() - 1].c;
    tube(part, &rings, 16, Cap::Point(sub(start, mul(d1, 0.02 * u))), Cap::Point(add(end, mul(d2, 0.005 * u))), &mat, 1., 0., Some(&wf));
}

fn hand(part: &mut Part, b: &Body, wr: &Wardrobe, side: usize, mitten: bool) {
    let u = b.u;
    let x = sfx(side);
    let [_, _, wrist, _] = b.arm[side];
    let [d, n, t] = b.hand_frame[side];
    let mat_id = wr.hand.unwrap_or(wr.skin);
    let hd = w1(j(&format!("hand_{x}")));
    let la = w1(j(&format!("lower_arm_{x}")));
    let pw = b.palm_w; let hl = b.hand_len;
    let thick = pw * 0.3 * mix(1., 1.2, b.s);
    // Palm: wrist to knuckles, flattened along the palm normal.
    let palm_keys = [(-0.02, 0.36, 0.32), (0.08, 0.44, 0.34), (0.28, 0.5, 0.33), (0.46, 0.52, 0.3), (0.53, 0.46, 0.26)];
    let rings: Vec<Ring> = palm_keys.iter().map(|&(k, wx, th)| {
        let c = add(add(wrist, mul(d, hl * k)), mul(n, -thick * 0.1));
        let mut r = Ring::new(c, t, n, pw * wx, thick * th / 0.3, if k < 0.05 { wmix(&la, &hd, 0.7) } else { hd.clone() });
        r.exp = 2.6;
        r
    }).collect();
    let mat = |_: usize, _: f64| mat_id;
    let end = rings[rings.len() - 1].c;
    tube(part, &rings, 14, Cap::Point(sub(wrist, mul(d, 0.012 * u))), Cap::Point(add(end, mul(d, 0.004 * u))), &mat, 1., 0., None);
    let fingers = hand_fingers(hl, pw, wrist, [d, n, t], b.s);
    let names = [("index", 0usize), ("fingers", 1), ("fingers", 2), ("fingers", 3), ("thumb", 4)];
    let curl_axis = norm(cross(d, n));
    if mitten {
        // One mitt for the four fingers plus the thumb.
        let (k0, dir, len, r) = fingers[1];
        let (f1, f2) = (w1(j(&format!("fingers_1_{x}"))), w1(j(&format!("fingers_2_{x}"))));
        let base = sub(k0, mul(d, 0.01 * u));
        let keys = [(0., 1.0), (0.35, 1.02), (0.7, 0.95), (0.92, 0.75), (1.0, 0.45)];
        let rings: Vec<Ring> = keys.iter().map(|&(k, s)| {
            let c = add(base, mul(dir, len * 1.05 * k));
            let mut rr = Ring::new(c, t, n, pw * 0.5 * s, r * 1.15 * s.max(0.6), wmix(&f1, &f2, smooth01(0.3, 0.6, k)));
            rr.exp = 2.4; rr
        }).collect();
        let e = rings[rings.len() - 1].c;
        tube(part, &rings, 18, Cap::Point(sub(base, mul(dir, 0.004 * u))), Cap::Point(add(e, mul(dir, 0.004 * u))), &mat, 1., 0., None);
        let _ = curl_axis;
    }
    for (fi, (name, idx)) in names.iter().enumerate() {
        if mitten && *name != "thumb" { continue; }
        let (k0, dir, len, r) = fingers[*idx];
        let (j1, j2) = (w1(j(&format!("{name}_1_{x}"))), w1(j(&format!("{name}_2_{x}"))));
        let start = if *name == "thumb" { k0 } else { sub(k0, mul(d, 0.012 * u)) };
        let mid = 0.45;
        let keys: [(f64, f64); 7] = [(0., 1.08), (0.2, 1.0), (mid - 0.03, 0.9), (mid + 0.04, 0.92), (0.78, 0.86), (0.93, 0.76), (1.0, 0.5)];
        let rings: Vec<Ring> = keys.iter().map(|&(k, s)| {
            let c = add(start, mul(dir, len * k));
            let mut w = wmix(&j1, &j2, smooth01(mid - 0.06, mid + 0.06, k));
            if k < 0.05 && *name != "thumb" { w = wmix(&hd, &j1, 0.5); }
            if *name == "thumb" && k < 0.1 { w = wmix(&hd, &j1, 0.6); }
            Ring::around(c, dir, t, r * s, r * s * 0.9, w)
        }).collect();
        let e = rings[rings.len() - 1].c;
        let _ = fi;
        tube(part, &rings, 8, Cap::Point(sub(start, mul(dir, r * 0.5))), Cap::Point(add(e, mul(dir, r * 0.35))), &mat, 1., 0., None);
    }
}

fn leg(part: &mut Part, b: &Body, wr: &Wardrobe, side: usize) {
    let u = b.u; let l = b.leg_limb();
    let x = sfx(side);
    let [hip, kn, an, _, _, _] = b.leg[side];
    let (hips, ul, ll, ft) = (w1(j("hips")), w1(j(&format!("upper_leg_{x}"))), w1(j(&format!("lower_leg_{x}"))), w1(j(&format!("foot_{x}"))));
    let d1 = norm(sub(kn, hip)); let d2 = norm(sub(an, kn));
    let l1 = length(sub(kn, hip)); let l2 = length(sub(an, kn));
    let sg = side_sign(side);
    // (t, radius, front/back scale): 0 hip joint, 1 knee, 2 ankle.
    let keys: [(f64, f64, f64); 12] = [(-0.13, 0.07, 1.0), (-0.03, 0.094, 1.0), (0.15, 0.095, 1.0), (0.45, 0.083, 1.0), (0.75, 0.068, 1.0), (0.93, 0.059, 1.0),
        (1.02, 0.056, 1.0), (1.15, 0.06, 1.12), (1.35, 0.064, 1.2), (1.6, 0.052, 1.08), (1.86, 0.041, 1.0), (2.02, 0.038, 1.0)];
    let pos = |t: f64| if t <= 1. { add(hip, mul(d1, t * l1)) } else { add(kn, mul(d2, (t - 1.) * l2)) };
    let weight = |t: f64| {
        let d_kn = if t <= 1. { (t - 1.) * l1 } else { (t - 1.) * l2 };
        let mut w = wmix(&ul, &ll, smooth01(-0.05 * u, 0.05 * u, d_kn));
        w = wmix(&w, &ft, smooth01(-0.025 * u, 0.03 * u, (t - 2.) * l2));
        w = wmix(&wmix(&hips, &ul, 0.5), &w, smooth01(-0.06 * u, 0.1 * u, t * l1));
        w
    };
    let rings: Vec<Ring> = keys.iter().map(|&(t, r, fb)| {
        let axis = if t <= 1. { d1 } else { d2 };
        let r = r * u * l;
        let mut ring = Ring::around(pos(t), axis, [1., 0., 0.], r, r, weight(t));
        // Calf bulges backward (+z), the shin is flatter at the front.
        if t > 1. { ring.rz = [r * fb, r * 0.92]; }
        // Knee cap at the front; the thigh inner side is fuller.
        if (t - 1.).abs() < 0.07 { ring.bumps.push((1.5 * PI, 0.45, 0.009 * u * l)); }
        if t < 0.6 { ring.bumps.push((if sg < 0. { 0. } else { PI }, 0.8, 0.006 * u * l)); }
        ring
    }).collect();
    let rings = densify(&rings, 2);
    let ts: Vec<f64> = { let mut v = Vec::new(); for i in 0..keys.len() - 1 { for k in 0..2 { v.push(mix(keys[i].0, keys[i + 1].0, k as f64 / 2.)); } } v.push(keys[keys.len() - 1].0); v };
    let wf = |_: [f64; 3], i: usize| weight(ts[i]);
    // Leg rings: x axis is world +X, so the outside of the leg is angle 0
    // on the right leg and PI on the left.
    let outside = if sg > 0. { 0. } else { PI };
    let mat = |i: usize, theta: f64| {
        let m = Wardrobe::range_mat(&wr.leg, (ts[i] + ts[(i + 1).min(ts.len() - 1)]) * 0.5, wr.skin);
        match wr.stripe { Some((s, w)) if m != wr.skin && m != wr.shoe => { let mut d = (theta - outside).rem_euclid(TAU); if d > PI { d -= TAU; } if d.abs() < w { s } else { m } } _ => m }
    };
    let s0 = rings[0].c; let e = rings[rings.len() - 1].c;
    tube(part, &rings, 18, Cap::Point(sub(s0, mul(d1, 0.02 * u))), Cap::Point(add(e, mul(d2, 0.01 * u))), &mat, 1., 0., Some(&wf));
}

/// The shoe: a boxy tube from heel to toe with a sole band.
pub(crate) fn foot_rings(b: &Body, side: usize, grow: f64) -> Vec<Ring> {
    let u = b.u;
    let x = sfx(side);
    let [_, _, an, ball, tip, heel] = b.leg[side];
    let (ft, toe) = (w1(j(&format!("foot_{x}"))), w1(j(&format!("toe_{x}"))));
    let fw = b.foot_w + grow;
    let len = length(sub(heel, tip));
    // (fraction heel→tip, half width, height, top above sole)
    let keys: [(f64, f64, f64); 8] = [(0.0, 0.55, 0.36), (0.06, 0.8, 0.62), (0.2, 0.88, 0.78), (0.42, 0.95, 0.62), (0.62, 1.0, 0.46), (0.8, 0.96, 0.38), (0.93, 0.8, 0.31), (1.0, 0.45, 0.2)];
    let sole_y = 0.;
    let ball_t = length(sub(ball, heel)) / len;
    let dir = norm([tip[0] - heel[0], 0., tip[2] - heel[2]]);
    let _ = an;
    keys.iter().map(|&(t, wx, ht)| {
        let h = ht * 0.2 * u + grow;
        let c = [mix(heel[0], tip[0], t), sole_y + h * 0.5, mix(heel[2], tip[2], t)];
        let w = wmix(&ft, &toe, smooth01(ball_t - 0.08, ball_t + 0.06, t));
        let mut r = Ring::around(c, dir, [1., 0., 0.], fw * wx, h * 0.5, w);
        r.exp = 3.2;
        r
    }).collect()
}

fn foot(part: &mut Part, b: &Body, wr: &Wardrobe, side: usize) {
    let rings = foot_rings(b, side, 0.);
    let (shoe, sole, trim) = (wr.shoe, wr.sole, wr.shoe_trim);
    let n = rings.len();
    // Ring z axis is world up for the foot; angle PI*1.5 points down.
    let mat = move |i: usize, theta: f64| {
        // The foot rings' z axis points down.
        let down = theta.sin();
        if down > 0.72 { sole } else if down > 0.45 { trim } else if i >= n - 3 && down < -0.2 { trim } else { shoe }
    };
    let dir = norm(sub(rings[n - 1].c, rings[0].c));
    let s = rings[0].c; let e = rings[n - 1].c;
    tube(part, &rings, 20, Cap::Point(sub(s, mul(dir, 0.012 * b.u))), Cap::Point(add(e, mul(dir, 0.01 * b.u))), &mat, 1., 0., None);
}
