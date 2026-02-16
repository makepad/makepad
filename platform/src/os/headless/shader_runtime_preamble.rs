// ─── Makepad Headless Shader Runtime Preamble ───
// This file is included verbatim in every JIT-compiled shader module.
// It provides Vec2f/Vec3f/Vec4f/Mat4f types, operators, constructors,
// swizzle methods, Sdf2d, Texture2D, and shader builtin functions.
// NO external crate dependencies.

use std::ops;

// ─── Vector types ───

#[derive(Clone, Copy, Default, PartialEq, Debug)]
#[repr(C)]
pub struct Vec2f {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
#[repr(C)]
pub struct Vec3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
#[repr(C)]
pub struct Vec4f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(C)]
pub struct Mat4f {
    pub v: [f32; 16],
}

impl Default for Mat4f {
    fn default() -> Self {
        Self {
            v: [
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        }
    }
}

// ─── Type aliases (shader compiler emits lowercase names) ───

pub type vec2f = Vec2f;
pub type vec3f = Vec3f;
pub type vec4f = Vec4f;
pub type mat4x4f = Mat4f;

// ─── Constructors ───

pub const fn vec2(x: f32, y: f32) -> Vec2f {
    Vec2f { x, y }
}
pub const fn vec3(x: f32, y: f32, z: f32) -> Vec3f {
    Vec3f { x, y, z }
}
pub const fn vec4(x: f32, y: f32, z: f32, w: f32) -> Vec4f {
    Vec4f { x, y, z, w }
}

// Constructor aliases (shader compiler emits vec2f(...) etc.)
pub const fn vec2f(x: f32, y: f32) -> Vec2f {
    Vec2f { x, y }
}
pub const fn vec3f(x: f32, y: f32, z: f32) -> Vec3f {
    Vec3f { x, y, z }
}
pub const fn vec4f(x: f32, y: f32, z: f32, w: f32) -> Vec4f {
    Vec4f { x, y, z, w }
}

// ─── Vec2f operators ───

impl ops::Add for Vec2f {
    type Output = Vec2f;
    fn add(self, r: Vec2f) -> Vec2f {
        vec2(self.x + r.x, self.y + r.y)
    }
}
impl ops::Sub for Vec2f {
    type Output = Vec2f;
    fn sub(self, r: Vec2f) -> Vec2f {
        vec2(self.x - r.x, self.y - r.y)
    }
}
impl ops::Mul for Vec2f {
    type Output = Vec2f;
    fn mul(self, r: Vec2f) -> Vec2f {
        vec2(self.x * r.x, self.y * r.y)
    }
}
impl ops::Div for Vec2f {
    type Output = Vec2f;
    fn div(self, r: Vec2f) -> Vec2f {
        vec2(self.x / r.x, self.y / r.y)
    }
}
impl ops::Neg for Vec2f {
    type Output = Vec2f;
    fn neg(self) -> Vec2f {
        vec2(-self.x, -self.y)
    }
}
impl ops::Mul<f32> for Vec2f {
    type Output = Vec2f;
    fn mul(self, s: f32) -> Vec2f {
        vec2(self.x * s, self.y * s)
    }
}
impl ops::Mul<Vec2f> for f32 {
    type Output = Vec2f;
    fn mul(self, v: Vec2f) -> Vec2f {
        vec2(self * v.x, self * v.y)
    }
}
impl ops::Div<f32> for Vec2f {
    type Output = Vec2f;
    fn div(self, s: f32) -> Vec2f {
        vec2(self.x / s, self.y / s)
    }
}
impl ops::Add<f32> for Vec2f {
    type Output = Vec2f;
    fn add(self, s: f32) -> Vec2f {
        vec2(self.x + s, self.y + s)
    }
}
impl ops::Sub<f32> for Vec2f {
    type Output = Vec2f;
    fn sub(self, s: f32) -> Vec2f {
        vec2(self.x - s, self.y - s)
    }
}

// ─── Vec3f operators ───

impl ops::Add for Vec3f {
    type Output = Vec3f;
    fn add(self, r: Vec3f) -> Vec3f {
        vec3(self.x + r.x, self.y + r.y, self.z + r.z)
    }
}
impl ops::Sub for Vec3f {
    type Output = Vec3f;
    fn sub(self, r: Vec3f) -> Vec3f {
        vec3(self.x - r.x, self.y - r.y, self.z - r.z)
    }
}
impl ops::Mul for Vec3f {
    type Output = Vec3f;
    fn mul(self, r: Vec3f) -> Vec3f {
        vec3(self.x * r.x, self.y * r.y, self.z * r.z)
    }
}
impl ops::Div for Vec3f {
    type Output = Vec3f;
    fn div(self, r: Vec3f) -> Vec3f {
        vec3(self.x / r.x, self.y / r.y, self.z / r.z)
    }
}
impl ops::Neg for Vec3f {
    type Output = Vec3f;
    fn neg(self) -> Vec3f {
        vec3(-self.x, -self.y, -self.z)
    }
}
impl ops::Mul<f32> for Vec3f {
    type Output = Vec3f;
    fn mul(self, s: f32) -> Vec3f {
        vec3(self.x * s, self.y * s, self.z * s)
    }
}
impl ops::Mul<Vec3f> for f32 {
    type Output = Vec3f;
    fn mul(self, v: Vec3f) -> Vec3f {
        vec3(self * v.x, self * v.y, self * v.z)
    }
}
impl ops::Div<f32> for Vec3f {
    type Output = Vec3f;
    fn div(self, s: f32) -> Vec3f {
        vec3(self.x / s, self.y / s, self.z / s)
    }
}
impl ops::Add<f32> for Vec3f {
    type Output = Vec3f;
    fn add(self, s: f32) -> Vec3f {
        vec3(self.x + s, self.y + s, self.z + s)
    }
}
impl ops::Sub<f32> for Vec3f {
    type Output = Vec3f;
    fn sub(self, s: f32) -> Vec3f {
        vec3(self.x - s, self.y - s, self.z - s)
    }
}

// ─── Vec4f operators ───

impl ops::Add for Vec4f {
    type Output = Vec4f;
    fn add(self, r: Vec4f) -> Vec4f {
        vec4(self.x + r.x, self.y + r.y, self.z + r.z, self.w + r.w)
    }
}
impl ops::Sub for Vec4f {
    type Output = Vec4f;
    fn sub(self, r: Vec4f) -> Vec4f {
        vec4(self.x - r.x, self.y - r.y, self.z - r.z, self.w - r.w)
    }
}
impl ops::Mul for Vec4f {
    type Output = Vec4f;
    fn mul(self, r: Vec4f) -> Vec4f {
        vec4(self.x * r.x, self.y * r.y, self.z * r.z, self.w * r.w)
    }
}
impl ops::Div for Vec4f {
    type Output = Vec4f;
    fn div(self, r: Vec4f) -> Vec4f {
        vec4(self.x / r.x, self.y / r.y, self.z / r.z, self.w / r.w)
    }
}
impl ops::Neg for Vec4f {
    type Output = Vec4f;
    fn neg(self) -> Vec4f {
        vec4(-self.x, -self.y, -self.z, -self.w)
    }
}
impl ops::Mul<f32> for Vec4f {
    type Output = Vec4f;
    fn mul(self, s: f32) -> Vec4f {
        vec4(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}
impl ops::Mul<Vec4f> for f32 {
    type Output = Vec4f;
    fn mul(self, v: Vec4f) -> Vec4f {
        vec4(self * v.x, self * v.y, self * v.z, self * v.w)
    }
}
impl ops::Div<f32> for Vec4f {
    type Output = Vec4f;
    fn div(self, s: f32) -> Vec4f {
        vec4(self.x / s, self.y / s, self.z / s, self.w / s)
    }
}
impl ops::Add<f32> for Vec4f {
    type Output = Vec4f;
    fn add(self, s: f32) -> Vec4f {
        vec4(self.x + s, self.y + s, self.z + s, self.w + s)
    }
}
impl ops::Sub<f32> for Vec4f {
    type Output = Vec4f;
    fn sub(self, s: f32) -> Vec4f {
        vec4(self.x - s, self.y - s, self.z - s, self.w - s)
    }
}

// ─── AddAssign / SubAssign / MulAssign / DivAssign ───

impl ops::AddAssign for Vec2f {
    fn add_assign(&mut self, r: Vec2f) {
        self.x += r.x;
        self.y += r.y;
    }
}
impl ops::SubAssign for Vec2f {
    fn sub_assign(&mut self, r: Vec2f) {
        self.x -= r.x;
        self.y -= r.y;
    }
}
impl ops::MulAssign<f32> for Vec2f {
    fn mul_assign(&mut self, s: f32) {
        self.x *= s;
        self.y *= s;
    }
}
impl ops::DivAssign<f32> for Vec2f {
    fn div_assign(&mut self, s: f32) {
        self.x /= s;
        self.y /= s;
    }
}

impl ops::AddAssign for Vec3f {
    fn add_assign(&mut self, r: Vec3f) {
        self.x += r.x;
        self.y += r.y;
        self.z += r.z;
    }
}
impl ops::SubAssign for Vec3f {
    fn sub_assign(&mut self, r: Vec3f) {
        self.x -= r.x;
        self.y -= r.y;
        self.z -= r.z;
    }
}
impl ops::MulAssign<f32> for Vec3f {
    fn mul_assign(&mut self, s: f32) {
        self.x *= s;
        self.y *= s;
        self.z *= s;
    }
}
impl ops::DivAssign<f32> for Vec3f {
    fn div_assign(&mut self, s: f32) {
        self.x /= s;
        self.y /= s;
        self.z /= s;
    }
}

impl ops::AddAssign for Vec4f {
    fn add_assign(&mut self, r: Vec4f) {
        self.x += r.x;
        self.y += r.y;
        self.z += r.z;
        self.w += r.w;
    }
}
impl ops::SubAssign for Vec4f {
    fn sub_assign(&mut self, r: Vec4f) {
        self.x -= r.x;
        self.y -= r.y;
        self.z -= r.z;
        self.w -= r.w;
    }
}
impl ops::MulAssign<f32> for Vec4f {
    fn mul_assign(&mut self, s: f32) {
        self.x *= s;
        self.y *= s;
        self.z *= s;
        self.w *= s;
    }
}
impl ops::DivAssign<f32> for Vec4f {
    fn div_assign(&mut self, s: f32) {
        self.x /= s;
        self.y /= s;
        self.z /= s;
        self.w /= s;
    }
}

// ─── Swizzle methods ───

impl Vec2f {
    pub fn xx(&self) -> Vec2f {
        vec2(self.x, self.x)
    }
    pub fn xy(&self) -> Vec2f {
        vec2(self.x, self.y)
    }
    pub fn yx(&self) -> Vec2f {
        vec2(self.y, self.x)
    }
    pub fn yy(&self) -> Vec2f {
        vec2(self.y, self.y)
    }
    pub fn xxx(&self) -> Vec3f {
        vec3(self.x, self.x, self.x)
    }
    pub fn xxy(&self) -> Vec3f {
        vec3(self.x, self.x, self.y)
    }
    pub fn xyx(&self) -> Vec3f {
        vec3(self.x, self.y, self.x)
    }
    pub fn xyy(&self) -> Vec3f {
        vec3(self.x, self.y, self.y)
    }
    pub fn yxx(&self) -> Vec3f {
        vec3(self.y, self.x, self.x)
    }
    pub fn yxy(&self) -> Vec3f {
        vec3(self.y, self.x, self.y)
    }
    pub fn yyx(&self) -> Vec3f {
        vec3(self.y, self.y, self.x)
    }
    pub fn yyy(&self) -> Vec3f {
        vec3(self.y, self.y, self.y)
    }
    pub fn xxxx(&self) -> Vec4f {
        vec4(self.x, self.x, self.x, self.x)
    }
    pub fn xxyy(&self) -> Vec4f {
        vec4(self.x, self.x, self.y, self.y)
    }
    pub fn xyxy(&self) -> Vec4f {
        vec4(self.x, self.y, self.x, self.y)
    }
    pub fn yyxx(&self) -> Vec4f {
        vec4(self.y, self.y, self.x, self.x)
    }
    pub fn mix(&self, other: Vec2f, t: f32) -> Vec2f {
        vec2(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }
    pub fn atan2(&self) -> f32 {
        self.y.atan2(self.x)
    }
}

impl Vec3f {
    pub fn xx(&self) -> Vec2f {
        vec2(self.x, self.x)
    }
    pub fn xy(&self) -> Vec2f {
        vec2(self.x, self.y)
    }
    pub fn xz(&self) -> Vec2f {
        vec2(self.x, self.z)
    }
    pub fn yx(&self) -> Vec2f {
        vec2(self.y, self.x)
    }
    pub fn yy(&self) -> Vec2f {
        vec2(self.y, self.y)
    }
    pub fn yz(&self) -> Vec2f {
        vec2(self.y, self.z)
    }
    pub fn zx(&self) -> Vec2f {
        vec2(self.z, self.x)
    }
    pub fn zy(&self) -> Vec2f {
        vec2(self.z, self.y)
    }
    pub fn zz(&self) -> Vec2f {
        vec2(self.z, self.z)
    }
    pub fn xxx(&self) -> Vec3f {
        vec3(self.x, self.x, self.x)
    }
    pub fn xxy(&self) -> Vec3f {
        vec3(self.x, self.x, self.y)
    }
    pub fn xxz(&self) -> Vec3f {
        vec3(self.x, self.x, self.z)
    }
    pub fn xyz(&self) -> Vec3f {
        vec3(self.x, self.y, self.z)
    }
    pub fn xzy(&self) -> Vec3f {
        vec3(self.x, self.z, self.y)
    }
    pub fn yxz(&self) -> Vec3f {
        vec3(self.y, self.x, self.z)
    }
    pub fn yzx(&self) -> Vec3f {
        vec3(self.y, self.z, self.x)
    }
    pub fn zxy(&self) -> Vec3f {
        vec3(self.z, self.x, self.y)
    }
    pub fn zyx(&self) -> Vec3f {
        vec3(self.z, self.y, self.x)
    }
    pub fn zzz(&self) -> Vec3f {
        vec3(self.z, self.z, self.z)
    }
    pub fn xxxx(&self) -> Vec4f {
        vec4(self.x, self.x, self.x, self.x)
    }
    pub fn xyzx(&self) -> Vec4f {
        vec4(self.x, self.y, self.z, self.x)
    }
    pub fn xyzz(&self) -> Vec4f {
        vec4(self.x, self.y, self.z, self.z)
    }
    pub fn mix(&self, other: Vec3f, t: f32) -> Vec3f {
        vec3(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
            self.z + (other.z - self.z) * t,
        )
    }
}

impl Vec4f {
    pub fn xx(&self) -> Vec2f {
        vec2(self.x, self.x)
    }
    pub fn xy(&self) -> Vec2f {
        vec2(self.x, self.y)
    }
    pub fn xz(&self) -> Vec2f {
        vec2(self.x, self.z)
    }
    pub fn xw(&self) -> Vec2f {
        vec2(self.x, self.w)
    }
    pub fn yx(&self) -> Vec2f {
        vec2(self.y, self.x)
    }
    pub fn yy(&self) -> Vec2f {
        vec2(self.y, self.y)
    }
    pub fn yz(&self) -> Vec2f {
        vec2(self.y, self.z)
    }
    pub fn yw(&self) -> Vec2f {
        vec2(self.y, self.w)
    }
    pub fn zx(&self) -> Vec2f {
        vec2(self.z, self.x)
    }
    pub fn zy(&self) -> Vec2f {
        vec2(self.z, self.y)
    }
    pub fn zz(&self) -> Vec2f {
        vec2(self.z, self.z)
    }
    pub fn zw(&self) -> Vec2f {
        vec2(self.z, self.w)
    }
    pub fn wx(&self) -> Vec2f {
        vec2(self.w, self.x)
    }
    pub fn wy(&self) -> Vec2f {
        vec2(self.w, self.y)
    }
    pub fn wz(&self) -> Vec2f {
        vec2(self.w, self.z)
    }
    pub fn ww(&self) -> Vec2f {
        vec2(self.w, self.w)
    }
    pub fn xxx(&self) -> Vec3f {
        vec3(self.x, self.x, self.x)
    }
    pub fn xyz(&self) -> Vec3f {
        vec3(self.x, self.y, self.z)
    }
    pub fn xyw(&self) -> Vec3f {
        vec3(self.x, self.y, self.w)
    }
    pub fn xzy(&self) -> Vec3f {
        vec3(self.x, self.z, self.y)
    }
    pub fn yxz(&self) -> Vec3f {
        vec3(self.y, self.x, self.z)
    }
    pub fn yzw(&self) -> Vec3f {
        vec3(self.y, self.z, self.w)
    }
    pub fn zxy(&self) -> Vec3f {
        vec3(self.z, self.x, self.y)
    }
    pub fn zyx(&self) -> Vec3f {
        vec3(self.z, self.y, self.x)
    }
    pub fn zwx(&self) -> Vec3f {
        vec3(self.z, self.w, self.x)
    }
    pub fn wxy(&self) -> Vec3f {
        vec3(self.w, self.x, self.y)
    }
    pub fn wzx(&self) -> Vec3f {
        vec3(self.w, self.z, self.x)
    }
    pub fn rgb(&self) -> Vec3f {
        vec3(self.x, self.y, self.z)
    }
    pub fn xyzw(&self) -> Vec4f {
        vec4(self.x, self.y, self.z, self.w)
    }
    pub fn xxxx(&self) -> Vec4f {
        vec4(self.x, self.x, self.x, self.x)
    }
    pub fn yyyy(&self) -> Vec4f {
        vec4(self.y, self.y, self.y, self.y)
    }
    pub fn zzzz(&self) -> Vec4f {
        vec4(self.z, self.z, self.z, self.z)
    }
    pub fn wwww(&self) -> Vec4f {
        vec4(self.w, self.w, self.w, self.w)
    }
    pub fn wzyx(&self) -> Vec4f {
        vec4(self.w, self.z, self.y, self.x)
    }
    pub fn zyxw(&self) -> Vec4f {
        vec4(self.z, self.y, self.x, self.w)
    }
    pub fn rgba(&self) -> Vec4f {
        vec4(self.x, self.y, self.z, self.w)
    }
    pub fn mix(&self, other: Vec4f, t: f32) -> Vec4f {
        vec4(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
            self.z + (other.z - self.z) * t,
            self.w + (other.w - self.w) * t,
        )
    }
}

// ─── Mat4f multiply with Vec4f ───

impl ops::Mul<Vec4f> for Mat4f {
    type Output = Vec4f;
    fn mul(self, v: Vec4f) -> Vec4f {
        vec4(
            self.v[0] * v.x + self.v[4] * v.y + self.v[8] * v.z + self.v[12] * v.w,
            self.v[1] * v.x + self.v[5] * v.y + self.v[9] * v.z + self.v[13] * v.w,
            self.v[2] * v.x + self.v[6] * v.y + self.v[10] * v.z + self.v[14] * v.w,
            self.v[3] * v.x + self.v[7] * v.y + self.v[11] * v.z + self.v[15] * v.w,
        )
    }
}

// ─── Texture2D/Cube (POD — fits inside #[repr(C)] RenderCx) ───

/// Texture stored as POD fields so the whole RenderCx can be #[repr(C)].
/// The host writes (data_ptr, data_len, width, height) and the shader
/// reconstructs a slice in `sample()`. For cubemaps, data is six faces
/// packed in +X, -X, +Y, -Y, +Z, -Z order.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Texture2D {
    pub data_ptr: usize, // *const f32 as usize (0 = no data)
    pub data_len: usize, // number of f32 elements
    pub width: usize,
    pub height: usize,
}

impl Default for Texture2D {
    fn default() -> Self {
        Self {
            data_ptr: 0,
            data_len: 0,
            width: 0,
            height: 0,
        }
    }
}

impl Texture2D {
    fn data_slice(&self) -> Option<&[f32]> {
        if self.width == 0 || self.height == 0 || self.data_ptr == 0 || self.data_len == 0 {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(self.data_ptr as *const f32, self.data_len) })
    }

    fn face_stride_f32(&self) -> usize {
        self.width.saturating_mul(self.height).saturating_mul(4)
    }

    fn face_count(&self) -> usize {
        let stride = self.face_stride_f32();
        if stride == 0 {
            return 0;
        }
        self.data_len / stride
    }

    fn sample_face_from_data(&self, data: &[f32], face: usize, coord: Vec2f) -> Vec4f {
        if self.width == 0 || self.height == 0 {
            return vec4(0.0, 0.0, 0.0, 0.0);
        }
        let stride = self.face_stride_f32();
        if stride == 0 || face >= self.face_count() {
            return vec4(0.0, 0.0, 0.0, 0.0);
        }
        let base = face.saturating_mul(stride);

        let u = coord.x.max(0.0).min(1.0);
        let v = coord.y.max(0.0).min(1.0);
        // Bilinear sample with clamp-to-edge, matching GPU filtered text sampling.
        let fx = u * self.width as f32 - 0.5;
        let fy = v * self.height as f32 - 0.5;

        let x0f = fx.floor();
        let y0f = fy.floor();
        let tx = fx - x0f;
        let ty = fy - y0f;

        let x0 = x0f.max(0.0).min((self.width - 1) as f32) as usize;
        let y0 = y0f.max(0.0).min((self.height - 1) as f32) as usize;
        // Clamp the two taps independently from the unclamped base coordinate.
        // This matches clamp-to-edge behavior at low edges (u/v near 0).
        let x1 = (x0f + 1.0).max(0.0).min((self.width - 1) as f32) as usize;
        let y1 = (y0f + 1.0).max(0.0).min((self.height - 1) as f32) as usize;

        let sample_px = |x: usize, y: usize| -> Vec4f {
            let idx = base + (y * self.width + x) * 4;
            if idx + 3 < data.len() {
                vec4(data[idx], data[idx + 1], data[idx + 2], data[idx + 3])
            } else {
                vec4(0.0, 0.0, 0.0, 0.0)
            }
        };

        let c00 = sample_px(x0, y0);
        let c10 = sample_px(x1, y0);
        let c01 = sample_px(x0, y1);
        let c11 = sample_px(x1, y1);

        let c0 = c00 * (1.0 - tx) + c10 * tx;
        let c1 = c01 * (1.0 - tx) + c11 * tx;
        c0 * (1.0 - ty) + c1 * ty
    }

    fn sample_2d(&self, coord: Vec2f) -> Vec4f {
        let Some(data) = self.data_slice() else {
            return vec4(0.0, 0.0, 0.0, 0.0);
        };
        let wrapped = vec2(coord.x.rem_euclid(1.0), coord.y.rem_euclid(1.0));
        self.sample_face_from_data(data, 0, wrapped)
    }

    fn sample_cube(&self, dir: Vec3f) -> Vec4f {
        let Some(data) = self.data_slice() else {
            return vec4(0.0, 0.0, 0.0, 0.0);
        };
        if self.face_count() < 6 {
            return vec4(0.0, 0.0, 0.0, 0.0);
        }
        let d = normalize_3f(dir);
        let ax = d.x.abs();
        let ay = d.y.abs();
        let az = d.z.abs();

        let (face, u, v) = if ax >= ay && ax >= az {
            if d.x >= 0.0 {
                (0usize, -d.z / ax.max(1e-8), -d.y / ax.max(1e-8))
            } else {
                (1usize, d.z / ax.max(1e-8), -d.y / ax.max(1e-8))
            }
        } else if ay >= az {
            if d.y >= 0.0 {
                (2usize, d.x / ay.max(1e-8), d.z / ay.max(1e-8))
            } else {
                (3usize, d.x / ay.max(1e-8), -d.z / ay.max(1e-8))
            }
        } else if d.z >= 0.0 {
            (4usize, d.x / az.max(1e-8), -d.y / az.max(1e-8))
        } else {
            (5usize, -d.x / az.max(1e-8), -d.y / az.max(1e-8))
        };

        let uv = vec2(u * 0.5 + 0.5, v * 0.5 + 0.5);
        self.sample_face_from_data(data, face, uv)
    }

    pub fn sample<C: TextureSampleCoord>(&self, coord: C) -> Vec4f {
        coord.sample_texture(self)
    }
}

pub trait TextureSampleCoord {
    fn sample_texture(self, texture: &Texture2D) -> Vec4f;
}

impl TextureSampleCoord for Vec2f {
    fn sample_texture(self, texture: &Texture2D) -> Vec4f {
        texture.sample_2d(self)
    }
}

impl TextureSampleCoord for Vec3f {
    fn sample_texture(self, texture: &Texture2D) -> Vec4f {
        texture.sample_cube(self)
    }
}

// ─── Shader builtin functions ───

pub fn step(edge: f32, x: f32) -> f32 {
    if x < edge {
        0.0
    } else {
        1.0
    }
}

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).max(0.0).min(1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn sign(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}
pub fn sqrt(x: f32) -> f32 {
    x.sqrt()
}
pub fn inverse_sqrt(x: f32) -> f32 {
    1.0 / x.sqrt()
}
pub fn modf(x: f32, y: f32) -> f32 {
    x - y * (x / y).floor()
}
pub fn atan2(y: f32, x: f32) -> f32 {
    y.atan2(x)
}
pub fn fract(x: f32) -> f32 {
    x - x.floor()
}
pub fn mix_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    x.max(lo).min(hi)
}
pub fn clamp_2f(x: Vec2f, lo: Vec2f, hi: Vec2f) -> Vec2f {
    vec2(x.x.max(lo.x).min(hi.x), x.y.max(lo.y).min(hi.y))
}
pub fn clamp_3f(x: Vec3f, lo: Vec3f, hi: Vec3f) -> Vec3f {
    vec3(
        x.x.max(lo.x).min(hi.x),
        x.y.max(lo.y).min(hi.y),
        x.z.max(lo.z).min(hi.z),
    )
}
pub fn clamp_4f(x: Vec4f, lo: Vec4f, hi: Vec4f) -> Vec4f {
    vec4(
        x.x.max(lo.x).min(hi.x),
        x.y.max(lo.y).min(hi.y),
        x.z.max(lo.z).min(hi.z),
        x.w.max(lo.w).min(hi.w),
    )
}

// ─── Math builtins as free functions (shader compiler emits these) ───

pub fn max(a: f32, b: f32) -> f32 {
    a.max(b)
}
pub fn max_2f(a: Vec2f, b: Vec2f) -> Vec2f {
    vec2(a.x.max(b.x), a.y.max(b.y))
}
pub fn max_3f(a: Vec3f, b: Vec3f) -> Vec3f {
    vec3(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z))
}
pub fn max_4f(a: Vec4f, b: Vec4f) -> Vec4f {
    vec4(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z), a.w.max(b.w))
}
pub fn min(a: f32, b: f32) -> f32 {
    a.min(b)
}
pub fn min_2f(a: Vec2f, b: Vec2f) -> Vec2f {
    vec2(a.x.min(b.x), a.y.min(b.y))
}
pub fn min_3f(a: Vec3f, b: Vec3f) -> Vec3f {
    vec3(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z))
}
pub fn min_4f(a: Vec4f, b: Vec4f) -> Vec4f {
    vec4(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z), a.w.min(b.w))
}
pub fn abs(x: f32) -> f32 {
    x.abs()
}
pub fn abs_2f(v: Vec2f) -> Vec2f {
    vec2(v.x.abs(), v.y.abs())
}
pub fn abs_3f(v: Vec3f) -> Vec3f {
    vec3(v.x.abs(), v.y.abs(), v.z.abs())
}
pub fn abs_4f(v: Vec4f) -> Vec4f {
    vec4(v.x.abs(), v.y.abs(), v.z.abs(), v.w.abs())
}
pub fn floor(x: f32) -> f32 {
    x.floor()
}
pub fn ceil(x: f32) -> f32 {
    x.ceil()
}
pub fn pow(x: f32, y: f32) -> f32 {
    x.powf(y)
}
pub fn exp(x: f32) -> f32 {
    x.exp()
}
pub fn exp2(x: f32) -> f32 {
    (x * std::f32::consts::LN_2).exp()
}
pub fn log(x: f32) -> f32 {
    x.ln()
}
pub fn log2(x: f32) -> f32 {
    x.log2()
}
pub fn sin(x: f32) -> f32 {
    x.sin()
}
pub fn cos(x: f32) -> f32 {
    x.cos()
}
pub fn tan(x: f32) -> f32 {
    x.tan()
}
pub fn asin(x: f32) -> f32 {
    x.asin()
}
pub fn acos(x: f32) -> f32 {
    x.acos()
}
pub fn atan(x: f32) -> f32 {
    x.atan()
}
pub fn round(x: f32) -> f32 {
    x.round()
}

