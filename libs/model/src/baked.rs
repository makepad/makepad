//! Finished indexed geometry. Array order, material runs and floating-point bits are source data.
use crate::{canon::{Reader, Writer}, mesh, Error, Result};
use std::{ops::Range, sync::Arc};

pub type MaterialRun = (u32, Range<u32>);

#[derive(Clone, Debug, PartialEq)]
pub struct BakedMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Option<Vec<[f32; 2]>>,
    pub material_bytes: Option<Vec<[u8; 4]>>,
    pub indices: Vec<u32>,
    pub runs: Vec<MaterialRun>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ObjectGeometry {
    Editable(mesh::Mesh),
    Baked(Arc<BakedMesh>),
}

impl From<mesh::Mesh> for ObjectGeometry {
    fn from(mesh: mesh::Mesh) -> Self { Self::Editable(mesh) }
}

// Both the public editing APIs and the document use the same mesh algorithms.
pub(crate) trait MeshEntry: From<mesh::Mesh> {
    fn editable(&self) -> Option<&mesh::Mesh>;
    fn editable_mut(&mut self) -> Option<&mut mesh::Mesh>;
}
impl MeshEntry for mesh::Mesh {
    fn editable(&self) -> Option<&mesh::Mesh> { Some(self) }
    fn editable_mut(&mut self) -> Option<&mut mesh::Mesh> { Some(self) }
}
impl MeshEntry for ObjectGeometry {
    fn editable(&self) -> Option<&mesh::Mesh> {
        match self { Self::Editable(m) => Some(m), Self::Baked(_) => None }
    }
    fn editable_mut(&mut self) -> Option<&mut mesh::Mesh> {
        match self { Self::Editable(m) => Some(m), Self::Baked(_) => None }
    }
}
impl ObjectGeometry {
    pub(crate) fn memory_bytes(&self) -> usize {
        match self { Self::Editable(m) => m.memory_bytes(), Self::Baked(m) => m.memory_bytes() }
    }
}

pub(crate) struct BakedView<'a> {
    pub positions: &'a [[f32; 3]],
    pub normals: &'a [[f32; 3]],
    pub uvs: Option<&'a [[f32; 2]]>,
    pub material_bytes: Option<&'a [[u8; 4]]>,
    pub indices: &'a [u32],
    pub runs: &'a [MaterialRun],
}

pub(crate) const MAGIC: &[u8; 8] = b"MPBAKE01";

