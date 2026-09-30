//! First-person arms from a character spec: the character's own arms,
//! hands, sleeves, gloves and arm gear, posed by IK onto a weapon and baked
//! into a static model in the weapon's `grip` frame (origin at the grip,
//! -Z down the barrel, +X the shooter's right, +Y up). Because the arms are
//! the character's, the view model matches the third-person body exactly.
use makepad_csg_math::portable::PortableFloat;
use super::*;
use super::clips::{qmul, qx, qy, Pose, RigInfo};
use crate::transform::*;
use std::f64::consts::TAU;

/// How the weapon is held, per weapon class.
/// - `Rifle` / `Long`: trigger hand on a raked pistol grip, index on the
///   trigger; support hand under the handguard (a long rifle's further out).
/// - `Pistol`: two hands, the support hand wrapping the trigger hand.
/// - `Blade`: a fist round a handle along the blade (-Z), thumb over,
///   blade forward, forearm up from below right.
/// - `One`: a thrown or carried object (grenade, charge) in a fist.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hold { Rifle, Long, Pistol, Blade, One }

/// Parts of a spec the arms need, plus the hold and the handle geometry the
/// hands wrap (all metres, in the weapon's grip frame: origin at the `grip`
/// socket, -Z down the barrel or blade, +X right, +Y up).
/// - `grip_radius`: the trigger hand's handle radius (half its width).
/// - `grip_front`: how far the front of the handle is ahead of the socket
///   along the fingers (a flat pistol grip is deeper than wide; the fingers
///   wrap its front edge). Equal to the radius for a round handle.
/// - `support` / `support_radius`: the support hand's wrap axis point and
///   radius (handguard, or the trigger hand for a pistol).
pub struct FpsArms { pub hold: Hold, pub support: Option<[f64; 3]>, pub grip_radius: f64, pub grip_front: f64, pub support_radius: f64 }

impl FpsArms {
    pub fn parse(v: &Value) -> std::result::Result<FpsArms, String> {
        let hold = match v.get("hold").and_then(Value::as_str).unwrap_or("rifle") {
            "rifle" => Hold::Rifle,
            "long" | "sniper" => Hold::Long,
            "pistol" | "sidearm" => Hold::Pistol,
            "knife" | "blade" => Hold::Blade,
            "one" | "grenade" => Hold::One,
            other => return Err(format!("fps_arms: hold '{other}' unknown (rifle, long, pistol, blade, one)")),
        };
        let support = v.get("support").and_then(Value::as_arr).filter(|a| a.len() >= 3).and_then(|a| Some([num(&a[0])?, num(&a[1])?, num(&a[2])?]));
        // Class defaults: strike's AK/M4/AWP pistol grips (3 cm wide, 4.3 cm
        // deep), a service pistol's (2.9 x 5.7 cm), a knife handle (2.8 cm).
        let (gr, gf, sr) = match hold {
            Hold::Rifle | Hold::Long => (0.015, 0.021, 0.022),
            Hold::Pistol => (0.0145, 0.028, 0.034),
            Hold::Blade => (0.0145, 0.0145, 0.0),
            Hold::One => (0.024, 0.024, 0.0),
        };
        let f = |k: &str, d: f64| v.get(k).and_then(num).filter(|x| *x > 0.0 && *x < 0.2).unwrap_or(d);
        Ok(FpsArms { hold, support, grip_radius: f("grip_radius", gr), grip_front: f("grip_front", gf), support_radius: f("support_radius", sr) })
    }
}

/// Palm half-thickness at the knuckles and finger radius, as fractions of
/// the palm width (`body::hand_fingers`, the palm rings in `body`).
const PALM_HALF: f64 = 0.23;
const FINGER_R: f64 = 0.12;

fn qaxis(axis: [f64; 3], deg: f64) -> [f64; 4] {
    let a = norm(axis);
    let h = deg.to_radians() * 0.5;
    [a[0] * h.psin(), a[1] * h.psin(), a[2] * h.psin(), h.pcos()]
}

