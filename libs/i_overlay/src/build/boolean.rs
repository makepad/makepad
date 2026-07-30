use crate::build::builder::{GraphBuilder, InclusionFilterStrategy};
use crate::build::sweep::{
    EvenOddStrategy, FillStrategy, NegativeStrategy, NonZeroStrategy, PositiveStrategy,
};
use crate::core::edge_data::OverlayEdgeData;
use crate::core::extract::VisitState;
use crate::core::fill_rule::FillRule;
use crate::core::graph::OverlayGraph;
use crate::core::graph::OverlayNode;
use crate::core::link::OverlayLink;
use crate::core::link::OverlayLinkFilter;
use crate::core::overlay::IntOverlayOptions;
use crate::core::overlay_rule::OverlayRule;
use crate::core::solver::Solver;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::segment::{
    ALL, BOTH_BOTTOM, BOTH_TOP, CLIP_BOTH, CLIP_BOTTOM, CLIP_TOP, SUBJ_BOTH, SUBJ_BOTTOM, SUBJ_TOP, Segment,
    SegmentFill,
};
use crate::segm::winding::WindingCount;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_shape::util::reserve::Reserve;
use i_tree::Expiration;

impl<I, D> GraphBuilder<ShapeCountBoolean, OverlayNode, I, D>
where
    I: IntNumber + Expiration + SortKey,
    D: OverlayEdgeData,
{
    #[inline]
    pub(crate) fn build_boolean_all(
        &mut self,
        fill_rule: FillRule,
        options: IntOverlayOptions<I::WideUInt>,
        solver: &Solver,
        segments: &[Segment<ShapeCountBoolean, I, D>],
    ) -> OverlayGraph<'_, I, D> {
        self.build_boolean_fills(fill_rule, solver, segments);
        self.build_links_all(segments);
        self.boolean_graph(options, solver)
    }

    #[inline]
    pub(crate) fn build_boolean_overlay(
        &mut self,
        fill_rule: FillRule,
        overlay_rule: OverlayRule,
        options: IntOverlayOptions<I::WideUInt>,
        solver: &Solver,
        segments: &[Segment<ShapeCountBoolean, I, D>],
    ) -> OverlayGraph<'_, I, D> {
        self.build_boolean_fills(fill_rule, solver, segments);
        match overlay_rule {
            OverlayRule::Subject => self.build_links_by_filter::<SubjectFilter>(segments),
            OverlayRule::Clip => self.build_links_by_filter::<ClipFilter>(segments),
            OverlayRule::Intersect => self.build_links_by_filter::<IntersectFilter>(segments),
            OverlayRule::Union => self.build_links_by_filter::<UnionFilter>(segments),
            OverlayRule::Difference => self.build_links_by_filter::<DifferenceFilter>(segments),
            OverlayRule::InverseDifference => self.build_links_by_filter::<InverseDifferenceFilter>(segments),
            OverlayRule::Xor => self.build_links_by_filter::<XorFilter>(segments),
        }
        self.boolean_graph(options, solver)
    }

    #[inline]
    fn build_boolean_fills(
        &mut self,
        fill_rule: FillRule,
        solver: &Solver,
        segments: &[Segment<ShapeCountBoolean, I, D>],
    ) {
        match fill_rule {
            FillRule::EvenOdd => self.build_fills_with_strategy::<EvenOddStrategy>(solver, segments),
            FillRule::NonZero => self.build_fills_with_strategy::<NonZeroStrategy>(solver, segments),
            FillRule::Positive => self.build_fills_with_strategy::<PositiveStrategy>(solver, segments),
            FillRule::Negative => self.build_fills_with_strategy::<NegativeStrategy>(solver, segments),
        }
    }

    #[inline]
    fn boolean_graph(
        &mut self,
        options: IntOverlayOptions<I::WideUInt>,
        solver: &Solver,
    ) -> OverlayGraph<'_, I, D> {
        self.build_nodes_and_connect_links(solver);
        OverlayGraph {
            nodes: &self.nodes,
            links: &self.links,
            options,
        }
    }
}

impl FillStrategy<ShapeCountBoolean> for EvenOddStrategy {
    #[inline(always)]
    fn add_and_fill(this: ShapeCountBoolean, bot: ShapeCountBoolean) -> (ShapeCountBoolean, SegmentFill) {
        let top = bot.add(this);
        let subj_top = 1 & top.subj as SegmentFill;
        let subj_bot = 1 & bot.subj as SegmentFill;
        let clip_top = 1 & top.clip as SegmentFill;
        let clip_bot = 1 & bot.clip as SegmentFill;

        let fill = subj_top | (subj_bot << 1) | (clip_top << 2) | (clip_bot << 3);

        (top, fill)
    }
}