impl BakedView<'_> {
    pub(crate) fn buffer_bytes(&self) -> usize {
        use std::mem::size_of_val;
        size_of_val(self.positions) + size_of_val(self.normals) + size_of_val(self.uvs.unwrap_or_default()) +
            size_of_val(self.material_bytes.unwrap_or_default()) + size_of_val(self.indices) + 12 * self.runs.len()
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let n = self.positions.len();
        if self.buffer_bytes().saturating_add(21) > u32::MAX as usize {
            return Err(Error::Invalid("baked geometry exceeds source blob length"));
        }
        if n == 0 || n > u32::MAX as usize || self.normals.len() != n || self.uvs.is_some_and(|uvs| uvs.len() != n)
            || self.material_bytes.is_some_and(|bytes| bytes.len() != n) {
            return Err(Error::Invalid("baked vertex attribute lengths"));
        }
        if self.positions.iter().chain(self.normals).flatten().chain(self.uvs.unwrap_or_default().iter().flatten()).any(|v| !v.is_finite()) {
            return Err(Error::Invalid("non-finite baked vertex attribute"));
        }
        if self.indices.is_empty() || self.indices.iter().any(|&i| i as usize >= n) {
            return Err(Error::Invalid("baked triangle indices"));
        }
        let mut end = 0;
        for (_, range) in self.runs {
            if range.start != end || range.end <= end || range.end % 3 != 0 {
                return Err(Error::Invalid("baked material run partition"));
            }
            end = range.end;
        }
        if end as usize != self.indices.len() { return Err(Error::Invalid("baked material run coverage")); }
        Ok(())
    }

    pub(crate) fn write(&self, w: &mut Writer) -> Result<()> {
        self.validate()?;
        self.write_checked(w, || Ok(()))
    }

    pub(crate) fn write_checked(&self, w: &mut Writer, mut poll: impl FnMut() -> Result<()>) -> Result<()> {
        w.raw(MAGIC)?;
        w.count(self.positions.len())?;
        w.count(self.indices.len())?;
        w.count(self.runs.len())?;
        w.u8(u8::from(self.uvs.is_some()) | (u8::from(self.material_bytes.is_some()) << 1))?;
        let floats = self.positions.iter().chain(self.normals).flatten()
            .chain(self.uvs.unwrap_or_default().iter().flatten());
        let material_words = self.material_bytes.unwrap_or_default().iter().copied().map(u32::from_le_bytes);
        let words = floats.map(|v| v.to_bits()).chain(material_words).chain(self.indices.iter().copied())
            .chain(self.runs.iter().flat_map(|(m, range)| [*m, range.start, range.end]));
        let bytes = self.buffer_bytes();
        if w.measure_or_reserve(bytes)? { return Ok(()); }
        let mut block = [0u8; 4096];
        let mut used = 0;
        for word in words {
            block[used..used+4].copy_from_slice(&word.to_le_bytes());
            used += 4;
            if used == block.len() {
                w.raw(&block)?;
                poll()?;
                used = 0;
            }
        }
        w.raw(&block[..used])?;
        Ok(())
    }
}
impl BakedMesh {
    pub(crate) fn view(&self) -> BakedView<'_> {
        BakedView { positions: &self.positions, normals: &self.normals, uvs: self.uvs.as_deref(),
            material_bytes: self.material_bytes.as_deref(), indices: &self.indices, runs: &self.runs }
    }
    pub fn memory_bytes(&self) -> usize {
        self.positions.capacity() * 12 + self.normals.capacity() * 12 + self.indices.capacity() * 4 +
            self.uvs.as_ref().map_or(0, |v| v.capacity() * 8) +
            self.material_bytes.as_ref().map_or(0, |v| v.capacity() * 4) + self.runs.capacity() * std::mem::size_of::<MaterialRun>()
    }
    pub(crate) fn render_bytes(&self) -> usize {
        use std::mem::size_of_val;
        size_of_val(self.positions.as_slice()) + size_of_val(self.normals.as_slice()) +
            size_of_val(self.uvs.as_deref().unwrap_or_default()) + size_of_val(self.material_bytes.as_deref().unwrap_or_default()) +
            size_of_val(self.indices.as_slice())
    }
    pub(crate) fn encoded_size(&self) -> usize {
        21 + self.view().buffer_bytes()
    }
    pub(crate) fn read(bytes: &[u8], limit: usize) -> Result<Self> {
        let mut r = Reader::new(bytes);
        if r.raw(8)? != MAGIC { return Err(Error::Corrupt("baked geometry magic")); }
        let n = r.u32()? as usize;
        let i = r.u32()? as usize;
        let runs = r.u32()? as usize;
        let flags = r.u8()?;
        if flags & !3 != 0 { return Err(Error::Corrupt("baked attribute flags")); }
        let has_uv = flags & 1 != 0;
        let has_material_bytes = flags & 2 != 0;
        let vertex_bytes = 24 + 8 * usize::from(has_uv) + 4 * usize::from(has_material_bytes);
        let size = n.checked_mul(vertex_bytes)
            .and_then(|v| v.checked_add(i.checked_mul(4)?))
            .and_then(|v| v.checked_add(runs.checked_mul(12)?))
            .ok_or(Error::Corrupt("baked geometry size overflow"))?;
        if n == 0 || i == 0 || i % 3 != 0 || runs == 0 || runs > i / 3 || size != bytes.len() - r.position() {
            return Err(Error::Corrupt("baked geometry counts"));
        }
        if size > limit { return Err(Error::WorkingBytes { what: "baked geometry", needed: size, limit }); }
        fn array<T>(count: usize, mut read: impl FnMut() -> Result<T>) -> Result<Vec<T>> {
            let mut values = Vec::with_capacity(count);
            for _ in 0..count { values.push(read()?); }
            Ok(values)
        }
        let mut vectors = || array(n, || Ok([f32::from_bits(r.u32()?), f32::from_bits(r.u32()?), f32::from_bits(r.u32()?)]));
        let positions = vectors()?;
        let normals = vectors()?;
        let uvs = if has_uv {Some(array(n, || Ok([f32::from_bits(r.u32()?), f32::from_bits(r.u32()?)]))?)} else {None};
        let material_bytes = if has_material_bytes { Some(array(n, || Ok(r.raw(4)?.try_into().unwrap()))?) } else { None };
        let indices = array(i, || r.u32())?;
        let runs = array(runs, || Ok((r.u32()?, r.u32()?..r.u32()?)))?;
        r.end()?;
        let mesh = Self { positions, normals, uvs, material_bytes, indices, runs };
        mesh.view().validate()?;
        Ok(mesh)
    }
}