// ─── Vec-overloaded math builtins ───

pub fn floor_2f(v: Vec2f) -> Vec2f {
    vec2(v.x.floor(), v.y.floor())
}
pub fn floor_3f(v: Vec3f) -> Vec3f {
    vec3(v.x.floor(), v.y.floor(), v.z.floor())
}
pub fn floor_4f(v: Vec4f) -> Vec4f {
    vec4(v.x.floor(), v.y.floor(), v.z.floor(), v.w.floor())
}

pub fn ceil_2f(v: Vec2f) -> Vec2f {
    vec2(v.x.ceil(), v.y.ceil())
}
pub fn ceil_3f(v: Vec3f) -> Vec3f {
    vec3(v.x.ceil(), v.y.ceil(), v.z.ceil())
}
pub fn ceil_4f(v: Vec4f) -> Vec4f {
    vec4(v.x.ceil(), v.y.ceil(), v.z.ceil(), v.w.ceil())
}

pub fn fract_2f(v: Vec2f) -> Vec2f {
    vec2(fract(v.x), fract(v.y))
}
pub fn fract_3f(v: Vec3f) -> Vec3f {
    vec3(fract(v.x), fract(v.y), fract(v.z))
}
pub fn fract_4f(v: Vec4f) -> Vec4f {
    vec4(fract(v.x), fract(v.y), fract(v.z), fract(v.w))
}

