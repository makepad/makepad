//! Numeric value types for type-preserving arithmetic operations
//!
//! This module provides the `NumericValue` enum which can hold f64, Vec2f, Vec3f, Vec4f,
//! Color, or Mat4x4f values and supports component-wise operations with proper type promotion.

use crate::heap::*;
use crate::value::*;
use crate::pod::*;
use crate::vm::ScriptCode;
use makepad_math::{Vec2f, Vec3f, Vec4f};

/// Represents a numeric value that can be f64, Vec2f, Vec3f, Vec4f, Color, or Mat4x4f
/// Used for type-preserving arithmetic operations
#[derive(Clone, Copy)]
pub enum NumericValue {
    F64(f64),
    Vec2(Vec2f),
    Vec3(Vec3f),
    Vec4(Vec4f),
    Color(Vec4f), // Color stored as Vec4f internally
    Mat4([f32; 16]), // 4x4 matrix in column-major order
}

impl NumericValue {
    /// Extract a numeric value from ScriptValue (heap-only version, no vm needed)
    pub fn from_script_value_heap(heap: &ScriptHeap, value: ScriptValue, ip: ScriptIp) -> Self {
        // Check for Color first (u32 encoded)
        if let Some(c) = value.as_color() {
            return NumericValue::Color(Vec4f::from_u32(c));
        }
        
        // Check for f64
        if let Some(f) = value.as_f64() {
            return NumericValue::F64(f);
        }
        
        // Check for other number types
        if let Some(f) = value.as_f32() {
            return NumericValue::F64(f as f64);
        }
        if let Some(f) = value.as_u32() {
            return NumericValue::F64(f as f64);
        }
        if let Some(f) = value.as_i32() {
            return NumericValue::F64(f as f64);
        }
        
        // Check for Pod (Vec and Mat types)
        if let Some(pod) = value.as_pod() {
            let pod_data = &heap.pods[pod.index as usize];
            let pod_type = &heap.pod_types[pod_data.ty.index as usize];
            match &pod_type.ty {
                ScriptPodTy::Vec(v) => {
                    match v {
                        ScriptPodVec::Vec2f => {
                            return NumericValue::Vec2(Vec2f {
                                x: f32::from_bits(pod_data.data[0]),
                                y: f32::from_bits(pod_data.data[1]),
                            });
                        }
                        ScriptPodVec::Vec3f => {
                            return NumericValue::Vec3(Vec3f {
                                x: f32::from_bits(pod_data.data[0]),
                                y: f32::from_bits(pod_data.data[1]),
                                z: f32::from_bits(pod_data.data[2]),
                            });
                        }
                        ScriptPodVec::Vec4f => {
                            return NumericValue::Vec4(Vec4f {
                                x: f32::from_bits(pod_data.data[0]),
                                y: f32::from_bits(pod_data.data[1]),
                                z: f32::from_bits(pod_data.data[2]),
                                w: f32::from_bits(pod_data.data[3]),
                            });
                        }
                        // Handle integer vectors by converting to float
                        ScriptPodVec::Vec2i | ScriptPodVec::Vec2u => {
                            return NumericValue::Vec2(Vec2f {
                                x: pod_data.data[0] as f32,
                                y: pod_data.data[1] as f32,
                            });
                        }
                        ScriptPodVec::Vec3i | ScriptPodVec::Vec3u => {
                            return NumericValue::Vec3(Vec3f {
                                x: pod_data.data[0] as f32,
                                y: pod_data.data[1] as f32,
                                z: pod_data.data[2] as f32,
                            });
                        }
                        ScriptPodVec::Vec4i | ScriptPodVec::Vec4u => {
                            return NumericValue::Vec4(Vec4f {
                                x: pod_data.data[0] as f32,
                                y: pod_data.data[1] as f32,
                                z: pod_data.data[2] as f32,
                                w: pod_data.data[3] as f32,
                            });
                        }
                        _ => {}
                    }
                }
                ScriptPodTy::Mat(m) => {
                    if let ScriptPodMat::Mat4x4f = m {
                        let mut mat = [0.0f32; 16];
                        for i in 0..16 {
                            mat[i] = f32::from_bits(pod_data.data[i]);
                        }
                        return NumericValue::Mat4(mat);
                    }
                }
                _ => {}
            }
        }
        
        // Fallback: cast to f64
        NumericValue::F64(heap.cast_to_f64(value, ip))
    }
    
