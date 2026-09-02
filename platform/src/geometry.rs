use crate::{cx::Cx, id_pool::*, makepad_error_log::*, makepad_script::*, os::CxOsGeometry};
use std::collections::HashMap;

#[derive(Debug)]
pub struct Geometry(PoolId);

impl ScriptHandleGc for Geometry {
    fn gc(&mut self) {
        self.0.free()
    }
}

impl Geometry {
    /// A non-owning handle to an existing geometry slot. Dropping it does NOT free
    /// the shared `cx.geometries` slot (its `PoolId` carries a detached free list),
    /// so it's safe to hand out per-VM references to a Cx-owned singleton geometry.
    /// Without this, each isolate VM allocated its own copy of the standard shader
    /// geometries and freed them on teardown — leaving the Cx-global shader cache
    /// pointing at a reclaimed slot (geometry generation mismatch).
    pub fn new_borrowed(id: GeometryId) -> Self {
        Geometry(PoolId {
            id: id.0,
            generation: id.1,
            free: IdPoolFree::default(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GeometryId(usize, u64);

impl Geometry {
    pub fn geometry_id(&self) -> GeometryId {
        GeometryId(self.0.id, self.0.generation)
    }
}

impl GeometryId {
    #[allow(dead_code)]
    pub(crate) fn slot_index(self) -> usize {
        self.0
    }

    #[allow(dead_code)]
    pub(crate) fn generation(self) -> u64 {
        self.1
    }
}

#[derive(Default)]
pub struct CxGeometryPool(
    pub(crate) IdPool<CxGeometry>,
    /// Cx-owned singleton geometries (e.g. the standard quad/triangle/cube shader
    /// meshes), keyed by name. Owned here so their slots live for the whole app and
    /// are never freed by a script VM being torn down; VMs get non-owning handles
    /// via [`Geometry::new_borrowed`].
    pub(crate) HashMap<LiveId, Geometry>,
);

impl CxGeometryPool {
    pub fn alloc(&mut self) -> Geometry {
        Geometry(self.0.alloc())
    }
}

impl Cx {
    /// Return the id of a Cx-owned singleton geometry named `key`, creating it via
    /// `make` on first use. All VMs share this one slot through non-owning handles,
    /// so it is never freed by an individual VM/isolate teardown.
    pub fn shared_geometry(&mut self, key: LiveId, make: impl FnOnce(&mut Cx) -> Geometry) -> GeometryId {
        if let Some(g) = self.geometries.1.get(&key) {
            return g.geometry_id();
        }
        let geometry = make(self);
        let id = geometry.geometry_id();
        self.geometries.1.insert(key, geometry);
        id
    }
}

impl std::ops::Index<GeometryId> for CxGeometryPool {
    type Output = CxGeometry;
    fn index(&self, index: GeometryId) -> &Self::Output {
        let d = &self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "Drawlist id generation wrong {} {} {}",
                index.0, d.generation, index.1
            )
        }
        &d.item
    }
}

impl std::ops::IndexMut<GeometryId> for CxGeometryPool {
    fn index_mut(&mut self, index: GeometryId) -> &mut Self::Output {
        let d = &mut self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "Drawlist id generation wrong {} {} {}",
                index.0, d.generation, index.1
            )
        }
        &mut d.item
    }
}

impl Geometry {
    pub fn into_script_handle(self, vm: &mut ScriptVm) -> ScriptValue {
        let ty = vm.handle_type(id!(geometry));
        let handle = vm.bx.heap.new_handle(ty, Box::new(self));
        handle.into()
    }

    pub fn new(cx: &mut Cx) -> Self {
        let geometry = cx.geometries.alloc();
        let cxgeom = &mut cx.geometries[geometry.geometry_id()];
        cxgeom.indices.clear();
        cxgeom.vertices.clear();
        cxgeom.index_count = 0;
        cxgeom.vertex_count = 0;
        cxgeom.dirty = true;
        cxgeom.dirty_vertices = true;
        cxgeom.dirty_indices = true;
        geometry
    }

