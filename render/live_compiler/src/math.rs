use crate::util::PrettyPrintedFloat;
use makepad_microserde::*;
use std::fmt;


#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Mat4 {
    pub v: [f32; 16],
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Transform {
    pub orientation: Vec4,
    pub position: Vec3
}

#[derive(Clone, Copy, Default, Debug, PartialEq, SerRon, DeRon)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    
    pub fn distance(&self, other: &Vec2) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}
/*
pub fn vec2(x:f32, y:f32)->Vec2{
    Vec2{x:x, y:y}
}*/

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32
}

impl Vec3 {
    pub fn to_vec2(&self) -> Vec2{
        Vec2{x:self.x, y:self.y}
    }

    pub fn scale(&self, f: f32) -> Vec3 {
        Vec3 {x: self.x * f, y: self.y * f, z: self.z * f}
    }

    pub fn add(a: Vec3, b: Vec3) -> Vec3 {
        Vec3 {x: a.x + b.x, y: a.y + b.y, z: a.z + b.z}
    }
    
    pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
        Vec3 {x: a.x - b.x, y: a.y - b.y, z: a.z - b.z}
    }
    
    pub fn neg(&self) -> Vec3 {
        Vec3 {x: -self.x, y: -self.y, z: -self.z}
    }
    
    pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
        Vec3 {
            x: a.y * b.z - a.z * b.y,
            y: a.z * b.x - a.x * b.z,
            z: a.x * b.y - a.y * b.x
        }
    }
    
    pub fn dot(&self, other: Vec3) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }
    
    pub fn normalize(&self) -> Vec3 {
        let sz = self.x * self.x + self.y * self.y + self.z * self.z;
        if sz > 0.0 {
            let sr = 1.0 / sz.sqrt();
            return Vec3 {
                x: self.x * sr,
                y: self.y * sr,
                z: self.z * sr
            };
        }
        Vec3::default()
    }
}

/*
pub fn vec3(x:f32, y:f32, z:f32)->Vec3{
    Vec3{x:x, y:y, z:z}
}*/


// equation ax + by + cz + d = 0
#[derive(Clone, Copy, Default, Debug, PartialEq, SerRon, DeRon)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32
}

impl Plane {
    pub fn from_point_normal(p: Vec3, normal: Vec3) -> Self {
        let n = normal.normalize();
        Self {
            a: n.x,
            b: n.y,
            c: n.z,
            d: -p.dot(n)
        }
    }
    
    pub fn from_points(p1: Vec3, p2: Vec3, p3: Vec3) -> Self {
        let normal = Vec3::cross(Vec3::sub(p2, p1), Vec3::sub(p3, p1));
        return Self::from_point_normal(p1, normal);
    }
    
    pub fn intersect_line(&self, v1: Vec3, v2: Vec3)->Vec3{
        let diff = Vec3::sub(v1, v2);
        let denom = self.a * diff.x + self.b * diff.y + self.c * diff.z;
        if denom == 0.0 {
            return Vec3::add(v1, v2).scale(0.5)
        }
        let u = (self.a * v1.x + self.b * v1.y + self.c * v1.z + self.d) / denom;
        return Vec3::add(v1, Vec3::sub(v2,v1).scale(u))
    }
}



#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32
}

impl fmt::Display for Vec4 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "vec4({}, {}, {}, {})",
            PrettyPrintedFloat(self.x),
            PrettyPrintedFloat(self.y),
            PrettyPrintedFloat(self.z),
            PrettyPrintedFloat(self.w),
        )
    }
}

impl Vec4 {
    pub fn to_vec3(&self)->Vec3{
        Vec3 {x: self.x, y: self.y, z: self.z}
    }
}

/*
pub fn vec4(x:f32, y:f32, z:f32, w:f32)->Vec4{
    Vec4{x:x, y:y, z:z, w:w}
}*/


impl Mat4 {
    pub fn identity() -> Mat4 {
        return Mat4 {v: [
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0
        ]}
    }
    
