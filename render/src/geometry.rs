pub use {
    std::{
        rc::Rc,
        cell::RefCell,
        hash::{Hash, Hasher},
    },
    crate::{
        makepad_shader_compiler::ShaderTy,
        platform::CxPlatformGeometry,
        cx::Cx,
        live_traits::*
    }
};

#[derive(Clone, Debug)]
pub struct GeometryRef(pub Rc<Geometry>);

#[derive(Debug, PartialEq)]
pub struct Geometry {
    pub geometry_id: usize,
    pub geometries_free: Rc<RefCell<Vec<usize >> >,
}

impl Drop for Geometry {
    fn drop(&mut self) {
        self.geometries_free.borrow_mut().push(self.geometry_id)
    }
}

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
        let geometries_free = cx.geometries_free.clone();
        let geometry_id = if let Some(geometry_id) = geometries_free.borrow_mut().pop() {
            cx.geometries[geometry_id].dirty = true;
            geometry_id
        }
        else {
            let geometry_id = cx.geometries.len();
            cx.geometries.push(CxGeometry{
                indices: Vec::new(),
                vertices: Vec::new(),
                dirty: true,
                platform: CxPlatformGeometry::default()
            });
            geometry_id
        };
        
        Self {
            geometry_id,
            geometries_free
        }
    }
}

pub struct CxGeometry{
    pub indices: Vec<u32>,
    pub vertices: Vec<f32>,
    pub dirty: bool,
    pub platform: CxPlatformGeometry
}


#[derive(Debug)]
pub struct GeometryField {
    pub id: LiveId,
    pub ty: ShaderTy
}

pub trait GeometryFields{
    fn geometry_fields(&self, fields: &mut Vec<GeometryField>);
    fn live_type_check(&self)->LiveType;
    fn get_geometry_id(&self)->Option<usize>;
}


