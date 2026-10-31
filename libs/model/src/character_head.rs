//! Head, face and hair. The head is a deformed ellipsoid sculpted by
//! analytic features (jaw, chin, cheeks, brow ridge, nose, lips, eye
//! sockets); the face is driven by bones: jaw, eyes (look), lids (blink),
//! brows and mouth corners. Hair is a shell over the scalp following a
//! hairline, draped below the ears for long styles, plus spikes, tails
//! and buns on their own geometry.
use makepad_csg_math::portable::PortableFloat;
use super::*;
use super::body::{sfx, side_sign};
use crate::transform::*;
use std::f64::consts::{PI, TAU};

/// Face layout in normalized head coordinates (x, y in -1..1 on the front
/// of the ellipsoid) plus metric results.
#[derive(Clone)]
pub(crate) struct Face {
    pub c: [f64; 3], pub r: [f64; 3],
    pub s: f64,
    pub eye_x: f64, pub eye_y: f64, pub re: f64,
    pub nose_y: f64, pub nose_h: f64, pub nose_w: f64, pub bridge: f64,
    pub mouth_y: f64, pub mouth_w: f64,
    pub jaw_narrow: f64, pub chin: f64,
    pub cheeks: f64,
    /// Brow ridge height; heavier jaws get heavier brows.
    pub ridge: f64,
}
fn gauss(x: f64) -> f64 { (-x * x).pexp() }

impl Face {
    pub fn new(sp: &CharacterSpec, b: &body::Body) -> Face {
        let s = sp.stylize;
        let (nose_h, nose_w, bridge) = match sp.nose.as_str() {
            "none" => (0., 0.1, 0.),
            "straight" => (0.11, 0.075, 0.1),
            "broad" => (0.1, 0.13, 0.04),
            "pointy" => (0.15, 0.07, 0.05),
            "hook" => (0.12, 0.08, 0.1),
            _ => (0.09, 0.1, 0.045),
        };
        let re = b.hh * mix(0.064, 0.105, s) * sp.eye_size;
        Face {
            c: b.head_c, r: b.head_r, s,
            eye_x: 0.37 * sp.eye_spacing * mix(1., 1.05, s), eye_y: mix(0.04, -0.05, s), re,
            nose_y: mix(-0.28, -0.3, s), nose_h: nose_h * sp.nose_size * mix(1., 0.7, s), nose_w: nose_w * sp.nose_size.sqrt(), bridge: bridge * sp.nose_size * mix(1., 0.4, s),
            mouth_y: mix(-0.54, -0.5, s), mouth_w: 0.25 * sp.mouth_width * mix(1., 0.85, s),
            jaw_narrow: mix(0.3, 0.16, s) / sp.jaw.ppowf(0.7), chin: sp.chin,
            cheeks: mix(0.03, 0.06, s),
            ridge: 0.035 * sp.jaw.ppowf(1.3),
        }
    }
    /// Head surface point for a unit direction (Y up, face toward -Z).
    pub fn point(&self, d: [f64; 3]) -> [f64; 3] { self.point_opt(d, true) }
    pub fn point_opt(&self, d: [f64; 3], sockets: bool) -> [f64; 3] {
        let [a, b, c] = self.r;
        let (dx, dy, dz) = (d[0], d[1], d[2]);
        let mut p = [dx * a, dy * b, dz * c];
        let front = (-dz).max(0.);
        if dy < 0. {
            let k = (-dy).ppowf(2.2);
            p[0] *= 1. - self.jaw_narrow * k;
            if dz > 0. { p[2] *= 1. - 0.32 * k; }
            p[2] -= self.chin * 0.1 * c * k * front.sqrt() * gauss(dx / 0.6);
            p[1] -= 0.04 * b * k * front;
        }
        if dy > 0. && dz > 0. { p[2] *= 1. + 0.07 * dy; }
        if dz < 0. { p[2] *= 1. - 0.08 * (1. - (-dy).max(0.)); }
        // Features, in face coordinates, fading off the front.
        let wf = smooth01(0.15, 0.75, front);
        let (fx, fy) = (dx, dy);
        let ax = fx.abs();
        let mut disp = 0.;
        // Nose: a ridge growing from the brow to a rounded tip, with wings.
        let ridge = smooth01(self.eye_y + 0.02, self.nose_y + 0.02, fy) * (1. - smooth01(self.nose_y - 0.02, self.nose_y - 0.09, fy));
        disp += c * (self.nose_h * 0.6 * ridge + self.bridge * 0.4 * ridge) * gauss(fx / (self.nose_w * 0.45));
        disp += c * self.nose_h * gauss(fx / (self.nose_w * 0.7)) * gauss((fy - self.nose_y - 0.01) / 0.065);
        disp += c * self.nose_h * 0.4 * gauss((ax - self.nose_w * 0.75) / 0.05) * gauss((fy - self.nose_y + 0.015) / 0.05);
        if sockets { disp -= self.re * 0.55 * gauss((ax - self.eye_x) / 0.14) * gauss((fy - self.eye_y) / 0.12); }
        disp += c * self.ridge * gauss((fy - self.eye_y - 0.2) / 0.07) * gauss((ax - self.eye_x) / 0.3);
        disp += c * self.cheeks * gauss((ax - 0.42) / 0.2) * gauss((fy - self.eye_y + 0.27) / 0.14);
        // Lips: fuller on realistic faces (stylised ones keep the small mouth).
        let lips = mix(1.35, 1., smooth01(0., 0.5, self.s));
        disp += c * 0.03 * lips * gauss(fx / self.mouth_w) * gauss((fy - self.mouth_y - 0.04) / 0.03);
        disp += c * 0.036 * lips * gauss(fx / (self.mouth_w * 0.85)) * gauss((fy - self.mouth_y + 0.05) / 0.035);
        // Cheekbones: a ridge under the outer eye that catches the light.
        disp += c * 0.025 * (1. - smooth01(0., 0.5, self.s)) * gauss((ax - 0.5) / 0.14) * gauss((fy - self.eye_y + 0.17) / 0.07);
        disp -= c * 0.01 * gauss(fx / self.mouth_w) * gauss((fy - self.mouth_y) / 0.02);
        // Chin ball.
        disp += c * 0.03 * self.chin * gauss(fx / 0.25) * gauss((fy + 0.88) / 0.1);
        let radial = norm(p);
        add(self.c, add(p, mul(radial, disp * wf)))
    }
    /// Direction for face coordinates on the front.
    pub fn dir(&self, fx: f64, fy: f64) -> [f64; 3] { let z = (1. - fx * fx - fy * fy).max(0.02).sqrt(); norm([fx, fy, -z]) }
    pub fn outward(&self, d: [f64; 3]) -> [f64; 3] {
        // Numerical surface normal from neighbouring directions.
        let e = 0.01;
        let t1 = norm(cross(d, if d[1].abs() < 0.9 { [0., 1., 0.] } else { [1., 0., 0.] }));
        let t2 = cross(d, t1);
        let p = self.point(d);
        let a = sub(self.point(norm(add(d, mul(t1, e)))), p);
        let b = sub(self.point(norm(add(d, mul(t2, e)))), p);
        let mut n = norm(cross(a, b));
        if dot(n, sub(p, self.c)) < 0. { n = mul(n, -1.); }
        n
    }
    pub fn eye_centre(&self, side: usize) -> [f64; 3] {
        let d = self.dir(side_sign(side) * self.eye_x, self.eye_y);
        let p = self.point_opt(d, false);
        add(p, [0., 0., self.re * 0.78])
    }
    pub fn mouth_corner(&self, side: usize) -> [f64; 3] { self.point(self.dir(side_sign(side) * self.mouth_w * 1.05, self.mouth_y)) }
}

