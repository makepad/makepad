#[derive(Clone, Debug)]
pub struct DecodedPrimitive {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub tangents: Option<Vec<[f32; 4]>>,
    pub texcoords0: Option<Vec<[f32; 2]>>,
    /// COLOR_0 vertex colors, normalized to 0..=1 RGBA (VEC3 sources get
    /// alpha 1.0; ubyte/ushort normalized sources are scaled).
    pub colors0: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
}
