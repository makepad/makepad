use crate::sdf3::Sdf3;
use crate::vector3::Vector3;

pub struct Union<A: Sdf3, B: Sdf3>(pub A, pub B);

impl<A: Sdf3, B: Sdf3> Sdf3 for Union<A, B> {
    fn distance(&self, p: Vector3) -> f32 {
        self.0.distance(p).min(self.1.distance(p))
    }
}
