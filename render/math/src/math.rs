use{
    std::{fmt,ops},
//    makepad_microserde::*,
//    crate::colorhex::*
};


pub struct PrettyPrintedF32(pub f32);

impl fmt::Display for PrettyPrintedF32 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.abs().fract() < 0.00000001 {
            write!(f, "{}.0", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}


pub struct PrettyPrintedF64(pub f64);

impl fmt::Display for PrettyPrintedF64 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.abs().fract() < 0.00000001 {
            write!(f, "{}.0", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}


#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Rect {
    pub pos: Vec2,
    pub size: Vec2,
}

impl Rect {
    
    pub fn translate(self, pos: Vec2) -> Rect {
        Rect {pos: self.pos + pos, size: self.size}
    }
    
    pub fn contains(&self, pos: Vec2) -> bool {
        return pos.x >= self.pos.x && pos.x <= self.pos.x + self.size.x &&
        pos.y >= self.pos.y && pos.y <= self.pos.y + self.size.y;
    }
    pub fn intersects(&self, r: Rect) -> bool {
        !(
            r.pos.x > self.pos.x + self.size.x ||
            r.pos.x + r.size.x < self.pos.x ||
            r.pos.y > self.pos.y + self.size.y ||
            r.pos.y + r.size.y < self.pos.y
        )
    }
    /*
    pub fn contains_with_margin(&self, pos: Vec2, margin: &Option<Margin>) -> bool {
        if let Some(margin) = margin {
            return
            pos.x >= self.pos.x - margin.l
                && pos.x <= self.pos.x + self.size.x + margin.r
                && pos.y >= self.pos.y - margin.t
                && pos.y <= self.pos.y + self.size.y + margin.b;
        }
        else {
            return self.contains(pos);
        }
    }
    */
    pub fn from_lerp(a: Rect, b: Rect, f: f32) -> Rect {
        Rect {
            pos: (b.pos - a.pos) * f + a.pos,
            size: (b.size - a.size) * f + a.size
        }
    }
}

#[derive(Clone, Copy, Default,PartialEq, Debug)]
pub struct Mat4 {
    pub v: [f32; 16],
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Transform {
    pub orientation: Quat,
    pub position: Vec3
}

impl Transform {
    pub fn to_mat4(&self) -> Mat4 {
        let q = self.orientation;
        let t = self.position;
        return Mat4 {v: [
            (1.0 - 2.0 * q.b * q.b - 2.0 * q.c * q.c),
            (2.0 * q.a * q.b - 2.0 * q.c * q.d),
            (2.0 * q.a * q.c + 2.0 * q.b * q.d),
            0.0,
            (2.0 * q.a * q.b + 2.0 * q.c * q.d),
            (1.0 - 2.0 * q.a * q.a - 2.0 * q.c * q.c),
            (2.0 * q.b * q.c - 2.0 * q.a * q.d),
            0.0,
            (2.0 * q.a * q.c - 2.0 * q.b * q.d),
            (2.0 * q.b * q.c + 2.0 * q.a * q.d),
            (1.0 - 2.0 * q.a * q.a - 2.0 * q.b * q.b),
            0.0,
            t.x,
            t.y,
            t.z,
            1.0
        ]}
    }
    
    pub fn from_lerp(a: Transform, b: Transform, f: f32) -> Self {
        Transform {
            orientation: Quat::from_slerp(a.orientation, b.orientation, f),
            position: Vec3::from_lerp(a.position, b.position, f)
        }
    }
    
