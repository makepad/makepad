//! Generic drawable items: what a Scene3D document, a kit or a kernel puts in
//! the world, beside the game features (entities, parts, terrain) Sandbox
//! fills. Handles are session-resident ids keyed by content in memory; the
//! renderer resolves them, the scene only carries them.
use makepad_math::*;
use std::sync::Arc;
use crate::material::MaterialId;

/// Resident geometry (a built mesh, 3D text, a primitive).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GeometryId(pub u64);
/// A kernel- or physics-written buffer slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferId(pub u64);
/// A reflected record layout (vertex, instance or line record).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayoutId(pub u64);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextureRef(pub u64);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelRef(pub u64);
/// A sampled pose of a model (clip, time, blend), resolved by the animator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PoseRef(pub u64);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BodyGroupRef(pub u64);

/// A published buffer generation: a reader only ever sees a complete one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BufferRef {
    pub buffer: BufferId,
    pub generation: u64,
    pub layout: LayoutId,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topology {
    /// Indices are validated against the vertex count before publish.
    Triangles { indices: Option<BufferRef> },
    Lines,
    Points,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryRef {
    Resident(GeometryId),
    Kernel { vertices: BufferRef, topology: Topology },
}

/// Per-instance records for `Instances` and `Points`.
#[derive(Clone, Debug, PartialEq)]
pub enum InstanceSource {
    /// Small, CPU-built (kits, document eval).
    Packed { data: Arc<[f32]>, layout: LayoutId },
    /// Written in place by a kernel.
    Kernel(BufferRef),
    /// Physics body transforms, through the same buffer path.
    Bodies(BodyGroupRef),
}

/// Fat polylines or segments.
#[derive(Clone, Debug, PartialEq)]
pub enum LineSource {
    /// xyz points, one polyline per `breaks` range (end indices, exclusive).
    Packed { points: Arc<[Vec3f]>, breaks: Arc<[u32]> },
    Kernel(BufferRef),
}

/// Per-item switches. Bits 16..24 are eight user layer bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemFlags(pub u32);

impl ItemFlags {
    pub const CAST_SHADOW: Self = Self(1 << 0);
    pub const RECEIVE_SHADOW: Self = Self(1 << 1);
    pub const OUTLINE: Self = Self(1 << 2);
    pub const NO_FOG: Self = Self(1 << 3);
    pub const CAMERA_FACING: Self = Self(1 << 4);
    pub const VIEWMODEL: Self = Self(1 << 5);
    pub const LAYER_SHIFT: u32 = 16;
    pub const LAYER_MASK: u32 = 0xff << Self::LAYER_SHIFT;

    pub const fn empty() -> Self { Self(0) }
    pub const fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
    pub fn insert(&mut self, other: Self) { self.0 |= other.0; }
    pub fn remove(&mut self, other: Self) { self.0 &= !other.0; }
    /// User layer bit `n` (0..8).
    pub const fn layer(n: u32) -> Self { Self(1 << (Self::LAYER_SHIFT + (n & 7))) }
    /// Whether any of this item's layers is in `mask` (a camera's layer
    /// mask); an item on no layer is always visible.
    pub const fn visible_in(self, mask: u8) -> bool {
        let layers = (self.0 & Self::LAYER_MASK) >> Self::LAYER_SHIFT;
        layers == 0 || layers & mask as u32 != 0
    }
}

impl Default for ItemFlags {
    fn default() -> Self { Self(Self::CAST_SHADOW.0 | Self::RECEIVE_SHADOW.0) }
}

impl std::ops::BitOr for ItemFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

/// Per-model material replacements, by the model's material slot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaterialOverrides {
    pub all: Option<MaterialId>,
    pub slots: Vec<(u32, MaterialId)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    Mesh { geometry: GeometryRef, material: MaterialId, transform: Mat4f },
    Instances { geometry: GeometryRef, material: MaterialId, source: InstanceSource, count: u32 },
    Lines { source: LineSource, material: MaterialId },
    /// Sprites and particles.
    Points { source: InstanceSource, material: MaterialId },
    /// Any 2D picture in the world: a Motion layer, a chart, a video frame.
    Card { texture: TextureRef, size: Vec2f, transform: Mat4f, material: MaterialId },
    Model { model: ModelRef, pose: Option<PoseRef>, transform: Mat4f, overrides: MaterialOverrides },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub flags: ItemFlags,
    /// Last frame's transform, for realtime velocity; `None` in locked time
    /// and for items that did not move.
    pub prev_transform: Option<Mat4f>,
    /// How far vertex hooks or kernels may move geometry past its authored
    /// bounds, in world units. Culling and shadow fitting grow by it.
    pub bounds_pad: f32,
}

