use crate::{Error, Result};

pub(crate) struct Writer { pub bytes: Vec<u8> }
impl Writer {
    pub fn new() -> Self { Self { bytes: Vec::new() } }
    pub fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.bytes.extend_from_slice(bytes); Ok(())
    }
    pub fn u8(&mut self, v: u8) -> Result<()> { self.raw(&[v]) }
    pub fn u32(&mut self, v: u32) -> Result<()> { self.raw(&v.to_le_bytes()) }
    pub fn u64(&mut self, v: u64) -> Result<()> { self.raw(&v.to_le_bytes()) }
    pub fn count(&mut self, v: usize) -> Result<()> { self.u32(v.try_into().map_err(|_| Error::Budget("model source u32 count"))?) }
    pub fn f64(&mut self, v: f64) -> Result<()> {
        if !v.is_finite() { return Err(Error::Invalid("non-finite number")); }
        self.u64(if v == 0.0 { 0 } else { v.to_bits() })
    }
    pub fn blob(&mut self, v: &[u8]) -> Result<()> { self.count(v.len())?; self.raw(v) }
    pub fn string(&mut self, v: &str) -> Result<()> { self.blob(v.as_bytes()) }
}

pub(crate) struct Reader<'a> { bytes: &'a [u8], pos: usize }
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self { Self { bytes, pos: 0 } }
    pub fn position(&self) -> usize { self.pos }
    pub fn raw(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.bytes.len().saturating_sub(self.pos) { return Err(Error::Corrupt("truncated source")); }
        let p = self.pos; self.pos += n; Ok(&self.bytes[p..self.pos])
    }
    pub fn u8(&mut self) -> Result<u8> { Ok(self.raw(1)?[0]) }
    pub fn u32(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.raw(4)?.try_into().unwrap())) }
    pub fn u64(&mut self) -> Result<u64> { Ok(u64::from_le_bytes(self.raw(8)?.try_into().unwrap())) }
    pub fn count(&mut self, min_record_bytes: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > self.bytes.len().saturating_sub(self.pos) / min_record_bytes.max(1) { return Err(Error::Budget("source count")); }
        Ok(n)
    }
    pub fn f64(&mut self) -> Result<f64> {
        let bits = self.u64()?; let v = f64::from_bits(bits);
        if !v.is_finite() || bits == (-0.0f64).to_bits() { return Err(Error::Corrupt("noncanonical number")); }
        Ok(v)
    }
    pub fn blob(&mut self) -> Result<&'a [u8]> { let n = self.u32()? as usize; self.raw(n) }
    pub fn string(&mut self) -> Result<String> {
        String::from_utf8(self.blob()?.to_vec()).map_err(|_| Error::Corrupt("UTF-8"))
    }
    pub fn end(&self) -> Result<()> {
        if self.pos == self.bytes.len() { Ok(()) } else { Err(Error::Corrupt("trailing bytes")) }
    }
}

/// The writer's zero rule held in memory: negative zero becomes +0, the
/// value `Writer::f64` writes. A state that holds it compiles the bytes its
/// own decoded source compiles (float bits key vertices and LOD wedges on
/// export). Every pattern below names all fields, so a new float field fails
/// to compile until it is added here.
pub(crate) trait CanonicalZeros { fn canonicalize_zeros(&mut self); }
macro_rules! zeros { ($($v:expr),* $(,)?) => {{ $( CanonicalZeros::canonicalize_zeros($v); )* }} }
impl CanonicalZeros for f64 { fn canonicalize_zeros(&mut self) { if self.to_bits() == 1 << 63 { *self = 0.0; } } }
impl CanonicalZeros for f32 { fn canonicalize_zeros(&mut self) { if self.to_bits() == 1 << 31 { *self = 0.0; } } }
impl<T: CanonicalZeros, const N: usize> CanonicalZeros for [T; N] { fn canonicalize_zeros(&mut self) { self.iter_mut().for_each(T::canonicalize_zeros) } }
impl<T: CanonicalZeros> CanonicalZeros for Vec<T> { fn canonicalize_zeros(&mut self) { self.iter_mut().for_each(T::canonicalize_zeros) } }
impl<T: CanonicalZeros> CanonicalZeros for Option<T> { fn canonicalize_zeros(&mut self) { if let Some(v) = self { v.canonicalize_zeros() } } }
impl<K, V: CanonicalZeros> CanonicalZeros for std::collections::BTreeMap<K, V> { fn canonicalize_zeros(&mut self) { self.values_mut().for_each(V::canonicalize_zeros) } }