    pub fn from_slerp_orientation(a: Transform, b: Transform, f: f32) -> Self {
        Transform {
            orientation: Quat::from_slerp(a.orientation, b.orientation, f),
            position: b.position
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}


impl Vec2 {
    pub fn new() -> Vec2 {
        Vec2::default()
    }
    
    pub fn all(x: f32) -> Vec2 {
        Vec2 {x: x, y: x}
    }
    
    pub fn from_lerp(a: Vec2, b: Vec2, f: f32) -> Vec2 {
        let nf = 1.0 - f;
        return Vec2{
            x: nf * a.x + f * b.x,
            y: nf * a.y + f * b.y,
        };
    }
    
    pub fn distance(&self, other: &Vec2) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }

    pub fn to_vec3(&self) -> Vec3 {
        Vec3 {x: self.x, y: self.y, z: 0.0}
    }
}


impl fmt::Display for Vec2 {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,"vec2({},{})",self.x, self.y)
    }
}

impl fmt::Display for Vec3 {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,"vec3({},{},{})",self.x, self.y, self.z)
    }
}

pub fn vec2(x: f32, y: f32) -> Vec2 {Vec2 {x, y}}
pub fn vec3(x: f32, y: f32, z: f32) -> Vec3 {Vec3 {x, y, z}}
pub fn vec4(x: f32, y: f32, z: f32, w: f32) -> Vec4 {Vec4 {x, y, z, w}}

const TORAD: f32 = 0.017453292519943295;
const TODEG: f32 = 57.295779513082321;

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
    
    pub fn from_lerp(a: Vec3, b: Vec3, f: f32) -> Vec3 {
        Vec3 {
            x: (b.x - a.x) * f + a.x,
            y: (b.y - a.y) * f + a.y,
            z: (b.z - a.z) * f + a.z
        }
    }
    
    pub fn all(x: f32) -> Vec3 {
        Vec3 {x: x, y: x, z: x}
    }
    
    pub fn to_vec2(&self) -> Vec2 {
        Vec2 {x: self.x, y: self.y}
    }
    
    pub fn scale(&self, f: f32) -> Vec3 {
        Vec3 {x: self.x * f, y: self.y * f, z: self.z * f}
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
#[derive(Clone, Copy, Default, Debug, PartialEq)]
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
        let normal = Vec3::cross(p2 - p1, p3 - p1);
        return Self::from_point_normal(p1, normal);
    }
    
    pub fn intersect_line(&self, v1: Vec3, v2: Vec3) -> Vec3 {
        let diff = v1 - v2;
        let denom = self.a * diff.x + self.b * diff.y + self.c * diff.z;
        if denom == 0.0 {
            return (v1 * v2) * 0.5
        }
        let u = (self.a * v1.x + self.b * v1.y + self.c * v1.z + self.d) / denom;
        return v1 + (v2 - v1) * u
    }
}



#[derive(Clone, Copy, Default, Debug,PartialEq)]
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
            PrettyPrintedF32(self.x),
            PrettyPrintedF32(self.y),
            PrettyPrintedF32(self.z),
            PrettyPrintedF32(self.w),
        )
    }
}

impl Vec4 {
    pub fn all(v: f32) -> Self {
        Self {x: v, y: v, z: v, w: v}
    }
    
    pub fn to_vec3(&self) -> Vec3 {
        Vec3 {x: self.x, y: self.y, z: self.z}
    }
    