pub(crate) fn place_face_joints(sp: &CharacterSpec, b: &mut body::Body) {
    let face = Face::new(sp, b);
    for side in 0..2 {
        let x = sfx(side);
        let e = face.eye_centre(side);
        b.joints[j(&format!("eye_{x}")) as usize] = e;
        b.joints[j(&format!("lid_{x}")) as usize] = e;
        b.joints[j(&format!("brow_{x}")) as usize] = face.point(face.dir(side_sign(side) * face.eye_x, face.eye_y + 0.22));
        b.joints[j(&format!("mouth_{x}")) as usize] = face.mouth_corner(side);
    }
}

pub(crate) fn build(g: &mut Gen) {
    let sp = g.spec;
    let face = Face::new(sp, &g.b);
    let u = g.b.u;
    place_face_joints(sp, &mut g.b);
    let covered = gear::head_cover(sp);
    let skin = g.mats.id(MatDef::new(sp.skin, Tex::Skin));
    let cover_mat = covered.balaclava.map(|c| g.mats.id(MatDef::new(c, Tex::Knit)));
    let lip = sp.lip_color.unwrap_or_else(|| { let s = sp.skin; [s[0] * 0.78, s[1] * 0.5, s[2] * 0.5] });
    // ── head ──
    let mut head = Part::new("head");
    let (jaw, headw, neckw) = (w1(j("jaw")), w1(j("head")), w1(j("neck")));
    let (ml, mr) = (w1(j("mouth_l")), w1(j("mouth_r")));
    let f2 = face.clone();
    let mouth_y_world = face.point(face.dir(0., face.mouth_y))[1];
    let wf = |p: [f64; 3]| {
        let d = norm(std::array::from_fn(|i| (p[i] - f2.c[i]) / f2.r[i]));
        let front = (-d[2]).max(0.);
        let mut w = headw.clone();
        // Lower face follows the jaw, fading toward the hinge.
        let below = smooth01(mouth_y_world + 0.004 * u, mouth_y_world - 0.03 * u, p[1]);
        let jawk = below * smooth01(0.05, 0.45, front + 0.25 * (d[1] < -0.5) as i32 as f64) * (1. - smooth01(0.72, 0.95, d[0].abs()));
        w = wmix(&w, &jaw, jawk.min(1.));
        for (side, mw) in [(0usize, &ml), (1, &mr)] {
            let corner = f2.mouth_corner(side);
            let k = 0.75 * gauss(length(sub(p, corner)) / (f2.r[0] * f2.mouth_w * 0.9));
            w = wmix(&w, mw, k);
        }
        let low_back = smooth01(-0.45, -0.85, d[1]) * smooth01(-0.1, 0.4, d[2]);
        w = wmix(&w, &neckw, low_back * 0.5);
        w
    };
    let (segs, rings) = (64usize, 44usize);
    let bal = covered.balaclava.is_some();
    let f3 = face.clone();
    let mat = |r: usize, theta: f64| {
        if !bal { return skin; }
        let phi = PI * (r as f64 + 0.5) / rings as f64;
        let d = [phi.psin() * theta.pcos(), -phi.pcos(), -phi.psin() * theta.psin()];
        // The eye slot runs from under the eyes to above the brows.
        let eye_band = d[2] < -0.3 && d[1] - f3.eye_y < f3.re / f3.r[1] * 3.4 && f3.eye_y - d[1] < f3.re / f3.r[1] * 1.5 && d[0].abs() < f3.eye_x + 0.34;
        if eye_band { skin } else { cover_mat.unwrap_or(skin) }
    };
    let f4 = face.clone();
    let grid = blob(&mut head, face.c, [0., 1., 0.], [1., 0., 0.], segs, rings, &|d| f4.point(d), &wf, &mat, 1.);
    // Skin shading in vertex colour: lips, cheeks, socket shade, and for
    // realistic faces the warmth of thin, blood-filled skin (nose, ears,
    // cheeks), a cooler under-eye, a lid crease, nasolabial folds and a
    // faint low-frequency mottle, so the skin never reads as flat paint.
    let blush = sp.blush;
    let real = 1. - smooth01(0., 0.5, sp.stylize);
    let beard_shadow = matches!(sp.facial_hair.as_str(), "stubble");
    let f5 = face.clone();
    let on_skin = move |d: [f64; 3]| !bal || (d[2] < -0.3 && d[1] - f5.eye_y < f5.re / f5.r[1] * 3.4 && f5.eye_y - d[1] < f5.re / f5.r[1] * 1.5 && d[0].abs() < f5.eye_x + 0.34);
    for row in &grid { for &v in row {
        let p = head.positions[v as usize];
        let d = norm(std::array::from_fn(|i| (p[i] - face.c[i]) / face.r[i]));
        let (fx, fy) = (d[0], d[1]);
        let mut col = [1., 1., 1., 1.];
        // Ears and the sides of the head: a warm, translucent flush.
        let ear = real * 0.35 * smooth01(0.75, 0.95, fx.abs()) * gauss((fy - (face.eye_y + face.nose_y) * 0.5) / 0.3);
        col[1] *= 1. - ear * 0.2; col[2] *= 1. - ear * 0.26;
        if d[2] <= -0.1 && on_skin(d) {
            let lipk = gauss(fx / (face.mouth_w * 0.95)) * gauss((fy - face.mouth_y) / 0.06);
            let corner = 0.25 * gauss((fx.abs() - face.mouth_w) / 0.04) * gauss((fy - face.mouth_y) / 0.035);
            let cheek = blush * 0.5 * gauss((fx.abs() - 0.45) / 0.16) * gauss((fy - face.eye_y + 0.28) / 0.12)
                + (0.18 + 0.12 * real) * gauss(fx / 0.12) * gauss((fy - face.nose_y) / 0.08)
                + real * 0.3 * gauss((fx.abs() - 0.42) / 0.14) * gauss((fy - face.eye_y + 0.3) / 0.1);
            let socket = mix(0.4, 0.12, smooth01(0., 0.5, sp.stylize)) * gauss((fx.abs() - face.eye_x) / 0.16) * gauss((fy - face.eye_y) / 0.14);
            let re_y = face.re / face.r[1];
            let under_eye = real * 0.2 * gauss((fx.abs() - face.eye_x) / 0.13) * gauss((fy - face.eye_y + re_y * 1.6) / 0.08);
            let crease = real * 0.3 * gauss((fx.abs() - face.eye_x) / 0.14) * gauss((fy - face.eye_y - re_y * 1.2) / 0.06);
            let fold = real * 0.22 * gauss((fx.abs() - face.nose_w - 0.1 + (fy - face.nose_y) * 0.25) / 0.06) * smooth01(face.nose_y + 0.02, face.nose_y - 0.05, fy) * smooth01(face.mouth_y - 0.06, face.mouth_y + 0.02, fy);
            let mottle = real * 0.06 * (p[0] * 83. + 1.3).psin() * (p[1] * 71. + 0.7).psin() * (p[2] * 97. + 2.1).psin();
            for i in 0..3 { let target = lip[i] / sp.skin[i].max(0.02); col[i] = mix(col[i], target.min(1.4), lipk * if bal { 0. } else { 1. }); }
            col[1] *= 1. - cheek * 0.35; col[2] *= 1. - cheek * 0.3;
            col[0] *= 1. - under_eye * 0.9; col[1] *= 1. - under_eye; col[2] *= 1. - under_eye * 0.5;
            for c in col.iter_mut().take(3) { *c *= (1. - socket) * (1. - corner) * (1. - crease) * (1. - fold) * (1. + mottle); }
            if beard_shadow {
                let beard = gear::beard_mask(&face, d);
                let hc = sp.hair_color;
                for i in 0..3 { col[i] = mix(col[i], (hc[i] / sp.skin[i].max(0.02)).min(1.), beard * 0.45); }
            }
        }
        head.colors[v as usize] = [col[0].clamp(0., 1.), col[1].clamp(0., 1.), col[2].clamp(0., 1.), 1.];
    } }
    g.parts.push(head);
    // ── eyes, lids, brows, mouth, ears ──
    eyes(g, &face);
    if covered.mouth_visible { mouth(g, &face, lip); }
    brows(g, &face);
    // Under a balaclava the ears still shape the knit.
    if !covered.ears { ears(g, &face, skin); } else if let Some(m) = cover_mat { ears(g, &face, m); }
    if !covered.hair { hair(g, &face); }
    if !bal { gear::facial_hair(g, &face); }
}

