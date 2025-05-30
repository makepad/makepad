use{
    std::{fmt,ops},
    crate::math_f64::*,
    makepad_micro_serde::*,
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

pub const VF00:Vec4 = Vec4{x:1.0,y:0.0,z:0.0,w:1.0};
pub const V0F0:Vec4 = Vec4{x:0.0,y:1.0,z:0.0,w:1.0};
pub const V00F:Vec4 = Vec4{x:0.0,y:0.0,z:1.0,w:1.0};

#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(C)]
pub struct Mat4 {
    pub v: [f32; 16],
}

impl Default for Mat4{
    fn default()->Self{
        Self{v:[1.,0.,0.,0., 0.,1.,0.,0., 0.,0.,1.,0., 0.,0.,0.,1.]}
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Vec2Index{
    X,
    Y
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Debug, SerBin, DeBin)]
pub struct Pose {
    pub orientation: Quat,
    pub position: Vec3
}

impl Pose {
    pub fn new(orientation:Quat, position:Vec3)->Self{
        Self{
            orientation,
            position
        }
    }
    
    pub fn transform_vec3(&self, v:&Vec3)->Vec3{
        let r0 = self.orientation.rotate_vec3(v);
        r0 + self.position
    }
    pub fn multiply(a:&Pose, b:&Pose)->Self{
        Self{
            orientation: Quat::multiply(&b.orientation, &a.orientation),
            position: a.transform_vec3(&b.position)
        }
    }
    pub fn invert(&self)->Self{
        let orientation = self.orientation.invert();
        let neg_pos = self.position.scale(-1.0);
        Self{
            orientation,
            position: orientation.rotate_vec3(&neg_pos),
        }
    }
    
    pub fn to_mat4(&self) -> Mat4 {
        
        let q = self.orientation;
        let t = self.position;/*
        Mat4 {v: [
            (1.0 - 2.0 * q.y * q.y - 2.0 * q.z * q.z),
            (2.0 * q.x * q.y - 2.0 * q.z * q.w),
            (2.0 * q.x * q.z + 2.0 * q.y * q.w),
            t.x,
            (2.0 * q.x * q.y + 2.0 * q.z * q.w),
            (1.0 - 2.0 * q.x * q.x - 2.0 * q.z * q.z),
            (2.0 * q.y * q.z - 2.0 * q.x * q.w),
            t.y,
            (2.0 * q.x * q.z - 2.0 * q.y * q.w),
            (2.0 * q.y * q.z + 2.0 * q.x * q.w),
            (1.0 - 2.0 * q.x * q.x - 2.0 * q.y * q.y),
            t.z,
            0.0,
            0.0,
            0.0,
            1.0
        ]}*/
        Mat4 {v: [
            (1.0 - 2.0 * q.y * q.y - 2.0 * q.z * q.z),
            (2.0 * q.x * q.y + 2.0 * q.z * q.w),
            (2.0 * q.x * q.z - 2.0 * q.y * q.w),
            0.0,
            (2.0 * q.x * q.y - 2.0 * q.z * q.w),
            (1.0 - 2.0 * q.x * q.x - 2.0 * q.z * q.z),
            (2.0 * q.y * q.z + 2.0 * q.x * q.w),
            0.0,
            (2.0 * q.x * q.z + 2.0 * q.y * q.w),
            (2.0 * q.y * q.z - 2.0 * q.x * q.w),
            (1.0 - 2.0 * q.x * q.x - 2.0 * q.y * q.y),
            0.0,
            t.x,
            t.y,
            t.z,
            1.0
        ]}
    }
    
    pub fn from_lerp(a: Pose, b: Pose, f: f32) -> Self {
        Pose {
            orientation: Quat::from_slerp(a.orientation, b.orientation, f),
            position: Vec3::from_lerp(a.position, b.position, f)
        }
    }
    
    pub fn from_slerp_orientation(a: Pose, b: Pose, f: f32) -> Self {
        Pose {
            orientation: Quat::from_slerp(a.orientation, b.orientation, f),
            position: b.position
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, SerBin, DeBin)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}


impl Vec2 {

    pub fn new() -> Vec2 {
        Vec2::default()
    }
    
    pub fn index(&self, index:Vec2Index)->f32{
        match index{
            Vec2Index::X=>self.x,
            Vec2Index::Y=>self.y
        }
    }

    pub fn set_index(&mut self, index:Vec2Index, v: f32){
        match index{
            Vec2Index::X=>{self.x = v},
            Vec2Index::Y=>{self.y = v}
        }
    }


    pub fn from_index_pair(index:Vec2Index, a: f32, b:f32)->Self{
        match index{
            Vec2Index::X=>{Self{x:a,y:b}},
            Vec2Index::Y=>{Self{x:b,y:a}}
        }
    }
    

    pub fn into_dvec2(self)->DVec2{
        DVec2{x:self.x as f64, y:self.y as f64}
    }
    
    pub fn all(x: f32) -> Vec2 {
        Vec2 {x, y: x}
    }
    
    pub fn from_lerp(a: Vec2, b: Vec2, f: f32) -> Vec2 {
        let nf = 1.0 - f;
        Vec2{
            x: nf * a.x + f * b.x,
            y: nf * a.y + f * b.y,
        }
    }
    
    pub fn distance(&self, other: &Vec2) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
    
    pub fn angle_in_radians(&self) -> f32 {
        self.y.atan2(self.x)
    }
    
    pub fn angle_in_degrees(&self) -> f32 {
        self.y.atan2(self.x) * (360.0 / (2. * std::f32::consts::PI))
    }


    pub fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    pub fn lengthsquared(&self) -> f32 {
        self.x * self.x + self.y * self.y
    }
    pub fn normalize(&self) -> Vec2
    {
        let l  = self.length();
        if l == 0.0 {return vec2(0.,0.);}
        return vec2(self.x/l, self.y/l);
    }
    pub fn normalize_to_x(&self) -> Vec2
    {
        let l  = self.x;
        if l == 0.0 {return vec2(1.,0.);}
        return vec2(1., self.y/l);
    }
   
    pub fn normalize_to_y(&self) -> Vec2
    {
        let l  = self.y;
        if l == 0.0 {return vec2(1.,0.);}
        return vec2(self.x/l, 1.);
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

pub const fn vec2(x: f32, y: f32) -> Vec2 {Vec2 {x, y}}
pub const fn vec3(x: f32, y: f32, z: f32) -> Vec3 {Vec3 {x, y, z}}
pub const fn vec4(x: f32, y: f32, z: f32, w: f32) -> Vec4 {Vec4 {x, y, z, w}}

const TORAD: f32 = 0.017453292;
const TODEG: f32 = 57.29578;

/*
pub fn vec2(x:f32, y:f32)->Vec2{
    Vec2{x:x, y:y}
}*/

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Debug, SerBin, DeBin)]
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
    
    pub fn zero(&mut self)
    {
        self.x = 0.0;
        self.y = 0.0;
        self.z = 0.0;
    }
    
    pub const fn all(x: f32) -> Vec3 {
        Vec3 {x, y: x, z: x}
    }
    
    pub const fn to_vec2(&self) -> Vec2 {
        Vec2 {x: self.x, y: self.y}
    }
        
    pub const fn to_vec4(&self) -> Vec4 {
        Vec4 {x: self.x, y: self.y, z: self.z, w: 1.0}
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
    
    pub fn length(&self) -> f32 {
        let sz = self.x * self.x + self.y * self.y + self.z * self.z;
        sz.sqrt()
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
        Self::from_point_normal(p1, normal)
    }
    
    pub fn intersect_line(&self, v1: Vec3, v2: Vec3) -> Vec3 {
        let diff = v1 - v2;
        let denom = self.a * diff.x + self.b * diff.y + self.c * diff.z;
        if denom == 0.0 {
            return (v1 * v2) * 0.5
        }
        let u = (self.a * v1.x + self.b * v1.y + self.c * v1.z + self.d) / denom;
        v1 + (v2 - v1) * u
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug,PartialEq, SerBin, DeBin)]
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
    pub const R: Vec4 = Vec4 {x: 1.0, y: 0.0, z: 0.0, w: 1.0};
    pub const G: Vec4 = Vec4 {x: 0.0, y: 1.0, z: 0.0, w: 1.0};
    pub const B: Vec4 = Vec4 {x: 0.0, y: 0.0, z: 1.0, w: 1.0};

    
    pub const fn all(v: f32) -> Self {
        Self {x: v, y: v, z: v, w: v}
    }
    
    pub const fn to_vec3(&self) -> Vec3 {
        Vec3 {x: self.x, y: self.y, z: self.z}
    }
    
    pub fn dot(&self, other: Vec4) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }
    
    pub fn from_lerp(a: Vec4, b: Vec4, f: f32) -> Vec4 {
        let nf = 1.0 - f;
        Vec4 {
            x: nf * a.x + f * b.x,
            y: nf * a.y + f * b.y,
            z: nf * a.z + f * b.z,
            w: nf * a.w + f * b.w,
        }
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
        Vec4 {
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
            w: (val & 0xff) as f32 / 255.0,
        }
    }
    
    pub fn to_u32(&self) -> u32 {
        let r = (self.x * 255.0) as u8 as u32;
        let g = (self.y * 255.0) as u8 as u32;
        let b = (self.z * 255.0) as u8 as u32;
        let a = (self.w * 255.0) as u8 as u32;
        (r<<24)|(g<<16)|(b<<8)|a
    }

    pub const fn xy(&self) -> Vec2 {
        Vec2{x:self.x, y:self.y}
    }

    pub const fn zw(&self) -> Vec2 {
        Vec2{x:self.z, y:self.w}
    }

}

impl From<(DVec2,DVec2)> for Vec4{
    fn from(other:(DVec2,DVec2))->Vec4{
        vec4(other.0.x as f32, other.0.y as f32, other.1.x as f32, other.1.y as f32)
    }
}


#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct CameraFov {
    pub angle_left: f32,
    pub angle_right: f32,
    pub angle_up: f32,
    pub angle_down: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, SerBin, DeBin)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32
}

