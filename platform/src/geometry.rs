use crate::{
    cx::Cx, draw_shader::DrawShaderInputs, id_pool::*, makepad_error_log::*, makepad_script::*,
    os::CxOsGeometry,
};
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
    /// Draw calls skipped because their geometry id was stale (see
    /// [`CxGeometryPool::skip_stale`]); reported with a power-of-ten backoff.
    pub(crate) u64,
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

impl CxGeometryPool {
    /// True (and counted) when `id` no longer names the geometry it was issued
    /// for: its slot was reclaimed and now holds a different mesh. A stale id
    /// must never be drawn — drawing the slot's current mesh with the caller's
    /// instance count is a runaway triangle count (a label quad's instances
    /// times a building mesh's index buffer), which is how a pan took the web
    /// map to 1 fps. Reported at the 1st, 10th, 100th… occurrence, never per
    /// frame.
    pub(crate) fn skip_stale(&mut self, id: GeometryId) -> bool {
        let live = self.0.pool.get(id.0).map_or(false, |d| d.generation == id.1);
        if live {
            return false;
        }
        self.2 += 1;
        let n = self.2;
        let power_of_ten = {
            let mut m = n;
            while m % 10 == 0 {
                m /= 10;
            }
            m == 1
        };
        if power_of_ten {
            error!(
                "draw call skipped: stale geometry id {} (slot generation {}, id generation {}); {} skipped so far",
                id.0,
                self.0.pool.get(id.0).map_or(0, |d| d.generation),
                id.1,
                n
            );
        }
        true
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

/// CPU vertex staging. The f32 path is the existing tightly-packed lane buffer;
/// `Bytes` is the compact fetch layout (`stride` is `CxGeometry::vertex_stride`).
#[derive(Debug, Clone)]
pub enum VertexData {
    F32(Vec<f32>),
    Bytes(Vec<u8>),
}

impl Default for VertexData {
    fn default() -> Self {
        Self::F32(Vec::new())
    }
}

impl VertexData {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::F32(v) => v.is_empty(),
            Self::Bytes(v) => v.is_empty(),
        }
    }

    pub fn clear(&mut self) {
        match self {
            Self::F32(v) => v.clear(),
            Self::Bytes(v) => v.clear(),
        }
    }

    pub fn byte_len(&self) -> usize {
        match self {
            Self::F32(v) => v.len() * 4,
            Self::Bytes(v) => v.len(),
        }
    }

    pub fn capacity_bytes(&self) -> usize {
        match self {
            Self::F32(v) => v.capacity() * 4,
            Self::Bytes(v) => v.capacity(),
        }
    }

    pub fn as_f32(&self) -> Option<&[f32]> {
        match self {
            Self::F32(v) => Some(v),
            Self::Bytes(_) => None,
        }
    }

    pub fn as_f32_mut(&mut self) -> Option<&mut Vec<f32>> {
        match self {
            Self::F32(v) => Some(v),
            Self::Bytes(_) => None,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::F32(v) => unsafe {
                std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4)
            },
            Self::Bytes(v) => v,
        }
    }

    pub fn is_f32(&self) -> bool {
        matches!(self, Self::F32(_))
    }

    pub fn vertex_count(&self, stride: usize) -> usize {
        match self {
            Self::F32(v) => {
                if stride > 4 {
                    v.len() * 4 / stride
                } else {
                    v.len()
                }
            }
            Self::Bytes(v) => {
                if stride == 0 {
                    0
                } else {
                    v.len() / stride
                }
            }
        }
    }
}

/// CPU index staging. `U32` is the existing path; `U16` is the compact path.
#[derive(Debug, Clone)]
pub enum IndexData {
    U16(Vec<u16>),
    U32(Vec<u32>),
}

impl Default for IndexData {
    fn default() -> Self {
        Self::U32(Vec::new())
    }
}