    pub fn dot(&self, other: Vec4) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }
    
    pub fn from_lerp(a: Vec4, b: Vec4, f: f32) -> Vec4 {
        let nf = 1.0 - f;
        return Vec4 {
            x: nf * a.x + f * b.x,
            y: nf * a.y + f * b.y,
            z: nf * a.z + f * b.z,
            w: nf * a.w + f * b.w,
        };
    }
    
    pub fn is_equal_enough(&self, other: &Vec4, epsilon:f32) -> bool {
        (self.x - other.x).abs() < epsilon
            && (self.y - other.y).abs() < epsilon
            && (self.z - other.z).abs() < epsilon
            && (self.w - other.w).abs() < epsilon
    }
    
    
    pub fn from_hsva(hsv: Vec4) -> Vec4 {
        fn mix(x: f32, y: f32, t: f32) -> f32 {x + (y - x) * t}
        fn clamp(x: f32, mi: f32, ma: f32) -> f32 {if x < mi {mi} else if x > ma {ma} else {x}}
        fn fract(x: f32) -> f32 {x.fract()}
        fn abs(x: f32) -> f32 {x.abs()}
        Vec4 {
            x: hsv.z * mix(1.0, clamp(abs(fract(hsv.x + 1.0) * 6.0 - 3.0) - 1.0, 0.0, 1.0), hsv.y),
            y: hsv.z * mix(1.0, clamp(abs(fract(hsv.x + 2.0 / 3.0) * 6.0 - 3.0) - 1.0, 0.0, 1.0), hsv.y),
            z: hsv.z * mix(1.0, clamp(abs(fract(hsv.x + 1.0 / 3.0) * 6.0 - 3.0) - 1.0, 0.0, 1.0), hsv.y),
            w: 1.0
        }
    }
    
    pub fn to_hsva(&self) -> Vec4 {
        let pc = self.y < self.z; //step(c[2],c[1])
        let p0 = if pc {self.z} else {self.y}; //mix(c[2],c[1],pc)
        let p1 = if pc {self.y} else {self.z}; //mix(c[1],c[2],pc)
        let p2 = if pc {-1.0} else {0.0}; //mix(-1,0,pc)
        let p3 = if pc {2.0 / 3.0} else {-1.0 / 3.0}; //mix(2/3,-1/3,pc)
        
        let qc = self.x < p0; //step(p0, c[0])
        let q0 = if qc {p0} else {self.x}; //mix(p0, c[0], qc)
        let q1 = p1;
        let q2 = if qc {p3} else {p2}; //mix(p3, p2, qc)
        let q3 = if qc {self.x} else {p0}; //mix(c[0], p0, qc)
        
        let d = q0 - q3.min(q1);
        let e = 1.0e-10;
        return Vec4 {
            x: (q2 + (q3 - q1) / (6.0 * d + e)).abs(),
            y: d / (q0 + e),
            z: q0,
            w: self.w
        }
    }
    
    
    pub fn from_u32(val: u32) -> Vec4 {
        Vec4 {
            x: ((val >> 24) & 0xff) as f32 / 255.0,
            y: ((val >> 16) & 0xff) as f32 / 255.0,
            z: ((val >> 8) & 0xff) as f32 / 255.0,
            w: ((val >> 0) & 0xff) as f32 / 255.0,
        }
    }
    
    pub fn to_u32(&self) -> u32 {
        let r = (self.x * 255.0) as u8 as u32;
        let g = (self.y * 255.0) as u8 as u32;
        let b = (self.z * 255.0) as u8 as u32;
        let a = (self.w * 255.0) as u8 as u32;
        return (r<<24)|(g<<16)|(b<<8)|a;
    }

}


#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Quat {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32
}

impl Quat {
    pub fn dot(&self, other: Quat) -> f32 {
        self.a * other.a + self.b * other.b + self.c * other.c + self.d * other.d
    }
    
    pub fn neg(&self) -> Quat {
        Quat {a: -self.a, b: -self.b, c: -self.c, d: -self.d}
    }
    
    pub fn get_angle_with(&self, other: Quat) -> f32 {
        let dot = self.dot(other);
        (2.0 * dot * dot - 1.0).acos() * TODEG
    }
    
    pub fn from_slerp(n: Quat, mut m: Quat, t: f32) -> Quat {
        // calc cosine
        let mut cosom = n.dot(m);
        // adjust signs (if necessary)
        if cosom < 0.0 {
            cosom = -cosom;
            m = m.neg();
        }
        // calculate coefficients
        let (scale0, scale1) = if 1.0 - cosom > 0.000001 {
            // standard case (slerp)
            let omega = cosom.acos();
            let sinom = omega.sin();
            (((1.0 - t) * omega).sin() / sinom, (t * omega).sin() / sinom)
        } else {
            (1.0 - t, t)
        };
        // calculate final values
        (Quat {
            a: scale0 * n.a + scale1 * m.a,
            b: scale0 * n.b + scale1 * m.b,
            c: scale0 * n.c + scale1 * m.c,
            d: scale0 * m.d + scale1 * m.d
        }).normalized()
    }
    