fn eyes(g: &mut Gen, face: &Face) {
    let sp = g.spec;
    let s = face.s;
    // Realistic sclera is a shaded off-white, never paper white.
    let sclera = g.mats.id(MatDef::new(lerp3([0.52, 0.48, 0.44], [0.86, 0.84, 0.8], smooth01(0., 0.5, s)), Tex::Plain).rough(0.22));
    let iris = g.mats.id(MatDef::new(sp.eye_color, Tex::Plain).rough(0.12));
    let pupil = g.mats.id(MatDef::new([0.01, 0.01, 0.012], Tex::Plain).rough(0.06));
    let skin = g.mats.id(MatDef::new(sp.skin, Tex::Skin));
    let lid_skin = skin;
    let lash_c = sp.brow_color.unwrap_or(sp.hair_color).map(|v| v * 0.35);
    let lash = g.mats.id(MatDef::new(lash_c, Tex::Plain).rough(0.6));
    let re = face.re;
    let iris_a = mix(30., 50., s).to_radians();
    let pupil_a = mix(11., 21., s).to_radians();
    let rings = 28;
    for side in 0..2 {
        let x = sfx(side);
        let c = face.eye_centre(side);
        let mut part = Part::new(&format!("eye_{x}"));
        let wf = |_: [f64; 3]| w1(j(&format!("eye_{x}")));
        let mat = |r: usize, _: f64| { let a = PI * (r as f64 + 0.5) / rings as f64; if a < pupil_a { pupil } else if a < iris_a { iris } else { sclera } };
        let f = |d: [f64; 3]| {
            let cosg = -d[2];
            let bulge = 1. + 0.07 * smooth01(iris_a.pcos(), 1., cosg);
            add(c, mul(d, re * bulge))
        };
        let grid = blob(&mut part, c, [0., 0., 1.], [1., 0., 0.], 24, rings, &f, &wf, &mat, 1.);
        // Iris: a darker limbal ring and a lighter centre.
        for (r, row) in grid.iter().enumerate() {
            let a = PI * r as f64 / rings as f64;
            let k = if a < iris_a { let t = (a - pupil_a) / (iris_a - pupil_a); if t < 0. { 1. } else { mix(1.25, 0.55, t.ppowf(1.6)) } } else { 1. - 0.12 * smooth01(PI * 0.5, PI * 0.9, a) };
            for &v in row { part.colors[v as usize] = [k.min(1.), k.min(1.), k.min(1.), 1.]; }
        }
        // Catch-light: a tiny bright speck on the cornea, up and to the left.
        let catch = g.mats.id(MatDef::new([1., 1., 1.], Tex::Emissive));
        let d = norm([-0.35, 0.4, -1.]);
        let cp = add(c, mul(d, re * 1.07));
        let r = re * 0.13;
        blob(&mut part, cp, [0., 0., 1.], [1., 0., 0.], 8, 5, &move |q| add(cp, [q[0] * r, q[1] * r, q[2] * r * 0.4]), &|_| w1(j(&format!("eye_{x}"))), &|_, _| catch, 1.);
        g.parts.push(part);
        // Upper lid shell, pivoting on the eye centre.
        let mut lid = Part::new(&format!("lid_{x}"));
        let tilt = sp.eye_tilt.to_radians() * side_sign(side);
        let open = mix(86., 46., 1. - sp.lid).to_radians();
        let edge = |beta: f64| open + 0.35 * tilt * beta.psin() + 0.08 * (beta * 1.2).pcos().powi(2) - 0.08;
        let lw = w1(j(&format!("lid_{x}")));
        shell_lid(&mut lid, c, re, true, &edge, lw, lid_skin, lash);
        // Lower lid: static, just hides the eyeball's lower seam.
        let low = |beta: f64| PI - mix(58., 64., s).to_radians() + 0.04 * beta.pcos();
        shell_lid(&mut lid, c, re, false, &low, w1(j("head")), skin, skin);
        g.parts.push(lid);
    }
}