impl Default for Quat{
    fn default()->Self{Self{x:0.0,y:0.0,z:0.0,w:1.0}}
}

impl Quat {
    pub fn multiply(a:&Quat, b:&Quat)->Self{
        Self{
            x:(b.w * a.x) + (b.x * a.w) + (b.y * a.z) - (b.z * a.y),
            y:(b.w * a.y) - (b.x * a.z) + (b.y * a.w) + (b.z * a.x),
            z:(b.w * a.z) + (b.x * a.y) - (b.y * a.x) + (b.z * a.w),
            w:(b.w * a.w) - (b.x * a.x) - (b.y * a.y) - (b.z * a.z)
        }
    }
        
    pub fn invert(&self)->Self{
        Self{
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }
        
    pub fn rotate_vec3(&self, v:&Vec3)->Vec3{
        let q = Quat{x:v.x, y:v.y, z:v.z, w:0.0};
        let aq = Quat::multiply(&q, self);
        let ainv = self.invert();
        let aqainv = Quat::multiply(&ainv, &aq);
        Vec3{x: aqainv.x, y: aqainv.y, z: aqainv.z}
    }
    
    pub fn dot(&self, other: Quat) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }
    
    pub fn neg(&self) -> Quat {
        Quat {x: -self.x, y: -self.y, z: -self.z, w: -self.w}
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
            x: scale0 * n.x + scale1 * m.x,
            y: scale0 * n.y + scale1 * m.y,
            z: scale0 * n.z + scale1 * m.z,
            w: scale0 * m.w + scale1 * m.w
        }).normalized()
    }
    
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    
    pub fn normalized(&mut self) -> Quat {
        let len = self.length();
        Quat {
            x: self.x / len,
            y: self.y / len,
            z: self.z / len,
            w: self.w / len,
        }
    }
    
    pub fn look_rotation(forward: Vec3, up:Vec3)->Self{
        let forward = forward.normalize();
        let up = up.normalize();
        let v2 = forward;
        let v0 = Vec3::cross(up, forward).normalize();
        let v1 = Vec3::cross(v2, v0);
        
        let num = (v0.x + v1.y) + v2.z;
        if num > 0.0{
            let num = (num+1.0).sqrt();
            let numh = 0.5 / num;
            return Quat{
                x: (v1.z - v2.y) * numh,
                y: (v2.x - v0.z) * numh,
                z: (v0.y - v1.x) * numh,
                w: num * 0.5,
            }
        }
        if (v0.x >= v1.y) && (v0.x >= v2.z){
            let num = (((1.0+v0.x) - v1.y) - v2.z).sqrt();
            let numh = 0.5 / num;
            return Quat{
                x: 0.5 * num,
                y: (v0.y + v1.x) * numh,
                z: (v0.z + v2.x) * numh,
                w: (v1.z - v2.y) * numh
            }
        }
        if v1.y > v2.z{
            let num = ((((1.0+v1.y) - v0.x) - v2.z)).sqrt();
            let numh = 0.5 / num;
            return Quat{
                x: (v1.x + v0.y) * numh,
                y: 0.5 * num,
                z: (v2.y + v1.z) * numh,
                w: (v2.x - v0.z) * numh
            }
        }
        let num = (((1.0 + v2.z) - v0.x) - v1.y).sqrt();
        let numh = 0.5 / num;
        Quat{
            x: (v2.x + v0.z) * numh,
            y: (v2.y + v1.z) * numh,
            z: 0.5 * num,
            w: (v0.y - v1.x) * numh
        }
    }
    
}