    pub fn length(self) -> f32 {
        return self.dot(self).sqrt()
    }
    
    pub fn normalized(&mut self) -> Quat {
        let len = self.length();
        Quat {
            a: self.a / len,
            b: self.b / len,
            c: self.c / len,
            d: self.d / len,
        }
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
    
    
    
    pub fn txyz_s_ry_rx_txyz(t1: Vec3, s: f32, ry: f32, rx: f32, t2: Vec3) -> Mat4 {
        
        let cx = f32::cos(rx * TORAD);
        let cy = f32::cos(ry * TORAD);
        //let cz = f32::cos(r.z * TORAD);
        let sx = f32::sin(rx * TORAD);
        let sy = f32::sin(ry * TORAD);
        //let sz = f32::sin(r.z * TORAD);
        // y first, then x, then z
        
        // Y
        // |  cy,  0,  sy  |
        // |  0,   1,  0  |
        // | -sy,  0,  cy  |
        
        // X:
        // |  1,  0,  0  |
        // |  0,  cx, -sx  |
        // |  0,  sx,  cx  |
        
        // Z:
        // |  cz, -sz,  0  |
        // |  sz,  cz,  0  |
        // |  0,    0,  1  |
        
        // X * Y
        // | cy,           0,    sy |
        // | -sx*-sy,     cx,   -sx*cy  |
        // | -sy * cx,    sx,  cx*cy  |
        
        // Z * X * Y
        // | cz * cy + -sz * -sx *-sy,   -sz * cx,    sy *cz + -sz * -sx * cy |
        // | sz * cy + -sx*-sy * cz,     sz * cx,   sy * sz + cz * -sz * cy  |
        // | -sy * cx,    sx,  cx*cy  |
        
        
        // Y * X * Z
        // | c*c,  c, s*s   |
        // |   0,  c,  -s   |
        // |  -s,  c*s, c*c |
        
        /*       
        let m0 = s * (cz * cy + (-sz) * (-sx) *(-sy));
        let m1 = s * (-sz * cx);
        let m2 = s * (sy *cz + (-sz) * (-sx) * cy);
        
        let m4 = s * (sz * cy + (-sx)*(-sy) * cz);
        let m5 = s * (sz * cx);
        let m6 = s * (sy * sz + cz * (-sx) * cy);
        
        let m8 = s * (-sy*cx);
        let m9 = s * (sx);
        let m10 = s * (cx * cy);
        */
        
        let m0 = s * (cy);
        let m1 = s * (0.0);
        let m2 = s * (sy);
        
        let m4 = s * (-sx * -sy);
        let m5 = s * (cx);
        let m6 = s * (-sx * cy);
        
        let m8 = s * (-sy * cx);
        let m9 = s * (sx);
        let m10 = s * (cx * cy);
        
        /*
        let m0 = s * (cy * cz + sx * sy * sz);
        let m1 = s * (-sz * cy + cz * sx * sy);
        let m2 = s * (sy * cx);
        
        let m4 = s * (sz * cx);
        let m5 = s * (cx * cz);
        let m6 = s * (-sx);
        
        let m8 = s * (-sy * cz + cy * sx * sz);
        let m9 = s * (sy * sz + cy * sx * cz);
        let m10 = s * (cx * cy);
        */
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
            t2.x + (m0 * t1.x + m1 * t1.y + m2 * t1.z),
            t2.y + (m4 * t1.x + m5 * t1.y + m6 * t1.z),
            t2.z + (m8 * t1.x + m9 * t1.y + m10 * t1.z),
            1.0
        ]}
    }
    
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / f32::tan(fov_y * TORAD / 2.0);
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
    
    pub fn translation(x: f32, y: f32, z: f32) -> Mat4 {
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
            x,
            y,
            z,
            1.0
        ]}
        
    }
    
    pub fn scaled_translation(s: f32, x: f32, y: f32, z: f32) -> Mat4 {
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
    
    pub fn rotation(rx: f32, ry: f32, rz: f32) -> Mat4 {
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
    
    pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
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


//------ Vec2 operators

impl ops::Add<Vec2> for Vec2 {
    type Output = Vec2;
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2 {x: self.x + rhs.x, y: self.y + rhs.y}
    }
}

impl ops::Sub<Vec2> for Vec2 {
    type Output = Vec2;
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2 {x: self.x - rhs.x, y: self.y - rhs.y}
    }
}

