use crate::Vec3f;

fn norm3(v: Vec3f) -> Vec3f {
    let length = v.length();
    if length < 1.0e-9 {
        v
    } else {
        v / length
    }
}

/// Solve a two-bone chain in model space. Returns the knee, the clamped end,
/// and whether the target lies in the inclusive geometric reach of the chain.
/// A zero pole preserves the bend toward `rest_knee`, as in a posed skeleton.
/// The segment lengths must admit the solve's 1e-4 inward reach margin.
pub fn two_bone_knee(
    a: Vec3f,
    target: Vec3f,
    l1: f32,
    l2: f32,
    pole: Vec3f,
    rest_knee: Vec3f,
) -> (Vec3f, Vec3f, bool) {
    let to = target - a;
    let distance = to.length();
    let d = distance.clamp((l1 - l2).abs() + 1.0e-4, l1 + l2 - 1.0e-4);
    let dt = norm3(to);
    let pole = if pole.length() < 1.0e-6 {
        norm3(rest_knee - (a + dt * (rest_knee - a).dot(dt)))
    } else {
        pole
    };
    let pn = norm3(pole - dt * pole.dot(dt));
    let cos_a = ((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0);
    let knee = a + (dt * (l1 * cos_a) + pn * (l1 * (1.0 - cos_a * cos_a).max(0.0).sqrt()));
    let end_pos = a + dt * d;
    (knee, end_pos, distance >= (l1 - l2).abs() && distance <= l1 + l2)
}