impl IndexData {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::U16(v) => v.is_empty(),
            Self::U32(v) => v.is_empty(),
        }
    }

    pub fn clear(&mut self) {
        match self {
            Self::U16(v) => v.clear(),
            Self::U32(v) => v.clear(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::U16(v) => v.len(),
            Self::U32(v) => v.len(),
        }
    }

    pub fn capacity_bytes(&self) -> usize {
        match self {
            Self::U16(v) => v.capacity() * 2,
            Self::U32(v) => v.capacity() * 4,
        }
    }

    pub fn as_u32(&self) -> Option<&[u32]> {
        match self {
            Self::U32(v) => Some(v),
            Self::U16(_) => None,
        }
    }

    pub fn as_u32_mut(&mut self) -> Option<&mut Vec<u32>> {
        match self {
            Self::U32(v) => Some(v),
            Self::U16(_) => None,
        }
    }

    pub fn as_u16(&self) -> Option<&[u16]> {
        match self {
            Self::U16(v) => Some(v),
            Self::U32(_) => None,
        }
    }

    pub fn is_u16(&self) -> bool {
        matches!(self, Self::U16(_))
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::U16(v) => unsafe {
                std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2)
            },
            Self::U32(v) => unsafe {
                std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4)
            },
        }
    }

    pub fn index_width(&self) -> usize {
        match self {
            Self::U16(_) => 2,
            Self::U32(_) => 4,
        }
    }

    fn max_index(&self) -> Option<usize> {
        match self {
            Self::U16(v) => v.iter().copied().max().map(usize::from),
            Self::U32(v) => v.iter().copied().max().map(|v| v as usize),
        }
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
        cxgeom.vertex_stride = 0;
        cxgeom.vertex_layout_signature = None;
        cxgeom.index_width = 4;
        cxgeom.dirty = true;
        cxgeom.dirty_vertices = true;
        cxgeom.dirty_indices = true;
        geometry
    }

    pub fn update(&self, cx: &mut Cx, indices: Vec<u32>, vertices: Vec<f32>) {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        cxgeom.indices = IndexData::U32(indices);
        cxgeom.vertices = VertexData::F32(vertices);
        cxgeom.index_count = cxgeom.indices.len();
        cxgeom.vertex_count = cxgeom.vertices.as_f32().map(|v| v.len()).unwrap_or(0);
        cxgeom.vertex_stride = 0;
        cxgeom.vertex_layout_signature = None;
        cxgeom.index_width = 4;
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
        let mut idx = IndexData::U32(std::mem::take(indices));
        let mut vtx = VertexData::F32(std::mem::take(vertices));
        std::mem::swap(&mut cxgeom.indices, &mut idx);
        std::mem::swap(&mut cxgeom.vertices, &mut vtx);
        if let IndexData::U32(prev) = idx {
            *indices = prev;
        }
        if let VertexData::F32(prev) = vtx {
            *vertices = prev;
        }
        cxgeom.index_count = cxgeom.indices.len();
        cxgeom.vertex_count = cxgeom.vertices.as_f32().map(|v| v.len()).unwrap_or(0);
        cxgeom.vertex_stride = 0;
        cxgeom.vertex_layout_signature = None;
        cxgeom.index_width = 4;
        indices.clear();
        vertices.clear();
        cxgeom.dirty = true;
        cxgeom.dirty_vertices = true;
        cxgeom.dirty_indices = true;
    }

    pub fn update_typed(
        &self,
        cx: &mut Cx,
        indices: IndexData,
        vertices: Vec<u8>,
        layout: &DrawShaderInputs,
    ) {
        let Ok(vertex_count) = validate_typed_geometry_data(&indices, &vertices, layout) else {
            return;
        };
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        cxgeom.index_width = indices.index_width();
        cxgeom.indices = indices;
        cxgeom.vertices = VertexData::Bytes(vertices);
        cxgeom.vertex_stride = layout.stride_bytes;
        cxgeom.vertex_layout_signature = Some(layout.layout_signature());
        cxgeom.index_count = cxgeom.indices.len();
        cxgeom.vertex_count = vertex_count;
        cxgeom.dirty = true;
        cxgeom.dirty_vertices = true;
        cxgeom.dirty_indices = true;
    }

    pub fn update_typed_with_recycled_buffers(
        &self,
        cx: &mut Cx,
        indices: &mut IndexData,
        vertices: &mut Vec<u8>,
        layout: &DrawShaderInputs,
    ) {
        let Ok(vertex_count) = validate_typed_geometry_data(indices, vertices, layout) else {
            return;
        };
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        let index_width = indices.index_width();
        let mut vtx = VertexData::Bytes(std::mem::take(vertices));
        std::mem::swap(&mut cxgeom.indices, indices);
        std::mem::swap(&mut cxgeom.vertices, &mut vtx);
        if let VertexData::Bytes(prev) = vtx {
            *vertices = prev;
        } else {
            vertices.clear();
        }
        indices.clear();
        vertices.clear();
        cxgeom.vertex_stride = layout.stride_bytes;
        cxgeom.vertex_layout_signature = Some(layout.layout_signature());
        cxgeom.index_width = index_width;
        cxgeom.index_count = cxgeom.indices.len();
        cxgeom.vertex_count = vertex_count;
        cxgeom.dirty = true;
        cxgeom.dirty_vertices = true;
        cxgeom.dirty_indices = true;
    }

    pub fn update_indices(&self, cx: &mut Cx, indices: Vec<u32>) {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        cxgeom.indices = IndexData::U32(indices);
        cxgeom.index_count = cxgeom.indices.len();
        cxgeom.index_width = 4;
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

    /// Hand the uploaded staging vectors to the caller instead of freeing
    /// them here: on the threaded web build a large `free` on the UI thread
    /// contends the allocator lock with the workers and a contended lock
    /// there is `Atomics.wait`, which the main thread may not call. The
    /// caller drops them on a worker (or gives them back with
    /// [`Geometry::restore_cpu_buffers`]). `None` when not uploaded yet or
    /// already released.
    pub fn take_cpu_buffers_if_uploaded(&self, cx: &mut Cx) -> Option<(IndexData, VertexData)> {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        if cxgeom.dirty_vertices || cxgeom.dirty_indices {
            return None;
        }
        if cxgeom.vertices.capacity_bytes() == 0 && cxgeom.indices.capacity_bytes() == 0 {
            return None;
        }
        Some((
            std::mem::take(&mut cxgeom.indices),
            std::mem::take(&mut cxgeom.vertices),
        ))
    }

    /// Put staging taken by [`Geometry::take_cpu_buffers_if_uploaded`] back
    /// (nothing could take the drop); the GPU copy is untouched.
    pub fn restore_cpu_buffers(&self, cx: &mut Cx, indices: IndexData, vertices: VertexData) {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        cxgeom.indices = indices;
        cxgeom.vertices = vertices;
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
            .capacity_bytes()
            .saturating_add(cxgeom.indices.capacity_bytes());
        cxgeom.vertices = VertexData::default();
        cxgeom.indices = IndexData::default();
        bytes
    }

    pub fn cpu_buffer_bytes(&self, cx: &Cx) -> usize {
        let cxgeom = &cx.geometries[self.geometry_id()];
        cxgeom
            .vertices
            .capacity_bytes()
            .saturating_add(cxgeom.indices.capacity_bytes())
    }
}