/// A lid is a thin closed shell over the eyeball between the pole and the
/// margin angle `edge(beta)` (upper) or from `edge` to the lower pole.
fn shell_lid(part: &mut Part, c: [f64; 3], re: f64, upper: bool, edge: &dyn Fn(f64) -> f64, w: W, skin: u32, lash: u32) {
    let (nb, na) = (20usize, 10usize);
    let (b0, b1) = (-1.75f64, 1.75f64);
    let (r_out, r_in) = (re * 1.1, re * 1.035);
    let dir = |alpha: f64, beta: f64| {
        // alpha from +Y (upper) or -Y (lower); beta around, 0 = front (-Z).
        let (sa, ca) = alpha.psin_cos();
        let v = [sa * beta.psin(), ca, -sa * beta.pcos()];
        if upper { v } else { [v[0], -v[1], v[2]] }
    };
    let mut rows_out = Vec::new(); let mut rows_in = Vec::new();
    for i in 0..=na {
        let t = i as f64 / na as f64;
        let mut ro = Vec::new(); let mut ri = Vec::new();
        for k in 0..=nb {
            let beta = mix(b0, b1, k as f64 / nb as f64);
            let e = if upper { edge(beta) } else { PI - edge(beta) };
            let a = mix(0.25, e, t.ppowf(0.8));
            let d = dir(a, beta);
            // The margin rolls in: thicker rim at the lash line.
            let bulge = if upper { 1. + 0.03 * smooth01(0.7, 1., t) } else { 1. };
            ro.push(part.vertex(add(c, mul(d, r_out * bulge)), w.clone()));
            ri.push(part.vertex(add(c, mul(d, r_in)), w.clone()));
        }
        rows_out.push(ro); rows_in.push(ri);
    }
    let centre_out = c;
    let quad = |part: &mut Part, v: [u32; 4], m: u32| {
        let p: Vec<[f64; 3]> = v.iter().map(|&i| part.positions[i as usize]).collect();
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let mid = mul(add(add(p[0], p[1]), add(p[2], p[3])), 0.25);
        let mut vv = v.to_vec();
        if dot(n, sub(mid, centre_out)) < 0. { vv.reverse(); }
        part.poly(vv, vec![[0., 0.], [1., 0.], [1., 1.], [0., 1.]], m);
    };
    for i in 0..na { for k in 0..nb {
        let m = if upper && i >= na - 1 { lash } else { skin };
        quad(part, [rows_out[i][k], rows_out[i][k + 1], rows_out[i + 1][k + 1], rows_out[i + 1][k]], m);
        // Inner surface faces the eyeball: reverse orientation.
        let v = [rows_in[i][k], rows_in[i + 1][k], rows_in[i + 1][k + 1], rows_in[i][k + 1]];
        let p: Vec<[f64; 3]> = v.iter().map(|&x| part.positions[x as usize]).collect();
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let mut vv = v.to_vec();
        if dot(n, sub(p[0], c)) > 0. { vv.reverse(); }
        part.poly(vv, vec![[0., 0.]; 4], skin);
    } }
    // Margin strip joins outer and inner at the edge; it faces away from the pole.
    let e = na;
    for k in 0..nb {
        let v = [rows_out[e][k], rows_out[e][k + 1], rows_in[e][k + 1], rows_in[e][k]];
        let p: Vec<[f64; 3]> = v.iter().map(|&x| part.positions[x as usize]).collect();
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let pole = add(c, mul(if upper { [0., 1., 0.] } else { [0., -1., 0.] }, re));
        let mut vv = v.to_vec();
        if dot(n, sub(p[0], pole)) < 0. { vv.reverse(); }
        part.poly(vv, vec![[0., 0.]; 4], if upper { lash } else { skin });
    }
    // Side and pole ends.
    for (a, b_) in [(0usize, 0usize), (nb, nb)] {
        for i in 0..na {
            let v = [rows_out[i][a], rows_out[i + 1][b_], rows_in[i + 1][b_], rows_in[i][a]];
            part.poly(v.to_vec(), vec![[0., 0.]; 4], skin);
        }
    }
    for k in 0..nb { part.poly(vec![rows_out[0][k], rows_in[0][k], rows_in[0][k + 1], rows_out[0][k + 1]], vec![[0., 0.]; 4], skin); }
}

