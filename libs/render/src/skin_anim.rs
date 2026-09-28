//! Pose-space animation tools on top of [`SkinnedModel`]: model-space node
//! frames, per-node weighted masks, additive layers, analytic two-bone IK,
//! look-at chains and phase-synchronised 1D blend spaces. Everything works
//! on caller-owned [`PoseBuffer`]s; per-entity animation state (phases,
//! timers, smoothed IK offsets) belongs to the caller, as with the rest of
//! this module.
use super::*;

/// A rotation+translation frame in model space (skeleton scale is carried
/// by the local TRS and applied when positions are composed).
#[derive(Clone, Copy, Debug)]
pub struct NodeFrame {
    pub r: Quat,
    pub t: Vec3f,
    pub s: f32,
}
impl Default for NodeFrame { fn default() -> Self { Self { r: Quat::default(), t: Vec3f::default(), s: 1.0 } } }

pub fn quat_mul(a: Quat, b: Quat) -> Quat {
    Quat {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    }
}
pub fn quat_conj(q: Quat) -> Quat { Quat { x: -q.x, y: -q.y, z: -q.z, w: q.w } }
pub fn quat_rotate(q: Quat, v: Vec3f) -> Vec3f {
    let p = quat_mul(quat_mul(q, Quat { x: v.x, y: v.y, z: v.z, w: 0.0 }), quat_conj(q));
    Vec3f { x: p.x, y: p.y, z: p.z }
}
pub fn quat_axis_angle(axis: Vec3f, angle: f32) -> Quat {
    let l = (axis.x * axis.x + axis.y * axis.y + axis.z * axis.z).sqrt();
    if l < 1.0e-9 { return Quat::default(); }
    let (s, c) = (angle * 0.5).sin_cos();
    Quat { x: axis.x / l * s, y: axis.y / l * s, z: axis.z / l * s, w: c }
}
/// Shortest rotation taking direction `a` onto `b`.
pub fn quat_from_to(a: Vec3f, b: Vec3f) -> Quat {
    let (a, b) = (norm3(a), norm3(b));
    let d = a.x * b.x + a.y * b.y + a.z * b.z;
    if d > 0.999_999 { return Quat::default(); }
    if d < -0.999_999 {
        let axis = if a.x.abs() < 0.9 { cross3(a, Vec3f { x: 1.0, y: 0.0, z: 0.0 }) } else { cross3(a, Vec3f { x: 0.0, y: 1.0, z: 0.0 }) };
        return quat_axis_angle(axis, std::f32::consts::PI);
    }
    let c = cross3(a, b);
    let q = Quat { x: c.x, y: c.y, z: c.z, w: 1.0 + d };
    let l = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    Quat { x: q.x / l, y: q.y / l, z: q.z / l, w: q.w / l }
}
/// Scale a rotation's angle by `k` (0 = identity, 1 = unchanged).
pub fn quat_scale(q: Quat, k: f32) -> Quat { nlerp(Quat::default(), q, k) }
fn norm3(v: Vec3f) -> Vec3f { let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt(); if l < 1.0e-9 { v } else { Vec3f { x: v.x / l, y: v.y / l, z: v.z / l } } }
fn cross3(a: Vec3f, b: Vec3f) -> Vec3f { Vec3f { x: a.y * b.z - a.z * b.y, y: a.z * b.x - a.x * b.z, z: a.x * b.y - a.y * b.x } }
fn add3(a: Vec3f, b: Vec3f) -> Vec3f { Vec3f { x: a.x + b.x, y: a.y + b.y, z: a.z + b.z } }
fn sub3(a: Vec3f, b: Vec3f) -> Vec3f { Vec3f { x: a.x - b.x, y: a.y - b.y, z: a.z - b.z } }
fn scale3(a: Vec3f, s: f32) -> Vec3f { Vec3f { x: a.x * s, y: a.y * s, z: a.z * s } }
fn len3(a: Vec3f) -> f32 { (a.x * a.x + a.y * a.y + a.z * a.z).sqrt() }
fn dot3(a: Vec3f, b: Vec3f) -> f32 { a.x * b.x + a.y * b.y + a.z * b.z }

/// One entry of a 1D blend space: the parameter value (usually ground
/// speed) at which `clip` plays at rate 1.
#[derive(Clone, Copy, Debug)]
pub struct BlendPoint { pub at: f32, pub clip: usize }