/// One hand on a handle: placed so its palm lies on the handle (palm
/// half-thickness + radius off the axis) with the knuckles a finger's
/// radius behind the axis along the flat fingers `d`, palm normal `n`. The
/// thumb curls over; the fingers are closed onto the handle afterwards by
/// [`close_fingers`] against the real finger geometry.
fn grip_hand(p: &mut Pose, rig: &RigInfo, side: usize, axis: [f64; 3], r: f64, d: [f64; 3], n: [f64; 3], pole: [f64; 3], thumb: f64) -> bool {
    let x = super::body::sfx(side);
    let [d0, n0, _] = rig.hand_frame[side];
    let wrist0 = rig.rest[crate::character::j(&format!("hand_{x}")) as usize];
    let comp = |name: &str| { let v = sub(rig.rest[crate::character::j(name) as usize], wrist0); (dot(v, d0), dot(v, n0)) };
    let (kd, kn) = comp(&format!("fingers_1_{x}"));
    let rf = rig.palm_w * FINGER_R;
    // Knuckle line on the handle: palm surface touching it, the axis a
    // finger's radius ahead of the knuckles.
    let h = r + rig.palm_w * PALM_HALF - kn;
    let lead = 0.4 * rf;
    let knuckle = sub(sub(axis, mul(n, h)), mul(d, lead));
    // The fingers' knuckles span the palm's width about the grip point.
    let wrist = sub(sub(knuckle, mul(d, kd)), mul(n, kn));
    p.hand(side, wrist, pole, Some((d, n)));
    let reached = length(sub(p.pos(&format!("hand_{x}")), wrist)) < 0.002;
    let curl_axis = norm(cross(d0, n0));
    let tax = norm(add(curl_axis, mul(d0, 0.6)));
    p.set(&format!("thumb_1_{x}"), qaxis(tax, thumb * 25.));
    p.set(&format!("thumb_2_{x}"), qaxis(tax, thumb * 45.));
    reached
}

/// Joints whose influence keeps a vertex in the view model.
fn arm_joint(name: &str) -> bool {
    name.starts_with("upper_arm") || name.starts_with("lower_arm") || name.starts_with("hand") || name.starts_with("thumb") || name.starts_with("index") || name.starts_with("fingers")
}

/// Pose both arms onto the weapon: the grip is placed where a first-person
/// camera holds it relative to the eyes, and each hand wraps its handle.
/// Also returns each wrapped handle: (hand side, axis point, axis
/// direction, radius), in the posed body's frame.
fn pose_on_weapon<'a>(rig: &'a RigInfo, arms: &FpsArms, grip: [f64; 3]) -> (Pose<'a>, Vec<Handle>) {
    let mut p = Pose::new(rig);
    let mut handles = Vec::new();
    // Shooting stance: shoulders square forward toward the weapon and the
    // support shoulder leads (the camera stays at the rest eyes).
    if matches!(arms.hold, Hold::Rifle | Hold::Long) {
        // Bladed to the weapon: the support shoulder rolls forward so the
        // hand reaches the handguard.
        p.rot("spine", qy(-26.));
        p.rot("chest", qmul(qy(-26.), qx(-10.)));
        p.rot("shoulder_l", qy(-12.));
        p.shift([0.04 * rig.u, 0., -0.1 * rig.u]);
    } else {
        p.rot("chest", qx(-8.));
        p.shift([0., 0., -0.05 * rig.u]);
    }
    for x in ["l", "r"] { p.rot(&format!("shoulder_{x}"), qx(-14.)); }
    let r = arms.grip_radius;
    match arms.hold {
        Hold::Blade | Hold::One => {
            // A fist: the handle diagonal across the palm (a hammer grip),
            // thumb side toward the blade (-Z), the forearm coming from
            // behind and below right, the palm on the handle's left.
            let d = norm([-0.45, 0.7, 0.55]);
            let n = norm(cross(d, [0., 0., 1.]));
            grip_hand(&mut p, rig, 1, grip, r, d, n, [1., -1., 0.6], 0.9);
            handles.push(Handle { side: 1, point: grip, dir: norm(cross(d, n)), r, index_extra: 0.0 });
        }
        _ => {
            // Trigger hand: palm on the grip's right side, fingers forward
            // (down the grip's rake) wrapping its front edge, thumb up, the
            // index round the trigger guard.
            let d = norm([0.08, -0.35, -1.]);
            let n = [-1., 0., 0.];
            let axis = add(grip, mul(d, arms.grip_front - r));
            grip_hand(&mut p, rig, 1, axis, r, d, n, [1., -1., 0.4], 0.75);
            handles.push(Handle { side: 1, point: axis, dir: norm(cross(d, n)), r, index_extra: 0.022 });
        }
    }
    match arms.hold {
        Hold::Rifle | Hold::Long => {
            let far = if arms.hold == Hold::Long { -0.36 } else { -0.32 };
            let mut s = add(grip, arms.support.unwrap_or([0., 0.075, far]));
            // Palm under the handguard, fingers wrapping up its right side;
            // as far forward as the arm reaches (sliding back along it).
            let d = norm([1., 0.1, 0.]);
            let n = norm(sub([0., 1., 0.], mul(d, dot([0., 1., 0.], d))));
            for _ in 0..30 {
                if grip_hand(&mut p, rig, 0, s, arms.support_radius, d, n, [-1., -1., 0.2], 0.8) { break; }
                s = add(s, [0., 0., 0.01]);
            }
            handles.push(Handle { side: 0, point: s, dir: norm(cross(d, n)), r: arms.support_radius, index_extra: 0.0 });
        }
        Hold::Pistol => {
            // Support hand wraps the trigger hand's fingers from the left:
            // the same grip, a hand's thickness wider.
            let d = norm([0.25, -0.45, -1.]);
            let n = [1., 0., 0.];
            let front = add(grip, mul(norm([0.08, -0.35, -1.]), arms.grip_front - r));
            let s = add(front, arms.support.unwrap_or([0., -0.012, 0.0]));
            grip_hand(&mut p, rig, 0, s, arms.support_radius, d, n, [-1., -1., 0.4], 0.7);
            handles.push(Handle { side: 0, point: s, dir: norm(cross(d, n)), r: arms.support_radius, index_extra: 0.0 });
        }
        Hold::Blade | Hold::One => {}
    }
    (p, handles)
}

