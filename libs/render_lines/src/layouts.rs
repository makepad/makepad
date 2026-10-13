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

use makepad_script_compute::boundary::{register_layout, LayoutFieldView, LayoutView};
use makepad_script_compute::kernel::FieldTy;

/// Words per `LinePoint` record.
pub const POINT_RECORD: usize = 9;
/// Words per `LineSegment` record.
pub const SEGMENT_RECORD: usize = 12;
/// Words per `Sprite` record.
pub const SPRITE_RECORD: usize = 8;

const fn field(name: &'static str, ty: FieldTy, offset: u32) -> LayoutFieldView<'static> {
    LayoutFieldView { name, ty, offset }
}

const LAYOUTS: [LayoutView<'static>; 3] = [
    LayoutView {
        name: "LinePoint",
        stride: POINT_RECORD as u32,
        fields: &[field("pos", FieldTy::Vec3, 0), field("width", FieldTy::F32, 3), field("color", FieldTy::Vec4, 4), field("birth", FieldTy::F32, 8)],
    },
    LayoutView {
        name: "LineSegment",
        stride: SEGMENT_RECORD as u32,
        fields: &[
            field("a", FieldTy::Vec3, 0),
            field("b", FieldTy::Vec3, 3),
            field("width", FieldTy::F32, 6),
            field("color", FieldTy::Vec4, 7),
            field("birth", FieldTy::F32, 11),
        ],
    },
    LayoutView {
        name: "Sprite",
        stride: SPRITE_RECORD as u32,
        fields: &[field("pos", FieldTy::Vec3, 0), field("size", FieldTy::F32, 3), field("color", FieldTy::Vec4, 4)],
    },
];

/// The line and point record layouts, registered with the kernel compiler
/// as these records' tables (a kernel compiled with a different
/// `LinePoint`, `LineSegment` or `Sprite` fails with a mismatch error).
pub fn layouts() -> &'static [LayoutView<'static>] {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        for layout in &LAYOUTS {
            // false: another table holds the name; compiles with this one report the mismatch.
            register_layout(layout);
        }
    });
    &LAYOUTS
}