impl SkinnedModel {
    /// Model-space frame of every node for `pose` (the mesh node's frame is
    /// not removed: callers comparing against skinned geometry use
    /// [`Self::node_mesh_transform`]).
    pub fn node_frames(&self, pose: &PoseBuffer, out: &mut Vec<NodeFrame>) {
        out.clear();
        out.resize(self.nodes.len(), NodeFrame::default());
        let mut done = vec![false; self.nodes.len()];
        fn visit(nodes: &[Node], pose: &PoseBuffer, out: &mut [NodeFrame], done: &mut [bool], i: usize) {
            if done[i] { return; }
            let local = pose.get(i).copied().unwrap_or(nodes[i].rest);
            let frame = match nodes[i].parent {
                Some(p) => {
                    visit(nodes, pose, out, done, p);
                    let pf = out[p];
                    NodeFrame { r: quat_mul(pf.r, local.r), t: add3(pf.t, quat_rotate(pf.r, scale3(local.t, pf.s))), s: pf.s * local.s.x }
                }
                None => NodeFrame { r: local.r, t: local.t, s: local.s.x },
            };
            out[i] = frame;
            done[i] = true;
        }
        for i in 0..self.nodes.len() { visit(&self.nodes, pose, out, &mut done, i); }
    }

    /// Set a node's model-space rotation, given its parent's model frame.
    fn set_global_rotation(&self, pose: &mut PoseBuffer, frames: &[NodeFrame], node: usize, global: Quat) {
        let parent = self.nodes[node].parent.map(|p| frames[p].r).unwrap_or_default();
        pose[node].r = quat_mul(quat_conj(parent), global);
    }

    /// Per-node weights for a subtree: 1 inside `root`'s subtree, 0 outside.
    pub fn subtree_weights(&self, root: usize) -> Vec<f32> {
        self.descendant_mask(root).map(|m| m.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect()).unwrap_or_else(|| vec![0.0; self.nodes.len()])
    }

    /// `out = a*(1-w_i) + b*w_i` with a weight per node.
    pub fn blend_pose_weighted(a: &PoseBuffer, b: &PoseBuffer, weights: &[f32], out: &mut PoseBuffer) {
        out.clear();
        for (i, (pa, pb)) in a.iter().zip(b.iter()).enumerate() {
            let w = weights.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            out.push(NodeTrs {
                t: Vec3f { x: pa.t.x + (pb.t.x - pa.t.x) * w, y: pa.t.y + (pb.t.y - pa.t.y) * w, z: pa.t.z + (pb.t.z - pa.t.z) * w },
                r: nlerp(pa.r, pb.r, w),
                s: Vec3f { x: pa.s.x + (pb.s.x - pa.s.x) * w, y: pa.s.y + (pb.s.y - pa.s.y) * w, z: pa.s.z + (pb.s.z - pa.s.z) * w },
            });
        }
    }

    /// Additive layer: apply `layer - reference` (per node, in local space)
    /// onto `pose` at `weight`, optionally per-node weighted.
    pub fn add_pose(pose: &mut PoseBuffer, layer: &PoseBuffer, reference: &PoseBuffer, weight: f32, weights: Option<&[f32]>) {
        for (i, p) in pose.iter_mut().enumerate() {
            let (Some(l), Some(r)) = (layer.get(i), reference.get(i)) else { break };
            let w = weight * weights.and_then(|m| m.get(i).copied()).unwrap_or(1.0);
            if w <= 0.0 { continue; }
            let delta = quat_scale(quat_mul(l.r, quat_conj(r.r)), w);
            p.r = quat_mul(delta, p.r);
            p.t = add3(p.t, scale3(sub3(l.t, r.t), w));
        }
    }

    /// Sample a 1D blend space (clips ordered by `at`) at `param`, all
    /// clips at the same normalised `phase` (0..1), into `out`.
    pub fn sample_blend_space(&self, points: &[BlendPoint], param: f32, phase: f32, out: &mut PoseBuffer, scratch: &mut PoseBuffer) -> f32 {
        let Some(first) = points.first() else { return 1.0 };
        let dur = |c: usize| self.clips.get(c).map_or(1.0, |c| c.duration.max(1.0e-3));
        if points.len() == 1 || param <= first.at {
            self.sample_clip(first.clip, phase * dur(first.clip), out);
            return dur(first.clip);
        }
        let last = points[points.len() - 1];
        if param >= last.at {
            self.sample_clip(last.clip, phase * dur(last.clip), out);
            return dur(last.clip);
        }
        let i = (0..points.len() - 1).find(|&i| param < points[i + 1].at).unwrap_or(points.len() - 2);
        let (a, b) = (points[i], points[i + 1]);
        let w = ((param - a.at) / (b.at - a.at).max(1.0e-6)).clamp(0.0, 1.0);
        self.sample_clip(a.clip, phase * dur(a.clip), scratch);
        self.sample_clip(b.clip, phase * dur(b.clip), out);
        for (o, a) in out.iter_mut().zip(scratch.iter()) {
            o.t = Vec3f { x: a.t.x + (o.t.x - a.t.x) * w, y: a.t.y + (o.t.y - a.t.y) * w, z: a.t.z + (o.t.z - a.t.z) * w };
            o.r = nlerp(a.r, o.r, w);
            o.s = Vec3f { x: a.s.x + (o.s.x - a.s.x) * w, y: a.s.y + (o.s.y - a.s.y) * w, z: a.s.z + (o.s.z - a.s.z) * w };
        }
        dur(a.clip) + (dur(b.clip) - dur(a.clip)) * w
    }