    /// Convert back to ScriptValue (heap + code version)
    pub fn to_script_value_heap(self, heap: &mut ScriptHeap, code: &ScriptCode) -> ScriptValue {
        match self {
            NumericValue::F64(f) => ScriptValue::from_f64(f),
            NumericValue::Vec2(v) => {
                let pod = heap.new_pod(code.builtins.pod.pod_vec2f);
                let pod_data = &mut heap.pods[pod.index as usize];
                pod_data.data[0] = v.x.to_bits();
                pod_data.data[1] = v.y.to_bits();
                pod.into()
            }
            NumericValue::Vec3(v) => {
                let pod = heap.new_pod(code.builtins.pod.pod_vec3f);
                let pod_data = &mut heap.pods[pod.index as usize];
                pod_data.data[0] = v.x.to_bits();
                pod_data.data[1] = v.y.to_bits();
                pod_data.data[2] = v.z.to_bits();
                pod.into()
            }
            NumericValue::Vec4(v) => {
                let pod = heap.new_pod(code.builtins.pod.pod_vec4f);
                let pod_data = &mut heap.pods[pod.index as usize];
                pod_data.data[0] = v.x.to_bits();
                pod_data.data[1] = v.y.to_bits();
                pod_data.data[2] = v.z.to_bits();
                pod_data.data[3] = v.w.to_bits();
                pod.into()
            }
            NumericValue::Color(v) => ScriptValue::from_color(v.to_u32()),
            NumericValue::Mat4(m) => {
                let pod = heap.new_pod(code.builtins.pod.pod_mat4x4f);
                let pod_data = &mut heap.pods[pod.index as usize];
                for i in 0..16 {
                    pod_data.data[i] = m[i].to_bits();
                }
                pod.into()
            }
        }
    }
    
    /// Apply a unary f32 operation component-wise
    pub fn map_f32<F: Fn(f32) -> f32>(self, f: F) -> Self {
        match self {
            NumericValue::F64(v) => NumericValue::F64(f(v as f32) as f64),
            NumericValue::Vec2(v) => NumericValue::Vec2(Vec2f { x: f(v.x), y: f(v.y) }),
            NumericValue::Vec3(v) => NumericValue::Vec3(Vec3f { x: f(v.x), y: f(v.y), z: f(v.z) }),
            NumericValue::Vec4(v) => NumericValue::Vec4(Vec4f { x: f(v.x), y: f(v.y), z: f(v.z), w: f(v.w) }),
            NumericValue::Color(v) => NumericValue::Color(Vec4f { x: f(v.x), y: f(v.y), z: f(v.z), w: f(v.w) }),
            NumericValue::Mat4(m) => {
                let mut result = [0.0f32; 16];
                for i in 0..16 {
                    result[i] = f(m[i]);
                }
                NumericValue::Mat4(result)
            }
        }
    }
    