impl Item {
    pub fn new(kind: ItemKind) -> Self {
        Self { kind, flags: ItemFlags::default(), prev_transform: None, bounds_pad: 0.0 }
    }

    pub fn transform(&self) -> Option<&Mat4f> {
        match &self.kind {
            ItemKind::Mesh { transform, .. } | ItemKind::Card { transform, .. } | ItemKind::Model { transform, .. } => Some(transform),
            _ => None,
        }
    }

    /// The materials this item draws with.
    pub fn materials(&self) -> impl Iterator<Item = MaterialId> + '_ {
        let single = match &self.kind {
            ItemKind::Mesh { material, .. } | ItemKind::Instances { material, .. } | ItemKind::Lines { material, .. }
            | ItemKind::Points { material, .. } | ItemKind::Card { material, .. } => Some(*material),
            ItemKind::Model { overrides, .. } => overrides.all,
        };
        let slots = match &self.kind {
            ItemKind::Model { overrides, .. } => overrides.slots.as_slice(),
            _ => &[],
        };
        single.into_iter().chain(slots.iter().map(|(_, m)| *m))
    }

    /// The buffers this item reads, for fences and generation checks.
    pub fn buffers(&self) -> impl Iterator<Item = &BufferRef> + '_ {
        let geometry = match &self.kind {
            ItemKind::Mesh { geometry, .. } | ItemKind::Instances { geometry, .. } => Some(geometry),
            _ => None,
        };
        let (vertices, indices) = match geometry {
            Some(GeometryRef::Kernel { vertices, topology }) => (Some(vertices), match topology {
                Topology::Triangles { indices } => indices.as_ref(),
                _ => None,
            }),
            _ => (None, None),
        };
        let source = match &self.kind {
            ItemKind::Instances { source: InstanceSource::Kernel(b), .. } | ItemKind::Points { source: InstanceSource::Kernel(b), .. }
            | ItemKind::Lines { source: LineSource::Kernel(b), .. } => Some(b),
            _ => None,
        };
        vertices.into_iter().chain(indices).chain(source)
    }

    /// Local bounds grown by `bounds_pad` (the box culling and shadow
    /// fitting use for this item).
    pub fn padded_bounds(&self, min: Vec3f, max: Vec3f) -> (Vec3f, Vec3f) {
        let pad = if self.bounds_pad.is_finite() { self.bounds_pad.max(0.0) } else { 0.0 };
        let p = vec3f(pad, pad, pad);
        (min - p, max + p)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let finite = |m: &Mat4f| m.v.iter().all(|v| v.is_finite());
        if let Some(t) = self.transform() {
            if !finite(t) { return Err("item transform must be finite"); }
        }
        if self.prev_transform.as_ref().is_some_and(|t| !finite(t)) {
            return Err("item prev_transform must be finite");
        }
        if !self.bounds_pad.is_finite() || self.bounds_pad < 0.0 {
            return Err("bounds_pad must be finite and non-negative");
        }
        match &self.kind {
            ItemKind::Card { size, .. } if !(size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0) => {
                return Err("a card needs a positive size");
            }
            ItemKind::Instances { source: InstanceSource::Kernel(b), count, .. } if *count > b.count => {
                return Err("instance count exceeds the published buffer's records");
            }
            ItemKind::Lines { source: LineSource::Packed { points, breaks }, .. } => {
                let mut last = 0;
                for &end in breaks.iter() {
                    if end < last || end as usize > points.len() {
                        return Err("line breaks must ascend within the point list");
                    }
                    last = end;
                }
                if points.iter().any(|p| !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite())) {
                    return Err("line points must be finite");
                }
            }
            ItemKind::Instances { source: InstanceSource::Packed { data, .. }, .. } | ItemKind::Points { source: InstanceSource::Packed { data, .. }, .. } => {
                if data.iter().any(|v| !v.is_finite()) {
                    return Err("packed instance data must be finite");
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> Item {
        Item::new(ItemKind::Mesh { geometry: GeometryRef::Resident(GeometryId(3)), material: MaterialId(7), transform: Mat4f::identity() })
    }

    #[test]
    fn flags_default_to_shadowed_and_layers_filter() {
        let mut f = ItemFlags::default();
        assert!(f.contains(ItemFlags::CAST_SHADOW) && f.contains(ItemFlags::RECEIVE_SHADOW));
        assert!(f.visible_in(0), "no layer: always visible");
        f.insert(ItemFlags::layer(2));
        assert!(f.visible_in(0b100) && !f.visible_in(0b011));
        f.remove(ItemFlags::CAST_SHADOW);
        assert!(!f.contains(ItemFlags::CAST_SHADOW));
        assert!((ItemFlags::OUTLINE | ItemFlags::NO_FOG).contains(ItemFlags::NO_FOG));
    }

    #[test]
    fn padding_grows_bounds_and_is_validated() {
        let mut i = mesh();
        i.bounds_pad = 0.5;
        let (a, b) = i.padded_bounds(vec3f(0.0, 0.0, 0.0), vec3f(1.0, 1.0, 1.0));
        assert_eq!((a, b), (vec3f(-0.5, -0.5, -0.5), vec3f(1.5, 1.5, 1.5)));
        assert!(i.validate().is_ok());
        i.bounds_pad = -1.0;
        assert!(i.validate().is_err());
    }

    #[test]
    fn materials_and_buffers_are_enumerated() {
        assert_eq!(mesh().materials().collect::<Vec<_>>(), vec![MaterialId(7)]);
        let b = |id| BufferRef { buffer: BufferId(id), generation: 1, layout: LayoutId(0), count: 10 };
        let i = Item::new(ItemKind::Instances {
            geometry: GeometryRef::Kernel { vertices: b(1), topology: Topology::Triangles { indices: Some(b(2)) } },
            material: MaterialId(1), source: InstanceSource::Kernel(b(3)), count: 10,
        });
        assert_eq!(i.buffers().map(|b| b.buffer.0).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(i.validate().is_ok());
        let over = Item::new(ItemKind::Instances { geometry: GeometryRef::Resident(GeometryId(0)), material: MaterialId(1), source: InstanceSource::Kernel(b(3)), count: 11 });
        assert!(over.validate().is_err());
        let model = Item::new(ItemKind::Model { model: ModelRef(1), pose: None, transform: Mat4f::identity(),
            overrides: MaterialOverrides { all: None, slots: vec![(0, MaterialId(4)), (2, MaterialId(5))] } });
        assert_eq!(model.materials().collect::<Vec<_>>(), vec![MaterialId(4), MaterialId(5)]);
    }

    #[test]
    fn line_breaks_and_cards_are_checked() {
        let pts: Arc<[Vec3f]> = vec![Vec3f::default(); 4].into();
        let ok = Item::new(ItemKind::Lines { source: LineSource::Packed { points: pts.clone(), breaks: vec![2, 4].into() }, material: MaterialId(0) });
        assert!(ok.validate().is_ok());
        let bad = Item::new(ItemKind::Lines { source: LineSource::Packed { points: pts, breaks: vec![3, 2].into() }, material: MaterialId(0) });
        assert!(bad.validate().is_err());
        let card = Item::new(ItemKind::Card { texture: TextureRef(1), size: vec2f(0.0, 1.0), transform: Mat4f::identity(), material: MaterialId(0) });
        assert!(card.validate().is_err());
    }
}
