use crate::core::overlay::ShapeType;
use crate::segm::winding::WindingCount;
use core::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeCountString {
    pub subj: i32,
    pub clip: u8,
}

pub(crate) const STRING_FORWARD_CLIP: u8 = 0b10;
pub(crate) const STRING_BACK_CLIP: u8 = 0b1;

impl WindingCount for ShapeCountString {
    #[inline(always)]
    fn is_not_empty(&self) -> bool {
        self.subj != 0 || self.clip != 0
    }

    #[inline(always)]
    fn new(subj: i32, clip: i32) -> Self {
        // 0 - bit - back
        // 1 - bit - forward
        let mask = match clip.cmp(&0) {
            Ordering::Greater => STRING_FORWARD_CLIP,
            Ordering::Less => STRING_BACK_CLIP,
            Ordering::Equal => 0,
        };
        Self { subj, clip: mask }
    }

    #[inline(always)]
    fn with_shape_type(shape_type: ShapeType) -> (Self, Self) {
        match shape_type {
            ShapeType::Subject => (Self { subj: 1, clip: 0 }, Self { subj: -1, clip: 0 }),
            ShapeType::Clip => (
                Self {
                    subj: 0,
                    clip: STRING_FORWARD_CLIP,
                },
                Self {
                    subj: 0,
                    clip: STRING_BACK_CLIP,
                },
            ),
        }
    }

    fn direct_count(shape_type: ShapeType) -> Self {
        match shape_type {
            ShapeType::Subject => Self { subj: 1, clip: 0 },
            ShapeType::Clip => Self {
                subj: 0,
                clip: STRING_FORWARD_CLIP,
            },
        }
    }

    fn invert_count(shape_type: ShapeType) -> Self {
        match shape_type {
            ShapeType::Subject => Self { subj: -1, clip: 0 },
            ShapeType::Clip => Self {
                subj: 0,
                clip: STRING_BACK_CLIP,
            },
        }
    }

    #[inline(always)]
    fn add(self, count: Self) -> Self {
        let subj = self.subj + count.subj;
        let clip = self.clip | count.clip;

        Self { subj, clip }
    }

    #[inline(always)]
    fn invert(self) -> Self {
        let b0 = self.clip & 0b01;
        let b1 = self.clip & 0b10;
        let clip = (b0 << 1) | (b1 >> 1);

        Self {
            subj: -self.subj,
            clip,
        }
    }
}
