//! Rig templates. A template is data the author can inspect and override: a
//! named joint table in rest pose plus procedural gait clips sampled into
//! ordinary keyframes. Characters face -Z (eyes toward -Z), Y up, so the
//! character's right side is +X; `_l` joints sit at -X.
use crate::{AnimationChannel, AnimationClip, AnimationPath, Joint, Keyframe, Skeleton};
use std::f64::consts::TAU;

/// (name, parent, world rest position as fractions of body height).
const HUMANOID: [(&str, Option<usize>, [f64; 3]); 21] = [
    ("hips", None, [0., 0.53, 0.]),
    ("spine", Some(0), [0., 0.60, 0.]),
    ("chest", Some(1), [0., 0.71, 0.]),
    ("neck", Some(2), [0., 0.835, 0.]),
    ("head", Some(3), [0., 0.875, 0.]),
    ("shoulder_l", Some(2), [-0.045, 0.815, 0.]),
    ("upper_arm_l", Some(5), [-0.115, 0.815, 0.]),
    ("lower_arm_l", Some(6), [-0.13, 0.64, 0.]),
    ("hand_l", Some(7), [-0.14, 0.475, 0.]),
    ("shoulder_r", Some(2), [0.045, 0.815, 0.]),
    ("upper_arm_r", Some(9), [0.115, 0.815, 0.]),
    ("lower_arm_r", Some(10), [0.13, 0.64, 0.]),
    ("hand_r", Some(11), [0.14, 0.475, 0.]),
    ("upper_leg_l", Some(0), [-0.06, 0.515, 0.]),
    ("lower_leg_l", Some(13), [-0.06, 0.28, 0.]),
    ("foot_l", Some(14), [-0.06, 0.045, 0.01]),
    ("toe_l", Some(15), [-0.06, 0.012, -0.075]),
    ("upper_leg_r", Some(0), [0.06, 0.515, 0.]),
    ("lower_leg_r", Some(17), [0.06, 0.28, 0.]),
    ("foot_r", Some(18), [0.06, 0.045, 0.01]),
    ("toe_r", Some(19), [0.06, 0.012, -0.075]),
];
pub const HUMANOID_CLIPS: [&str; 3] = ["idle", "walk", "run"];
/// Pose clips for action games, sampled like the gaits. Opt in by name via
/// `humanoid_rig {clips: [...]}`.
pub const HUMANOID_ACTION_CLIPS: [&str; 12] = ["jump", "fall", "land", "crouch", "long_jump", "ground_pound", "wall_slide", "hang", "backflip", "spin", "victory", "hurt"];