    /// Analytic two-bone IK in model space: moves `end` onto `target`,
    /// bending the chain toward `pole` (a model-space direction). Bone
    /// lengths are preserved; out-of-reach targets straighten the chain.
    /// `weight` blends from the input pose. Returns false for a bad chain.
    pub fn two_bone_ik(&self, pose: &mut PoseBuffer, frames: &mut Vec<NodeFrame>, upper: usize, lower: usize, end: usize, target: Vec3f, pole: Vec3f, weight: f32) -> bool {
        if weight <= 0.0 || upper >= pose.len() || lower >= pose.len() || end >= pose.len() { return false; }
        self.node_frames(pose, frames);
        let (a, b, c) = (frames[upper].t, frames[lower].t, frames[end].t);
        let (l1, l2) = (len3(sub3(b, a)), len3(sub3(c, b)));
        if l1 < 1.0e-6 || l2 < 1.0e-6 { return false; }
        let to = sub3(target, a);
        let d = len3(to).clamp((l1 - l2).abs() + 1.0e-4, l1 + l2 - 1.0e-4);
        let dt = norm3(to);
        let pole = if len3(pole) < 1.0e-6 { norm3(sub3(b, add3(a, scale3(dt, dot3(sub3(b, a), dt))))) } else { pole };
        let pn = norm3(sub3(pole, scale3(dt, dot3(pole, dt))));
        let cos_a = ((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0);
        let knee = add3(a, add3(scale3(dt, l1 * cos_a), scale3(pn, l1 * (1.0 - cos_a * cos_a).max(0.0).sqrt())));
        let end_pos = add3(a, scale3(dt, d));
        let (orig_u, orig_l) = (pose[upper].r, pose[lower].r);
        let g_upper = quat_mul(quat_from_to(sub3(b, a), sub3(knee, a)), frames[upper].r);
        self.set_global_rotation(pose, frames, upper, g_upper);
        self.node_frames(pose, frames);
        let (b2, c2) = (frames[lower].t, frames[end].t);
        let g_lower = quat_mul(quat_from_to(sub3(c2, b2), sub3(end_pos, b2)), frames[lower].r);
        self.set_global_rotation(pose, frames, lower, g_lower);
        if weight < 1.0 {
            pose[upper].r = nlerp(orig_u, pose[upper].r, weight);
            pose[lower].r = nlerp(orig_l, pose[lower].r, weight);
        }
        true
    }

    /// Turn a chain of nodes (e.g. neck, head) so the last node's
    /// model-space `forward` (in that node's rest frame) points at `target`.
    /// Each node takes its share of the rotation, capped at `max_angle`
    /// radians in total; `weight` fades the whole effect.
    pub fn look_at(&self, pose: &mut PoseBuffer, frames: &mut Vec<NodeFrame>, chain: &[(usize, f32)], forward: Vec3f, target: Vec3f, max_angle: f32, weight: f32) {
        let Some(&(tip, _)) = chain.last() else { return };
        if weight <= 0.0 || tip >= pose.len() { return; }
        for &(node, share) in chain {
            if node >= pose.len() { continue; }
            self.node_frames(pose, frames);
            let f = quat_rotate(frames[tip].r, forward);
            let to = sub3(target, frames[tip].t);
            if len3(to) < 1.0e-4 { return; }
            let full = quat_from_to(f, to);
            let angle = 2.0 * full.w.clamp(-1.0, 1.0).acos();
            let limit = if angle > max_angle { max_angle / angle } else { 1.0 };
            let step = quat_scale(full, share * limit * weight);
            let g = quat_mul(step, frames[node].r);
            self.set_global_rotation(pose, frames, node, g);
        }
    }
}