impl CanonicalZeros for crate::document::State {
    fn canonicalize_zeros(&mut self) {
        use crate::*;
        let Self { objects, materials, skeleton, clips, scene, selections, rig, surface, soft_body } = self;
        objects.values_mut().for_each(mesh::Mesh::canonicalize_zeros);
        for Material { color, base_color_png: _ } in materials.values_mut() { zeros!(color); }
        if let Some(Skeleton { joints }) = skeleton { for Joint { name: _, parent: _, translation } in joints { zeros!(translation); } }
        for AnimationClip { name: _, channels } in clips.values_mut() {
            for AnimationChannel { joint: _, path: _, keys } in channels { for Keyframe { time, value } in keys { zeros!(time, value); } }
        }
        zeros!(scene, rig, surface, soft_body);
        let SelectionState { groups } = selections;
        for SelectionGroup { object: _, name: _, vertices: _, faces: _, stale: _ } in groups.values_mut() {}
    }
}
impl CanonicalZeros for crate::Transform {
    fn canonicalize_zeros(&mut self) { let Self { translation, rotation, scale } = self; zeros!(translation, rotation, scale); }
}
fn attachment(value: &crate::Attachment) { match value { crate::Attachment::Root | crate::Attachment::Object(_) | crate::Attachment::Joint(_) => {} } }
impl CanonicalZeros for crate::SceneState {
    fn canonicalize_zeros(&mut self) {
        use crate::*;
        let Self { nodes, emitters, sockets, lods, colliders, modifiers, wheels } = self;
        for SceneNode { parent: _, linked_to: _, transform } in nodes.values_mut() { zeros!(transform); }
        for LightEmitter { name: _, attachment: at, transform, kind, color, intensity, range } in emitters.values_mut() {
            attachment(at);
            match kind { LightKind::Point => {} LightKind::Spot { inner, outer } => zeros!(inner, outer) }
            zeros!(transform, color, intensity, range);
        }
        for Socket { name: _, attachment: at, transform } in sockets.values_mut() { attachment(at); zeros!(transform); }
        for LodLevel { target_faces: _, max_error, distance } in lods.values_mut().flatten() { zeros!(max_error, distance); }
        for proxy in colliders.values_mut() {
            match proxy { CollisionProxy::Box | CollisionProxy::Sphere | CollisionProxy::Mesh { object: _ } | CollisionProxy::Hull | CollisionProxy::Capsule => {} }
        }
        for NamedModifier { name: _, enabled: _, operation } in modifiers.values_mut().flatten() { zeros!(operation); }
        for VehicleWheel { connection: _, pivot, radius, width, visual } in wheels.values_mut() {
            zeros!(pivot, radius, width);
            if let Some(VisualWheelMotion { steer_gain, steer_max, compression, droop }) = visual { zeros!(steer_gain, steer_max, compression, droop); }
        }
    }
}
impl CanonicalZeros for crate::MeshEditingOperation {
    fn canonicalize_zeros(&mut self) {
        use crate::{mesh::{Deformation, SculptBrush}, Action::*};
        let Self { object: _, action } = self;
        match action {
            Region { faces: _, offset: v } | Array { count: _, offset: v } => zeros!(v),
            Split { edges: _, factor: v } | Collapse { edge: _, factor: v } | LoopCut { edge: _, factor: v }
            | Slide { vertices: _, towards: _, factor: v } | Relax { faces: _, iterations: _, factor: v }
            | Bevel { edges: _, width: v } | Solidify { thickness: v } | Mirror { axis: _, offset: v }
            | Pack { faces: _, padding: v } | Normals { smooth: _, angle: v } | Decimate { target_faces: _, max_error: v } => zeros!(v),
            RoundBevel { angle: a, width: b, segments: _ } | Crease { angle: a, weight: b }
            | Shrinkwrap { vertices: _, reference: _, max_distance: a, offset: b } => zeros!(a, b),
            BoxUv { faces: _, scale, offset } | CylinderUv { faces: _, axis: _, scale, offset } => zeros!(scale, offset),
            Lattice { vertices: _, bounds, divisions: _, displacements } => zeros!(bounds, displacements),
            Deform { vertices: _, deformation } => match deformation {
                Deformation::Bend { axis: _, angle: a, range } | Deformation::Twist { axis: _, angle: a, range } => zeros!(a, range),
                Deformation::Taper { axis: _, scales, range } => zeros!(scales, range),
            },
            Sculpt { vertices: _, brush: SculptBrush { kind: _, center, normal, radius, strength, max_displacement, masks, symmetry: _ } } => {
                zeros!(center, normal, radius, strength, max_displacement);
                for (_, mask) in masks { zeros!(mask); }
            }
            Dissolve { edge: _ } | Fill { vertices: _, material: _ } | Bridge { a: _, b: _, material: _ } | Subdivide { levels: _ }
            | FlipFaces { faces: _ } | Unwrap { faces: _ } | Pin { corners: _, pinned: _ } => {}
        }
    }
}
impl CanonicalZeros for crate::RigState {
    fn canonicalize_zeros(&mut self) {
        use crate::*;
        let Self { rests, poses, constraints, morphs, clip_options, locked_groups: _ } = self;
        zeros!(rests);
        for Pose { name: _, joints } in poses.values_mut() { zeros!(joints); }
        for RigConstraint { name: _, joint: _, enabled: _, kind } in constraints.values_mut() {
            match kind {
                ConstraintKind::Copy { target: _, translation: _, rotation: _, scale: _, influence } => zeros!(influence),
                ConstraintKind::Aim { target: _, axis, influence } => zeros!(axis, influence),
                ConstraintKind::Limit { min_translation, max_translation, min_scale, max_scale, max_angle } =>
                    zeros!(min_translation, max_translation, min_scale, max_scale, max_angle),
                ConstraintKind::TwoBoneIk { middle: _, end: _, target: _, pole, clamp_reach: _ } => zeros!(pole),
            }
        }
        for MorphTarget { name: _, object: _, topology: _, deltas, weight } in morphs.values_mut() { zeros!(deltas, weight); }
        for ClipOptions { interpolation: _, root_motion: _, events, morph_keys } in clip_options.values_mut() {
            for ClipEvent { time, name: _, payload: _ } in events { zeros!(time); }
            for MorphKey { time, weight } in morph_keys.values_mut().flatten() { zeros!(time, weight); }
        }
    }
}
impl CanonicalZeros for crate::SurfaceState {
    fn canonicalize_zeros(&mut self) {
        use crate::*;
        let Self { materials, vertex_colors } = self;
        zeros!(vertex_colors);
        for SurfaceMaterial { fur, shading, base_color, metallic, roughness, normal_scale, occlusion_strength, emissive,
            emissive_strength, alpha: _, alpha_cutoff, double_sided: _, channels } in materials.values_mut() {
            if let Some(GlbFurMaterial { length, density, scale, seed: _ }) = fur { zeros!(length, density, scale); }
            if let Some(GlbShading { wind, clearcoat, flake, impostor, rim, crown, plate, msdf, msdf_fill, msdf_border,
                msdf_border_width, msdf_range, msdf_plate_cells, lamp: _ }) = shading {
                zeros!(wind, clearcoat, flake, impostor, rim, crown, plate, msdf, msdf_fill, msdf_border, msdf_border_width, msdf_range, msdf_plate_cells);
            }
            zeros!(base_color, metallic, roughness, normal_scale, occlusion_strength, emissive, emissive_strength, alpha_cutoff);
            for SurfaceLayer { id: _, image: _, mask: _, opacity, blend: _, visible: _, pattern, derived, mips: _ } in channels.values_mut().flatten() {
                zeros!(opacity);
                if let Some(SurfacePattern { kind: _, color_a, color_b, scale, seed: _ }) = pattern { zeros!(color_a, color_b, scale); }
                if let Some(SurfaceDerivation { source_channel: _, source_layer: _, strength, roughness_min, roughness_max, metallic, wrap: _ }) = derived {
                    zeros!(strength, roughness_min, roughness_max, metallic);
                }
            }
        }
    }
}
impl CanonicalZeros for crate::SoftBodyMetadata {
    fn canonicalize_zeros(&mut self) {
        use crate::{SoftBodyAttachment, SoftBodyBinding, SoftBodySettings};
        let Self { version: _, object: _, deform_objects: _, root_joint: _, rest_positions, tetrahedra: _, surface_samples,
            anchors: _, settings, tet_joints: _, attachments, geometry_hash: _ } = self;
        zeros!(rest_positions);
        for SoftBodyBinding { tetrahedron: _, weights } in surface_samples { zeros!(weights); }
        let SoftBodySettings { mass, edge_compliance, volume_compliance, pose_compliance, damping, gravity, contact_radius,
            friction, substeps: _, iterations: _, max_speed, max_displacement } = settings;
        zeros!(mass, edge_compliance, volume_compliance, pose_compliance, damping, gravity, contact_radius, friction, max_speed, max_displacement);
        for SoftBodyAttachment { name: _, joint: _, binding: SoftBodyBinding { tetrahedron: _, weights }, rest_pivot, objects: _ } in attachments {
            zeros!(weights, rest_pivot);
        }
    }
}
