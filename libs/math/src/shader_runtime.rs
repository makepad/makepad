// Shader runtime types for headless/JIT rendering
// These types mirror GPU shader primitives for CPU-side evaluation

use crate::math_f32::*;

// ============================================================
// Swizzle methods for Vec2f, Vec3f, Vec4f
// Generated to match GLSL swizzle syntax: v.xy(), v.xyz(), etc.
// ============================================================

impl Vec2f {
    // 2-component swizzles returning Vec2f
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
    // 3-component swizzles returning Vec3f
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
    // 4-component swizzles returning Vec4f
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
}

impl Vec3f {
    // 2-component swizzles returning Vec2f
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
    // 3-component swizzles returning Vec3f
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
    // 4-component swizzles returning Vec4f
    pub fn xxxx(&self) -> Vec4f {
        vec4(self.x, self.x, self.x, self.x)
    }
    pub fn xyzx(&self) -> Vec4f {
        vec4(self.x, self.y, self.z, self.x)
    }
    pub fn xyzz(&self) -> Vec4f {
        vec4(self.x, self.y, self.z, self.z)
    }
    // mix method
    pub fn mix(&self, other: Vec3f, t: f32) -> Vec3f {
        vec3(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
            self.z + (other.z - self.z) * t,
        )
    }
}

impl Vec4f {
    // 2-component swizzles returning Vec2f
    // Note: xy() and zw() are already defined in math_f32.rs
    pub fn xx(&self) -> Vec2f {
        vec2(self.x, self.x)
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
    // 3-component swizzles returning Vec3f
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
    // 4-component swizzles returning Vec4f
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
}

impl Vec2f {
    pub fn mix(&self, other: Vec2f, t: f32) -> Vec2f {
        vec2(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }
}

// ============================================================
// Texture2D - CPU-side texture sampling
// ============================================================

pub struct Texture2D {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>, // RGBA f32 pixels, row-major
}

impl Default for Texture2D {
    fn default() -> Self {
        Self::new()
    }
}

impl Texture2D {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            data: Vec::new(),
        }
    }

    pub fn from_rgba_f32(width: usize, height: usize, data: Vec<f32>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    pub fn sample(&self, coord: Vec2f) -> Vec4f {
        if self.width == 0 || self.height == 0 || self.data.is_empty() {
            return Vec4f {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 0.0,
            };
        }
        // Clamp coordinates to [0, 1]
        let u = coord.x.max(0.0).min(1.0);
        let v = coord.y.max(0.0).min(1.0);

        // Nearest-neighbor sampling
        let px = ((u * self.width as f32) as usize).min(self.width - 1);
        let py = ((v * self.height as f32) as usize).min(self.height - 1);
        let idx = (py * self.width + px) * 4;

        if idx + 3 < self.data.len() {
            Vec4f {
                x: self.data[idx],
                y: self.data[idx + 1],
                z: self.data[idx + 2],
                w: self.data[idx + 3],
            }
        } else {
            Vec4f {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 0.0,
            }
        }
    }

    pub fn sample_r8(&self, coord: Vec2f) -> Vec4f {
        if self.width == 0 || self.height == 0 || self.data.is_empty() {
            return Vec4f {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 0.0,
            };
        }
        let u = coord.x.max(0.0).min(1.0);
        let v = coord.y.max(0.0).min(1.0);
        let px = ((u * self.width as f32) as usize).min(self.width - 1);
        let py = ((v * self.height as f32) as usize).min(self.height - 1);
        let idx = py * self.width + px;
        if idx < self.data.len() {
            let r = self.data[idx];
            Vec4f {
                x: r,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            }
        } else {
            Vec4f {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 0.0,
            }
        }
    }
}

// ============================================================
// Vec4f mix method (shader-style linear interpolation)
// ============================================================

impl Vec4f {
    pub fn mix(&self, other: Vec4f, t: f32) -> Vec4f {
        Vec4f {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
            z: self.z + (other.z - self.z) * t,
            w: self.w + (other.w - self.w) * t,
        }
    }
}

impl Vec2f {
    pub fn atan2(&self) -> f32 {
        self.y.atan2(self.x)
    }
}

// ============================================================
// Shader builtin free functions
// ============================================================

pub fn mix_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

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
    let len = length_2f(v);
    if len > 0.0 {
        Vec2f {
            x: v.x / len,
            y: v.y / len,
        }
    } else {
        Vec2f { x: 0.0, y: 0.0 }
    }
}

pub fn dot_4f(a: Vec4f, b: Vec4f) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w
}

pub fn length_4f(v: Vec4f) -> f32 {
    (v.x * v.x + v.y * v.y + v.z * v.z + v.w * v.w).sqrt()
}

// ============================================================
// Color conversion functions
// ============================================================

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

    Vec4f {
        x: r + m,
        y: g + m,
        z: b + m,
        w: hsv.w,
    }
}

// ============================================================
// Sdf2d - 2D signed distance field for CPU-side shape rendering
// ============================================================

