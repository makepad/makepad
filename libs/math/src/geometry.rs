#[derive(Clone, Debug)]
pub struct DecodedPrimitive {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub tangents: Option<Vec<[f32; 4]>>,
    pub texcoords0: Option<Vec<[f32; 2]>>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
}
