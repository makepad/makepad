use crate::core::fill_rule::FillRule;
use crate::core::solver::Solver;
use crate::geom::end::End;
use crate::geom::v_segment::VSegment;
use crate::segm::segment::{Segment, SegmentFill};
use crate::segm::winding::WindingCount;
use crate::util::log::Int;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use i_float::int::number::int::IntNumber;
use i_float::triangle::Triangle;
use i_tree::Expiration;
use i_tree::key::exp::KeyExpCollection;
use i_tree::key::list::KeyExpList;
use i_tree::key::tree::KeyExpTree;

pub(crate) trait FillStrategy<C> {
    fn add_and_fill(this: C, bot: C) -> (C, SegmentFill);
}

pub(crate) trait FillHandler<C, I: IntNumber, D = ()> {
    type Output;
    fn handle(
        &mut self,
        index: usize,
        segment: &Segment<C, I, D>,
        fill: SegmentFill,
    ) -> ControlFlow<Self::Output>;
    fn finalize(self) -> Self::Output;
}

#[inline]
fn sweep_with_handler<I, C, D, F, S, H>(
    scan: &mut S,
    segments: &[Segment<C, I, D>],
    mut handler: H,
) -> H::Output
where
    I: IntNumber + Expiration,
    C: WindingCount,
    F: FillStrategy<C>,
    S: KeyExpCollection<VSegment<I>, I, C>,
    H: FillHandler<C, I, D>,
{
    let mut node = Vec::with_capacity(4);
    let n = segments.len();
    let mut i = 0;

    while i < n {
        let p = segments[i].x_segment.a;

        node.push(End {
            index: i,
            point: segments[i].x_segment.b,
        });
        i += 1;

        while i < n && segments[i].x_segment.a == p {
            node.push(End {
                index: i,
                point: segments[i].x_segment.b,
            });
            i += 1;
        }

        if node.len() > 1 {
            node.sort_by(|s0, s1| Triangle::clock_order(p, s1.point, s0.point));
        }

        let mut sum_count = scan.first_less_or_equal_by(p.x, C::new(0, 0), |s| s.is_under_point_order(p));

        for se in node.iter() {
            let sid = unsafe { segments.get_unchecked(se.index) };
            let (new_sum, fill) = F::add_and_fill(sid.count, sum_count);
            sum_count = new_sum;

            if let ControlFlow::Break(result) = handler.handle(se.index, sid, fill) {
                return result;
            }

            if sid.x_segment.is_not_vertical() {
                scan.insert(sid.x_segment.into(), sum_count, p.x);
            }
        }

        node.clear();
    }

    handler.finalize()
}

pub(crate) struct SweepRunner<C, I: IntNumber + Expiration> {
    list: Option<KeyExpList<VSegment<I>, I, C>>,
    tree: Option<KeyExpTree<VSegment<I>, I, C>>,
}

impl<C: WindingCount, I: IntNumber + Expiration> SweepRunner<C, I> {
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            list: None,
            tree: None,
        }
    }

    #[inline]
    pub(crate) fn run<D, F, H>(
        &mut self,
        solver: &Solver,
        segments: &[Segment<C, I, D>],
        handler: H,
    ) -> H::Output
    where
        F: FillStrategy<C>,
        H: FillHandler<C, I, D>,
        D: Send,
    {
        let count = segments.len();
        if solver.is_list_fill(segments) {
            let capacity = count.log2_sqrt().max(4) * 2;
            let mut list = self.take_scan_list(capacity);
            let result = sweep_with_handler::<I, C, D, F, _, _>(&mut list, segments, handler);
            self.list = Some(list);
            result
        } else {
            let capacity = count.log2_sqrt().max(8);
            let mut tree = self.take_scan_tree(capacity);
            let result = sweep_with_handler::<I, C, D, F, _, _>(&mut tree, segments, handler);
            self.tree = Some(tree);
            result
        }
    }

    #[inline]
    pub(crate) fn run_with_fill_rule<D, H>(
        &mut self,
        fill_rule: FillRule,
        solver: &Solver,
        segments: &[Segment<C, I, D>],
        handler: H,
    ) -> H::Output
    where
        H: FillHandler<C, I, D>,
        D: Send,
        EvenOddStrategy: FillStrategy<C>,
        NonZeroStrategy: FillStrategy<C>,
        PositiveStrategy: FillStrategy<C>,
        NegativeStrategy: FillStrategy<C>,
    {
        match fill_rule {
            FillRule::EvenOdd => self.run::<D, EvenOddStrategy, H>(solver, segments, handler),
            FillRule::NonZero => self.run::<D, NonZeroStrategy, H>(solver, segments, handler),
            FillRule::Positive => self.run::<D, PositiveStrategy, H>(solver, segments, handler),
            FillRule::Negative => self.run::<D, NegativeStrategy, H>(solver, segments, handler),
        }
    }

    #[inline]
    fn take_scan_list(&mut self, capacity: usize) -> KeyExpList<VSegment<I>, I, C> {
        if let Some(mut list) = self.list.take() {
            list.clear();
            list.reserve_capacity(capacity);
            list
        } else {
            KeyExpList::new(capacity)
        }
    }

    #[inline]
    fn take_scan_tree(&mut self, capacity: usize) -> KeyExpTree<VSegment<I>, I, C> {
        if let Some(mut tree) = self.tree.take() {
            tree.clear();
            tree.reserve_capacity(capacity);
            tree
        } else {
            KeyExpTree::new(capacity)
        }
    }
}

pub(crate) struct EvenOddStrategy;
pub(crate) struct NonZeroStrategy;
pub(crate) struct PositiveStrategy;
pub(crate) struct NegativeStrategy;
