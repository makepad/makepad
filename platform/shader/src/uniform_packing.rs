//! Slot placement shared by shader host code and platform uniform buffers.
use makepad_relax_api::relax_api;
#[relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum UniformPacking {
    Tight,
    Std140,
    Hlsl,
    Metal,
}
impl UniformPacking {
    /// Place one field and advance the caller's slot cursor.
    #[inline]
    pub fn place(self, cursor: &mut usize, slots: usize) -> usize {
        let size = if matches!(self, Self::Metal) && slots == 3 {
            4
        } else {
            slots
        };
        let align = match self {
            Self::Tight => 1,
            Self::Hlsl => {
                if slots > 4 || (*cursor & 3) + slots > 4 {
                    4
                } else {
                    1
                }
            }
            Self::Std140 | Self::Metal => match size {
                1 => 1,
                2 => 2,
                _ => 4,
            },
        };
        if *cursor % align != 0 {
            *cursor += align - (*cursor % align);
        }
        let offset = *cursor;
        *cursor += size;
        offset
    }
}