impl ops::Mul<Vec2> for Vec2 {
    type Output = Vec2;
    fn mul(self, rhs: Vec2) -> Vec2 {
        Vec2 {x: self.x * rhs.x, y: self.y * rhs.y}
    }
}

impl ops::Div<Vec2> for Vec2 {
    type Output = Vec2;
    fn div(self, rhs: Vec2) -> Vec2 {
        Vec2 {x: self.x / rhs.x, y: self.y / rhs.y}
    }
}


impl ops::Add<Vec2> for f32 {
    type Output = Vec2;
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2 {x: self + rhs.x, y: self + rhs.y}
    }
}

impl ops::Sub<Vec2> for f32 {
    type Output = Vec2;
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2 {x: self -rhs.x, y: self -rhs.y}
    }
}

impl ops::Mul<Vec2> for f32 {
    type Output = Vec2;
    fn mul(self, rhs: Vec2) -> Vec2 {
        Vec2 {x: self *rhs.x, y: self *rhs.y}
    }
}

impl ops::Div<Vec2> for f32 {
    type Output = Vec2;
    fn div(self, rhs: Vec2) -> Vec2 {
        Vec2 {x: self / rhs.x, y: self / rhs.y}
    }
}


impl ops::Add<f32> for Vec2 {
    type Output = Vec2;
    fn add(self, rhs: f32) -> Vec2 {
        Vec2 {x: self.x + rhs, y: self.y + rhs}
    }
}

impl ops::Sub<f32> for Vec2 {
    type Output = Vec2;
    fn sub(self, rhs: f32) -> Vec2 {
        Vec2 {x: self.x - rhs, y: self.y - rhs}
    }
}

impl ops::Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, rhs: f32) -> Vec2 {
        Vec2 {x: self.x * rhs, y: self.y * rhs}
    }
}

impl ops::Div<f32> for Vec2 {
    type Output = Vec2;
    fn div(self, rhs: f32) -> Vec2 {
        Vec2 {x: self.x / rhs, y: self.y / rhs}
    }
}

impl ops::AddAssign<Vec2> for Vec2 {
    fn add_assign(&mut self, rhs: Vec2) {
        self.x = self.x + rhs.x;
        self.y = self.y + rhs.y;
    }
}

impl ops::SubAssign<Vec2> for Vec2 {
    fn sub_assign(&mut self, rhs: Vec2) {
        self.x = self.x - rhs.x;
        self.y = self.y - rhs.y;
    }
}

impl ops::MulAssign<Vec2> for Vec2 {
    fn mul_assign(&mut self, rhs: Vec2) {
        self.x = self.x * rhs.x;
        self.y = self.y * rhs.y;
    }
}

impl ops::DivAssign<Vec2> for Vec2 {
    fn div_assign(&mut self, rhs: Vec2) {
        self.x = self.x / rhs.x;
        self.y = self.y / rhs.y;
    }
}


impl ops::AddAssign<f32> for Vec2 {
    fn add_assign(&mut self, rhs: f32) {
        self.x = self.x + rhs;
        self.y = self.y + rhs;
    }
}

