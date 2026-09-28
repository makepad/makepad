//! First-person arms from a character spec: the character's own arms,
//! hands, sleeves, gloves and arm gear, posed by IK onto a weapon and baked
//! into a static model in the weapon's `grip` frame (origin at the grip,
//! -Z down the barrel, +X the shooter's right, +Y up). Because the arms are
//! the character's, the view model matches the third-person body exactly.
use super::*;
use super::clips::{qmul, qx, qy, Pose, RigInfo};
use crate::transform::*;

/// How the weapon is held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hold { Rifle, Pistol, One }

/// Parts of a spec the arms need, plus the hold. `support` is the left
/// hand's grip point in the weapon grip frame (rifle fore-end, pistol
/// wrap); `None` uses the hold's default.
pub struct FpsArms { pub hold: Hold, pub support: Option<[f64; 3]> }

impl FpsArms {
    pub fn parse(v: &Value) -> std::result::Result<FpsArms, String> {
        let hold = match v.get("hold").and_then(Value::as_str).unwrap_or("rifle") {
            "rifle" | "long" => Hold::Rifle,
            "pistol" => Hold::Pistol,
            "one" | "knife" | "grenade" => Hold::One,
            other => return Err(format!("fps_arms: hold '{other}' unknown (rifle, pistol, one)")),
        };
        let support = v.get("support").and_then(Value::as_arr).filter(|a| a.len() >= 3).and_then(|a| Some([num(&a[0])?, num(&a[1])?, num(&a[2])?]));
        Ok(FpsArms { hold, support })
    }
}

/// Joints whose influence keeps a vertex in the view model.
fn arm_joint(name: &str) -> bool {
    name.starts_with("upper_arm") || name.starts_with("lower_arm") || name.starts_with("hand") || name.starts_with("thumb") || name.starts_with("index") || name.starts_with("fingers")
}

/// Pose both arms onto the weapon: the grip is placed where a first-person
/// camera holds it relative to the eyes.
fn pose_on_weapon<'a>(rig: &'a RigInfo, arms: &FpsArms, grip: [f64; 3]) -> Pose<'a> {
    let mut p = Pose::new(rig);
    // Shooting stance: shoulders square forward toward the weapon and the
    // support shoulder leads (the camera stays at the rest eyes).
    if arms.hold == Hold::Rifle {
        p.rot("spine", qy(-18.));
        p.rot("chest", qmul(qy(-16.), qx(-8.)));
        p.shift([0., 0., -0.06 * rig.u]);
    } else {
        p.rot("chest", qx(-8.));
        p.shift([0., 0., -0.05 * rig.u]);
    }
    for x in ["l", "r"] { p.rot(&format!("shoulder_{x}"), qx(-14.)); }
    // Right hand: fingers point forward and a little down, palm facing the
    // grip (to the left); the wrist sits back and right of it.
    let d = norm([0.08, -0.35, -1.]);
    let n = [-1., 0., 0.];
    let target = sub(sub(grip, mul(d, rig.hand_len * 0.4)), mul(n, rig.palm_w * 0.32));
    p.hand(1, target, [1., -1., 0.4], Some((d, n)));
    p.fingers(1, 0.95, 0.5, 0.7);
    match arms.hold {
        Hold::Rifle => {
            let s = add(grip, arms.support.unwrap_or([0., 0.075, -0.32]));
            // Palm under the fore-end, fingers wrapping up its right side.
            let d = norm([0.75, 0.15, -0.55]);
            let n = [0., 1., 0.];
            let t = sub(sub(add(s, [0., -0.035, 0.]), mul(d, rig.hand_len * 0.35)), mul(n, rig.palm_w * 0.25));
            p.hand(0, t, [-1., -1., 0.2], Some((d, n)));
            p.fingers(0, 0.8, 0.7, 0.5);
        }
        Hold::Pistol => {
            let s = add(grip, arms.support.unwrap_or([-0.012, -0.02, 0.006]));
            // Support hand wraps the grip hand from the left.
            let d = norm([0.25, -0.45, -1.]);
            let n = [1., 0., 0.];
            let t = sub(sub(add(s, [-0.03, 0., 0.]), mul(d, rig.hand_len * 0.4)), mul(n, rig.palm_w * 0.3));
            p.hand(0, t, [-1., -1., 0.4], Some((d, n)));
            p.fingers(0, 0.9, 0.85, 0.6);
        }
        Hold::One => {}
    }
    p
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
    let mut pose = pose_on_weapon(&rig, arms, grip);
    let frames: Vec<([f64; 4], [f64; 3])> = (0..rig.rest.len()).map(|i| pose.global(i)).collect();
    let keep: Vec<bool> = CHARACTER_JOINTS.iter().map(|(n, _)| arm_joint(n)).collect();
    let one_handed = arms.hold == Hold::One;
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
    let (op, paint) = part.export_static(limits)?;
    let mut late = paint;
    let arr = |v: &[f64]| Value::Arr(v.iter().map(|x| Value::F64(*x)).collect());
    late.push(json::obj(vec![("op", json::s("socket")), ("name", json::s("grip")), ("attachment", json::obj(vec![("object", json::s("arms"))])),
        ("transform", json::obj(vec![("translation", arr(&[0., 0., 0.]))]))]));
    Ok(CharacterBuild { operations: vec![op], late, early: g.mats.ops(), warnings: spec.warnings.clone(), joints: Vec::new() })
}