    pub fn from_transform(transform: Transform) -> Mat4 {
        let q = transform.orientation;
        let t = transform.position;
        return Mat4 {v: [
            (1.0 - 2.0 * q.y * q.y - 2.0 * q.z * q.z),
            (2.0 * q.x * q.y - 2.0 * q.z * q.w),
            (2.0 * q.x * q.z + 2.0 * q.y * q.w),
            0.0,
            (2.0 * q.x * q.y + 2.0 * q.z * q.w),
            (1.0 - 2.0 * q.x * q.x - 2.0 * q.z * q.z),
            (2.0 * q.y * q.z - 2.0 * q.x * q.w),
            0.0,
            (2.0 * q.x * q.z - 2.0 * q.y * q.w),
            (2.0 * q.y * q.z + 2.0 * q.x * q.w),
            (1.0 - 2.0 * q.x * q.x - 2.0 * q.y * q.y),
            0.0,
            t.x,
            t.y,
            t.z,
            1.0
        ]}
    }
    
    pub fn rotate_tsrt(t1: Vec3, s: f32, r: Vec3, t2: Vec3) -> Mat4 {
        const TORAD: f32 = 0.017453292519943295;
        let cx = f32::cos(r.x * TORAD);
        let cy = f32::cos(r.y * TORAD);
        let cz = f32::cos(r.z * TORAD);
        let sx = f32::sin(r.x * TORAD);
        let sy = f32::sin(r.y * TORAD);
        let sz = f32::sin(r.z * TORAD);
        let m0 = s * (cy * cz + sx * sy * sz);
        let m1 = s * (-sz * cy + cz * sx * sy);
        let m2 = s * (sy * cx);
        let m4 = s * (sz * cx);
        let m5 = s * (cx * cz);
        let m6 = s * (-sx);
        let m8 = s * (-sy * cz + cy * sx * sz);
        let m9 = s * (sy * sz + cy * sx * cz);
        let m10 = s * (cx * cy);
        return Mat4 {v: [
            m0,
            m4,
            m8,
            0.0,
            m1,
            m5,
            m9,
            0.0,
            m2,
            m6,
            m10,
            0.0,
            t2.x + (m0 * t1.x + m1 * t1.y + m1 * t1.z),
            t2.y + (m4 * t1.x + m5 * t1.y + m6 * t1.z),
            t2.z + (m8 * t1.x + m9 * t1.y + m10 * t1.z),
            1.0
        ]}
    }
    
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / f32::tan(fov_y / 2.0);
        let nf = 1.0 / (near - far);
        return Mat4 {v: [
            f / aspect,
            0.0,
            0.0,
            0.0,
            0.0,
            f,
            0.0,
            0.0,
            0.0,
            0.0,
            (far + near) * nf,
            -1.0,
            0.0,
            0.0,
            (2.0 * far * near) * nf,
            0.0
        ]}
    }
    
    pub fn scale_translate(s: f32, x: f32, y: f32, z: f32) -> Mat4 {
        return Mat4 {v: [
            s,
            0.0,
            0.0,
            0.0,
            0.0,
            s,
            0.0,
            0.0,
            0.0,
            0.0,
            s,
            0.0,
            x,
            y,
            z,
            1.0
        ]}
        
    }
    
    pub fn rotate(rx: f32, ry: f32, rz: f32) -> Mat4 {
        const TORAD: f32 = 0.017453292519943295;
        let cx = f32::cos(rx * TORAD);
        let cy = f32::cos(ry * TORAD);
        let cz = f32::cos(rz * TORAD);
        let sx = f32::sin(rx * TORAD);
        let sy = f32::sin(ry * TORAD);
        let sz = f32::sin(rz * TORAD);
        let m0 = cy * cz + sx * sy * sz;
        let m1 = -sz * cy + cz * sx * sy;
        let m2 = sy * cx;
        let m4 = sz * cx;
        let m5 = cx * cz;
        let m6 = -sx;
        let m8 = -sy * cz + cy * sx * sz;
        let m9 = sy * sz + cy * sx * cz;
        let m10 = cx * cy;
        return Mat4 {v: [
            m0,
            m4,
            m8,
            0.0,
            m1,
            m5,
            m9,
            0.0,
            m2,
            m6,
            m10,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0
        ]}
    }
    
    pub fn ortho(left: f32, right: f32, top: f32, bottom: f32, near: f32, far: f32, scalex: f32, scaley: f32) -> Mat4 {
        let lr = 1.0 / (left - right);
        let bt = 1.0 / (bottom - top);
        let nf = 1.0 / (near - far);
        /*return Mat4{v:[
            -2.0 * lr * scalex, 0.0, 0.0, (left+right) * lr,
            0.0, -2.0 * bt * scaley, 0.0, (top+bottom) * bt,
            0.0, 0.0, 2.0 * nf, (far + near) * nf,
            0.0, 0.0, 0.0, 1.0
        ]}*/
        return Mat4 {v: [
            -2.0 * lr * scalex,
            0.0,
            0.0,
            0.0,
            0.0,
            -2.0 * bt * scaley,
            0.0,
            0.0,
            0.0,
            0.0,
            -1.0 * nf,
            0.0,
            (left + right) * lr,
            (top + bottom) * bt,
            0.5 + (far + near) * nf,
            1.0
        ]}
    }
    
    pub fn transform_vec4(&self, v: Vec4) -> Vec4 {
        let m = &self.v;
        Vec4 {
            x: m[0] * v.x + m[4] * v.y + m[8] * v.z + m[12] * v.w,
            y: m[1] * v.x + m[5] * v.y + m[9] * v.z + m[13] * v.w,
            z: m[2] * v.x + m[6] * v.y + m[10] * v.z + m[14] * v.w,
            w: m[3] * v.x + m[7] * v.y + m[11] * v.z + m[15] * v.w
        }
    }
    
    pub fn from_mul(a: &Mat4, b: &Mat4) -> Mat4 {
        // this is probably stupid. Programmed JS for too long.
        let a = &a.v;
        let b = &b.v;
        fn d(i: &[f32; 16], x: usize, y: usize) -> f32 {return i[x + 4 * y]}
        Mat4 {
            v: [
                d(a, 0, 0) * d(b, 0, 0) + d(a, 1, 0) * d(b, 0, 1) + d(a, 2, 0) * d(b, 0, 2) + d(a, 3, 0) * d(b, 0, 3),
                d(a, 0, 0) * d(b, 1, 0) + d(a, 1, 0) * d(b, 1, 1) + d(a, 2, 0) * d(b, 1, 2) + d(a, 3, 0) * d(b, 1, 3),
                d(a, 0, 0) * d(b, 2, 0) + d(a, 1, 0) * d(b, 2, 1) + d(a, 2, 0) * d(b, 2, 2) + d(a, 3, 0) * d(b, 2, 3),
                d(a, 0, 0) * d(b, 3, 0) + d(a, 1, 0) * d(b, 3, 1) + d(a, 2, 0) * d(b, 3, 2) + d(a, 3, 0) * d(b, 3, 3),
                d(a, 0, 1) * d(b, 0, 0) + d(a, 1, 1) * d(b, 0, 1) + d(a, 2, 1) * d(b, 0, 2) + d(a, 3, 1) * d(b, 0, 3),
                d(a, 0, 1) * d(b, 1, 0) + d(a, 1, 1) * d(b, 1, 1) + d(a, 2, 1) * d(b, 1, 2) + d(a, 3, 1) * d(b, 1, 3),
                d(a, 0, 1) * d(b, 2, 0) + d(a, 1, 1) * d(b, 2, 1) + d(a, 2, 1) * d(b, 2, 2) + d(a, 3, 1) * d(b, 2, 3),
                d(a, 0, 1) * d(b, 3, 0) + d(a, 1, 1) * d(b, 3, 1) + d(a, 2, 1) * d(b, 3, 2) + d(a, 3, 1) * d(b, 3, 3),
                d(a, 0, 2) * d(b, 0, 0) + d(a, 1, 2) * d(b, 0, 1) + d(a, 2, 2) * d(b, 0, 2) + d(a, 3, 2) * d(b, 0, 3),
                d(a, 0, 2) * d(b, 1, 0) + d(a, 1, 2) * d(b, 1, 1) + d(a, 2, 2) * d(b, 1, 2) + d(a, 3, 2) * d(b, 1, 3),
                d(a, 0, 2) * d(b, 2, 0) + d(a, 1, 2) * d(b, 2, 1) + d(a, 2, 2) * d(b, 2, 2) + d(a, 3, 2) * d(b, 2, 3),
                d(a, 0, 2) * d(b, 3, 0) + d(a, 1, 2) * d(b, 3, 1) + d(a, 2, 2) * d(b, 3, 2) + d(a, 3, 2) * d(b, 3, 3),
                d(a, 0, 3) * d(b, 0, 0) + d(a, 1, 3) * d(b, 0, 1) + d(a, 2, 3) * d(b, 0, 2) + d(a, 3, 3) * d(b, 0, 3),
                d(a, 0, 3) * d(b, 1, 0) + d(a, 1, 3) * d(b, 1, 1) + d(a, 2, 3) * d(b, 1, 2) + d(a, 3, 3) * d(b, 1, 3),
                d(a, 0, 3) * d(b, 2, 0) + d(a, 1, 3) * d(b, 2, 1) + d(a, 2, 3) * d(b, 2, 2) + d(a, 3, 3) * d(b, 2, 3),
                d(a, 0, 3) * d(b, 3, 0) + d(a, 1, 3) * d(b, 3, 1) + d(a, 2, 3) * d(b, 3, 2) + d(a, 3, 3) * d(b, 3, 3),
            ]
        }
    }
    
    pub fn invert(&self) -> Mat4 {
        let a = &self.v;
        let a00 = a[0];
        let a01 = a[1];
        let a02 = a[2];
        let a03 = a[3];
        let a10 = a[4];
        let a11 = a[5];
        let a12 = a[6];
        let a13 = a[7];
        let a20 = a[8];
        let a21 = a[9];
        let a22 = a[10];
        let a23 = a[11];
        let a30 = a[12];
        let a31 = a[13];
        let a32 = a[14];
        let a33 = a[15];
        
        let b00 = a00 * a11 - a01 * a10;
        let b01 = a00 * a12 - a02 * a10;
        let b02 = a00 * a13 - a03 * a10;
        let b03 = a01 * a12 - a02 * a11;
        let b04 = a01 * a13 - a03 * a11;
        let b05 = a02 * a13 - a03 * a12;
        let b06 = a20 * a31 - a21 * a30;
        let b07 = a20 * a32 - a22 * a30;
        let b08 = a20 * a33 - a23 * a30;
        let b09 = a21 * a32 - a22 * a31;
        let b10 = a21 * a33 - a23 * a31;
        let b11 = a22 * a33 - a23 * a32;
        
        // Calculate the determinant
        let det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
        
        if det == 0.0 {
            return Mat4::identity();
        }
        
        let idet = 1.0 / det;
        return Mat4 {
            v: [
                (a11 * b11 - a12 * b10 + a13 * b09) * idet,
                (a02 * b10 - a01 * b11 - a03 * b09) * idet,
                (a31 * b05 - a32 * b04 + a33 * b03) * idet,
                (a22 * b04 - a21 * b05 - a23 * b03) * idet,
                (a12 * b08 - a10 * b11 - a13 * b07) * idet,
                (a00 * b11 - a02 * b08 + a03 * b07) * idet,
                (a32 * b02 - a30 * b05 - a33 * b01) * idet,
                (a20 * b05 - a22 * b02 + a23 * b01) * idet,
                (a10 * b10 - a11 * b08 + a13 * b06) * idet,
                (a01 * b08 - a00 * b10 - a03 * b06) * idet,
                (a30 * b04 - a31 * b02 + a33 * b00) * idet,
                (a21 * b02 - a20 * b04 - a23 * b00) * idet,
                (a11 * b07 - a10 * b09 - a12 * b06) * idet,
                (a00 * b09 - a01 * b07 + a02 * b06) * idet,
                (a31 * b01 - a30 * b03 - a32 * b00) * idet,
                (a20 * b03 - a21 * b01 + a22 * b00) * idet,
            ]
        }
    }
}