/// World rest positions of the humanoid template joints for a body height.
pub fn humanoid_joints(height: f64) -> Vec<(&'static str, Option<usize>, [f64; 3])> {
    HUMANOID.iter().map(|(name, parent, p)| (*name, *parent, p.map(|v| v * height))).collect()
}
pub fn humanoid_skeleton(height: f64) -> Skeleton {
    let joints = humanoid_joints(height);
    Skeleton {
        joints: joints.iter().map(|(name, parent, p)| {
            let base = parent.map_or([0.; 3], |i| joints[i].2);
            Joint { name: (*name).into(), parent: parent.map(|i| i as u32), translation: std::array::from_fn(|d| p[d] - base[d]) }
        }).collect(),
    }
}
fn quat_x(deg: f64) -> [f64; 4] { let h = deg.to_radians() * 0.5; [h.sin(), 0., 0., h.cos()] }
fn quat_y(deg: f64) -> [f64; 4] { let h = deg.to_radians() * 0.5; [0., h.sin(), 0., h.cos()] }
fn quat_z(deg: f64) -> [f64; 4] { let h = deg.to_radians() * 0.5; [0., 0., h.sin(), h.cos()] }
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1], a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
     a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3], a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2]]
}
struct Gait { duration: f64, hip: f64, knee: f64, knee_base: f64, arm: f64, elbow: f64, bob: f64, lean: f64, twist: f64 }
/// Procedural loops sampled at 24 keys per cycle (plus the closing key).
pub fn humanoid_clip(name: &str, height: f64) -> Option<AnimationClip> {
    let joints = humanoid_joints(height);
    let index = |n: &str| joints.iter().position(|j| j.0 == n).unwrap() as u32;
    let hips_rest = joints[0].2;
    let samples = 24;
    let mut channels: Vec<AnimationChannel> = Vec::new();
    let rotation = |channels: &mut Vec<AnimationChannel>, joint: &str, f: &dyn Fn(f64) -> [f64; 4], duration: f64| {
        let keys = (0..=samples).map(|k| { let t = k as f64 / samples as f64; Keyframe { time: t * duration, value: f(t * TAU) } }).collect();
        channels.push(AnimationChannel { joint: index(joint), path: AnimationPath::Rotation, keys });
    };
    let gait = match name {
        "walk" => Gait { duration: 1.0, hip: 26., knee: 42., knee_base: 5., arm: 20., elbow: 14., bob: 0.012, lean: 3., twist: 5. },
        "run" => Gait { duration: 0.64, hip: 42., knee: 85., knee_base: 20., arm: 38., elbow: 70., bob: 0.03, lean: 10., twist: 8. },
        "idle" => {
            let d = 3.2;
            rotation(&mut channels, "chest", &|p| quat_x(-1.4 * p.sin()), d);
            rotation(&mut channels, "head", &|p| quat_mul(quat_y(3. * (p * 0.5).sin()), quat_x(1.2 * p.sin())), d);
            rotation(&mut channels, "upper_arm_l", &|p| quat_z(-4. - 1.5 * p.sin()), d);
            rotation(&mut channels, "upper_arm_r", &|p| quat_z(4. + 1.5 * p.sin()), d);
            rotation(&mut channels, "lower_arm_l", &|_| quat_x(8.), d);
            rotation(&mut channels, "lower_arm_r", &|_| quat_x(8.), d);
            let keys = (0..=samples).map(|k| { let t = k as f64 / samples as f64; Keyframe { time: t * d, value: [hips_rest[0], hips_rest[1] - 0.004 * height * (0.5 - 0.5 * (t * TAU).cos()), hips_rest[2], 0.] } }).collect();
            channels.push(AnimationChannel { joint: 0, path: AnimationPath::Translation, keys });
            return Some(AnimationClip { name: name.into(), channels });
        }
        other => return action_clip(other, height),
    };
    let d = gait.duration;
    // Positive X rotation swings a hanging limb toward -Z (forward).
    for (side, phase) in [("l", 0.), ("r", std::f64::consts::PI)] {
        let (hip, knee, knee_base, arm, elbow) = (gait.hip, gait.knee, gait.knee_base, gait.arm, gait.elbow);
        rotation(&mut channels, &format!("upper_leg_{side}"), &move |p| quat_x(hip * (p + phase).sin()), d);
        rotation(&mut channels, &format!("lower_leg_{side}"), &move |p| quat_x(-(knee_base + knee * (p + phase).cos().max(0.).powf(1.5))), d);
        rotation(&mut channels, &format!("foot_{side}"), &move |p| quat_x(0.35 * (knee_base + knee * (p + phase).cos().max(0.).powf(1.5)) - 0.3 * hip * (p + phase).sin()), d);
        rotation(&mut channels, &format!("upper_arm_{side}"), &move |p| quat_mul(quat_z(if side == "l" { -5. } else { 5. }), quat_x(-arm * (p + phase).sin())), d);
        rotation(&mut channels, &format!("lower_arm_{side}"), &move |p| quat_x(elbow + 0.4 * elbow * (-(p + phase).sin()).max(0.)), d);
    }
    let (lean, twist) = (gait.lean, gait.twist);
    // Negative X pitches the upper body forward, toward -Z.
    rotation(&mut channels, "spine", &move |_| quat_x(-lean), d);
    rotation(&mut channels, "hips", &move |p| quat_y(twist * p.sin()), d);
    rotation(&mut channels, "chest", &move |p| quat_y(-1.4 * twist * p.sin()), d);
    let bob = gait.bob * height;
    let keys = (0..=samples).map(|k| { let t = k as f64 / samples as f64; Keyframe { time: t * d, value: [hips_rest[0], hips_rest[1] - bob * (0.5 + 0.5 * (2. * t * TAU).cos()), hips_rest[2], 0.] } }).collect();
    channels.push(AnimationChannel { joint: 0, path: AnimationPath::Translation, keys });
    Some(AnimationClip { name: name.into(), channels })
}