/// A wrapped handle: hand side, axis point and direction, radius, and the
/// extra radius the index keeps (a trigger finger rides the guard, not the
/// grip; 0 closes it like the others).
pub(crate) struct Handle { pub side: usize, pub point: [f64; 3], pub dir: [f64; 3], pub r: f64, pub index_extra: f64 }

/// A finger segment's vertices, by where they lie along the finger (the
/// weights blend across the middle joint and over the short fingers): the
/// proximal from a fifth of its length to the middle joint, the distal
/// beyond it. `chain` is "fingers", "index" or "thumb", `seg` "1" or "2".
fn segment_vertices<'p>(rig: &RigInfo, parts: &'p [Part], side: usize, chain: &str, seg: &str) -> Vec<(&'p [f64; 3], &'p W)> {
    let x = super::body::sfx(side);
    let j1 = crate::character::j(&format!("{chain}_1_{x}"));
    let j2 = crate::character::j(&format!("{chain}_2_{x}"));
    let (k1, l1) = (rig.rest[j1 as usize], length(sub(rig.rest[j2 as usize], rig.rest[j1 as usize])));
    // Along the chain's own rest direction (the thumb angles off the hand).
    let d0 = norm(sub(rig.rest[j2 as usize], k1));
    let span = if seg == "1" { (0.2 * l1, l1) } else { (l1, f64::MAX) };
    parts.iter().flat_map(|part| part.positions.iter().zip(&part.weights))
        .filter(|(pos, w)| {
            let chain_w: f64 = w.iter().filter(|t| t.joint == j1 || t.joint == j2).map(|t| t.weight).sum();
            let along = dot(sub(**pos, k1), d0);
            chain_w >= 0.5 && along >= span.0 && along < span.1
        })
        .collect()
}

/// Close each finger chain onto its handle, joint by joint from the
/// knuckle: the curl is found by bisection so the segment's own skinned
/// surface (the real finger geometry, every finger on the joint) just
/// touches the handle cylinder — no gap, no cut — whatever the handle's
/// radius or place in the hand.
fn close_fingers(p: &mut Pose, rig: &RigInfo, parts: &[Part], handles: &[Handle]) {
    const CONTACT: f64 = 0.0005;
    const HALF_LEN: f64 = 0.05;
    for h in handles {
        let x = super::body::sfx(h.side);
        let n0 = rig.hand_frame[h.side][1];
        // Two passes: closing the middle joint swings the proximal's far end
        // (next to it) off the handle; the second pass closes it again. The
        // thumb closes over last.
        for (chain, extra) in [("fingers", 0.0), ("index", h.index_extra), ("fingers", 0.0), ("index", h.index_extra), ("thumb", 0.0)] {
            // Each chain curls toward the palm about the axis across its
            // own rest direction.
            let (a, b) = (rig.rest[crate::character::j(&format!("{chain}_1_{x}")) as usize], rig.rest[crate::character::j(&format!("{chain}_2_{x}")) as usize]);
            let curl = norm(cross(norm(sub(b, a)), n0));
            for seg in ["1", "2"] {
                let verts = segment_vertices(rig, parts, h.side, chain, seg);
                if verts.is_empty() { continue; }
                let name = format!("{chain}_{seg}_{x}");
                let gap = |p: &mut Pose, deg: f64| -> f64 {
                    p.set(&name, qaxis(curl, deg));
                    let mut best = f64::MAX;
                    for (pos, w) in &verts {
                        let mut q = [0.; 3];
                        for jw in w.iter() {
                            let (rq, t) = p.global(jw.joint as usize);
                            q = add(q, mul(add(t, quat_rotate(rq, sub(**pos, rig.rest[jw.joint as usize]))), jw.weight));
                        }
                        let rel = sub(q, h.point);
                        let along = dot(rel, h.dir);
                        if along.abs() > HALF_LEN { continue; }
                        best = best.min(length(sub(rel, mul(h.dir, along))) - h.r - extra - CONTACT);
                    }
                    best
                };
                // Curl (toward the palm) to the first contact: scan in 5°
                // steps, then bisect inside the step. A segment already
                // touching stays straight; one that never reaches stops
                // where it comes closest (folding on past the handle would
                // turn the next segment away).
                if gap(p, 0.0) <= 0.0 { p.set(&name, qaxis(curl, 0.0)); continue; }
                let mut hit = None;
                let mut closest = (f64::MAX, 0.0);
                for k in 1..=24 {
                    let a = k as f64 * 5.0;
                    let g = gap(p, a);
                    if g <= 0.0 { hit = Some(a); break; }
                    if g < closest.0 { closest = (g, a); }
                }
                let Some(hi) = hit else { p.set(&name, qaxis(curl, closest.1)); continue; };
                let (mut lo, mut hi) = (hi - 5.0, hi);
                for _ in 0..16 {
                    let mid = 0.5 * (lo + hi);
                    if gap(p, mid) > 0.0 { lo = mid; } else { hi = mid; }
                }
                p.set(&name, qaxis(curl, lo));
            }
        }
    }
}

