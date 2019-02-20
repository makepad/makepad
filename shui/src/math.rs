
#[derive(Debug)]
pub struct Mat4{
    pub v: [f32; 16],
}

pub struct Vec2{
    pub x: f32,
    pub y: f32,
}

pub struct Vec3{
    pub x: f32,
    pub y: f32,
    pub z: f32
}

pub struct Vec4{
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32
}

impl Mat4{
    pub fn identity() -> Mat4{
        return Mat4{
            v:[
                1.0,0.0,0.0,0.0,
                0.0,1.0,0.0,0.0,
                0.0,0.0,1.0,0.0,
                0.0,0.0,0.0,1.0
            ]
        }
    }

    pub fn rotate_tsrt(t1: Vec3, s: Vec3, r: Vec3, t2: Vec3) -> Mat4{
        let cx = f32::cos(r.x);
        let cy = f32::cos(r.y);
        let cz = f32::cos(r.z);
        let sx = f32::sin(r.x);
        let sy = f32::sin(r.y);
        let sz = f32::sin(r.z);
        let m0 = s.x * (cy * cz + sx * sy * sz);
        let m1 = s.y * (-sz * cy + cz * sx * sy);
        let m2 = s.z * (sy * cx);
        let m4 = s.x * (sz * cx);
        let m5 = s.y * (cx * cz);
        let m6 = s.z * (-sx);
        let m8 = s.x * (-sy * cz + cy * sx * sz);
        let m9 = s.y * (sy * sz + cy * sx * cz);
        let m10 = s.z * (cx * cy);
        return Mat4{
            v:[
                m0, m4, m8, 0.0,
                m1, m5, m9, 0.0,
                m2, m6, m10, 0.0,
                t2.x + (m0 * t1.x + m1 * t1.y + m1 * t1.z),
                t2.y + (m4 * t1.x + m5 * t1.y + m6 * t1.z),
                t2.z + (m8 * t1.x + m9 * t1.y + m10 * t1.z),
                1.0
            ]
        }
    }

    pub fn perspective(fov_y:f32, aspect:f32, near:f32, far:f32) -> Mat4{
        let f = 1.0 / f32::tan(fov_y / 2.0);
        let nf = 1.0 / (near - far);
        return Mat4{
            v:[
                f / aspect, 0.0, 0.0, 0.0,
                0.0, f , 0.0, 0.0,
                0.0, 0.0, (far + near) * nf, -1.0,
                0.0, 0.0, (2.0 * far * near) * nf, 0.0

            ]
        }
    }
} 