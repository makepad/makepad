use crate::cx::*;

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Geometry {
    pub geometry_id: usize,
}

impl Cx{
    pub fn new_geometry(&mut self)->Geometry{
        let geometry_id = self.geometries.len();
        self.geometries.push(CxGeometry{
            indices: Vec::new(),
            vertices: Vec::new(),
            dirty: true,
            platform: CxPlatformGeometry::default()
        });
        Geometry{geometry_id}
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
    pub id: Id,
    pub ty: Ty
}

pub trait GeometryFields{
    fn geometry_fields(&self, fields: &mut Vec<GeometryField>);
    fn live_type_check(&self)->LiveType;
    fn get_geometry(&self)->Geometry;
}