pub fn round_2f(v: Vec2f) -> Vec2f {
    vec2(v.x.round(), v.y.round())
}
pub fn round_3f(v: Vec3f) -> Vec3f {
    vec3(v.x.round(), v.y.round(), v.z.round())
}
pub fn round_4f(v: Vec4f) -> Vec4f {
    vec4(v.x.round(), v.y.round(), v.z.round(), v.w.round())
}

pub fn sign_2f(v: Vec2f) -> Vec2f {
    vec2(sign(v.x), sign(v.y))
}
pub fn sign_3f(v: Vec3f) -> Vec3f {
    vec3(sign(v.x), sign(v.y), sign(v.z))
}
pub fn sign_4f(v: Vec4f) -> Vec4f {
    vec4(sign(v.x), sign(v.y), sign(v.z), sign(v.w))
}

pub fn sqrt_2f(v: Vec2f) -> Vec2f {
    vec2(v.x.sqrt(), v.y.sqrt())
}
pub fn sqrt_3f(v: Vec3f) -> Vec3f {
    vec3(v.x.sqrt(), v.y.sqrt(), v.z.sqrt())
}
pub fn sqrt_4f(v: Vec4f) -> Vec4f {
    vec4(v.x.sqrt(), v.y.sqrt(), v.z.sqrt(), v.w.sqrt())
}

