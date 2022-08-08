use {
    std::{
        rc::Rc,
        hash::{Hash, Hasher},
    },
    crate::{
        id_pool::*,
        makepad_error_log::*,
        makepad_live_compiler::{
            LiveType,
            LiveId,
        },
        makepad_shader_compiler::ShaderTy,
        os::CxOsGeometry,
        cx::Cx,
    }
};


#[derive(Debug)]
pub struct Geometry(PoolId);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryId(usize, u64);

impl Geometry{
    pub fn geometry_id(&self)->GeometryId{GeometryId(self.0.id, self.0.generation)}
}

#[derive(Default)]
pub struct CxGeometryPool(IdPool<CxGeometry>);

impl CxGeometryPool{
    pub fn alloc(&mut self)->Geometry{
        Geometry(self.0.alloc())
    }
}

impl std::ops::Index<GeometryId> for CxGeometryPool{
    type Output = CxGeometry;
    fn index(&self, index: GeometryId) -> &Self::Output{
        let d = &self.0.pool[index.0];
        if d.generation != index.1{
            error!("Drawlist id generation wrong {} {} {}", index.0, d.generation, index.1)
        }
        &d.item
    }
}

impl std::ops::IndexMut<GeometryId> for CxGeometryPool{
    fn index_mut(&mut self, index: GeometryId) -> &mut Self::Output{
        let d = &mut self.0.pool[index.0];
        if d.generation != index.1{
            error!("Drawlist id generation wrong {} {} {}", index.0, d.generation, index.1)
        }
        &mut d.item
    }
}

#[derive(Clone, Debug)]
pub struct GeometryRef(pub Rc<Geometry>);


const MAX_GEOM_FINGERPRINT:usize = 16;
#[derive(Clone, Debug)]
pub struct GeometryFingerprint {
    pub live_type: LiveType,
    pub inputs_stored:usize,
    pub inputs: [f32;MAX_GEOM_FINGERPRINT]
}

impl GeometryFingerprint{
    pub fn new(live_type:LiveType)->Self{Self{live_type, inputs_stored:0, inputs:[0f32;MAX_GEOM_FINGERPRINT]}}
    pub fn push(&mut self, f:f32){self.inputs[self.inputs_stored] = f;self.inputs_stored += 1;}
}

impl Hash for GeometryFingerprint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.live_type.hash(state);
        for i in 0..self.inputs_stored{
            self.inputs[i].to_bits().hash(state);
        }
    }
}

impl PartialEq for GeometryFingerprint {
    fn eq(&self, other: &Self) -> bool {
        if self.inputs_stored != other.inputs_stored{
            return false
        }
        for i in 0..self.inputs_stored{
            if self.inputs[i] != other.inputs[i]{
                return false
            }
        }
        self.live_type == other.live_type
    }
}
impl Eq for GeometryFingerprint {}

impl Cx{
    
    pub fn get_geometry_ref(&mut self, fingerprint:GeometryFingerprint)->GeometryRef{
        // we have a finger print, now we need to find a geometry that has this fingerprint
        if let Some(gr) = self.geometries_refs.get(&fingerprint){
            if let Some(gr) = gr.upgrade(){
                return GeometryRef(gr)
            }
        }
        let geometry = Rc::new(Geometry::new(self));
        let weak = Rc::downgrade(&geometry);
        self.geometries_refs.insert(fingerprint, weak);
        GeometryRef(geometry)
    }
    
   
}

impl Geometry{
    pub fn new(cx: &mut Cx) -> Self {
        let geometry = cx.geometries.alloc();
        cx.geometries[geometry.geometry_id()].indices.clear();
        cx.geometries[geometry.geometry_id()].vertices.clear();
        cx.geometries[geometry.geometry_id()].dirty = true;
        geometry
    }
    
    pub fn update(&self, cx:&mut Cx, indices: Vec<u32>, vertices: Vec<f32>){
        let cxgeom = &mut cx.geometries[self.geometry_id()];
        cxgeom.indices = indices;
        cxgeom.vertices = vertices;
        cxgeom.dirty = true;
    }
}

#[derive(Default)]
pub struct CxGeometry{
    pub indices: Vec<u32>,
    pub vertices: Vec<f32>,
    pub dirty: bool,
    pub os: CxOsGeometry
}


#[derive(Debug)]
pub struct GeometryField {
    pub id: LiveId,
    pub ty: ShaderTy
}

pub trait GeometryFields{
    fn geometry_fields(&self, fields: &mut Vec<GeometryField>);
    fn live_type_check(&self)->LiveType;
    fn get_geometry_id(&self)->Option<GeometryId>;
}