impl FillStrategy<ShapeCountBoolean> for NonZeroStrategy {
    #[inline(always)]
    fn add_and_fill(this: ShapeCountBoolean, bot: ShapeCountBoolean) -> (ShapeCountBoolean, SegmentFill) {
        let top = bot.add(this);
        let subj_top = (top.subj != 0) as SegmentFill;
        let subj_bot = (bot.subj != 0) as SegmentFill;
        let clip_top = (top.clip != 0) as SegmentFill;
        let clip_bot = (bot.clip != 0) as SegmentFill;

        let fill = subj_top | (subj_bot << 1) | (clip_top << 2) | (clip_bot << 3);

        (top, fill)
    }
}

impl FillStrategy<ShapeCountBoolean> for PositiveStrategy {
    #[inline(always)]
    fn add_and_fill(this: ShapeCountBoolean, bot: ShapeCountBoolean) -> (ShapeCountBoolean, SegmentFill) {
        let top = bot.add(this);
        let subj_top = (top.subj > 0) as SegmentFill;
        let subj_bot = (bot.subj > 0) as SegmentFill;
        let clip_top = (top.clip > 0) as SegmentFill;
        let clip_bot = (bot.clip > 0) as SegmentFill;

        let fill = subj_top | (subj_bot << 1) | (clip_top << 2) | (clip_bot << 3);

        (top, fill)
    }
}

impl FillStrategy<ShapeCountBoolean> for NegativeStrategy {
    #[inline(always)]
    fn add_and_fill(this: ShapeCountBoolean, bot: ShapeCountBoolean) -> (ShapeCountBoolean, SegmentFill) {
        let top = bot.add(this);
        let subj_top = (top.subj < 0) as SegmentFill;
        let subj_bot = (bot.subj < 0) as SegmentFill;
        let clip_top = (top.clip < 0) as SegmentFill;
        let clip_bot = (bot.clip < 0) as SegmentFill;

        let fill = subj_top | (subj_bot << 1) | (clip_top << 2) | (clip_bot << 3);

        (top, fill)
    }
}

struct SubjectFilter;
struct ClipFilter;
struct IntersectFilter;
struct UnionFilter;
struct DifferenceFilter;
struct InverseDifferenceFilter;
struct XorFilter;

impl InclusionFilterStrategy for SubjectFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_subject()
    }
}

impl InclusionFilterStrategy for ClipFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_clip()
    }
}

impl InclusionFilterStrategy for IntersectFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_intersect()
    }
}

impl InclusionFilterStrategy for UnionFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_union()
    }
}

impl InclusionFilterStrategy for DifferenceFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_difference()
    }
}

impl InclusionFilterStrategy for InverseDifferenceFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_inverse_difference()
    }
}

impl InclusionFilterStrategy for XorFilter {
    #[inline(always)]
    fn is_included(fill: SegmentFill) -> bool {
        fill.is_xor()
    }
}

trait BooleanFillFilter {
    fn is_subject(&self) -> bool;
    fn is_clip(&self) -> bool;
    fn is_intersect(&self) -> bool;
    fn is_union(&self) -> bool;
    fn is_difference(&self) -> bool;
    fn is_inverse_difference(&self) -> bool;
    fn is_xor(&self) -> bool;
}

impl BooleanFillFilter for SegmentFill {
    #[inline(always)]
    fn is_subject(&self) -> bool {
        let fill = *self;
        let subj = fill & SUBJ_BOTH;
        subj == SUBJ_TOP || subj == SUBJ_BOTTOM
    }

    #[inline(always)]
    fn is_clip(&self) -> bool {
        let fill = *self;
        let clip = fill & CLIP_BOTH;
        clip == CLIP_TOP || clip == CLIP_BOTTOM
    }

    #[inline(always)]
    fn is_intersect(&self) -> bool {
        let fill = *self;
        let top = fill & BOTH_TOP;
        let bottom = fill & BOTH_BOTTOM;

        (top == BOTH_TOP || bottom == BOTH_BOTTOM) && fill != ALL
    }

    #[inline(always)]
    fn is_union(&self) -> bool {
        let fill = *self;
        let top = fill & BOTH_TOP;
        let bottom = fill & BOTH_BOTTOM;
        (top == 0 || bottom == 0) && fill != 0
    }

