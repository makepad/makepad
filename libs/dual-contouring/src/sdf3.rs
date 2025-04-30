use crate::vector3::Vector3;

pub trait Sdf3 {
    fn distance(&self, p: Vector3) -> f32;

    fn normal(&self, p: Vector3) -> Vector3 {
        const H: f32 = 1E-3;

        let Vector3 { x, y, z } = p;
        Vector3 {
            x: self.distance(Vector3 { x: x + H, y, z })
                - self.distance(Vector3 { x: x - H, y, z }),
            y: self.distance(Vector3 { x, y: y + H, z })
                - self.distance(Vector3 { x, y: y - H, z }),
            z: self.distance(Vector3 { x, y, z: z + H })
                - self.distance(Vector3 { x, y, z: z - H }),
        }
        .normalize()
    }
}