    /// Apply a binary f32 operation component-wise with proper type promotion
    /// This handles +, -, and component-wise * and /
    pub fn zip_f32<F: Fn(f32, f32) -> f32>(self, other: Self, f: F) -> Self {
        match (self, other) {
            // Same types
            (NumericValue::F64(a), NumericValue::F64(b)) => {
                NumericValue::F64(f(a as f32, b as f32) as f64)
            }
            (NumericValue::Vec2(a), NumericValue::Vec2(b)) => {
                NumericValue::Vec2(Vec2f { x: f(a.x, b.x), y: f(a.y, b.y) })
            }
            (NumericValue::Vec3(a), NumericValue::Vec3(b)) => {
                NumericValue::Vec3(Vec3f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, b.z) })
            }
            (NumericValue::Vec4(a), NumericValue::Vec4(b)) => {
                NumericValue::Vec4(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, b.z), w: f(a.w, b.w) })
            }
            (NumericValue::Color(a), NumericValue::Color(b)) => {
                NumericValue::Color(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, b.z), w: f(a.w, b.w) })
            }
            (NumericValue::Mat4(a), NumericValue::Mat4(b)) => {
                let mut result = [0.0f32; 16];
                for i in 0..16 {
                    result[i] = f(a[i], b[i]);
                }
                NumericValue::Mat4(result)
            }
            
            // Scalar * Vec -> Vec (broadcast)
            (NumericValue::F64(a), NumericValue::Vec2(b)) => {
                let a = a as f32;
                NumericValue::Vec2(Vec2f { x: f(a, b.x), y: f(a, b.y) })
            }
            (NumericValue::Vec2(a), NumericValue::F64(b)) => {
                let b = b as f32;
                NumericValue::Vec2(Vec2f { x: f(a.x, b), y: f(a.y, b) })
            }
            (NumericValue::F64(a), NumericValue::Vec3(b)) => {
                let a = a as f32;
                NumericValue::Vec3(Vec3f { x: f(a, b.x), y: f(a, b.y), z: f(a, b.z) })
            }
            (NumericValue::Vec3(a), NumericValue::F64(b)) => {
                let b = b as f32;
                NumericValue::Vec3(Vec3f { x: f(a.x, b), y: f(a.y, b), z: f(a.z, b) })
            }
            (NumericValue::F64(a), NumericValue::Vec4(b)) => {
                let a = a as f32;
                NumericValue::Vec4(Vec4f { x: f(a, b.x), y: f(a, b.y), z: f(a, b.z), w: f(a, b.w) })
            }
            (NumericValue::Vec4(a), NumericValue::F64(b)) => {
                let b = b as f32;
                NumericValue::Vec4(Vec4f { x: f(a.x, b), y: f(a.y, b), z: f(a.z, b), w: f(a.w, b) })
            }
            
            // Scalar * Color -> Color (broadcast)
            (NumericValue::F64(a), NumericValue::Color(b)) => {
                let a = a as f32;
                NumericValue::Color(Vec4f { x: f(a, b.x), y: f(a, b.y), z: f(a, b.z), w: f(a, b.w) })
            }
            (NumericValue::Color(a), NumericValue::F64(b)) => {
                let b = b as f32;
                NumericValue::Color(Vec4f { x: f(a.x, b), y: f(a.y, b), z: f(a.z, b), w: f(a.w, b) })
            }
            
            // Scalar * Mat -> Mat (broadcast)
            (NumericValue::F64(a), NumericValue::Mat4(b)) => {
                let a = a as f32;
                let mut result = [0.0f32; 16];
                for i in 0..16 {
                    result[i] = f(a, b[i]);
                }
                NumericValue::Mat4(result)
            }
            (NumericValue::Mat4(a), NumericValue::F64(b)) => {
                let b = b as f32;
                let mut result = [0.0f32; 16];
                for i in 0..16 {
                    result[i] = f(a[i], b);
                }
                NumericValue::Mat4(result)
            }
            
            // Vec4 <-> Color operations preserve the first operand's type
            (NumericValue::Vec4(a), NumericValue::Color(b)) => {
                NumericValue::Vec4(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, b.z), w: f(a.w, b.w) })
            }
            (NumericValue::Color(a), NumericValue::Vec4(b)) => {
                NumericValue::Color(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, b.z), w: f(a.w, b.w) })
            }
            
            // Mixed Vec sizes - use larger type
            (NumericValue::Vec2(a), NumericValue::Vec3(b)) => {
                NumericValue::Vec3(Vec3f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(0.0, b.z) })
            }
            (NumericValue::Vec3(a), NumericValue::Vec2(b)) => {
                NumericValue::Vec3(Vec3f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, 0.0) })
            }
            (NumericValue::Vec2(a), NumericValue::Vec4(b)) => {
                NumericValue::Vec4(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(0.0, b.z), w: f(0.0, b.w) })
            }
            (NumericValue::Vec4(a), NumericValue::Vec2(b)) => {
                NumericValue::Vec4(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, 0.0), w: f(a.w, 0.0) })
            }
            (NumericValue::Vec3(a), NumericValue::Vec4(b)) => {
                NumericValue::Vec4(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, b.z), w: f(0.0, b.w) })
            }
            (NumericValue::Vec4(a), NumericValue::Vec3(b)) => {
                NumericValue::Vec4(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, b.z), w: f(a.w, 0.0) })
            }
            
            // Vec <-> Color mixed - convert to vec4 style operation
            (NumericValue::Vec2(a), NumericValue::Color(b)) => {
                NumericValue::Vec4(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(0.0, b.z), w: f(0.0, b.w) })
            }
            (NumericValue::Color(a), NumericValue::Vec2(b)) => {
                NumericValue::Color(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, 0.0), w: f(a.w, 0.0) })
            }
            (NumericValue::Vec3(a), NumericValue::Color(b)) => {
                NumericValue::Vec4(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, b.z), w: f(0.0, b.w) })
            }
            (NumericValue::Color(a), NumericValue::Vec3(b)) => {
                NumericValue::Color(Vec4f { x: f(a.x, b.x), y: f(a.y, b.y), z: f(a.z, b.z), w: f(a.w, 0.0) })
            }
            
            // Mat * Vec or Vec * Mat - for non-multiply ops, just return first operand
            (NumericValue::Mat4(_), NumericValue::Vec4(_)) |
            (NumericValue::Vec4(_), NumericValue::Mat4(_)) |
            (NumericValue::Mat4(_), NumericValue::Vec3(_)) |
            (NumericValue::Vec3(_), NumericValue::Mat4(_)) |
            (NumericValue::Mat4(_), NumericValue::Vec2(_)) |
            (NumericValue::Vec2(_), NumericValue::Mat4(_)) |
            (NumericValue::Mat4(_), NumericValue::Color(_)) |
            (NumericValue::Color(_), NumericValue::Mat4(_)) => self,
        }
    }
    
    /// Multiply operation with proper matrix-vector semantics
    pub fn multiply(self, other: Self) -> Self {
        match (self, other) {
            // Mat4 * Vec4 -> Vec4 (matrix-vector multiplication)
            (NumericValue::Mat4(m), NumericValue::Vec4(v)) => {
                NumericValue::Vec4(mat4_mul_vec4(&m, &v))
            }
            // Vec4 * Mat4 -> Vec4 (vector-matrix multiplication, treats vector as row)
            (NumericValue::Vec4(v), NumericValue::Mat4(m)) => {
                NumericValue::Vec4(vec4_mul_mat4(&v, &m))
            }
            // Mat4 * Mat4 -> Mat4 (matrix multiplication)
            (NumericValue::Mat4(a), NumericValue::Mat4(b)) => {
                NumericValue::Mat4(mat4_mul_mat4(&a, &b))
            }
            // Mat4 * Vec3 -> Vec3 (treat as Vec4 with w=1, return xyz)
            (NumericValue::Mat4(m), NumericValue::Vec3(v)) => {
                let v4 = Vec4f { x: v.x, y: v.y, z: v.z, w: 1.0 };
                let result = mat4_mul_vec4(&m, &v4);
                NumericValue::Vec3(Vec3f { x: result.x, y: result.y, z: result.z })
            }
            // Vec3 * Mat4 -> Vec3
            (NumericValue::Vec3(v), NumericValue::Mat4(m)) => {
                let v4 = Vec4f { x: v.x, y: v.y, z: v.z, w: 1.0 };
                let result = vec4_mul_mat4(&v4, &m);
                NumericValue::Vec3(Vec3f { x: result.x, y: result.y, z: result.z })
            }
            // Mat4 * Color -> Color (treat as Vec4)
            (NumericValue::Mat4(m), NumericValue::Color(c)) => {
                NumericValue::Color(mat4_mul_vec4(&m, &c))
            }
            // Color * Mat4 -> Color
            (NumericValue::Color(c), NumericValue::Mat4(m)) => {
                NumericValue::Color(vec4_mul_mat4(&c, &m))
            }
            // Mat4 * scalar -> Mat4 (component-wise)
            (NumericValue::Mat4(m), NumericValue::F64(s)) => {
                let s = s as f32;
                let mut result = [0.0f32; 16];
                for i in 0..16 {
                    result[i] = m[i] * s;
                }
                NumericValue::Mat4(result)
            }
            // scalar * Mat4 -> Mat4
            (NumericValue::F64(s), NumericValue::Mat4(m)) => {
                let s = s as f32;
                let mut result = [0.0f32; 16];
                for i in 0..16 {
                    result[i] = s * m[i];
                }
                NumericValue::Mat4(result)
            }
            // All other cases: component-wise multiplication
            _ => self.zip_f32(other, |a, b| a * b)
        }
    }
    
    /// Mix two values with a scalar alpha
    pub fn mix_scalar(self, other: Self, alpha: f64) -> Self {
        let a = alpha as f32;
        let one_minus_a = 1.0 - a;
        match (self, other) {
            (NumericValue::F64(x), NumericValue::F64(y)) => {
                NumericValue::F64((x as f32 * one_minus_a + y as f32 * a) as f64)
            }
            (NumericValue::Vec2(x), NumericValue::Vec2(y)) => NumericValue::Vec2(Vec2f {
                x: x.x * one_minus_a + y.x * a,
                y: x.y * one_minus_a + y.y * a,
            }),
            (NumericValue::Vec3(x), NumericValue::Vec3(y)) => NumericValue::Vec3(Vec3f {
                x: x.x * one_minus_a + y.x * a,
                y: x.y * one_minus_a + y.y * a,
                z: x.z * one_minus_a + y.z * a,
            }),
            (NumericValue::Vec4(x), NumericValue::Vec4(y)) => NumericValue::Vec4(Vec4f {
                x: x.x * one_minus_a + y.x * a,
                y: x.y * one_minus_a + y.y * a,
                z: x.z * one_minus_a + y.z * a,
                w: x.w * one_minus_a + y.w * a,
            }),
            (NumericValue::Color(x), NumericValue::Color(y)) => NumericValue::Color(Vec4f {
                x: x.x * one_minus_a + y.x * a,
                y: x.y * one_minus_a + y.y * a,
                z: x.z * one_minus_a + y.z * a,
                w: x.w * one_minus_a + y.w * a,
            }),
            // Fallback
            _ => self,
        }
    }
    
    /// Mix two values with component-wise alpha (alpha has same type as self/other)
    pub fn mix_componentwise(self, other: Self, alpha: Self) -> Self {
        match (self, other, alpha) {
            (NumericValue::F64(x), NumericValue::F64(y), NumericValue::F64(a)) => {
                let a = a as f32;
                NumericValue::F64((x as f32 * (1.0 - a) + y as f32 * a) as f64)
            }
            (NumericValue::Vec2(x), NumericValue::Vec2(y), NumericValue::Vec2(a)) => {
                NumericValue::Vec2(Vec2f {
                    x: x.x * (1.0 - a.x) + y.x * a.x,
                    y: x.y * (1.0 - a.y) + y.y * a.y,
                })
            }
            (NumericValue::Vec3(x), NumericValue::Vec3(y), NumericValue::Vec3(a)) => {
                NumericValue::Vec3(Vec3f {
                    x: x.x * (1.0 - a.x) + y.x * a.x,
                    y: x.y * (1.0 - a.y) + y.y * a.y,
                    z: x.z * (1.0 - a.z) + y.z * a.z,
                })
            }
            (NumericValue::Vec4(x), NumericValue::Vec4(y), NumericValue::Vec4(a)) => {
                NumericValue::Vec4(Vec4f {
                    x: x.x * (1.0 - a.x) + y.x * a.x,
                    y: x.y * (1.0 - a.y) + y.y * a.y,
                    z: x.z * (1.0 - a.z) + y.z * a.z,
                    w: x.w * (1.0 - a.w) + y.w * a.w,
                })
            }
            (NumericValue::Color(x), NumericValue::Color(y), NumericValue::Color(a)) => {
                NumericValue::Color(Vec4f {
                    x: x.x * (1.0 - a.x) + y.x * a.x,
                    y: x.y * (1.0 - a.y) + y.y * a.y,
                    z: x.z * (1.0 - a.z) + y.z * a.z,
                    w: x.w * (1.0 - a.w) + y.w * a.w,
                })
            }
            // Fallback: treat alpha as scalar using its first component
            _ => {
                let a_f = match alpha {
                    NumericValue::F64(v) => v,
                    NumericValue::Vec2(v) => v.x as f64,
                    NumericValue::Vec3(v) => v.x as f64,
                    NumericValue::Vec4(v) => v.x as f64,
                    NumericValue::Color(v) => v.x as f64,
                    NumericValue::Mat4(_) => 0.5,
                };
                self.mix_scalar(other, a_f)
            }
        }
    }
    
    /// Clamp with scalar min/max
    pub fn clamp_scalar(self, min_val: f64, max_val: f64) -> Self {
        let min_f = min_val as f32;
        let max_f = max_val as f32;
        self.map_f32(|v| v.max(min_f).min(max_f))
    }
    
    /// Step function with scalar edge
    pub fn step_scalar(edge: f64, self_val: Self) -> Self {
        let edge_f = edge as f32;
        self_val.map_f32(|v| if v < edge_f { 0.0 } else { 1.0 })
    }
    
    /// Smoothstep with scalar edges
    pub fn smoothstep_scalar(e0: f64, e1: f64, self_val: Self) -> Self {
        let e0_f = e0 as f32;
        let e1_f = e1 as f32;
        self_val.map_f32(|x| {
            let t = ((x - e0_f) / (e1_f - e0_f)).max(0.0).min(1.0);
            t * t * (3.0 - 2.0 * t)
        })
    }
    
    /// Get the length (magnitude) of a vector, or absolute value for scalar
    pub fn length(&self) -> f64 {
        match self {
            NumericValue::F64(v) => v.abs(),
            NumericValue::Vec2(v) => (v.x * v.x + v.y * v.y).sqrt() as f64,
            NumericValue::Vec3(v) => (v.x * v.x + v.y * v.y + v.z * v.z).sqrt() as f64,
            NumericValue::Vec4(v) => (v.x * v.x + v.y * v.y + v.z * v.z + v.w * v.w).sqrt() as f64,
            NumericValue::Color(v) => (v.x * v.x + v.y * v.y + v.z * v.z + v.w * v.w).sqrt() as f64,
            NumericValue::Mat4(_) => 0.0, // undefined for matrices
        }
    }
    
    /// Dot product (returns scalar)
    pub fn dot(self, other: Self) -> f64 {
        match (self, other) {
            (NumericValue::F64(a), NumericValue::F64(b)) => a * b,
            (NumericValue::Vec2(a), NumericValue::Vec2(b)) => (a.x * b.x + a.y * b.y) as f64,
            (NumericValue::Vec3(a), NumericValue::Vec3(b)) => (a.x * b.x + a.y * b.y + a.z * b.z) as f64,
            (NumericValue::Vec4(a), NumericValue::Vec4(b)) => (a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w) as f64,
            (NumericValue::Color(a), NumericValue::Color(b)) => (a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w) as f64,
            _ => 0.0, // Mismatched types
        }
    }
    
    /// Normalize a vector (returns unit vector in same direction)
    pub fn normalize(self) -> Self {
        let len = self.length() as f32;
        if len == 0.0 {
            return self;
        }
        let inv_len = 1.0 / len;
        match self {
            NumericValue::F64(v) => NumericValue::F64(if v >= 0.0 { 1.0 } else { -1.0 }),
            NumericValue::Vec2(v) => NumericValue::Vec2(Vec2f { x: v.x * inv_len, y: v.y * inv_len }),
            NumericValue::Vec3(v) => NumericValue::Vec3(Vec3f { x: v.x * inv_len, y: v.y * inv_len, z: v.z * inv_len }),
            NumericValue::Vec4(v) => NumericValue::Vec4(Vec4f { x: v.x * inv_len, y: v.y * inv_len, z: v.z * inv_len, w: v.w * inv_len }),
            NumericValue::Color(v) => NumericValue::Color(Vec4f { x: v.x * inv_len, y: v.y * inv_len, z: v.z * inv_len, w: v.w * inv_len }),
            NumericValue::Mat4(m) => NumericValue::Mat4(m), // undefined for matrices
        }
    }
    
    /// Cross product (only defined for Vec3)
    pub fn cross(self, other: Self) -> Self {
        match (self, other) {
            (NumericValue::Vec3(a), NumericValue::Vec3(b)) => NumericValue::Vec3(Vec3f {
                x: a.y * b.z - a.z * b.y,
                y: a.z * b.x - a.x * b.z,
                z: a.x * b.y - a.y * b.x,
            }),
            _ => self.zero_like(), // Undefined for other types
        }
    }
    
    /// Create a zero value of the same type
    pub fn zero_like(&self) -> Self {
        match self {
            NumericValue::F64(_) => NumericValue::F64(0.0),
            NumericValue::Vec2(_) => NumericValue::Vec2(Vec2f { x: 0.0, y: 0.0 }),
            NumericValue::Vec3(_) => NumericValue::Vec3(Vec3f { x: 0.0, y: 0.0, z: 0.0 }),
            NumericValue::Vec4(_) => NumericValue::Vec4(Vec4f { x: 0.0, y: 0.0, z: 0.0, w: 0.0 }),
            NumericValue::Color(_) => NumericValue::Color(Vec4f { x: 0.0, y: 0.0, z: 0.0, w: 0.0 }),
            NumericValue::Mat4(_) => NumericValue::Mat4([0.0; 16]),
        }
    }
}

