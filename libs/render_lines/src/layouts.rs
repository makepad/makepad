//! The record layouts kernels write lines and points in (KERNELS.md §3.5,
//! PDOOM-PARITY AK9). A host compiles its kernels with [`layouts`] so a
//! kernel can write `out[i].pos = ...` into them; the words go straight to
//! [`crate::LineBatch::push_point_records`] /
//! [`crate::LineBatch::push_segment_records`] /
//! [`crate::PointBatch::push_sprite_records`].
//!
//! | layout | words | fields |
//! |---|---|---|
//! | `LinePoint` | 9 | pos (vec3), width, color (vec4), birth |
//! | `LineSegment` | 12 | a (vec3), b (vec3), width, color (vec4), birth |
//! | `Sprite` | 8 | pos (vec3), size, color (vec4) |

use makepad_script_compute::kernel::{FieldTy, Layout, LayoutField};

/// Words per `LinePoint` record.
pub const POINT_RECORD: usize = 9;
/// Words per `LineSegment` record.
pub const SEGMENT_RECORD: usize = 12;
/// Words per `Sprite` record.
pub const SPRITE_RECORD: usize = 8;

fn field(name: &str, ty: FieldTy, offset: u32) -> LayoutField {
    LayoutField { name: name.into(), ty, offset }
}

/// The line and point record layouts.
pub fn layouts() -> Vec<Layout> {
    vec![
        Layout {
            name: "LinePoint".into(),
            stride: POINT_RECORD as u32,
            fields: vec![field("pos", FieldTy::Vec3, 0), field("width", FieldTy::F32, 3), field("color", FieldTy::Vec4, 4), field("birth", FieldTy::F32, 8)],
        },
        Layout {
            name: "LineSegment".into(),
            stride: SEGMENT_RECORD as u32,
            fields: vec![
                field("a", FieldTy::Vec3, 0),
                field("b", FieldTy::Vec3, 3),
                field("width", FieldTy::F32, 6),
                field("color", FieldTy::Vec4, 7),
                field("birth", FieldTy::F32, 11),
            ],
        },
        Layout {
            name: "Sprite".into(),
            stride: SPRITE_RECORD as u32,
            fields: vec![field("pos", FieldTy::Vec3, 0), field("size", FieldTy::F32, 3), field("color", FieldTy::Vec4, 4)],
        },
    ]
}