fn brows(g: &mut Gen, face: &Face) {
    let sp = g.spec;
    if sp.brow <= 0.01 { return; }
    let col = sp.brow_color.unwrap_or(sp.hair_color).map(|v| v * 0.8);
    let m = g.mats.id(MatDef::new(col, Tex::Hair).rough(0.6));
    let u = g.b.u;
    for side in 0..2 {
        let x = sfx(side);
        let sg = side_sign(side);
        let mut part = Part::new(&format!("brow_{x}"));
        let n = 9;
        let ang = sp.brow_angle.to_radians();
        let pts: Vec<([f64; 3], f64)> = (0..n).map(|k| {
            let t = k as f64 / (n - 1) as f64;
            let fx = face.eye_x + mix(-0.21, 0.23, t);
            let arch = 0.045 * (1. - (2. * t - 0.9).powi(2)).max(0.);
            let fy = face.eye_y + face.re / face.r[1] * 1.55 + 0.1 + arch - ang * 0.22 * (1. - t) + ang * 0.05 * t;
            let d = face.dir(sg * fx, fy);
            let p = face.point(d);
            (add(p, mul(face.outward(d), 0.0025 * u)), mix(1.0, 0.45, t.ppowf(1.3)) * if t < 0.1 { 0.85 } else { 1. })
        }).collect();
        // Realistic faces get slimmer, softer brows.
    let thick = face.re * mix(0.34, 0.36, face.s) * sp.brow;
        let rings: Vec<Ring> = pts.iter().enumerate().map(|(k, (p, s))| {
            let tan = norm(sub(pts[(k + 1).min(n - 1)].0, pts[k.saturating_sub(1)].0));
            let d = norm(sub(*p, face.c));
            Ring::around(*p, tan, d, thick * 0.3 * s, thick * s * 0.5, w1(j(&format!("brow_{x}"))))
        }).collect();
        let (s0, e) = (rings[0].c, rings[n - 1].c);
        let t0 = norm(sub(rings[1].c, s0)); let t1 = norm(sub(e, rings[n - 2].c));
        tube(&mut part, &rings, 8, Cap::Point(sub(s0, mul(t0, thick * 0.2))), Cap::Point(add(e, mul(t1, thick * 0.3))), &|_, _| m, 1., 0., None);
        g.parts.push(part);
    }
}

/// A dark lens between the lips: upper edge on the head, lower edge on
/// the jaw, corners on the mouth joints — opening the jaw opens it.
fn mouth(g: &mut Gen, face: &Face, lip: [f64; 3]) {
    let sp = g.spec;
    let u = g.b.u;
    let dark = g.mats.id(MatDef::new([lip[0] * 0.25, lip[1] * 0.12, lip[2] * 0.12], Tex::Plain).rough(0.7));
    let mut part = Part::new("mouth");
    let n = 12;
    let (headw, jaw, ml, mr) = (w1(j("head")), w1(j("jaw")), w1(j("mouth_l")), w1(j("mouth_r")));
    let smile = sp.smile;
    let at = |fx: f64, fy: f64| { let d = face.dir(fx, fy); add(face.point(d), mul(face.outward(d), 0.0012 * u)) };
    let curve = |t: f64| -> (f64, f64) {
        let fx = face.mouth_w * (2. * t - 1.);
        let q = (2. * t - 1.).powi(2);
        (fx, face.mouth_y + smile * 0.05 * q - 0.004 + 0.01 * (1. - q) * 0.2)
    };
    let corner_w = |fx: f64| {
        let k = smooth01(0.5, 1., fx.abs() / face.mouth_w);
        if fx < 0. { wmix(&headw, &ml, k) } else { wmix(&headw, &mr, k) }
    };
    let mut upper = Vec::new(); let mut lower = Vec::new();
    for k in 0..=n {
        let t = k as f64 / n as f64;
        let (fx, fy) = curve(t);
        let q = 1. - (2. * t - 1.).powi(2);
        // Cupid's bow: the upper lip dips at the centre.
        let gap = 0.018 * q.ppowf(0.8) - 0.008 * gauss((2. * t - 1.) / 0.12);
        let wu = corner_w(fx);
        let wl = if k == 0 || k == n { wu.clone() } else { wmix(&wmix(&jaw, &wu, 0.3), &jaw, q) };
        upper.push(part.vertex(at(fx, fy + gap * 0.3), wu));
        lower.push(if k == 0 || k == n { upper[k] } else { part.vertex(at(fx, fy - gap * 0.7), wl) });
    }
    let centre = { let (fx, fy) = curve(0.5); part.vertex(at(fx, fy - 0.004), wmix(&headw, &jaw, 0.5)) };
    let outward = face.outward(face.dir(0., face.mouth_y));
    let tri = |part: &mut Part, a: u32, b: u32| {
        let p = [part.positions[a as usize], part.positions[b as usize], part.positions[centre as usize]];
        let nrm = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let mut v = vec![a, b, centre];
        if dot(nrm, outward) < 0. { v.reverse(); }
        part.poly(v, vec![[0., 0.]; 3], dark);
    };
    // Quads between the lips; the ends close on the shared corners.
    for k in 0..n {
        let q = [upper[k], upper[k + 1], lower[k + 1], lower[k]];
        let v: Vec<u32> = { let mut v = q.to_vec(); v.dedup(); if v.first() == v.last() && v.len() > 1 { v.pop(); } v };
        if v.len() < 3 { continue; }
        let p: Vec<[f64; 3]> = v.iter().map(|&x| part.positions[x as usize]).collect();
        let nrm = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let mut v = v;
        if dot(nrm, outward) < 0. { v.reverse(); }
        part.poly(v.clone(), vec![[0., 0.]; v.len()], dark);
    }
    let _ = (centre, &tri);
    g.parts.push(part);
}