impl crate::document::State {
    pub(crate) fn check_editable(&self, index: usize, op: &'static str, object: &str) -> Result<()> {
        if matches!(self.objects.get(object), Some(ObjectGeometry::Baked(_))) {
            return Err(Error::BakedObject { index, op, object: object.to_owned() });
        }
        Ok(())
    }

    pub(crate) fn check_geometry_op(&self, index: usize, op: &crate::Operation) -> Result<()> {
        use crate::{Construction as C, Operation as O, RigOperation as R, SceneOperation as S};
        let kind = crate::build_detail::operation_kind(op);
        let check = |name: &str| self.check_editable(index, kind, name);
        match op {
            O::DeleteObject { .. } => return Ok(()),
            O::Scene(S::Node { object, node }) => {
                if let Some(source) = &node.linked_to { check(source)?; check(object)?; }
                return Ok(());
            }
            O::Scene(S::Snap { .. } | S::Light(_) | S::Socket(_)) => return Ok(()),
            _ => {}
        }
        if let Some(object) = op.object_name() { check(object)?; }
        match op {
            O::Construction(c) => match &c.kind {
                C::Boolean { a, b, .. } => { check(a)?; check(b)?; }
                C::Voxel { source, .. } | C::Fibers { source, .. } => check(source)?,
                C::Decal { decal, .. } => check(&decal.target)?,
                _ => {}
            },
            O::Scene(S::Separate { name, .. }) => check(name)?,
            O::Scene(S::Join { sources, .. }) => { for source in sources { check(source)?; } }
            O::Scene(S::Instance { source, .. } | S::Duplicate { source, .. }) => check(source)?,
            O::Scene(S::Collider { proxy: crate::CollisionProxy::Mesh {object}, .. }) => check(object)?,
            O::Scene(S::Modifier { modifier, .. }) => { check(modifier.operation.object_name())?; for source in modifier.operation.dependencies() { check(source)?; } }
            O::MeshEditing(m) => { for source in m.dependencies() { check(source)?; } }
            O::Selection(crate::SelectionOperation::Use { operation, .. }) => self.check_geometry_op(index, operation)?,
            O::Rig(R::CaptureMorph { target, .. }) => check(target)?,
            O::Rig(R::Weights { edit: crate::WeightEdit::Transfer { source, .. }, .. }) => check(source)?,
            O::Surface(crate::SurfaceOperation::Bake { source, .. }) => check(source)?,
            O::SoftBody(b) => {
                for name in b.deform_objects.iter().chain(b.attachments.iter().flat_map(|a| &a.objects)) { check(name)?; }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn validate_baked_references(&self) -> Result<()> {
        if self.baked_meshes().next().is_none() { return Ok(()); }
        for (name, g) in &self.objects {
            let ObjectGeometry::Baked(b) = g else { continue };
            if b.runs.iter().any(|(id, _)| !self.materials.contains_key(id)) { return Err(Error::Invalid("unknown baked material")); }
            if self.scene.modifiers.contains_key(name) || self.scene.lods.contains_key(name) || self.scene.colliders.contains_key(name)
                || self.scene.wheels.contains_key(name) || self.scene.paint.contains(name) || self.rig.locked_groups.contains_key(name)
                || self.scene.nodes.get(name).is_some_and(|node| node.linked_to.is_some()) {
                self.check_editable(0, "apply geometry metadata to", name)?;
            }
        }
        for node in self.scene.nodes.values() {
            if let Some(name) = &node.linked_to { self.check_editable(0, "link", name)?; }
        }
        for stack in self.scene.modifiers.values() {
            for m in stack { for name in m.operation.dependencies() { self.check_editable(0, "modifier", name)?; } }
        }
        for proxy in self.scene.colliders.values() {
            if let crate::CollisionProxy::Mesh { object } = proxy { self.check_editable(0, "collider", object)?; }
        }
        Ok(())
    }
}
