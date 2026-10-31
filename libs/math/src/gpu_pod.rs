//! Bitwise GPU views of the existing repr(C) math records.
use crate::*;

/// Implements bitwise GPU views for a repr(C) record, checking every field
/// and its exact layout at compile time. List all fields in memory order.
#[macro_export]
macro_rules! impl_gpu_pod {
    ($name:ty, $($field:tt: $ty:ty),+ $(,)?) => {
        const _: () = {
            let mut end = 0;
            let mut alignment = 1;
            $(
                assert!(core::mem::offset_of!($name, $field) == end);
                end += core::mem::size_of::<$ty>();
                let field_align = core::mem::align_of::<$ty>();
                if field_align > alignment { alignment = field_align; }
            )+
            assert!(core::mem::size_of::<$name>() == end);
            assert!(core::mem::align_of::<$name>() == alignment);
            fn fields_are_pod(value: &$name) {
                fn pod<T: $crate::bytemuck::Pod>(_: &T) {}
                $(let field: &$ty = &value.$field; pod(field);)+
            }
            let _ = fields_are_pod;
        };
        // SAFETY: repr(C), POD fields, and the assertions rule out all padding.
        unsafe impl $crate::bytemuck::Zeroable for $name {}
        unsafe impl $crate::bytemuck::Pod for $name {}
    };
}

impl_gpu_pod!(Vec2f, x: f32, y: f32);
impl_gpu_pod!(Vec3f, x: f32, y: f32, z: f32);
impl_gpu_pod!(Vec4f, x: f32, y: f32, z: f32, w: f32);
impl_gpu_pod!(Mat3f, c0: Vec3f, c1: Vec3f, c2: Vec3f);
impl_gpu_pod!(Mat4f, v: [f32; 16]);
impl_gpu_pod!(F16x2, x: u16, y: u16);
impl_gpu_pod!(F16x4, x: u16, y: u16, z: u16, w: u16);
impl_gpu_pod!(U16x2, x: u16, y: u16);
impl_gpu_pod!(I16x2, x: i16, y: i16);
impl_gpu_pod!(UNorm16x2, x: u16, y: u16);
impl_gpu_pod!(SNorm16x2, x: i16, y: i16);
impl_gpu_pod!(UNorm8x4, 0: [u8; 4]);
impl_gpu_pod!(SNorm8x4, 0: [i8; 4]);