/*
pub fn vec4(x:f32, y:f32, z:f32, w:f32)->Vec4{
    Vec4{x:x, y:y, z:z, w:w}
}*/


impl Mat4 {
    pub const fn identity() -> Mat4 {
        Mat4 {v: [
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
    
    pub fn transpose(&self) -> Mat4 {
        Mat4 {v: [
            self.v[0],
            self.v[4],
            self.v[8],
            self.v[12],
            self.v[1],
            self.v[5],
            self.v[9],
            self.v[13],
            self.v[2],
            self.v[6],
            self.v[10],
            self.v[14],
            self.v[3],
            self.v[7],
            self.v[11],
            self.v[15],
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
        Mat4 {v: [
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
        Mat4 {v: [
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
    
    pub fn from_camera_fov(fov:&CameraFov, near: f32, far: f32) -> Mat4 {
        let tan_left = fov.angle_left.tan();
        let tan_right = fov.angle_right.tan();
        let tan_down = fov.angle_down.tan();
        let tan_up = fov.angle_up.tan();
        
        let tan_height = tan_up - tan_down;
        let tan_width = tan_right - tan_left;
        
        if far <= near{
            Mat4 {v: [
                2.0 / tan_width,
                0.0,
                0.0,
                0.0,
                
                0.0,
                2.0 / tan_height,
                0.0,
                0.0,
                
                (tan_right + tan_left) / tan_width,
                (tan_up + tan_down) / tan_height,
                -1.0,
                -1.0,
                
                0.0,
                0.0,
                - 2.0 * near,
                0.0,
            ]}
        }
        else{
            Mat4 {v: [
                2.0 / tan_width,
                0.0,
                0.0,
                0.0,
                                
                0.0,
                2.0 / tan_height,
                0.0,
                0.0,
                                
                (tan_right + tan_left) / tan_width,
                (tan_up + tan_down) / tan_height,
                -(far + near) / (far - near),
                -1.0,
                                
                0.0,
                0.0,
                -(far * 2.0 * near)/ (far - near),
                0.0,
            ]}
        }
    }
    
    pub const fn translation(v:Vec3) -> Mat4 {
        Mat4 {v: [
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
            v.x,
            v.y,
            v.z,
            1.0
        ]}
        
    }
    
    pub const fn nonuniform_scaled_translation(s:Vec3,  t:Vec3) -> Mat4 {
        Mat4 {v: [
            s.x,
            0.0,
            0.0,
            0.0,
            0.0,
            s.y,
            0.0,
            0.0,
            0.0,
            0.0,
            s.z,
            0.0,
            t.x,
            t.y,
            t.z,
            1.0
        ]}
    }
    
    pub const fn scaled_translation(s:f32,  t:Vec3) -> Mat4 {
        Mat4 {v: [
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
            t.x,
            t.y,
            t.z,
            1.0
        ]}
    }
        
    pub const fn scale(s: f32) -> Mat4 {
        Mat4 {v: [
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
            0.0,
            0.0,
            0.0,
            1.0
        ]}
                
    }
    
    pub fn rotation(r:Vec3) -> Mat4 {
        //const TORAD: f32 = 0.017453292;
        let cx = f32::cos(r.x);
        let cy = f32::cos(r.y);
        let cz = f32::cos(r.z);
        let sx = f32::sin(r.x);
        let sy = f32::sin(r.y);
        let sz = f32::sin(r.z);
        let m0 = cy * cz + sx * sy * sz;
        let m1 = -sz * cy + cz * sx * sy;
        let m2 = sy * cx;
        let m4 = sz * cx;
        let m5 = cx * cz;
        let m6 = -sx;
        let m8 = -sy * cz + cy * sx * sz;
        let m9 = sy * sz + cy * sx * cz;
        let m10 = cx * cy;
        Mat4 {v: [
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
        Mat4 {v: [
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
        #[inline]
        fn d(i: &[f32; 16], x: usize, y: usize) -> f32 {i[x + 4 * y]}
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
        Mat4 {
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