pub fn sin_2f(v: Vec2f) -> Vec2f {
    vec2(v.x.sin(), v.y.sin())
}
pub fn sin_3f(v: Vec3f) -> Vec3f {
    vec3(v.x.sin(), v.y.sin(), v.z.sin())
}
pub fn sin_4f(v: Vec4f) -> Vec4f {
    vec4(v.x.sin(), v.y.sin(), v.z.sin(), v.w.sin())
}

pub fn cos_2f(v: Vec2f) -> Vec2f {
    vec2(v.x.cos(), v.y.cos())
}
pub fn cos_3f(v: Vec3f) -> Vec3f {
    vec3(v.x.cos(), v.y.cos(), v.z.cos())
}
pub fn cos_4f(v: Vec4f) -> Vec4f {
    vec4(v.x.cos(), v.y.cos(), v.z.cos(), v.w.cos())
}

pub fn step_2f(edge: Vec2f, x: Vec2f) -> Vec2f {
    vec2(step(edge.x, x.x), step(edge.y, x.y))
}
pub fn step_3f(edge: Vec3f, x: Vec3f) -> Vec3f {
    vec3(step(edge.x, x.x), step(edge.y, x.y), step(edge.z, x.z))
}
pub fn step_4f(edge: Vec4f, x: Vec4f) -> Vec4f {
    vec4(
        step(edge.x, x.x),
        step(edge.y, x.y),
        step(edge.z, x.z),
        step(edge.w, x.w),
    )
}