/// Matrix-vector multiplication: M * v (column vector)
/// Matrix is in column-major order
#[inline]
pub fn mat4_mul_vec4(m: &[f32; 16], v: &Vec4f) -> Vec4f {
    Vec4f {
        x: m[0] * v.x + m[4] * v.y + m[8]  * v.z + m[12] * v.w,
        y: m[1] * v.x + m[5] * v.y + m[9]  * v.z + m[13] * v.w,
        z: m[2] * v.x + m[6] * v.y + m[10] * v.z + m[14] * v.w,
        w: m[3] * v.x + m[7] * v.y + m[11] * v.z + m[15] * v.w,
    }
}

/// Vector-matrix multiplication: v * M (row vector)
/// Matrix is in column-major order
#[inline]
pub fn vec4_mul_mat4(v: &Vec4f, m: &[f32; 16]) -> Vec4f {
    Vec4f {
        x: v.x * m[0] + v.y * m[1] + v.z * m[2]  + v.w * m[3],
        y: v.x * m[4] + v.y * m[5] + v.z * m[6]  + v.w * m[7],
        z: v.x * m[8] + v.y * m[9] + v.z * m[10] + v.w * m[11],
        w: v.x * m[12] + v.y * m[13] + v.z * m[14] + v.w * m[15],
    }
}

/// Matrix-matrix multiplication: A * B
/// Both matrices in column-major order
#[inline]
pub fn mat4_mul_mat4(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut result = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0f32;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            result[col * 4 + row] = sum;
        }
    }
    result
}