    pub fn update(&self, cx: &mut Cx, indices: Vec<u32>, vertices: Vec<f32>) {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        cxgeom.indices = indices;
        cxgeom.vertices = vertices;
        cxgeom.index_count = cxgeom.indices.len();
        cxgeom.vertex_count = cxgeom.vertices.len();
        cxgeom.dirty = true;
        cxgeom.dirty_vertices = true;
        cxgeom.dirty_indices = true;
    }

    /// Swap geometry buffers with caller-owned buffers without cloning.
    ///
    /// The caller receives the previous geometry buffers (cleared), preserving
    /// their capacity for re-use on subsequent frames.
    pub fn update_with_recycled_buffers(
        &self,
        cx: &mut Cx,
        indices: &mut Vec<u32>,
        vertices: &mut Vec<f32>,
    ) {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        std::mem::swap(&mut cxgeom.indices, indices);
        std::mem::swap(&mut cxgeom.vertices, vertices);
        cxgeom.index_count = cxgeom.indices.len();
        cxgeom.vertex_count = cxgeom.vertices.len();
        indices.clear();
        vertices.clear();
        cxgeom.dirty = true;
        cxgeom.dirty_vertices = true;
        cxgeom.dirty_indices = true;
    }

    pub fn update_indices(&self, cx: &mut Cx, indices: Vec<u32>) {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        cxgeom.indices = indices;
        cxgeom.index_count = cxgeom.indices.len();
        cxgeom.dirty = true;
        cxgeom.dirty_indices = true;
    }

    /// Release the CPU staging vectors once the backend has consumed them.
    /// The resident GPU buffers remain authoritative. Calling this before an
    /// upload is harmless and leaves the vectors intact.
    pub fn discard_cpu_buffers_if_uploaded(&self, cx: &mut Cx) -> usize {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        if cxgeom.dirty_vertices || cxgeom.dirty_indices {
            return 0;
        }
        Self::discard_cpu_buffers_inner(cxgeom)
    }

    /// Drop staging for geometry that is itself being evicted, including a
    /// buffer which never became visible and therefore was never uploaded.
    pub fn discard_cpu_buffers(&self, cx: &mut Cx) -> usize {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        Self::discard_cpu_buffers_inner(cxgeom)
    }

    fn discard_cpu_buffers_inner(cxgeom: &mut CxGeometry) -> usize {
        let bytes = cxgeom
            .vertices
            .capacity()
            .saturating_add(cxgeom.indices.capacity())
            .saturating_mul(4);
        cxgeom.vertices = Vec::new();
        cxgeom.indices = Vec::new();
        bytes
    }

    pub fn cpu_buffer_bytes(&self, cx: &Cx) -> usize {
        let cxgeom = &cx.geometries[self.geometry_id()];
        cxgeom
            .vertices
            .capacity()
            .saturating_add(cxgeom.indices.capacity())
            .saturating_mul(4)
    }
}

#[derive(Default)]
pub struct CxGeometry {
    pub indices: Vec<u32>,
    pub vertices: Vec<f32>,
    /// Element counts survive releasing the CPU staging vectors so resident
    /// GPU buffers remain drawable.
    pub index_count: usize,
    pub vertex_count: usize,
    pub dirty: bool,
    pub dirty_vertices: bool,
    pub dirty_indices: bool,
    #[allow(unused)]
    pub os: CxOsGeometry,
}

#[cfg(test)]
#[test]
fn discarded_staging_preserves_resident_geometry_counts() {
    let mut geometry = CxGeometry {
        indices: vec![0, 1, 2],
        vertices: vec![0.0; 12],
        index_count: 3,
        vertex_count: 12,
        ..Default::default()
    };
    Geometry::discard_cpu_buffers_inner(&mut geometry);
    assert!(geometry.indices.is_empty());
    assert!(geometry.vertices.is_empty());
    assert_eq!(geometry.index_count, 3);
    assert_eq!(geometry.vertex_count, 12);
}