impl ops::SubAssign<f32> for Vec2 {
    fn sub_assign(&mut self, rhs: f32) {
        self.x = self.x - rhs;
        self.y = self.y - rhs;
    }
}

impl ops::MulAssign<f32> for Vec2 {
    fn mul_assign(&mut self, rhs: f32) {
        self.x = self.x * rhs;
        self.y = self.y * rhs;
    }
}

impl ops::DivAssign<f32> for Vec2 {
    fn div_assign(&mut self, rhs: f32) {
        self.x = self.x / rhs;
        self.y = self.y / rhs;
    }
}

impl ops::Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Self { Vec2{x:-self.x, y:-self.y}}
}

impl ops::Neg for Vec3{
    type Output = Vec3;
    fn neg(self) -> Self { Vec3{x:-self.x, y:-self.y, z:-self.z}}
}

impl ops::Neg for Vec4 {
    type Output = Vec4;
    fn neg(self) -> Self { Vec4{x:-self.x, y:-self.y, z:-self.z, w:-self.w}}
}


//------ Vec3 operators

impl ops::Add<Vec3> for Vec3 {
    type Output = Vec3;
    fn add(self, rhs: Vec3) -> Vec3 {
        Vec3 {x: self.x + rhs.x, y: self.y + rhs.y, z: self.z + rhs.z}
    }
}

impl ops::Sub<Vec3> for Vec3 {
    type Output = Vec3;
    fn sub(self, rhs: Vec3) -> Vec3 {
        Vec3 {x: self.x - rhs.x, y: self.y - rhs.y, z: self.z - rhs.z}
    }
}

impl ops::Mul<Vec3> for Vec3 {
    type Output = Vec3;
    fn mul(self, rhs: Vec3) -> Vec3 {
        Vec3 {x: self.x * rhs.x, y: self.y * rhs.y, z: self.z * rhs.z}
    }
}

impl ops::Div<Vec3> for Vec3 {
    type Output = Vec3;
    fn div(self, rhs: Vec3) -> Vec3 {
        Vec3 {x: self.x / rhs.x, y: self.y / rhs.y, z: self.z / rhs.z}
    }
}

impl ops::Add<Vec3> for f32 {
    type Output = Vec3;
    fn add(self, rhs: Vec3) -> Vec3 {
        Vec3 {x: self + rhs.x, y: self + rhs.y, z: self + rhs.z}
    }
}

impl ops::Sub<Vec3> for f32 {
    type Output = Vec3;
    fn sub(self, rhs: Vec3) -> Vec3 {
        Vec3 {x: self -rhs.x, y: self -rhs.y, z: self -rhs.z}
    }
}

impl ops::Mul<Vec3> for f32 {
    type Output = Vec3;
    fn mul(self, rhs: Vec3) -> Vec3 {
        Vec3 {x: self *rhs.x, y: self *rhs.y, z: self *rhs.z}
    }
}

impl ops::Div<Vec3> for f32 {
    type Output = Vec3;
    fn div(self, rhs: Vec3) -> Vec3 {
        Vec3 {x: self / rhs.x, y: self / rhs.y, z: self / rhs.z}
    }
}


impl ops::Add<f32> for Vec3 {
    type Output = Vec3;
    fn add(self, rhs: f32) -> Vec3 {
        Vec3 {x: self.x + rhs, y: self.y + rhs, z: self.z + rhs}
    }
}

impl ops::Sub<f32> for Vec3 {
    type Output = Vec3;
    fn sub(self, rhs: f32) -> Vec3 {
        Vec3 {x: self.x - rhs, y: self.y - rhs, z: self.z - rhs}
    }
}

impl ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, rhs: f32) -> Vec3 {
        Vec3 {x: self.x * rhs, y: self.y * rhs, z: self.z * rhs}
    }
}

