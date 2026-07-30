use crate::build::builder::{GraphBuilder, InclusionFilterStrategy};
use crate::build::sweep::FillStrategy;
use crate::core::fill_rule::FillRule;
use crate::core::solver::Solver;
use crate::segm::segment::{CLIP_BOTH, SUBJ_BOTH, Segment, SegmentFill};
use crate::segm::string::ShapeCountString;
use crate::string::clip::ClipRule;
use crate::string::graph::StringGraph;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_tree::Expiration;

impl<I> GraphBuilder<ShapeCountString, Vec<usize>, I>
where
    I: IntNumber + Expiration + SortKey,
{
    #[inline]
    pub(crate) fn build_string_all(
        &mut self,
        fill_rule: FillRule,
        solver: &Solver,
        segments: &[Segment<ShapeCountString, I>],
    ) -> StringGraph<'_, I> {
        self.build_string_fills(fill_rule, solver, segments);
        self.build_links_all(segments);
        self.string_graph(solver)
    }

    #[inline]
    pub(crate) fn build_string_clip(
        &mut self,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        solver: &Solver,
        segments: &[Segment<ShapeCountString, I>],
    ) -> StringGraph<'_, I> {
        self.build_string_fills(fill_rule, solver, segments);
        match clip_rule {
            ClipRule {
                invert: true,
                boundary_included: true,
            } => self.build_links_by_filter::<ClipOutsideBoundaryIncludedFilter>(segments),
            ClipRule {
                invert: true,
                boundary_included: false,
            } => self.build_links_by_filter::<ClipOutsideBoundaryExcludedFilter>(segments),
            ClipRule {
                invert: false,
                boundary_included: true,
            } => self.build_links_by_filter::<ClipInsideBoundaryIncludedFilter>(segments),
            ClipRule {
                invert: false,
                boundary_included: false,
            } => self.build_links_by_filter::<ClipInsideBoundaryExcludedFilter>(segments),
        }
        self.string_graph(solver)
    }

    #[inline]
    fn build_string_fills(
        &mut self,
        fill_rule: FillRule,
        solver: &Solver,
        segments: &[Segment<ShapeCountString, I>],
    ) {
        match fill_rule {
            FillRule::EvenOdd => self.build_fills_with_strategy::<EvenOddStrategy>(solver, segments),
            FillRule::NonZero => self.build_fills_with_strategy::<NonZeroStrategy>(solver, segments),
            FillRule::Positive => self.build_fills_with_strategy::<PositiveStrategy>(solver, segments),
            FillRule::Negative => self.build_fills_with_strategy::<NegativeStrategy>(solver, segments),
        }
    }

    #[inline]
    fn string_graph(&mut self, solver: &Solver) -> StringGraph<'_, I> {
        self.build_nodes_and_connect_links(solver);
        StringGraph {
            nodes: &self.nodes,
            links: &mut self.links,
        }
    }
}

struct EvenOddStrategy;
struct NonZeroStrategy;
struct PositiveStrategy;
struct NegativeStrategy;

impl FillStrategy<ShapeCountString> for EvenOddStrategy {
    #[inline(always)]
    fn add_and_fill(this: ShapeCountString, bot: ShapeCountString) -> (ShapeCountString, SegmentFill) {
        let subj = bot.subj + this.subj;
        let top = ShapeCountString { subj, clip: 0 };

        let subj_top = 1 & top.subj as SegmentFill;
        let subj_bot = 1 & bot.subj as SegmentFill;

        let fill = subj_top | (subj_bot << 1) | (this.clip << 2);

        (top, fill)
    }
}

impl FillStrategy<ShapeCountString> for NonZeroStrategy {
    #[inline(always)]
    fn add_and_fill(this: ShapeCountString, bot: ShapeCountString) -> (ShapeCountString, SegmentFill) {
        let subj = bot.subj + this.subj;
        let top = ShapeCountString { subj, clip: 0 }; // clip not need

        let subj_top = (top.subj != 0) as SegmentFill;
        let subj_bot = (bot.subj != 0) as SegmentFill;

        let fill = subj_top | (subj_bot << 1) | (this.clip << 2);

        (top, fill)
    }
}

impl FillStrategy<ShapeCountString> for PositiveStrategy {
    #[inline(always)]
    fn add_and_fill(this: ShapeCountString, bot: ShapeCountString) -> (ShapeCountString, SegmentFill) {
        let subj = bot.subj + this.subj;
        let top = ShapeCountString { subj, clip: 0 }; // clip not need

        let subj_top = (top.subj > 0) as SegmentFill;
        let subj_bot = (bot.subj > 0) as SegmentFill;

        let fill = subj_top | (subj_bot << 1) | (this.clip << 2);

        (top, fill)
    }
}

impl FillStrategy<ShapeCountString> for NegativeStrategy {
    #[inline(always)]
    fn add_and_fill(this: ShapeCountString, bot: ShapeCountString) -> (ShapeCountString, SegmentFill) {
        let subj = bot.subj + this.subj;
        let top = ShapeCountString { subj, clip: 0 }; // clip not need

        let subj_top = (top.subj < 0) as SegmentFill;
        let subj_bot = (bot.subj < 0) as SegmentFill;

        let fill = subj_top | (subj_bot << 1) | (this.clip << 2);

        (top, fill)
    }
}

struct ClipInsideBoundaryExcludedFilter;
struct ClipInsideBoundaryIncludedFilter;
struct ClipOutsideBoundaryExcludedFilter;
struct ClipOutsideBoundaryIncludedFilter;

impl InclusionFilterStrategy for ClipInsideBoundaryExcludedFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_clip_inside_boundary_excluded()
    }
}

impl InclusionFilterStrategy for ClipInsideBoundaryIncludedFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_clip_inside_boundary_included()
    }
}

impl InclusionFilterStrategy for ClipOutsideBoundaryExcludedFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_clip_outside_boundary_excluded()
    }
}

impl InclusionFilterStrategy for ClipOutsideBoundaryIncludedFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_clip_outside_boundary_included()
    }
}

trait StringFillFilter {
    fn is_clip_outside_boundary_excluded(&self) -> bool;
    fn is_clip_outside_boundary_included(&self) -> bool;
    fn is_clip_inside_boundary_included(&self) -> bool;
    fn is_clip_inside_boundary_excluded(&self) -> bool;
}

impl StringFillFilter for SegmentFill {
    #[inline(always)]
    fn is_clip_outside_boundary_excluded(&self) -> bool {
        let fill = *self;
        if fill & CLIP_BOTH != 0 {
            (fill & SUBJ_BOTH).count_ones() < 2
        } else {
            false
        }
    }

    #[inline(always)]
    fn is_clip_outside_boundary_included(&self) -> bool {
        let fill = *self;
        if fill & CLIP_BOTH != 0 {
            (fill & SUBJ_BOTH).count_ones() == 0
        } else {
            false
        }
    }

    #[inline(always)]
    fn is_clip_inside_boundary_included(&self) -> bool {
        let fill = *self;
        if fill & CLIP_BOTH != 0 {
            (fill & SUBJ_BOTH).count_ones() >= 1
        } else {
            false
        }
    }

    #[inline(always)]
    fn is_clip_inside_boundary_excluded(&self) -> bool {
        let fill = *self;
        if fill & CLIP_BOTH != 0 {
            (fill & SUBJ_BOTH).count_ones() == 2
        } else {
            false
        }
    }
}