/// Build the static first-person arms model.
pub fn build_fps_arms(spec: &CharacterSpec, arms: &FpsArms, limits: &Limits) -> Result<CharacterBuild> {
    let b = body::Body::new(spec);
    let mut g = Gen { spec, b, mats: Mats { list: Vec::new(), base: spec.material_base }, parts: Vec::new(), sockets: Vec::new() };
    body::build(&mut g);
    head::place_face_joints(spec, &mut g.b);
    gear::build(&mut g);
    let rig = RigInfo::new(&g.b);
    // First-person grip placement relative to the eyes (camera space).
    let eyes = mul(add(g.b.jp("eye_l"), g.b.jp("eye_r")), 0.5);
    let grip = add(eyes, [0.17, -0.215, -0.36]);
    let (mut pose, handles) = pose_on_weapon(&rig, arms, grip);
    close_fingers(&mut pose, &rig, &g.parts, &handles);
    let frames: Vec<([f64; 4], [f64; 3])> = (0..rig.rest.len()).map(|i| pose.global(i)).collect();
    let keep: Vec<bool> = CHARACTER_JOINTS.iter().map(|(n, _)| arm_joint(n)).collect();
    let one_handed = matches!(arms.hold, Hold::One | Hold::Blade);
    let mut part = Part::new("arms");
    for src in &g.parts {
        let mut map = vec![u32::MAX; src.positions.len()];
        for poly in &src.polys {
            // Keep a face only if every corner is mostly an arm vertex.
            let ok = poly.vertices.iter().all(|&v| {
                let w = &src.weights[v as usize];
                let arm: f64 = w.iter().filter(|x| keep[x.joint as usize] && !(one_handed && CHARACTER_JOINTS[x.joint as usize].0.ends_with("_l"))).map(|x| x.weight).sum();
                arm > 0.6
            });
            if !ok { continue; }
            let verts: Vec<u32> = poly.vertices.iter().map(|&v| {
                if map[v as usize] == u32::MAX {
                    let p = src.positions[v as usize];
                    let mut q = [0.; 3];
                    for x in &src.weights[v as usize] {
                        let (r, t) = frames[x.joint as usize];
                        let local = sub(p, rig.rest[x.joint as usize]);
                        q = add(q, mul(add(t, quat_rotate(r, local)), x.weight));
                    }
                    // Into the grip frame.
                    let q = sub(q, grip);
                    map[v as usize] = part.vertex(q, Vec::new());
                    part.colors[map[v as usize] as usize] = src.colors[v as usize];
                }
                map[v as usize]
            }).collect();
            // Posing bends quads; triangles stay valid faces.
            let uv = |i: usize| poly.uvs.get(i).copied().unwrap_or([0.; 2]);
            for k in 1..verts.len() - 1 {
                let (a, b_, c) = (verts[0], verts[k], verts[k + 1]);
                if a == b_ || b_ == c || a == c { continue; }
                part.poly(vec![a, b_, c], vec![uv(0), uv(k), uv(k + 1)], poly.material);
            }
        }
    }
    let sides: &[usize] = if one_handed { &[1] } else { &[0, 1] };
    fps_detail(&mut g, &mut part, &frames, grip, sides);
    let (op, paint) = part.export_static(limits)?;
    let mut late = paint;
    let arr = |v: &[f64]| Value::Arr(v.iter().map(|x| Value::F64(*x)).collect());
    late.push(json::obj(vec![("op", json::s("socket")), ("name", json::s("grip")), ("attachment", json::obj(vec![("object", json::s("arms"))])),
        ("transform", json::obj(vec![("translation", arr(&[0., 0., 0.]))]))]));
    Ok(CharacterBuild { operations: vec![op], late, early: g.mats.ops(), warnings: spec.warnings.clone(), joints: Vec::new() })
}