pub fn smoothstep_2f(edge0: Vec2f, edge1: Vec2f, x: Vec2f) -> Vec2f {
    vec2(
        smoothstep(edge0.x, edge1.x, x.x),
        smoothstep(edge0.y, edge1.y, x.y),
    )
}
pub fn smoothstep_3f(edge0: Vec3f, edge1: Vec3f, x: Vec3f) -> Vec3f {
    vec3(
        smoothstep(edge0.x, edge1.x, x.x),
        smoothstep(edge0.y, edge1.y, x.y),
        smoothstep(edge0.z, edge1.z, x.z),
    )
}
pub fn smoothstep_4f(edge0: Vec4f, edge1: Vec4f, x: Vec4f) -> Vec4f {
    vec4(
        smoothstep(edge0.x, edge1.x, x.x),
        smoothstep(edge0.y, edge1.y, x.y),
        smoothstep(edge0.z, edge1.z, x.z),
        smoothstep(edge0.w, edge1.w, x.w),
    )
}

// mix() free function - trait-based overloading for all types
pub trait Mix<T> {
    type Output;
    fn mix_impl(self, b: Self, t: T) -> Self::Output;
}

impl Mix<f32> for f32 {
    type Output = f32;
    fn mix_impl(self, b: f32, t: f32) -> f32 {
        self + (b - self) * t
    }
}
impl Mix<f32> for Vec2f {
    type Output = Vec2f;
    fn mix_impl(self, b: Vec2f, t: f32) -> Vec2f {
        vec2(self.x + (b.x - self.x) * t, self.y + (b.y - self.y) * t)
    }
}
impl Mix<Vec2f> for Vec2f {
    type Output = Vec2f;
    fn mix_impl(self, b: Vec2f, t: Vec2f) -> Vec2f {
        vec2(self.x + (b.x - self.x) * t.x, self.y + (b.y - self.y) * t.y)
    }
}
impl Mix<f32> for Vec3f {
    type Output = Vec3f;
    fn mix_impl(self, b: Vec3f, t: f32) -> Vec3f {
        vec3(
            self.x + (b.x - self.x) * t,
            self.y + (b.y - self.y) * t,
            self.z + (b.z - self.z) * t,
        )
    }
}
impl Mix<f32> for Vec4f {
    type Output = Vec4f;
    fn mix_impl(self, b: Vec4f, t: f32) -> Vec4f {
        vec4(
            self.x + (b.x - self.x) * t,
            self.y + (b.y - self.y) * t,
            self.z + (b.z - self.z) * t,
            self.w + (b.w - self.w) * t,
        )
    }
}