fn ears(g: &mut Gen, face: &Face, skin: u32) {
    let sp = g.spec;
    let pointy = sp.ears == "pointy" || sp.ears == "elf";
    let big = if sp.ears == "big" { 1.35 } else { 1. };
    let eh = g.b.hh * 0.27 * sp.ear_size * big * mix(1., 0.85, face.s);
    for side in 0..2 {
        let sg = side_sign(side);
        let d = norm([sg, (face.eye_y + face.nose_y) * 0.5 - 0.04, 0.12]);
        let p = face.point(d);
        let out = face.outward(d);
        let centre = add(add(p, mul(out, eh * 0.06)), [0., 0., eh * 0.05]);
        let (w_, h_, t_) = (eh * 0.33, eh * 0.5, eh * 0.14);
        let mut part = Part::new(&format!("ear_{}", sfx(side)));
        // Tilted back ~12°, cupped toward the front-outside.
        let back = 0.21f64;
        let f = |q: [f64; 3]| {
            let mut l = [q[0] * t_, q[1] * h_, q[2] * w_];
            if pointy && q[1] > 0.3 { let k = (q[1] - 0.3) / 0.7; l[1] += k * k * h_ * 0.55; l[2] += k * k * w_ * 0.9; }
            // Concave bowl on the outer face.
            let cup = (1. - (q[1] * q[1] + q[2] * q[2]).min(1.)) * t_ * 1.4;
            if q[0] > 0. { l[0] -= cup * q[0].ppowf(0.5); }
            let (s_, c_) = back.psin_cos();
            let y = l[1] * c_ - l[2] * s_; let z = l[1] * s_ + l[2] * c_;
            add(centre, [sg * l[0], y, z])
        };
        let wf = |_: [f64; 3]| w1(j("head"));
        blob(&mut part, centre, [1., 0., 0.], [0., 1., 0.], 16, 10, &f, &wf, &|_, _| skin, 1.);
        g.parts.push(part);
    }
}

// ── hair ────────────────────────────────────────────────────────────────

struct HairShape {
    /// Hairline polar angle (degrees from the crown) at front, side, back.
    edge: [f64; 3],
    /// Where the shell stops following the skull and hangs (front, side, back).
    drape: Option<[f64; 3]>,
    /// Hanging length below the drape start, metres per radian of polar angle.
    hang: f64,
    thick: f64,
    top: f64,
    quiff: f64,
    flare: f64,
    grooves: f64,
    shaved: bool,
    mohawk: bool,
}

fn hair_shape(style: &str, a: f64) -> Option<HairShape> {
    let base = HairShape { edge: [64., 88., 114.], drape: None, hang: 0., thick: a * 0.1, top: a * 0.08, quiff: 0., flare: 0., grooves: 0.35, shaved: false, mohawk: false };
    Some(match style {
        "bald" | "none" => return None,
        "buzz" => HairShape { thick: a * 0.022, top: 0., grooves: 0., edge: [52., 86., 112.], ..base },
        "short" | "crew" => base,
        "swept" | "quiff" => HairShape { quiff: a * 0.34, top: a * 0.12, ..base },
        "spiky" => HairShape { thick: a * 0.08, top: a * 0.05, ..base },
        "mohawk" => HairShape { thick: a * 0.018, top: 0., grooves: 0., shaved: true, mohawk: true, ..base },
        "bob" => HairShape { edge: [74., 122., 126.], drape: Some([58., 88., 92.]), hang: a * 0.55, thick: a * 0.12, top: a * 0.1, flare: a * 0.1, ..base },
        "long" => HairShape { edge: [70., 150., 158.], drape: Some([56., 86., 90.]), hang: a * 1.1, thick: a * 0.12, top: a * 0.1, flare: a * 0.06, ..base },
        "ponytail" | "bun" => HairShape { thick: a * 0.07, top: a * 0.04, grooves: 0.5, edge: [60., 88., 116.], ..base },
        "afro" => HairShape { thick: a * 0.42, top: a * 0.22, grooves: 0., edge: [50., 86., 110.], flare: a * 0.05, ..base },
        _ => base,
    })
}

/// How far visible hair stands off the scalp at the crown (shell, crown
/// lift, quiff and the strand clumps on top), so hats and helmets sit over
/// the hair instead of vanishing inside it.
pub(crate) fn hair_clearance(sp: &CharacterSpec, a: f64) -> f64 {
    if gear::head_cover(sp).hair { return 0.; }
    hair_shape(&sp.hair, a).map_or(0., |h| (h.thick + h.top + h.quiff * 0.5) * sp.hair_volume + a * 0.03)
}