impl ops::Div<f32> for Vec3 {
    type Output = Vec3;
    fn div(self, rhs: f32) -> Vec3 {
        Vec3 {x: self.x / rhs, y: self.y / rhs, z: self.z / rhs}
    }
}


impl ops::AddAssign<Vec3> for Vec3 {
    fn add_assign(&mut self, rhs: Vec3) {
        self.x = self.x + rhs.x;
        self.y = self.y + rhs.y;
        self.z = self.z + rhs.z;
    }
}

impl ops::SubAssign<Vec3> for Vec3 {
    fn sub_assign(&mut self, rhs: Vec3) {
        self.x = self.x - rhs.x;
        self.y = self.y - rhs.y;
        self.z = self.z - rhs.z;
    }
}

impl ops::MulAssign<Vec3> for Vec3 {
    fn mul_assign(&mut self, rhs: Vec3) {
        self.x = self.x * rhs.x;
        self.y = self.y * rhs.y;
        self.z = self.z * rhs.z;
    }
}

impl ops::DivAssign<Vec3> for Vec3 {
    fn div_assign(&mut self, rhs: Vec3) {
        self.x = self.x / rhs.x;
        self.y = self.y / rhs.y;
        self.z = self.z / rhs.z;
    }
}


impl ops::AddAssign<f32> for Vec3 {
    fn add_assign(&mut self, rhs: f32) {
        self.x = self.x + rhs;
        self.y = self.y + rhs;
        self.z = self.z + rhs;
    }
}

impl ops::SubAssign<f32> for Vec3 {
    fn sub_assign(&mut self, rhs: f32) {
        self.x = self.x - rhs;
        self.y = self.y - rhs;
        self.z = self.z - rhs;
    }
}

impl ops::MulAssign<f32> for Vec3 {
    fn mul_assign(&mut self, rhs: f32) {
        self.x = self.x * rhs;
        self.y = self.y * rhs;
        self.z = self.z * rhs;
    }
}

impl ops::DivAssign<f32> for Vec3 {
    fn div_assign(&mut self, rhs: f32) {
        self.x = self.x / rhs;
        self.y = self.y / rhs;
        self.z = self.z / rhs;
    }
}

//------ Vec4 operators

impl ops::Add<Vec4> for Vec4 {
    type Output = Vec4;
    fn add(self, rhs: Vec4) -> Vec4 {
        Vec4 {x: self.x + rhs.x, y: self.y + rhs.y, z: self.z + rhs.z, w: self.w + rhs.w}
    }
}

impl ops::Sub<Vec4> for Vec4 {
    type Output = Vec4;
    fn sub(self, rhs: Vec4) -> Vec4 {
        Vec4 {x: self.x - rhs.x, y: self.y - rhs.y, z: self.z - rhs.z, w: self.w - rhs.w}
    }
}

impl ops::Mul<Vec4> for Vec4 {
    type Output = Vec4;
    fn mul(self, rhs: Vec4) -> Vec4 {
        Vec4 {x: self.x * rhs.x, y: self.y * rhs.y, z: self.z * rhs.z, w: self.w * rhs.w}
    }
}

impl ops::Div<Vec4> for Vec4 {
    type Output = Vec4;
    fn div(self, rhs: Vec4) -> Vec4 {
        Vec4 {x: self.x / rhs.x, y: self.y / rhs.y, z: self.z / rhs.z, w: self.w / rhs.w}
    }
}

impl ops::Add<Vec4> for f32 {
    type Output = Vec4;
    fn add(self, rhs: Vec4) -> Vec4 {
        Vec4 {x: self + rhs.x, y: self + rhs.y, z: self + rhs.z, w: self + rhs.z}
    }
}

impl ops::Sub<Vec4> for f32 {
    type Output = Vec4;
    fn sub(self, rhs: Vec4) -> Vec4 {
        Vec4 {x: self -rhs.x, y: self -rhs.y, z: self -rhs.z, w: self -rhs.z}
    }
}