pub fn mix<A: Mix<T> + Copy, T: Copy>(a: A, b: A, t: T) -> A::Output {
    a.mix_impl(b, t)
}

pub fn distance_2f(a: Vec2f, b: Vec2f) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}
pub fn length_2f(v: Vec2f) -> f32 {
    (v.x * v.x + v.y * v.y).sqrt()
}
pub fn dot_2f(a: Vec2f, b: Vec2f) -> f32 {
    a.x * b.x + a.y * b.y
}
pub fn normalize_2f(v: Vec2f) -> Vec2f {
    let l = length_2f(v);
    if l > 0.0 {
        v / l
    } else {
        vec2(0.0, 0.0)
    }
}
pub fn dot_3f(a: Vec3f, b: Vec3f) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}
pub fn length_3f(v: Vec3f) -> f32 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}
pub fn normalize_3f(v: Vec3f) -> Vec3f {
    let l = length_3f(v);
    if l > 0.0 {
        v / l
    } else {
        vec3(0.0, 0.0, 0.0)
    }
}
pub fn cross(a: Vec3f, b: Vec3f) -> Vec3f {
    vec3(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}
pub fn dot_4f(a: Vec4f, b: Vec4f) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w
}
pub fn length_4f(v: Vec4f) -> f32 {
    (v.x * v.x + v.y * v.y + v.z * v.z + v.w * v.w).sqrt()
}

