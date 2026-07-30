use crate::core::overlay::ShapeType;
use crate::segm::winding::WindingCount;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeCountBoolean {
    pub subj: i32,
    pub clip: i32,
}

impl ShapeCountBoolean {
    pub(crate) const SUBJ_DIRECT: ShapeCountBoolean = ShapeCountBoolean { subj: 1, clip: 0 };
    pub(crate) const SUBJ_INVERT: ShapeCountBoolean = ShapeCountBoolean { subj: -1, clip: 0 };
    pub(crate) const CLIP_DIRECT: ShapeCountBoolean = ShapeCountBoolean { subj: 0, clip: 1 };
    pub(crate) const CLIP_INVERT: ShapeCountBoolean = ShapeCountBoolean { subj: 0, clip: -1 };
}

impl WindingCount for ShapeCountBoolean {
    #[inline(always)]
    fn is_not_empty(&self) -> bool {
        self.subj != 0 || self.clip != 0
    }

    #[inline(always)]
    fn new(subj: i32, clip: i32) -> Self {
        Self { subj, clip }
    }

    #[inline(always)]
    fn with_shape_type(shape_type: ShapeType) -> (Self, Self) {
        match shape_type {
            ShapeType::Subject => (ShapeCountBoolean::SUBJ_DIRECT, ShapeCountBoolean::SUBJ_INVERT),
            ShapeType::Clip => (ShapeCountBoolean::CLIP_DIRECT, ShapeCountBoolean::CLIP_INVERT),
        }
    }

    #[inline(always)]
    fn direct_count(shape_type: ShapeType) -> Self {
        match shape_type {
            ShapeType::Subject => ShapeCountBoolean::SUBJ_DIRECT,
            ShapeType::Clip => ShapeCountBoolean::CLIP_DIRECT,
        }
    }

    #[inline(always)]
    fn invert_count(shape_type: ShapeType) -> Self {
        match shape_type {
            ShapeType::Subject => ShapeCountBoolean::SUBJ_INVERT,
            ShapeType::Clip => ShapeCountBoolean::CLIP_INVERT,
        }
    }

    #[inline(always)]
    fn add(self, count: Self) -> Self {
        let subj = self.subj + count.subj;
        let clip = self.clip + count.clip;

        Self { subj, clip }
    }

    #[inline(always)]
    fn invert(self) -> Self {
        Self {
            subj: -self.subj,
            clip: -self.clip,
        }
    }
}