/// One pose per sample time `t` in 0..1: joint rotations (degrees about X,
/// then Y, then Z as three quaternions multiplied) and a hips offset as a
/// fraction of body height.
struct Pose { rot: Vec<(&'static str, [f64; 3])>, hips: [f64; 3] }
fn euler(e: [f64; 3]) -> [f64; 4] { quat_mul(quat_z(e[2]), quat_mul(quat_y(e[1]), quat_x(e[0]))) }
fn action_clip(name: &str, height: f64) -> Option<AnimationClip> {
    use std::f64::consts::PI;
    let (duration, pose): (f64, Box<dyn Fn(f64) -> Pose>) = match name {
        "jump" => (0.4, Box::new(|t: f64| { let k = 0.6 + 0.4 * t; Pose { rot: vec![
            ("upper_leg_l", [45. * k, 0., 0.]), ("upper_leg_r", [20. * k, 0., 0.]), ("lower_leg_l", [-80. * k, 0., 0.]), ("lower_leg_r", [-40. * k, 0., 0.]),
            ("upper_arm_l", [150. * k, 0., -15.]), ("upper_arm_r", [150. * k, 0., 15.]), ("lower_arm_l", [20., 0., 0.]), ("lower_arm_r", [20., 0., 0.]), ("spine", [-6., 0., 0.])], hips: [0., 0.02 * k, 0.] } })),
        "fall" => (0.8, Box::new(|t: f64| { let w = (t * 2. * PI).sin(); Pose { rot: vec![
            ("upper_arm_l", [15. * w, 0., -75. + 12. * w]), ("upper_arm_r", [-15. * w, 0., 75. + 12. * w]), ("lower_arm_l", [25., 0., 0.]), ("lower_arm_r", [25., 0., 0.]),
            ("upper_leg_l", [20. + 15. * w, 0., -14.]), ("upper_leg_r", [20. - 15. * w, 0., 14.]), ("lower_leg_l", [-35., 0., 0.]), ("lower_leg_r", [-35., 0., 0.]), ("head", [-8., 0., 0.])], hips: [0., 0., 0.] } })),
        "land" => (0.15, Box::new(|t: f64| { let k = (PI * (0.3 + 0.7 * t)).sin().max(0.); Pose { rot: vec![
            ("upper_leg_l", [70. * k, 0., 0.]), ("upper_leg_r", [70. * k, 0., 0.]), ("lower_leg_l", [-110. * k, 0., 0.]), ("lower_leg_r", [-110. * k, 0., 0.]),
            ("foot_l", [40. * k, 0., 0.]), ("foot_r", [40. * k, 0., 0.]), ("spine", [-22. * k, 0., 0.]), ("upper_arm_l", [30. * k, 0., -20. * k]), ("upper_arm_r", [30. * k, 0., 20. * k])], hips: [0., -0.15 * k, 0.] } })),
        "crouch" => (1.2, Box::new(|t: f64| { let b = 0.01 * (t * 2. * PI).sin(); Pose { rot: vec![
            ("upper_leg_l", [75., 0., -6.]), ("upper_leg_r", [75., 0., 6.]), ("lower_leg_l", [-115., 0., 0.]), ("lower_leg_r", [-115., 0., 0.]), ("foot_l", [40., 0., 0.]), ("foot_r", [40., 0., 0.]),
            ("spine", [-16., 0., 0.]), ("upper_arm_l", [25., 0., -8.]), ("upper_arm_r", [25., 0., 8.]), ("lower_arm_l", [35., 0., 0.]), ("lower_arm_r", [35., 0., 0.])], hips: [0., -0.18 + b, 0.] } })),
        "long_jump" => (0.6, Box::new(|t: f64| { let w = 4. * (t * 2. * PI).sin(); Pose { rot: vec![
            ("hips", [-70., 0., 0.]), ("upper_arm_l", [170., 0., -10.]), ("upper_arm_r", [170., 0., 10.]), ("upper_leg_l", [-20. + w, 0., 0.]), ("upper_leg_r", [-15. - w, 0., 0.]),
            ("lower_leg_l", [-25., 0., 0.]), ("lower_leg_r", [-20., 0., 0.]), ("head", [50., 0., 0.])], hips: [0., -0.05, 0.] } })),
        "ground_pound" => (0.4, Box::new(|_t: f64| Pose { rot: vec![
            ("upper_leg_l", [115., 0., -8.]), ("upper_leg_r", [115., 0., 8.]), ("lower_leg_l", [-135., 0., 0.]), ("lower_leg_r", [-135., 0., 0.]), ("spine", [-30., 0., 0.]),
            ("upper_arm_l", [60., 0., -10.]), ("upper_arm_r", [60., 0., 10.]), ("lower_arm_l", [90., 0., 0.]), ("lower_arm_r", [90., 0., 0.]), ("head", [-15., 0., 0.])], hips: [0., 0.05, 0.] })),
        "wall_slide" => (0.8, Box::new(|t: f64| { let w = 3. * (t * 2. * PI).sin(); Pose { rot: vec![
            ("upper_arm_r", [10., 0., 130. + w]), ("lower_arm_r", [20., 0., 0.]), ("upper_arm_l", [20., 0., -35.]), ("lower_arm_l", [30., 0., 0.]),
            ("upper_leg_l", [35., 0., 0.]), ("upper_leg_r", [20., 0., 0.]), ("lower_leg_l", [-55., 0., 0.]), ("lower_leg_r", [-40., 0., 0.]), ("spine", [0., 0., 6.]), ("head", [0., -20., 0.])], hips: [0., -0.02, 0.] } })),
        "hang" => (1.6, Box::new(|t: f64| { let w = 8. * (t * 2. * PI).sin(); Pose { rot: vec![
            ("upper_arm_l", [175., 0., -8.]), ("upper_arm_r", [175., 0., 8.]), ("lower_arm_l", [5., 0., 0.]), ("lower_arm_r", [5., 0., 0.]),
            ("upper_leg_l", [w, 0., 0.]), ("upper_leg_r", [w * 0.8, 0., 0.]), ("lower_leg_l", [-10., 0., 0.]), ("lower_leg_r", [-12., 0., 0.]), ("head", [10., 0., 0.])], hips: [0., 0., 0.] } })),
        "backflip" => (0.6, Box::new(|t: f64| { let tuck = (PI * t).sin(); Pose { rot: vec![
            ("hips", [360. * t, 0., 0.]), ("upper_leg_l", [110. * tuck, 0., 0.]), ("upper_leg_r", [110. * tuck, 0., 0.]), ("lower_leg_l", [-130. * tuck, 0., 0.]), ("lower_leg_r", [-130. * tuck, 0., 0.]),
            ("upper_arm_l", [60. * tuck, 0., -20.]), ("upper_arm_r", [60. * tuck, 0., 20.]), ("spine", [-25. * tuck, 0., 0.])], hips: [0., 0.3 * tuck, 0.] } })),
        "spin" => (0.4, Box::new(|t: f64| Pose { rot: vec![
            ("hips", [0., 360. * t, 0.]), ("upper_arm_l", [0., 0., -88.]), ("upper_arm_r", [0., 0., 88.]), ("upper_leg_l", [5., 0., -5.]), ("upper_leg_r", [5., 0., 5.])], hips: [0., 0.02, 0.] })),
        "victory" => (1.0, Box::new(|t: f64| { let w = (t * 2. * PI).sin(); Pose { rot: vec![
            ("upper_arm_r", [20., 0., 150. + 20. * w]), ("lower_arm_r", [45. + 25. * w, 0., 0.]), ("upper_arm_l", [-10., 0., -25.]), ("lower_arm_l", [70., 0., 0.]),
            ("spine", [4. * w, 0., 0.]), ("head", [-10. - 5. * w, 0., 0.])], hips: [0., 0.01 * (0.5 + 0.5 * w), 0.] } })),
        "hurt" => (0.4, Box::new(|t: f64| { let k = (PI * t).sin(); Pose { rot: vec![
            ("spine", [18. * k, 0., 0.]), ("chest", [10. * k, 0., 0.]), ("head", [25. * k, 0., 0.]), ("upper_arm_l", [-20. * k, 0., -40. * k]), ("upper_arm_r", [-20. * k, 0., 40. * k]),
            ("upper_leg_l", [-10. * k, 0., 0.]), ("lower_leg_r", [-25. * k, 0., 0.])], hips: [0., 0., 0.03 * k] } })),
        _ => return None,
    };
    let joints = humanoid_joints(height);
    let index = |n: &str| joints.iter().position(|j| j.0 == n).map(|i| i as u32);
    let hips_rest = joints[0].2;
    let samples = 24;
    let poses: Vec<(f64, Pose)> = (0..=samples).map(|k| { let t = k as f64 / samples as f64; (t * duration, pose(t)) }).collect();
    let mut channels = Vec::new();
    let mut names: Vec<&'static str> = Vec::new();
    for (_, p) in &poses { for (n, _) in &p.rot { if !names.contains(n) { names.push(n); } } }
    for n in names {
        let Some(joint) = index(n) else { continue };
        let keys = poses.iter().map(|(time, p)| Keyframe { time: *time, value: euler(p.rot.iter().find(|(m, _)| *m == n).map_or([0.; 3], |(_, e)| *e)) }).collect();
        channels.push(AnimationChannel { joint, path: AnimationPath::Rotation, keys });
    }
    let keys = poses.iter().map(|(time, p)| Keyframe { time: *time, value: [hips_rest[0] + p.hips[0] * height, hips_rest[1] + p.hips[1] * height, hips_rest[2] + p.hips[2] * height, 0.] }).collect();
    channels.push(AnimationChannel { joint: 0, path: AnimationPath::Translation, keys });
    Some(AnimationClip { name: name.into(), channels })
}