// ── first-person detail ───────────────────────────────────────────────────
// The body's arms are modelled for a character seen at 3-40 m: a sleeve is a
// 16-sided shell, a glove is the hand painted. Held 40 cm from the eye they
// read as tubes and mittens. Everything below is added only to the view
// model, measured from the cut arms themselves (so it fits any body and any
// outfit): a fitted high-resolution sleeve with folds bunched at the elbow
// and above the cuff, a cuff band with its tab, and on the glove a moulded
// knuckle guard, finger knuckle pads and a wrist strap.

/// Per sample along `a -> b`: the radius (85th percentile of the vertices
/// around the axis) and the material most common among the outermost
/// quarter (the outer layer: sleeve, not skin).
fn outer_profile(part: &Part, vmat: &[u32], a: [f64; 3], b: [f64; 3], samples: usize, reach: f64) -> Vec<(f64, u32)> {
    let ax = sub(b, a);
    let len = length(ax).max(1e-6);
    let d = mul(ax, 1. / len);
    let mut buckets: Vec<Vec<(f64, u32)>> = vec![Vec::new(); samples + 1];
    // `vmat` covers the cut arms only: detail added since is not measured.
    for (i, &p) in part.positions.iter().enumerate().take(vmat.len()) {
        if vmat[i] == u32::MAX { continue; }
        let t = dot(sub(p, a), d) / len;
        if !(-0.02..=1.02).contains(&t) { continue; }
        let r = length(sub(sub(p, a), mul(d, t * len)));
        if r > reach { continue; }
        let k = ((t.clamp(0., 1.)) * samples as f64).round() as usize;
        buckets[k].push((r, vmat[i]));
    }
    buckets.into_iter().map(|mut b| {
        if b.is_empty() { return (0., u32::MAX); }
        b.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let top = b[((b.len() as f64 * 0.85) as usize).min(b.len() - 1)];
        // The material of the outermost vertices.
        let out = &b[b.len() * 3 / 4..];
        let mut best = (0usize, top.1);
        for &(_, m) in out { let n = out.iter().filter(|x| x.1 == m).count(); if n > best.0 { best = (n, m); } }
        (top.0, best.1)
    }).collect()
}