// dFdx/dFdy are generated per-shader to access RenderCx derivative data

pub fn hsv_to_rgb(hsv: Vec4f) -> Vec4f {
    let h = hsv.x;
    let s = hsv.y;
    let v = hsv.z;
    let c = v * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 1.0 / 6.0 {
        (c, x, 0.0)
    } else if h < 2.0 / 6.0 {
        (x, c, 0.0)
    } else if h < 3.0 / 6.0 {
        (0.0, c, x)
    } else if h < 4.0 / 6.0 {
        (0.0, x, c)
    } else if h < 5.0 / 6.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    vec4(r + m, g + m, b + m, hsv.w)
}

// ─── Sdf2d ───

pub struct Sdf2d {
    pub pos: Vec2f,
    pub result: Vec4f,
    pub last_pos: Vec2f,
    pub start_pos: Vec2f,
    pub shape: f32,
    pub clip: f32,
    pub has_clip: f32,
    pub old_shape: f32,
    pub blur: f32,
    pub aa: f32,
    pub scale_factor: f32,
    pub dist: f32,
}

impl Sdf2d {
    pub fn viewport_f2(pos: Vec2f) -> Self {
        Self {
            pos,
            result: vec4(0.0, 0.0, 0.0, 0.0),
            last_pos: vec2(0.0, 0.0),
            start_pos: vec2(0.0, 0.0),
            shape: 1e+20,
            clip: -1e+20,
            has_clip: 0.0,
            old_shape: 1e+20,
            blur: 0.00001,
            aa: 1.5,
            scale_factor: 1.0,
            dist: 0.0,
        }
    }