pub struct Sdf2d {
    pub pos: Vec2f,
    pub result: Vec4f,
    pub last_pos: Vec2f,
    pub start_pos: Vec2f,
    pub shape: f32,
    pub clip: f32,
    pub has_clip: bool,
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
            result: Vec4f {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 0.0,
            },
            last_pos: Vec2f { x: 0.0, y: 0.0 },
            start_pos: Vec2f { x: 0.0, y: 0.0 },
            shape: 1e+20f32,
            clip: -1e+20f32,
            has_clip: false,
            old_shape: 1e+20f32,
            blur: 0.00001,
            aa: 1.5,
            scale_factor: 1.0,
            dist: 0.0,
        }
    }

    // --- Shape primitives ---

    pub fn circle_f3(&mut self, x: f32, y: f32, r: f32) {
        let dx = self.pos.x - x;
        let dy = self.pos.y - y;
        let d = (dx * dx + dy * dy).sqrt() - r;
        self.shape = d;
        self.clip = d;
        self.has_clip = false;
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
        self.has_clip = false;
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
        self.has_clip = false;
        self.old_shape = d;
    }

    pub fn hexagon_f3(&mut self, x: f32, y: f32, r: f32) {
        let dx = (self.pos.x - x).abs();
        let dy = (self.pos.y - y).abs();
        let d = (dx * 0.866025 + dy * 0.5).max(dy) - r;
        self.shape = d;
        self.clip = d;
        self.has_clip = false;
        self.old_shape = d;
    }

    // --- Path operations ---

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.last_pos = Vec2f { x, y };
        self.start_pos = Vec2f { x, y };
        self.shape = 1e+20f32;
        self.clip = -1e+20f32;
        self.has_clip = false;
        self.old_shape = 1e+20f32;
    }

    pub fn move_to_f2(&mut self, x: f32, y: f32) {
        self.move_to(x, y);
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        let target = Vec2f { x, y };
        let pa = Vec2f {
            x: self.pos.x - self.last_pos.x,
            y: self.pos.y - self.last_pos.y,
        };
        let ba = Vec2f {
            x: target.x - self.last_pos.x,
            y: target.y - self.last_pos.y,
        };
        let dot_ba = ba.x * ba.x + ba.y * ba.y;
        let h = if dot_ba > 0.0 {
            ((pa.x * ba.x + pa.y * ba.y) / dot_ba).max(0.0).min(1.0)
        } else {
            0.0
        };
        let diff = Vec2f {
            x: pa.x - ba.x * h,
            y: pa.y - ba.y * h,
        };
        let d = (diff.x * diff.x + diff.y * diff.y).sqrt();
        self.shape = self.shape.min(d);
        self.old_shape = self.shape;
        self.last_pos = target;
    }

    pub fn close_path(&mut self) {
        self.line_to(self.start_pos.x, self.start_pos.y);
    }

    // --- Boolean operations ---

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

    // --- Fill and stroke ---

    pub fn fill_keep(&mut self, color: Vec4f) -> Vec4f {
        let d = self.shape;
        let aa = self.aa;
        let alpha = (-d / aa + 0.5).max(0.0).min(1.0);
        let premul = Vec4f {
            x: color.x * color.w * alpha,
            y: color.y * color.w * alpha,
            z: color.z * color.w * alpha,
            w: color.w * alpha,
        };
        let inv_a = 1.0 - premul.w;
        self.result = Vec4f {
            x: premul.x + self.result.x * inv_a,
            y: premul.y + self.result.y * inv_a,
            z: premul.z + self.result.z * inv_a,
            w: premul.w + self.result.w * inv_a,
        };
        self.result
    }

    pub fn fill(&mut self, color: Vec4f) -> Vec4f {
        let res = self.fill_keep(color);
        self.old_shape = 1e+20f32;
        self.shape = 1e+20f32;
        self.clip = -1e+20f32;
        self.has_clip = false;
        res
    }

    pub fn stroke_keep(&mut self, color: Vec4f, width: f32) -> Vec4f {
        let d = (self.shape.abs() - width * 0.5).max(0.0);
        let aa = self.aa;
        let alpha = (-d / aa + 0.5).max(0.0).min(1.0);
        let premul = Vec4f {
            x: color.x * color.w * alpha,
            y: color.y * color.w * alpha,
            z: color.z * color.w * alpha,
            w: color.w * alpha,
        };
        let inv_a = 1.0 - premul.w;
        self.result = Vec4f {
            x: premul.x + self.result.x * inv_a,
            y: premul.y + self.result.y * inv_a,
            z: premul.z + self.result.z * inv_a,
            w: premul.w + self.result.w * inv_a,
        };
        self.result
    }

    pub fn stroke(&mut self, color: Vec4f, width: f32) -> Vec4f {
        let res = self.stroke_keep(color, width);
        self.old_shape = 1e+20f32;
        self.shape = 1e+20f32;
        self.clip = -1e+20f32;
        self.has_clip = false;
        res
    }

    pub fn glow_keep(&mut self, color: Vec4f, width: f32) -> Vec4f {
        let d = self.shape.abs();
        let alpha = (-(d * d) / (width * width * 0.1)).exp();
        let premul = Vec4f {
            x: color.x * color.w * alpha,
            y: color.y * color.w * alpha,
            z: color.z * color.w * alpha,
            w: color.w * alpha,
        };
        let inv_a = 1.0 - premul.w;
        self.result = Vec4f {
            x: premul.x + self.result.x * inv_a,
            y: premul.y + self.result.y * inv_a,
            z: premul.z + self.result.z * inv_a,
            w: premul.w + self.result.w * inv_a,
        };
        self.result
    }

    pub fn glow(&mut self, color: Vec4f, width: f32) -> Vec4f {
        let res = self.glow_keep(color, width);
        self.old_shape = 1e+20f32;
        self.shape = 1e+20f32;
        self.clip = -1e+20f32;
        self.has_clip = false;
        res
    }
}