fn validate_typed_geometry_data(
    indices: &IndexData,
    vertices: &[u8],
    layout: &DrawShaderInputs,
) -> Result<usize, ()> {
    let stride = layout.stride_bytes;
    if stride == 0 {
        error!("typed geometry vertex stride must be non-zero; update rejected");
        return Err(());
    }
    if vertices.len() % stride != 0 {
        error!(
            "typed geometry vertex byte length {} is not a multiple of stride {}; update rejected",
            vertices.len(),
            stride
        );
        return Err(());
    }
    let vertex_count = vertices.len() / stride;
    if let Some(max_index) = indices.max_index() {
        if max_index >= vertex_count {
            error!(
                "typed geometry index {} is out of range for {} vertices; update rejected",
                max_index,
                vertex_count
            );
            return Err(());
        }
    }
    Ok(vertex_count)
}

/// Returns false (and logs once) when the bound geometry's physical layout
/// does not match the shader's geometry inputs. Callers skip the draw.
pub fn geometry_layout_matches_shader(
    geom: &mut CxGeometry,
    shader_layout: &DrawShaderInputs,
) -> bool {
    let shader_stride = shader_layout.stride_bytes;
    let matches = match geom.vertex_layout_signature {
        Some(signature) => {
            geom.vertex_stride == shader_stride && signature == shader_layout.layout_signature()
        }
        None => !shader_layout.has_compact(),
    };
    if shader_stride == 0 || matches {
        return true;
    }
    if !geom.logged_stride_mismatch {
        geom.logged_stride_mismatch = true;
        error!(
            "geometry vertex layout (stride {}, signature {:?}) does not match shader layout (stride {}, signature {}); skipping draw",
            geom.vertex_stride,
            geom.vertex_layout_signature,
            shader_stride,
            shader_layout.layout_signature()
        );
    }
    false
}