fn fps_detail(g: &mut Gen, part: &mut Part, frames: &[([f64; 4], [f64; 3])], grip: [f64; 3], sides: &[usize]) {
    let u = g.b.u;
    let pos = |name: &str| sub(frames[j(name) as usize].1, grip);
    let mut vmat = vec![u32::MAX; part.positions.len()];
    for poly in &part.polys { for &v in &poly.vertices { vmat[v as usize] = poly.material; } }
    let up = [0., 1., 0.];
    let glove = g.spec.layer("gloves").map(|l| (l.color, l.tex));
    // Moulded knuckles on tactical (nylon) gloves, a thicker leather pad on
    // leather ones; the strap is dark webbing either way.
    let (guard_m, strap_m) = match glove {
        Some((c, Some(Tex::Leather))) => (g.mats.id(MatDef::new(c.map(|v| v * 0.8), Tex::Leather)), g.mats.id(MatDef::new(c.map(|v| v * 0.6), Tex::Leather))),
        Some((c, _)) => (g.mats.id(MatDef::new(c.map(|v| v * 0.55), Tex::Rubber)), g.mats.id(MatDef::new(c.map(|v| v * 0.7), Tex::Nylon))),
        None => (u32::MAX, u32::MAX),
    };
    let w0: W = Vec::new();
    for &side in sides {
        let x = if side == 0 { "l" } else { "r" };
        let (sh, el, wr) = (pos(&format!("upper_arm_{x}")), pos(&format!("lower_arm_{x}")), pos(&format!("hand_{x}")));
        let fore = outer_profile(part, &vmat, el, wr, 24, 0.09 * u);
        let upper = outer_profile(part, &vmat, sh, el, 24, 0.11 * u);
        // The sleeve is the commonest outer material over the mid forearm.
        let mut counts: Vec<(u32, usize)> = Vec::new();
        for &(_, m) in &fore[4..14] { if m == u32::MAX { continue; } match counts.iter_mut().find(|c| c.0 == m) { Some(c) => c.1 += 1, None => counts.push((m, 1)) } }
        let Some(&(sleeve, _)) = counts.iter().max_by_key(|c| c.1) else { continue };
        // Where the sleeve ends on the forearm (the cuff).
        let end = (0..fore.len()).rev().find(|&k| fore[k].1 == sleeve && fore[k].0 > 0.).map_or(0.8, |k| k as f64 / 24.);
        if end < 0.3 { continue; }
        let flen = length(sub(wr, el));
        let ulen = length(sub(el, sh));
        let fd = norm(sub(wr, el));
        let ud = norm(sub(el, sh));
        let radius = |prof: &[(f64, u32)], t: f64| {
            let k = (t * 24.).clamp(0., 24.);
            let (i0, i1) = (k.floor() as usize, (k.ceil() as usize).min(24));
            let (r0, r1) = (prof[i0].0, prof[i1].0);
            let (r0, r1) = (if r0 > 0. { r0 } else { r1 }, if r1 > 0. { r1 } else { r0 });
            mix(r0, r1, k - k.floor())
        };
        // Centre line from 40% down the upper arm, round the elbow, to the
        // cuff: (point, axis, radius, distance from the elbow in metres).
        let mut line: Vec<([f64; 3], [f64; 3], f64, f64)> = Vec::new();
        let step = 0.006 * u;
        let mut t = 0.4;
        while t < 1. { let d = (t - 1.) * ulen; let blend = smooth01(-0.04 * u, 0.0, d) * 0.5; line.push((lerp3(sh, el, t), norm(lerp3(ud, fd, blend)), radius(&upper, t), d)); t += step / ulen; }
        let mut t = 0.;
        while t <= end - 0.02 { let d = t * flen; let blend = 0.5 + smooth01(0.0, 0.04 * u, d) * 0.5; line.push((lerp3(el, wr, t), norm(lerp3(ud, fd, blend)), radius(&fore, t), d)); t += step / flen; }
        if line.len() < 4 { continue; }
        let cuff_d = (end - 0.02) * flen;
        let rings: Vec<Ring> = line.iter().enumerate().map(|(i, &(c, axis, r, d))| {
            // Folds: rings of fabric bunched either side of the elbow and
            // stacked above the cuff, each crease a little tilted, plus a
            // slow spiral of shallow drag lines along the forearm.
            let elbow = (-(d / (0.07 * u)).powi(2)).pexp();
            let cuff = smooth01(cuff_d - 0.12 * u, cuff_d - 0.02 * u, d);
            let fold = (d / (0.018 * u) * TAU).psin().max(0.).powi(3) * (0.0045 * elbow + 0.0035 * cuff) * u;
            let mut ring = Ring::around(c, axis, up, r + 0.004 * u + fold, r + 0.004 * u + fold, w0.clone());
            let phase = i as f64 * 0.11 + side as f64 * 1.7;
            ring.bumps = vec![(phase, 0.35, 0.0022 * u), (phase + 2.4, 0.3, -0.0016 * u), (phase + 4.1, 0.45, 0.0018 * u * (1. + elbow))];
            ring
        }).collect();
        let mat = |_: usize, _: f64| sleeve;
        tube(part, &rings, 28, Cap::Open, Cap::Open, &mat, 1., 0., None);
        // Cuff: a doubled band just proud of the sleeve end, turned in at
        // the lip, and its hook-and-loop tab on top.
        if let Some(&(c, axis, r, _)) = line.last() {
            let band = 0.035 * u;
            let rb = r + 0.0075 * u;
            let back = sub(c, mul(axis, band));
            let cuff_rings = vec![
                Ring::around(back, axis, up, r + 0.003 * u, r + 0.003 * u, w0.clone()),
                Ring::around(add(back, mul(axis, 0.004 * u)), axis, up, rb, rb, w0.clone()),
                Ring::around(sub(c, mul(axis, 0.004 * u)), axis, up, rb, rb, w0.clone()),
                Ring::around(c, axis, up, rb - 0.002 * u, rb - 0.002 * u, w0.clone()),
                Ring::around(add(c, mul(axis, 0.001 * u)), axis, up, r - 0.002 * u, r - 0.002 * u, w0.clone()),
            ];
            tube(part, &cuff_rings, 28, Cap::Open, Cap::Open, &mat, 1., 0., None);
            let top = norm(sub(up, mul(axis, dot(up, axis))));
            let side_v = cross(axis, top);
            let tab_c = add(sub(c, mul(axis, band * 0.5)), mul(top, rb + 0.0025 * u));
            rounded_box(part, add(tab_c, mul(side_v, 0.012 * u)), [0.016 * u, 0.0025 * u, band * 0.42], [side_v, top, axis], 5., w0.clone(), sleeve, 12);
        }
        if guard_m == u32::MAX { continue; }
        // ── the glove ──
        let (i1, i2) = (pos(&format!("index_1_{x}")), pos(&format!("index_2_{x}")));
        let (f1, f2) = (pos(&format!("fingers_1_{x}")), pos(&format!("fingers_2_{x}")));
        let knuck = mul(add(i1, f1), 0.5);
        let fwd = norm(sub(knuck, wr));
        let spread = norm(sub(f1, i1));
        let mut back = norm(cross(fwd, spread));
        // The back of the hand faces away from the weapon's bore line.
        let away = sub(knuck, [0., 0.03, knuck[2]]);
        if dot(back, away) < 0. { back = mul(back, -1.); }
        // How far the glove surface stands out from a joint along `n`.
        fn thick(part: &Part, vmat: &[u32], c: [f64; 3], n: [f64; 3], reach: f64, floor: f64) -> f64 {
            part.positions.iter().zip(vmat).filter(|(p, m)| **m != u32::MAX && length(sub(**p, c)) < reach)
                .map(|(p, _)| dot(sub(*p, c), n)).fold(floor, f64::max)
        }
        let tk = thick(part, &vmat, knuck, back, 0.03 * u, 0.004 * u);
        let span = length(sub(f1, i1));
        let guard_c = add(add(knuck, mul(spread, span * 0.55)), mul(back, tk + 0.002 * u));
        let across = norm(cross(back, fwd));
        rounded_box(part, guard_c, [span * 1.05 + 0.008 * u, 0.0045 * u, 0.011 * u], [across, back, fwd], 3.2, w0.clone(), guard_m, 16);
        // Segmented ridges on the guard.
        for k in 0..3 {
            let o = (k as f64 - 1.) * (span * 0.7 + 0.005 * u);
            rounded_box(part, add(add(guard_c, mul(across, o)), mul(back, 0.004 * u)), [0.0065 * u, 0.0025 * u, 0.008 * u], [across, back, fwd], 3., w0.clone(), guard_m, 10);
        }
        // Knuckle pads on the middle joints: index, then the three fingers.
        let pads = [(i2, i1, 0.), (f2, f1, -0.019), (f2, f1, 0.), (f2, f1, 0.019)];
        for (jm, jb, o) in pads {
            let fd2 = norm(sub(jm, jb));
            let nb = norm(sub(back, mul(fd2, dot(back, fd2))));
            let c = add(jm, mul(spread, o * u));
            let t = thick(part, &vmat, c, nb, 0.012 * u, 0.004 * u);
            let ac = norm(cross(nb, fd2));
            rounded_box(part, add(c, mul(nb, t + 0.001 * u)), [0.0072 * u, 0.0022 * u, 0.008 * u], [ac, nb, fd2], 3., w0.clone(), guard_m, 10);
        }
        // Wrist strap round the glove's gauntlet, its tab on the back.
        let wax = fd;
        let wc = sub(wr, mul(wax, 0.012 * u));
        let rw = part.positions.iter().zip(&vmat).filter(|(p, m)| **m != u32::MAX && length(sub(**p, wc)) < 0.05 * u && dot(sub(**p, wc), wax).abs() < 0.008 * u)
            .map(|(p, _)| length(sub(sub(*p, wc), mul(wax, dot(sub(*p, wc), wax))))).fold(0.02 * u, f64::max);
        let strap: Vec<Ring> = [(-0.011, 0.0), (-0.008, 0.0025), (0.008, 0.0025), (0.011, 0.0)].iter()
            .map(|&(o, dr)| Ring::around(add(wc, mul(wax, o * u)), wax, back, rw + dr * u, rw + dr * u, w0.clone())).collect();
        let smat = |_: usize, _: f64| strap_m;
        tube(part, &strap, 24, Cap::Open, Cap::Open, &smat, 1., 0., None);
        let sb = norm(sub(back, mul(wax, dot(back, wax))));
        rounded_box(part, add(wc, mul(sb, rw + 0.004 * u)), [0.013 * u, 0.0025 * u, 0.012 * u], [norm(cross(sb, wax)), sb, wax], 5., w0.clone(), strap_m, 12);
    }
}

