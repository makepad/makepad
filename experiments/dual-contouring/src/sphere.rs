use crate::sdf3::Sdf3;
use crate::vector3::Vector3;

pub struct Sphere {
    pub center: Vector3,
    pub radius: f32,
}

impl Sdf3 for Sphere {
    fn distance(&self, p: Vector3) -> f32 {
        (p - self.center).length() - self.radius
    }
}
