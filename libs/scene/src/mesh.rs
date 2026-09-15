//! Prepared model-space geometry without physics acceleration structures.
use makepad_math::Vec3f;

#[derive(Clone, Debug)]
pub struct PreparedMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub min: Vec3f,
    pub max: Vec3f,
}
