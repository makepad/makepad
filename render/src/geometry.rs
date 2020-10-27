use crate::cx::*;

pub trait GeometryCx {
    fn from_geometry_gen(cx:&mut Cx, gen:GeometryGen)->Geometry;
}

impl GeometryCx for Geometry{
    fn from_geometry_gen(cx:&mut Cx, gen:GeometryGen)->Geometry{
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