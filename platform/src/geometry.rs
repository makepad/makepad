use crate::{cx::Cx, id_pool::*, makepad_error_log::*, makepad_script::*, os::CxOsGeometry};

#[derive(Debug)]
pub struct Geometry(PoolId);

impl ScriptHandleGc for Geometry {
    fn gc(&mut self) {
        self.0.free()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryId(usize, u64);

impl Geometry {
    pub fn geometry_id(&self) -> GeometryId {
        GeometryId(self.0.id, self.0.generation)
    }
}

#[derive(Default)]
pub struct CxGeometryPool(pub(crate) IdPool<CxGeometry>);

impl CxGeometryPool {
    pub fn alloc(&mut self) -> Geometry {
        Geometry(self.0.alloc())
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
        cx.geometries[geometry.geometry_id()].indices.clear();
        cx.geometries[geometry.geometry_id()].vertices.clear();
        cx.geometries[geometry.geometry_id()].dirty = true;
        geometry
    }

    pub fn update(&self, cx: &mut Cx, indices: Vec<u32>, vertices: Vec<f32>) {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        cxgeom.indices = indices;
        cxgeom.vertices = vertices;
        cxgeom.dirty = true;
    }

    pub fn update_indices(&self, cx: &mut Cx, indices: Vec<u32>) {
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        cxgeom.indices = indices;
        cxgeom.dirty = true;
    }
}

#[derive(Default)]
pub struct CxGeometry {
    pub indices: Vec<u32>,
    pub vertices: Vec<f32>,
    pub dirty: bool,
    #[allow(unused)]
    pub os: CxOsGeometry,
}
