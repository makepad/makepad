use crate::cx::*;

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Geometry {
    pub geometry_id: usize,
}

impl Geometry{
    pub fn from_geometry_gen(cx:&mut Cx, gen:GeometryGen)->Geometry{
        let geometry_id = cx.geometries.len();
        cx.geometries.push(CxGeometry{
            indices: gen.indices,
            vertices: gen.vertices,
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
    fn get_geometry(&self)->Option<Geometry>;
}