/// Backends that have not yet implemented compact fetch (Vulkan / OpenGL /
/// D3D11) log once and skip the draw.
#[allow(dead_code)]
pub fn geometry_backend_supports_typed(
    geom: &mut CxGeometry,
    backend: &str,
    shader_compact: bool,
) -> bool {
    let typed = shader_compact || geom.vertex_layout_signature.is_some() || geom.index_width == 2;
    if !typed {
        return true;
    }
    if !geom.logged_unsupported_typed {
        geom.logged_unsupported_typed = true;
        error!(
            "{}: compact vertex formats / u16 indices are not implemented; skipping draw",
            backend
        );
    }
    false
}

pub struct CxGeometry {
    pub indices: IndexData,
    pub vertices: VertexData,
    /// Bytes per vertex. 0 means the f32-lane path (`shader.total_slots * 4`).
    pub vertex_stride: usize,
    /// Exact physical typed layout. `None` is the byte-identical legacy f32 path.
    pub vertex_layout_signature: Option<u64>,
    /// Resident index element width. This survives releasing CPU staging.
    pub index_width: usize,
    /// Element counts survive releasing the CPU staging vectors so resident
    /// GPU buffers remain drawable.
    pub index_count: usize,
    pub vertex_count: usize,
    pub dirty: bool,
    pub dirty_vertices: bool,
    pub dirty_indices: bool,
    pub logged_stride_mismatch: bool,
    pub logged_unsupported_typed: bool,
    #[allow(unused)]
    pub os: CxOsGeometry,
}

impl Default for CxGeometry {
    fn default() -> Self {
        Self {
            indices: IndexData::default(),
            vertices: VertexData::default(),
            vertex_stride: 0,
            vertex_layout_signature: None,
            index_width: 4,
            index_count: 0,
            vertex_count: 0,
            dirty: false,
            dirty_vertices: false,
            dirty_indices: false,
            logged_stride_mismatch: false,
            logged_unsupported_typed: false,
            os: CxOsGeometry::default(),
        }
    }
}

#[cfg(test)]
#[test]
fn discarded_staging_preserves_resident_geometry_counts() {
    let mut geometry = CxGeometry {
        indices: IndexData::U32(vec![0, 1, 2]),
        vertices: VertexData::F32(vec![0.0; 12]),
        index_width: 4,
        index_count: 3,
        vertex_count: 12,
        ..Default::default()
    };
    Geometry::discard_cpu_buffers_inner(&mut geometry);
    assert!(geometry.indices.is_empty());
    assert!(geometry.vertices.is_empty());
    assert_eq!(geometry.index_count, 3);
    assert_eq!(geometry.vertex_count, 12);
    assert_eq!(geometry.index_width, 4);
}

#[cfg(test)]
#[test]
fn discarded_u16_staging_preserves_index_width() {
    let mut geometry = CxGeometry {
        indices: IndexData::U16(vec![0, 1, 2]),
        vertices: VertexData::Bytes(vec![0; 24]),
        vertex_stride: 8,
        vertex_layout_signature: Some(17),
        index_width: 2,
        index_count: 3,
        vertex_count: 3,
        ..Default::default()
    };
    Geometry::discard_cpu_buffers_inner(&mut geometry);
    assert!(geometry.indices.is_empty());
    assert_eq!(geometry.index_width, 2);
}

#[cfg(test)]
fn compact_test_layout() -> DrawShaderInputs {
    use crate::draw_shader::{DrawShaderAttrFormat, DrawShaderInputPacking};
    let mut layout = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
    layout.push(live_id!(pos), 2, DrawShaderAttrFormat::U16x2Norm);
    layout.push(live_id!(color), 4, DrawShaderAttrFormat::U8x4Norm);
    layout.finalize();
    layout
}