impl ops::Mul<Vec4> for f32 {
    type Output = Vec4;
    fn mul(self, rhs: Vec4) -> Vec4 {
        Vec4 {x: self *rhs.x, y: self *rhs.y, z: self *rhs.z, w: self *rhs.z}
    }
}

impl ops::Div<Vec4> for f32 {
    type Output = Vec4;
    fn div(self, rhs: Vec4) -> Vec4 {
        Vec4 {x: self / rhs.x, y: self / rhs.y, z: self / rhs.z, w: self / rhs.z}
    }
}


impl ops::Add<f32> for Vec4 {
    type Output = Vec4;
    fn add(self, rhs: f32) -> Vec4 {
        Vec4 {x: self.x + rhs, y: self.y + rhs, z: self.z + rhs, w: self.w + rhs}
    }
}

impl ops::Sub<f32> for Vec4 {
    type Output = Vec4;
    fn sub(self, rhs: f32) -> Vec4 {
        Vec4 {x: self.x - rhs, y: self.y - rhs, z: self.z - rhs, w: self.w - rhs}
    }
}

impl ops::Mul<f32> for Vec4 {
    type Output = Vec4;
    fn mul(self, rhs: f32) -> Vec4 {
        Vec4 {x: self.x * rhs, y: self.y * rhs, z: self.z * rhs, w: self.w * rhs}
    }
}

impl ops::Div<f32> for Vec4 {
    type Output = Vec4;
    fn div(self, rhs: f32) -> Vec4 {
        Vec4 {x: self.x / rhs, y: self.y / rhs, z: self.z / rhs, w: self.w / rhs}
    }
}

impl ops::AddAssign<Vec4> for Vec4 {
    fn add_assign(&mut self, rhs: Vec4) {
        self.x = self.x + rhs.x;
        self.y = self.y + rhs.y;
        self.z = self.z + rhs.z;
        self.w = self.w + rhs.w;
    }
}

impl ops::SubAssign<Vec4> for Vec4 {
    fn sub_assign(&mut self, rhs: Vec4) {
        self.x = self.x - rhs.x;
        self.y = self.y - rhs.y;
        self.z = self.z - rhs.z;
        self.w = self.w - rhs.w;
    }
}

impl ops::MulAssign<Vec4> for Vec4 {
    fn mul_assign(&mut self, rhs: Vec4) {
        self.x = self.x * rhs.x;
        self.y = self.y * rhs.y;
        self.z = self.z * rhs.z;
        self.w = self.w * rhs.w;
    }
}

impl ops::DivAssign<Vec4> for Vec4 {
    fn div_assign(&mut self, rhs: Vec4) {
        self.x = self.x / rhs.x;
        self.y = self.y / rhs.y;
        self.z = self.z / rhs.z;
        self.w = self.w / rhs.w;
    }
}


impl ops::AddAssign<f32> for Vec4 {
    fn add_assign(&mut self, rhs: f32) {
        self.x = self.x + rhs;
        self.y = self.y + rhs;
        self.z = self.z + rhs;
        self.w = self.w + rhs;
    }
}

impl ops::SubAssign<f32> for Vec4 {
    fn sub_assign(&mut self, rhs: f32) {
        self.x = self.x - rhs;
        self.y = self.y - rhs;
        self.z = self.z - rhs;
        self.w = self.w - rhs;
    }
}

impl ops::MulAssign<f32> for Vec4 {
    fn mul_assign(&mut self, rhs: f32) {
        self.x = self.x * rhs;
        self.y = self.y * rhs;
        self.z = self.z * rhs;
        self.w = self.w * rhs;
    }
}

impl ops::DivAssign<f32> for Vec4 {
    fn div_assign(&mut self, rhs: f32) {
        self.x = self.x / rhs;
        self.y = self.y / rhs;
        self.z = self.z / rhs;
        self.w = self.w / rhs;
    }
}


