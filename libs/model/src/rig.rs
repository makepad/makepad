//! Persisted rest hierarchy and linear TRS animation channels. A skeleton is
//! shared by document objects. Local rest translations determine inverse bind
//! matrices; rest rotations, custom bind matrices, IK and constraints require
//! separate future contracts and are not silently approximated here.
use crate::{
    canon::{Reader, Writer},
    mesh, Error, Limits, Result,
};
use std::collections::BTreeSet;

/// Parent-local rest translation; this tranche deliberately supports identity
/// rest rotation/scale. The index is a weight/animation joint ordinal.
#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    pub name: String,
    pub parent: Option<u32>,
    pub translation: [f64; 3],
}
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnimationPath {
    Translation,
    Rotation,
    Scale,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe {
    pub time: f64,
    pub value: [f64; 4],
}
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationChannel {
    pub joint: u32,
    pub path: AnimationPath,
    pub keys: Vec<Keyframe>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    pub channels: Vec<AnimationChannel>,
}

fn name(value: &str, limits: &Limits) -> Result<()> {
    if value.is_empty()
        || value.len() > limits.max_name_bytes
        || value.chars().any(char::is_control)
    {
        Err(Error::Invalid("rig name"))
    } else {
        Ok(())
    }
}
fn render_number(v: f64) -> Result<()> {
    if !v.is_finite() || !(v as f32).is_finite() {
        Err(Error::Invalid("rig number exceeds render precision"))
    } else {
        Ok(())
    }
}
impl Skeleton {
    pub(crate) fn memory_bytes(&self) -> usize {
        self.joints
            .iter()
            .fold(0usize, |n, j| {
                n.saturating_add(std::mem::size_of::<Joint>() + j.name.len())
                    .saturating_add(j.name.len())
            })
            .saturating_mul(2)
    }
    pub fn validate(&self, limits: &Limits, ctx: &mut mesh::Context<'_>) -> Result<()> {
        if self.joints.is_empty() || self.joints.len() > limits.max_joints.min(128) {
            return Err(Error::Budget("skeleton joints"));
        }
        let mut names = BTreeSet::new();
        for (i, joint) in self.joints.iter().enumerate() {
            ctx.checkpoint(1)?;
            name(&joint.name, limits)?;
            if !names.insert(joint.name.as_str()) {
                return Err(Error::Invalid("duplicate joint name"));
            }
            match joint.parent {
                None if i == 0 => {}
                Some(parent) if (parent as usize) < i => {}
                _ => {
                    return Err(Error::Invalid(
                        "skeleton must have one root first and parents before children",
                    ))
                }
            }
            for &v in &joint.translation {
                render_number(v)?;
            }
        }
        self.global_translations()?;
        Ok(())
    }
    pub fn global_translations(&self) -> Result<Vec<[f64; 3]>> {
        if self.joints.is_empty() || self.joints.len() > 128 {
            return Err(Error::Budget("skeleton joints"));
        }
        let mut global = Vec::<[f64; 3]>::with_capacity(self.joints.len());
        for (i, j) in self.joints.iter().enumerate() {
            let mut p = j.translation;
            match j.parent {
                None if i == 0 => {}
                Some(parent) if (parent as usize) < i => {
                    for d in 0..3 {
                        p[d] += global[parent as usize][d];
                    }
                }
                _ => return Err(Error::Invalid("invalid skeleton hierarchy")),
            }
            for v in p {
                render_number(v)?;
            }
            global.push(p);
        }
        Ok(global)
    }
    /// Row-major inverse global bind transforms, derived exactly from the
    /// persisted translation-only rest hierarchy.
    pub fn inverse_bind_matrices(&self) -> Result<Vec<[[f64; 4]; 4]>> {
        Ok(self
            .global_translations()?
            .into_iter()
            .map(|p| {
                [
                    [1., 0., 0., -p[0]],
                    [0., 1., 0., -p[1]],
                    [0., 0., 1., -p[2]],
                    [0., 0., 0., 1.],
                ]
            })
            .collect())
    }
    /// Explicit deterministic rigid binding to the nearest parent-to-joint
    /// segment. Ties choose the lowest joint ordinal. This is a useful editable
    /// initial proposal, not a replacement for authored smooth weights.
    pub(crate) fn nearest_weights(
        &self,
        mesh: &mesh::Mesh,
        heads: &[[f64;3]],
        owner: crate::transform::Matrix4,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<Vec<(mesh::VertexId, Vec<mesh::JointWeight>)>> {
        if mesh
            .memory_bytes()
            .saturating_add(mesh.vertices().len().saturating_mul(128))
            > ctx.limits.max_bytes
        {
            return Err(Error::Budget("auto-weight working bytes"));
        }
        if heads.len()!=self.joints.len(){return Err(Error::Invalid("auto weights rest count"));}
        let mut out = Vec::with_capacity(mesh.vertices().len());
        for vertex in mesh.vertices() {
            let position = crate::transform::transform_point(owner, vertex.position);
            let mut best = (f64::INFINITY, 0usize);
            for (i, j) in self.joints.iter().enumerate() {
                ctx.checkpoint(1)?;
                let b = heads[i];
                let a = j.parent.map(|p| heads[p as usize]).unwrap_or(b);
                let ab = std::array::from_fn::<_, 3, _>(|d| b[d] - a[d]);
                let ap = std::array::from_fn::<_, 3, _>(|d| position[d] - a[d]);
                let den = ab.iter().map(|v| v * v).sum::<f64>();
                let t = if den == 0. {
                    0.
                } else {
                    (ab.iter().zip(ap).map(|(a, b)| a * b).sum::<f64>() / den).clamp(0., 1.)
                };
                let delta = std::array::from_fn::<_, 3, _>(|d| ap[d] - ab[d] * t);
                let distance = delta[0].hypot(delta[1]).hypot(delta[2]);
                if distance < best.0 {
                    best = (distance, i);
                }
            }
            out.push((
                vertex.id,
                vec![mesh::JointWeight {
                    joint: best.1 as u32,
                    weight: 1.,
                }],
            ));
        }
        Ok(out)
    }
}
impl AnimationClip {
    pub(crate) fn memory_bytes(&self) -> usize {
        self.channels
            .iter()
            .fold(self.name.len().saturating_mul(2), |n, c| {
                n.saturating_add(std::mem::size_of::<AnimationChannel>())
                    .saturating_add(
                        c.keys
                            .len()
                            .saturating_mul(std::mem::size_of::<Keyframe>())
                            .saturating_mul(2),
                    )
            })
    }
    pub fn duration(&self) -> f64 {
        self.channels
            .iter()
            .filter_map(|c| c.keys.last().map(|k| k.time))
            .fold(0., f64::max)
    }
    pub fn validate(
        &self,
        skeleton: &Skeleton,
        limits: &Limits,
        ctx: &mut mesh::Context<'_>,
    ) -> Result<()> {
        name(&self.name, limits)?;
        if self.channels.is_empty() || self.channels.len() > skeleton.joints.len().saturating_mul(3)
        {
            return Err(Error::Budget("animation channels"));
        }
        let mut targets = BTreeSet::new();
        let mut total = 0usize;
        for channel in &self.channels {
            ctx.checkpoint(1)?;
            if channel.joint as usize >= skeleton.joints.len() {
                return Err(Error::Invalid("animation references unknown joint"));
            }
            if !targets.insert((channel.joint, channel.path)) {
                return Err(Error::Invalid("duplicate animation target"));
            }
            total = total.saturating_add(channel.keys.len());
            if channel.keys.is_empty() || total > limits.max_keyframes {
                return Err(Error::Budget("animation keyframes"));
            }
            let mut previous = None;
            for key in &channel.keys {
                ctx.checkpoint(1)?;
                render_number(key.time)?;
                if key.time < 0.
                    || key.time > limits.max_clip_duration
                    || previous.is_some_and(|t| key.time as f32 <= t)
                {
                    return Err(Error::Invalid(
                        "key times must be increasing at f32 precision and within duration limit",
                    ));
                }
                previous = Some(key.time as f32);
                for v in key.value {
                    render_number(v)?;
                }
                match channel.path {
                    AnimationPath::Rotation => {
                        let len = key.value.iter().map(|v| v * v).sum::<f64>().sqrt();
                        if (len - 1.).abs() > 1e-6 {
                            return Err(Error::Invalid(
                                "rotation key must be a unit XYZW quaternion",
                            ));
                        }
                    }
                    AnimationPath::Translation | AnimationPath::Scale => {
                        if key.value[3] != 0. {
                            return Err(Error::Invalid("XYZ animation key padding must be zero"));
                        }
                        if channel.path == AnimationPath::Scale
                            && key.value[..3].iter().any(|v| *v <= 0.)
                        {
                            return Err(Error::Invalid("scale keys must be positive"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn write_skeleton(w: &mut Writer, skeleton: &Skeleton) -> Result<()> {
    w.count(skeleton.joints.len())?;
    for joint in &skeleton.joints {
        w.string(&joint.name)?;
        w.u32(joint.parent.unwrap_or(u32::MAX))?;
        for v in joint.translation {
            w.f64(v)?;
        }
    }
    Ok(())
}
pub(crate) fn read_skeleton(r: &mut Reader<'_>, limits: &Limits) -> Result<Skeleton> {
    let mut joints = Vec::new();
    for _ in 0..r.count(limits.max_joints.min(128))? {
        let name = r.string(limits.max_name_bytes)?;
        let parent = r.u32()?;
        joints.push(Joint {
            name,
            parent: if parent == u32::MAX {
                None
            } else {
                Some(parent)
            },
            translation: [r.f64()?, r.f64()?, r.f64()?],
        });
    }
    Ok(Skeleton { joints })
}
pub(crate) fn write_clip(w: &mut Writer, clip: &AnimationClip) -> Result<()> {
    w.string(&clip.name)?;
    w.count(clip.channels.len())?;
    for channel in &clip.channels {
        w.u32(channel.joint)?;
        w.u8(match channel.path {
            AnimationPath::Translation => 0,
            AnimationPath::Rotation => 1,
            AnimationPath::Scale => 2,
        })?;
        w.count(channel.keys.len())?;
        for key in &channel.keys {
            w.f64(key.time)?;
            for v in key.value {
                w.f64(v)?;
            }
        }
    }
    Ok(())
}
pub(crate) fn read_clip(r: &mut Reader<'_>, limits: &Limits) -> Result<AnimationClip> {
    let name = r.string(limits.max_name_bytes)?;
    let mut channels = Vec::new();
    let mut total = 0usize;
    for _ in 0..r.count(limits.max_joints.min(128).saturating_mul(3))? {
        let joint = r.u32()?;
        let path = match r.u8()? {
            0 => AnimationPath::Translation,
            1 => AnimationPath::Rotation,
            2 => AnimationPath::Scale,
            _ => return Err(Error::Corrupt("animation path")),
        };
        let count = r.count(limits.max_keyframes.saturating_sub(total))?;
        total += count;
        let mut keys = Vec::new();
        for _ in 0..count {
            keys.push(Keyframe {
                time: r.f64()?,
                value: [r.f64()?, r.f64()?, r.f64()?, r.f64()?],
            });
        }
        channels.push(AnimationChannel { joint, path, keys });
    }
    Ok(AnimationClip { name, channels })
}