#[cfg(test)]
#[test]
fn typed_recycled_buffers_preserve_capacity_and_metadata() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let geometry = Geometry::new(&mut cx);
    let layout = compact_test_layout();
    geometry.update_typed(
        &mut cx,
        IndexData::U16(vec![0, 1, 2]),
        vec![0; layout.stride_bytes * 3],
        &layout,
    );

    let mut indices = IndexData::U16(vec![0, 2, 1]);
    let mut vertices = Vec::with_capacity(layout.stride_bytes * 8);
    vertices.resize(layout.stride_bytes * 3, 1);
    geometry.update_typed_with_recycled_buffers(
        &mut cx,
        &mut indices,
        &mut vertices,
        &layout,
    );

    let cxgeom = &cx.geometries[geometry.geometry_id()];
    assert_eq!(cxgeom.index_width, 2);
    assert_eq!(cxgeom.vertex_stride, 8);
    assert_eq!(cxgeom.vertex_layout_signature, Some(layout.layout_signature()));
    assert!(matches!(indices, IndexData::U16(_)));
    assert!(vertices.capacity() >= layout.stride_bytes * 3);
}

#[cfg(test)]
#[test]
fn invalid_typed_updates_leave_previous_geometry_untouched() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let geometry = Geometry::new(&mut cx);
    let layout = compact_test_layout();
    geometry.update_typed(
        &mut cx,
        IndexData::U16(vec![0, 1, 2]),
        vec![0; 24],
        &layout,
    );

    let empty_layout = DrawShaderInputs::new(crate::draw_shader::DrawShaderInputPacking::Attribute);
    geometry.update_typed(&mut cx, IndexData::U16(vec![]), vec![], &empty_layout);
    geometry.update_typed(&mut cx, IndexData::U16(vec![0]), vec![0; 7], &layout);
    geometry.update_typed(&mut cx, IndexData::U16(vec![3]), vec![0; 24], &layout);

    let cxgeom = &cx.geometries[geometry.geometry_id()];
    assert_eq!(cxgeom.index_count, 3);
    assert_eq!(cxgeom.vertex_count, 3);
    assert_eq!(cxgeom.vertices.byte_len(), 24);
}

#[cfg(test)]
#[test]
fn draw_validation_rejects_legacy_or_different_same_stride_compact_layouts() {
    use crate::draw_shader::{DrawShaderAttrFormat, DrawShaderInputPacking};

    let expected = compact_test_layout();
    let mut different = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
    different.push(live_id!(pos), 2, DrawShaderAttrFormat::F16x2);
    different.push(live_id!(color), 4, DrawShaderAttrFormat::U8x4Norm);
    different.finalize();
    assert_eq!(different.stride_bytes, expected.stride_bytes);
    assert_ne!(different.layout_signature(), expected.layout_signature());

    let mut typed = CxGeometry {
        vertex_stride: expected.stride_bytes,
        vertex_layout_signature: Some(expected.layout_signature()),
        ..Default::default()
    };
    assert!(!geometry_layout_matches_shader(&mut typed, &different));

    let mut legacy = CxGeometry::default();
    assert!(!geometry_layout_matches_shader(&mut legacy, &expected));

    let mut f32_layout = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
    f32_layout.push(live_id!(pos), 4, DrawShaderAttrFormat::F32x4);
    f32_layout.finalize();
    assert!(geometry_layout_matches_shader(&mut legacy, &f32_layout));
}

#[cfg(test)]
#[test]
fn staging_take_and_restore_preserve_enum_variants() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let geometry = Geometry::new(&mut cx);
    let layout = compact_test_layout();
    geometry.update_typed(
        &mut cx,
        IndexData::U16(vec![0, 1, 2]),
        vec![0; 24],
        &layout,
    );
    {
        let cxgeom = &mut cx.geometries[geometry.geometry_id()];
        cxgeom.dirty_vertices = false;
        cxgeom.dirty_indices = false;
    }
    let (indices, vertices) = geometry.take_cpu_buffers_if_uploaded(&mut cx).unwrap();
    assert!(matches!(indices, IndexData::U16(_)));
    assert!(matches!(vertices, VertexData::Bytes(_)));
    geometry.restore_cpu_buffers(&mut cx, indices, vertices);
    let cxgeom = &cx.geometries[geometry.geometry_id()];
    assert!(matches!(cxgeom.indices, IndexData::U16(_)));
    assert!(matches!(cxgeom.vertices, VertexData::Bytes(_)));
    assert_eq!(cxgeom.index_width, 2);
}