    #[inline(always)]
    fn is_difference(&self) -> bool {
        let fill = *self;
        let top = fill & BOTH_TOP;
        let bottom = fill & BOTH_BOTTOM;
        let is_not_inner_subj = fill != SUBJ_BOTH;
        (top == SUBJ_TOP || bottom == SUBJ_BOTTOM) && is_not_inner_subj
    }

    #[inline(always)]
    fn is_inverse_difference(&self) -> bool {
        let fill = *self;
        let top = fill & BOTH_TOP;
        let bottom = fill & BOTH_BOTTOM;
        let is_not_inner_clip = fill != CLIP_BOTH;
        (top == CLIP_TOP || bottom == CLIP_BOTTOM) && is_not_inner_clip
    }

    #[inline(always)]
    fn is_xor(&self) -> bool {
        let fill = *self;
        let top = fill & BOTH_TOP;
        let bottom = fill & BOTH_BOTTOM;

        let is_any_top = top == SUBJ_TOP || top == CLIP_TOP;
        let is_any_bottom = bottom == SUBJ_BOTTOM || bottom == CLIP_BOTTOM;

        // only one of it must be true
        is_any_top != is_any_bottom
    }
}

impl<I: IntNumber, D> OverlayLinkFilter for [OverlayLink<I, D>] {
    #[inline]
    fn filter_by_overlay_into(&self, overlay_rule: OverlayRule, buffer: &mut Vec<VisitState>) {
        match overlay_rule {
            OverlayRule::Subject => filter_subject_into(self, buffer),
            OverlayRule::Clip => filter_clip_into(self, buffer),
            OverlayRule::Intersect => filter_intersect_into(self, buffer),
            OverlayRule::Union => filter_union_into(self, buffer),
            OverlayRule::Difference => filter_difference_into(self, buffer),
            OverlayRule::Xor => filter_xor_into(self, buffer),
            OverlayRule::InverseDifference => filter_inverse_difference_into(self, buffer),
        }
    }
}

#[inline]
fn filter_subject_into<I: IntNumber, D>(links: &[OverlayLink<I, D>], buffer: &mut Vec<VisitState>) {
    buffer.clear();
    buffer.reserve_capacity(links.len());
    for link in links.iter() {
        buffer.push(VisitState::new(!link.fill.is_subject()));
    }
}

#[inline]
fn filter_clip_into<I: IntNumber, D>(links: &[OverlayLink<I, D>], buffer: &mut Vec<VisitState>) {
    buffer.clear();
    buffer.reserve_capacity(links.len());
    for link in links.iter() {
        buffer.push(VisitState::new(!link.fill.is_clip()));
    }
}

#[inline]
fn filter_intersect_into<I: IntNumber, D>(links: &[OverlayLink<I, D>], buffer: &mut Vec<VisitState>) {
    buffer.clear();
    buffer.reserve_capacity(links.len());
    for link in links.iter() {
        buffer.push(VisitState::new(!link.fill.is_intersect()));
    }
}

#[inline]
fn filter_union_into<I: IntNumber, D>(links: &[OverlayLink<I, D>], buffer: &mut Vec<VisitState>) {
    buffer.clear();
    buffer.reserve_capacity(links.len());
    for link in links.iter() {
        buffer.push(VisitState::new(!link.fill.is_union()));
    }
}

#[inline]
fn filter_difference_into<I: IntNumber, D>(links: &[OverlayLink<I, D>], buffer: &mut Vec<VisitState>) {
    buffer.clear();
    buffer.reserve_capacity(links.len());
    for link in links.iter() {
        buffer.push(VisitState::new(!link.fill.is_difference()));
    }
}

#[inline]
fn filter_inverse_difference_into<I: IntNumber, D>(
    links: &[OverlayLink<I, D>],
    buffer: &mut Vec<VisitState>,
) {
    buffer.clear();
    buffer.reserve_capacity(links.len());
    for link in links.iter() {
        buffer.push(VisitState::new(!link.fill.is_inverse_difference()));
    }
}

#[inline]
fn filter_xor_into<I: IntNumber, D>(links: &[OverlayLink<I, D>], buffer: &mut Vec<VisitState>) {
    buffer.clear();
    buffer.reserve_capacity(links.len());
    for link in links.iter() {
        buffer.push(VisitState::new(!link.fill.is_xor()));
    }
}