fn hair(g: &mut Gen, face: &Face) {
    let sp = g.spec;
    let a = face.r[0];
    let Some(shape) = hair_shape(&sp.hair, a) else { return };
    let vol = sp.hair_volume;
    let hc = sp.hair_color;
    let hair_m = g.mats.id(MatDef { color2: sp.hair_color2, ..MatDef::new(hc, Tex::Hair) });
    let shaved_m = g.mats.id(MatDef::new([mix(sp.skin[0], hc[0], 0.45), mix(sp.skin[1], hc[1], 0.45), mix(sp.skin[2], hc[2], 0.45)], Tex::Skin));
    let u = g.b.u;
    let (headw, chest, neck) = (w1(j("head")), w1(j("chest")), w1(j("neck")));
    let chin_y = g.b.chin_y; let nb = g.b.nb;
    let mut part = Part::new("hair");
    let (nth, nph) = (56usize, 20usize);
    // Azimuth: 0 = front. Blend front/side/back values around the head.
    let az = |theta: f64, v: [f64; 3]| { let f = theta.pcos(); if f >= 0. { mix(v[1], v[0], f.ppowf(1.3)) } else { mix(v[1], v[2], (-f).ppowf(1.1)) } };
    let dir_of = |phi: f64, theta: f64| [phi.psin() * theta.psin(), phi.pcos(), -phi.psin() * theta.pcos()];
    let mohawk_w = 0.24;
    let thickness = |phi: f64, theta: f64, t_edge: f64| {
        let d = dir_of(phi, theta);
        let mut t = shape.thick * vol;
        t += shape.top * vol * d[1].max(0.).powi(2);
        t += shape.quiff * vol * gauss((phi - 0.35) / 0.35) * theta.pcos().max(0.).powi(2);
        if shape.mohawk { t += a * 0.5 * vol * gauss(d[0] / (mohawk_w * 0.5)) * smooth01(-0.2, 0.3, d[1] + 0.3 * theta.pcos().max(0.)); }
        let groove = 1. - shape.grooves * (0.5 + 0.5 * (theta * 13.).pcos()).powi(3) * smooth01(0.2, 0.8, phi);
        t *= groove;
        // Hairline taper keeps the edge on the skin.
        t * smooth01(0.0, 0.18, 1. - t_edge).max(if shape.drape.is_some() { 0.35 } else { 0. }) + 0.0012 * u
    };
    let hang_len = shape.hang * vol.sqrt();
    let point = |phi: f64, theta: f64, t: f64, grow: f64| -> [f64; 3] {
        let drape = shape.drape.map(|dv| az(theta, dv).to_radians());
        let (p_phi, extra) = match drape { Some(dp) if phi > dp => (dp, phi - dp), _ => (phi, 0.) };
        let d = dir_of(p_phi, theta);
        let s = face.point_opt(d, false);
        let out = norm(sub(s, face.c));
        let mut p = add(s, mul(out, grow));
        if extra > 0. {
            // Hang straight down with a gentle outward flare.
            let horiz = norm([out[0], 0., out[2]]);
            p = add(p, [0., -extra * hang_len / 0.9, 0.]);
            p = add(p, mul(horiz, shape.flare * vol * (extra / 0.9).min(1.5) + grow * 0.2));
            // Keep the curtain outside the neck/shoulders.
            let _ = t;
        }
        p
    };
    let edge_phi = |theta: f64| {
        let mut e = az(theta, shape.edge);
        // Clear the ears on short styles.
        if shape.drape.is_none() { e -= 10. * gauss((theta.abs() - PI * 0.5) / 0.35); }
        e.to_radians()
    };
    let weight = |p: [f64; 3]| { let k = smooth01(chin_y - 0.02 * u, nb - 0.06 * u, p[1]); if k <= 0. { headw.clone() } else { wmix(&wmix(&headw, &neck, k.min(0.5) * 2.), &chest, (k - 0.5).max(0.) * 2.) } };
    let mut outer = Vec::new(); let mut inner = Vec::new();
    for i in 0..=nph {
        let t = i as f64 / nph as f64;
        let mut ro = Vec::new(); let mut ri = Vec::new();
        for k in 0..nth {
            let theta = TAU * k as f64 / nth as f64;
            let phi = t * edge_phi(theta);
            let th = thickness(phi, theta, t);
            let po = point(phi, theta, t, th);
            let pi = point(phi, theta, t, -0.004 * u);
            ro.push(part.vertex(po, weight(po)));
            ri.push(part.vertex(pi, weight(pi)));
        }
        outer.push(ro); inner.push(ri);
    }
    let centre = face.c;
    let shaved = shape.shaved;
    let mat_at = |theta: f64, _t: f64| {
        if shaved { let d = theta.psin().abs(); if d < mohawk_w * 0.9 || theta.pcos() < -0.95 { hair_m } else { shaved_m } } else { hair_m }
    };
    let orient = |part: &mut Part, v: Vec<u32>, uv: Vec<[f64; 2]>, m: u32, out: bool| {
        let p: Vec<[f64; 3]> = v.iter().map(|&x| part.positions[x as usize]).collect();
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let mid = mul(add(add(p[0], p[1]), p[2]), 1. / 3.);
        let mut v = v; let mut uv = uv;
        let away = dot(n, sub(mid, centre)) > 0.;
        if away != out { v.reverse(); uv.reverse(); }
        part.poly(v, uv, m);
    };
    let perim = TAU * a * 1.1;
    for i in 0..nph {
        for k in 0..nth {
            let k1 = (k + 1) % nth;
            let theta = TAU * (k as f64 + 0.5) / nth as f64;
            let m = mat_at(theta, i as f64 / nph as f64);
            let uv = |kk: usize, ii: usize| [perim * kk as f64 / nth as f64, ii as f64 / nph as f64 * 0.3];
            if i == 0 {
                // Crown: a fan to the pole ring (all ring-0 points coincide).
                orient(&mut part, vec![outer[0][k], outer[1][k1], outer[1][k]], vec![uv(k, 0), uv(k + 1, 1), uv(k, 1)], m, true);
                continue;
            }
            orient(&mut part, vec![outer[i][k], outer[i][k1], outer[i + 1][k1], outer[i + 1][k]], vec![uv(k, i), uv(k + 1, i), uv(k + 1, i + 1), uv(k, i + 1)], m, true);
            if shape.drape.is_some() {
                orient(&mut part, vec![inner[i][k], inner[i + 1][k], inner[i + 1][k1], inner[i][k1]], vec![uv(k, i), uv(k, i + 1), uv(k + 1, i + 1), uv(k + 1, i)], hair_m, false);
            }
        }
    }
    if shape.drape.is_some() {
        // Close the hanging edge: outer to inner at the last row.
        let l = nph;
        for k in 0..nth {
            let k1 = (k + 1) % nth;
            let v = vec![outer[l][k], outer[l][k1], inner[l][k1], inner[l][k]];
            let p: Vec<[f64; 3]> = v.iter().map(|&x| part.positions[x as usize]).collect();
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let mut v = v;
            if n[1] > 0. { v.reverse(); }
            part.poly(v, vec![[0., 0.]; 4], hair_m);
        }
    }
    // Spikes, tails and buns.
    let lock = |part: &mut Part, root: [f64; 3], path: &[[f64; 3]], r0: f64, flat: f64, w: &dyn Fn(f64) -> W| {
        let n = path.len();
        let rings: Vec<Ring> = (0..n).map(|i| {
            let t = i as f64 / (n - 1) as f64;
            let tan = norm(sub(path[(i + 1).min(n - 1)], path[i.saturating_sub(1)]));
            let out = norm(sub(path[i], face.c));
            Ring::around(path[i], tan, out, r0 * (1. - t * 0.92) * flat, r0 * (1. - t * 0.92), w(t))
        }).collect();
        let e = path[n - 1]; let tan = norm(sub(e, path[n - 2]));
        let _ = root;
        tube(part, &rings, 10, Cap::Point(sub(path[0], mul(norm(sub(path[1], path[0])), r0 * 0.3))), Cap::Point(add(e, mul(tan, r0 * 0.15))), &|_, _| hair_m, 1., 0., None);
    };
    let spike_list: Vec<(f64, f64, f64)> = match sp.hair.as_str() {
        "spiky" => { let mut v = Vec::new(); for i in 0..14 { let f = i as f64; let th = f * 2.39996; v.push((mix(0.2, 1.25, (f * 0.618).fract()), th, mix(0.45, 0.75, (f * 0.37).fract()) * (0.8 + 0.3 * th.pcos().max(0.)))); } v }
        "mohawk" => (0..8).map(|i| { let t = i as f64 / 7.; (mix(0.25, 1.9, t), 0., mix(0.5, 0.36, t)) }).collect(),
        _ => Vec::new(),
    };
    for (phi, theta, len) in spike_list {
        // Mohawk spikes run front to back along the midline.
        let (phi, theta) = if sp.hair == "mohawk" { (phi.min(1.2), if phi > 1.2 { PI } else { 0. }) } else { (phi, theta) };
        let phi = if sp.hair == "mohawk" && theta == PI { mix(0.1, 1.3, 1. - (phi / 1.2).min(1.)) } else { phi };
        let d = dir_of(phi, theta);
        let s = face.point_opt(d, false);
        let out = norm(sub(s, face.c));
        // Tufts sweep up and back; front ones lean back over the crown.
        let back = [0., 0.75, 0.65];
        let dirn = norm(add(mul(out, 0.55), mul(back, 0.7 + 0.5 * theta.pcos().max(0.))));
        let l = a * len * vol;
        let path: Vec<[f64; 3]> = (0..5).map(|i| { let t = i as f64 / 4.; add(add(s, mul(dirn, l * t)), mul([0., -1., 0.], l * 0.12 * t * t)) }).collect();
        lock(&mut part, s, &path, a * if sp.hair == "mohawk" { 0.13 } else { 0.2 }, 0.6, &|_| headw.clone());
    }
    // Clumps: flat locks lying on the shell, radiating from the crown along
    // the flow, so the surface reads as strands rather than a helmet. A few
    // loose locks break the fringe.
    if !matches!(sp.hair.as_str(), "buzz" | "mohawk" | "afro") {
        let n = if shape.drape.is_some() { 44 } else { 34 };
        for i in 0..n {
            let f = i as f64;
            let theta = f * 2.39996 + 0.3;
            let e = edge_phi(theta);
            let p0 = mix(0.12, 0.55, (f * 0.618).fract()) * e;
            let len = mix(0.35, 0.7, (f * 0.37).fract()) * e;
            let path: Vec<[f64; 3]> = (0..6).map(|k| {
                let t = k as f64 / 5.;
                let phi = (p0 + len * t).min(e * 1.02);
                let th = thickness(phi, theta, phi / e.max(1e-3));
                point(phi, theta, phi / e.max(1e-3), th * 0.82 + a * 0.012 * (1. - t))
            }).collect();
            lock(&mut part, path[0], &path, a * mix(0.09, 0.14, (f * 0.73).fract()), 0.35, &|_| headw.clone());
        }
        if matches!(sp.hair.as_str(), "swept" | "bob" | "long" | "short" | "ponytail") {
            for k in 0..3 {
                let theta = (k as f64 - 1.) * 0.32 + 0.12;
                let e = edge_phi(theta);
                let path: Vec<[f64; 3]> = (0..6).map(|q| {
                    let t = q as f64 / 5.;
                    let phi = e * mix(0.75, 1.08, t);
                    let th = thickness(phi.min(e), theta, (phi / e).min(1.));
                    let p = point(phi.min(e), theta, (phi / e).min(1.), th * 0.9);
                    add(p, [0., -a * 0.12 * t * t, -a * 0.04 * t])
                }).collect();
                lock(&mut part, path[0], &path, a * 0.08, 0.4, &|_| headw.clone());
            }
        }
    }
    if sp.hair == "ponytail" {
        let root = face.point_opt(dir_of(1.72, PI), false);
        let (h1, h2) = (w1(j("hair_1")), w1(j("hair_2")));
        let len = a * 2.2 * vol;
        let path: Vec<[f64; 3]> = (0..8).map(|i| { let t = i as f64 / 7.; add(root, [0., -len * t * 0.95 + a * 0.2 * (1. - t), a * (0.45 * t.sqrt() - 0.1 * t)]) }).collect();
        lock(&mut part, root, &path, a * 0.36, 0.8, &|t| wmix(&wmix(&headw, &h1, smooth01(0., 0.2, t)), &h2, smooth01(0.35, 0.7, t)));
        // Tie band.
        let band = g.mats.id(MatDef::new(sp.accent, Tex::Plastic));
        let mut tie = Part::new("hair_tie");
        let tie_rings: Vec<Ring> = (0..3).map(|i| Ring::around(add(root, [0., -a * 0.02 * i as f64 - a * 0.05, a * 0.08]), [0., -0.4, 1.], [1., 0., 0.], a * 0.2, a * 0.2, w1(j("hair_1")))).collect();
        tube(&mut tie, &tie_rings, 14, Cap::Open, Cap::Open, &|_, _| band, 1., 0., None);
        g.parts.push(tie);
        g.b.joints[j("hair_1") as usize] = root;
        g.b.joints[j("hair_2") as usize] = path[4];
    }
    if sp.hair == "bun" {
        let d = dir_of(0.75, PI);
        let s = face.point_opt(d, false);
        let c = add(s, mul(norm(sub(s, face.c)), a * 0.25));
        let r = a * 0.36 * vol;
        let f = move |q: [f64; 3]| add(c, [q[0] * r, q[1] * r * 0.85, q[2] * r]);
        blob(&mut part, c, [0., 1., 0.], [1., 0., 0.], 20, 12, &f, &|_| headw.clone(), &|_, _| hair_m, 1.);
    }
    if !part.is_empty() { g.parts.push(part); }
}