#[cfg(test)]
mod grip_tests {
    use super::*;

    /// Every hold wraps its handles: no arm vertex cuts into a handle
    /// cylinder, and the palm and both finger segments reach its surface
    /// (the fist closes on the handle, no floating fingers).
    #[test]
    fn hands_wrap_their_handles_without_gaps_or_cuts() {
        for preset in ["halcyon_warden", "kestrel_striker"] {
            for hold in ["blade", "pistol", "rifle", "long"] {
                let v = json::parse(format!(r#"{{"preset":"{preset}","hold":"{hold}"}}"#).as_bytes()).unwrap();
                let spec = CharacterSpec::parse(&v).unwrap();
                let arms = FpsArms::parse(&v).unwrap();
                let b = body::Body::new(&spec);
                let mut g = Gen { spec: &spec, b, mats: Mats { list: Vec::new(), base: spec.material_base }, parts: Vec::new(), sockets: Vec::new() };
                body::build(&mut g);
                head::place_face_joints(&spec, &mut g.b);
                gear::build(&mut g);
                let rig = RigInfo::new(&g.b);
                let eyes = mul(add(g.b.jp("eye_l"), g.b.jp("eye_r")), 0.5);
                let grip = add(eyes, [0.17, -0.215, -0.36]);
                let (mut pose, handles) = pose_on_weapon(&rig, &arms, grip);
                close_fingers(&mut pose, &rig, &g.parts, &handles);
                let frames: Vec<([f64; 4], [f64; 3])> = (0..rig.rest.len()).map(|i| pose.global(i)).collect();
                for Handle { side, point, dir, r, .. } in handles {
                    let x = super::body::sfx(side);
                    let skin = |pos: &[f64; 3], w: &W| -> Option<f64> {
                        let mut q = [0.; 3];
                        for jw in w {
                            let (rq, t) = frames[jw.joint as usize];
                            q = add(q, mul(add(t, quat_rotate(rq, sub(*pos, rig.rest[jw.joint as usize]))), jw.weight));
                        }
                        let rel = sub(q, point);
                        let along = dot(rel, dir);
                        (along.abs() <= 0.05).then(|| length(sub(rel, mul(dir, along))) - r)
                    };
                    // (group, deepest cut, closest approach to the surface)
                    let mut groups: Vec<(&str, f64, f64)> = Vec::new();
                    for (label, chain, seg) in [("fingers_1", "fingers", "1"), ("fingers_2", "fingers", "2"), ("index_1", "index", "1"), ("index_2", "index", "2")] {
                        let (mut cut, mut gap) = (0.0f64, f64::MAX);
                        for (pos, w) in segment_vertices(&rig, &g.parts, side, chain, seg) {
                            if let Some(d) = skin(pos, w) { cut = cut.max(-d); gap = gap.min(d); }
                        }
                        groups.push((label, cut, gap));
                    }
                    // The whole arm never cuts in; the palm rests on the handle.
                    let (mut cut, mut gap) = (0.0f64, f64::MAX);
                    for part in &g.parts {
                        for (pos, w) in part.positions.iter().zip(&part.weights) {
                            let Some(top) = w.iter().max_by(|a, b| a.weight.total_cmp(&b.weight)) else { continue };
                            let name = CHARACTER_JOINTS[top.joint as usize].0;
                            if !name.ends_with(&format!("_{x}")) || !(name.starts_with("hand") || name.starts_with("lower_arm") || name.starts_with("fingers") || name.starts_with("index") || name.starts_with("thumb")) { continue; }
                            if let Some(d) = skin(pos, w) {
                                cut = cut.max(-d);
                                if name.starts_with("hand") { gap = gap.min(d); }
                            }
                        }
                    }
                    groups.insert(0, ("hand", cut, gap));
                    let line: Vec<String> = groups.iter().map(|(n, cut, gap)| format!("{n} cut {:.1} gap {:.1} mm", cut * 1000., gap * 1000.)).collect();
                    println!("GRIP {preset} {hold} side {side} r {:.1} mm at z {:+.0} mm: {}", r * 1000., (point[2] - grip[2]) * 1000., line.join(" | "));
                    for (n, cut, gap) in &groups {
                        assert!(*cut < 0.004, "{preset} {hold} side {side}: {n} cuts {:.1} mm into the handle", cut * 1000.);
                        // A trigger finger rides the guard, off the grip.
                        if !(n.starts_with("index") && side == 1 && hold != "blade") {
                            assert!(*gap < 0.004, "{preset} {hold} side {side}: {n} stays {:.1} mm off the handle", gap * 1000.);
                        }
                    }
                }
            }
        }
    }
}
