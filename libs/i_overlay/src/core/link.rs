use crate::core::extract::VisitState;
use crate::core::overlay_rule::OverlayRule;
use crate::geom::id_point::IdPoint;
use crate::segm::segment::SegmentFill;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;

#[derive(Clone)]
pub(crate) struct OverlayLink<I: IntNumber, D = ()> {
    pub(crate) a: IdPoint<I>,
    pub(crate) b: IdPoint<I>,
    pub(crate) fill: SegmentFill,
    pub(crate) data: D,
}

impl<I: IntNumber, D> OverlayLink<I, D> {
    #[inline(always)]
    pub(crate) fn new_with_data(
        a: IdPoint<I>,
        b: IdPoint<I>,
        fill: SegmentFill,
        data: D,
    ) -> OverlayLink<I, D> {
        OverlayLink { a, b, fill, data }
    }

    #[inline(always)]
    pub(crate) fn other(&self, node_id: usize) -> IdPoint<I> {
        if self.a.id == node_id { self.b } else { self.a }
    }

    #[inline(always)]
    pub(crate) fn is_direct(&self) -> bool {
        self.a.point < self.b.point
    }
}

pub(crate) trait OverlayLinkFilter {
    fn filter_by_overlay_into(&self, overlay_rule: OverlayRule, buffer: &mut Vec<VisitState>);
}