    pub fn circle_f3(&mut self, x: f32, y: f32, r: f32) {
        let dx = self.pos.x - x;
        let dy = self.pos.y - y;
        let d = (dx * dx + dy * dy).sqrt() - r;
        self.shape = d;
        self.clip = d;
        self.has_clip = 0.0;
        self.old_shape = d;
    }

    pub fn box_f4(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let dx = (self.pos.x - cx).abs() - w * 0.5;
        let dy = (self.pos.y - cy).abs() - h * 0.5;
        let d = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - r;
        self.shape = d;
        self.clip = d;
        self.has_clip = 0.0;
        self.old_shape = d;
    }

    pub fn rect_f4(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let dx = (self.pos.x - cx).abs() - w * 0.5;
        let dy = (self.pos.y - cy).abs() - h * 0.5;
        let d = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0);
        self.shape = d;
        self.clip = d;
        self.has_clip = 0.0;
        self.old_shape = d;
    }

    pub fn hexagon_f3(&mut self, x: f32, y: f32, r: f32) {
        let dx = (self.pos.x - x).abs();
        let dy = (self.pos.y - y).abs();
        let d = (dx * 0.866025 + dy * 0.5).max(dy) - r;
        self.shape = d;
        self.clip = d;
        self.has_clip = 0.0;
        self.old_shape = d;
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.last_pos = vec2(x, y);
        self.start_pos = vec2(x, y);
        self.shape = 1e+20;
        self.clip = -1e+20;
        self.has_clip = 0.0;
        self.old_shape = 1e+20;
    }
    pub fn move_to_f2(&mut self, x: f32, y: f32) {
        self.move_to(x, y);
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        let target = vec2(x, y);
        let pa = self.pos - self.last_pos;
        let ba = target - self.last_pos;
        let dot_ba = ba.x * ba.x + ba.y * ba.y;
        let h = if dot_ba > 0.0 {
            ((pa.x * ba.x + pa.y * ba.y) / dot_ba).max(0.0).min(1.0)
        } else {
            0.0
        };
        let diff = vec2(pa.x - ba.x * h, pa.y - ba.y * h);
        let d = (diff.x * diff.x + diff.y * diff.y).sqrt();
        self.shape = self.shape.min(d);
        self.old_shape = self.shape;
        self.last_pos = target;
    }

    pub fn close_path(&mut self) {
        let sp = self.start_pos;
        self.line_to(sp.x, sp.y);
    }

    pub fn union(&mut self) {
        self.old_shape = self.old_shape.min(self.shape);
        self.shape = self.old_shape;
    }
    pub fn intersect(&mut self) {
        self.old_shape = self.old_shape.max(self.shape);
        self.shape = self.old_shape;
    }
    pub fn subtract(&mut self) {
        self.old_shape = (-self.shape).max(self.old_shape);
        self.shape = self.old_shape;
    }

    pub fn fill_keep(&mut self, color: Vec4f) -> Vec4f {
        let d = self.shape;
        let alpha = (-d / self.aa + 0.5).max(0.0).min(1.0);
        let premul = vec4(
            color.x * color.w * alpha,
            color.y * color.w * alpha,
            color.z * color.w * alpha,
            color.w * alpha,
        );
        let inv_a = 1.0 - premul.w;
        self.result = vec4(
            premul.x + self.result.x * inv_a,
            premul.y + self.result.y * inv_a,
            premul.z + self.result.z * inv_a,
            premul.w + self.result.w * inv_a,
        );
        self.result
    }

    pub fn fill(&mut self, color: Vec4f) -> Vec4f {
        let res = self.fill_keep(color);
        self.old_shape = 1e+20;
        self.shape = 1e+20;
        self.clip = -1e+20;
        self.has_clip = 0.0;
        res
    }

    pub fn stroke_keep(&mut self, color: Vec4f, width: f32) -> Vec4f {
        let d = (self.shape.abs() - width * 0.5).max(0.0);
        let alpha = (-d / self.aa + 0.5).max(0.0).min(1.0);
        let premul = vec4(
            color.x * color.w * alpha,
            color.y * color.w * alpha,
            color.z * color.w * alpha,
            color.w * alpha,
        );
        let inv_a = 1.0 - premul.w;
        self.result = vec4(
            premul.x + self.result.x * inv_a,
            premul.y + self.result.y * inv_a,
            premul.z + self.result.z * inv_a,
            premul.w + self.result.w * inv_a,
        );
        self.result
    }

    pub fn stroke(&mut self, color: Vec4f, width: f32) -> Vec4f {
        let res = self.stroke_keep(color, width);
        self.old_shape = 1e+20;
        self.shape = 1e+20;
        self.clip = -1e+20;
        self.has_clip = 0.0;
        res
    }

    pub fn glow_keep(&mut self, color: Vec4f, width: f32) -> Vec4f {
        let d = self.shape.abs();
        let alpha = (-(d * d) / (width * width * 0.1)).exp();
        let premul = vec4(
            color.x * color.w * alpha,
            color.y * color.w * alpha,
            color.z * color.w * alpha,
            color.w * alpha,
        );
        let inv_a = 1.0 - premul.w;
        self.result = vec4(
            premul.x + self.result.x * inv_a,
            premul.y + self.result.y * inv_a,
            premul.z + self.result.z * inv_a,
            premul.w + self.result.w * inv_a,
        );
        self.result
    }

    pub fn glow(&mut self, color: Vec4f, width: f32) -> Vec4f {
        let res = self.glow_keep(color, width);
        self.old_shape = 1e+20;
        self.shape = 1e+20;
        self.clip = -1e+20;
        self.has_clip = 0.0;
        res
    }
}
