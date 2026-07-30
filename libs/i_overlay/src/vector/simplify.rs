use crate::core::edge_data::OverlayEdgeData;
use crate::vector::edge::{DataVectorEdge, DataVectorPath, DataVectorShape};
use alloc::vec;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;
use i_float::int::vector::IntVector;

/// Simplifies vector contours by removing collinear points when possible.
pub(super) trait VectorSimplify {
    /// Removes redundant points/edges when they lie on a straight line.
    ///
    /// Returns `true` when the contour was modified, `false` otherwise.
    fn simplify_contour(&mut self) -> bool;
}

pub(super) trait VectorSimpleContour {
    type Int: IntNumber;
    type Data: OverlayEdgeData;
    fn is_simple(&self) -> bool;
    fn simplified(&self) -> Option<DataVectorPath<Self::Int, Self::Data>>;
}

pub(super) trait VectorSimpleShape {
    type Int: IntNumber;
    type Data: OverlayEdgeData;
    fn is_simple(&self) -> bool;
    fn simplified(&self) -> Option<DataVectorShape<Self::Int, Self::Data>>;
}

impl<I: IntNumber, D: OverlayEdgeData> VectorSimplify for DataVectorPath<I, D> {
    #[inline]
    fn simplify_contour(&mut self) -> bool {
        if self.is_simple() {
            return false;
        }
        if let Some(simple) = self.simplified() {
            *self = simple;
        } else {
            self.clear();
        }
        true
    }
}

impl<I: IntNumber, D: OverlayEdgeData> VectorSimplify for DataVectorShape<I, D> {
    #[inline]
    fn simplify_contour(&mut self) -> bool {
        let mut any_simplified = false;
        let mut any_empty = false;

        for (index, contour) in self.iter_mut().enumerate() {
            if contour.is_simple() {
                continue;
            }
            any_simplified = true;

            if let Some(simple_contour) = contour.simplified() {
                *contour = simple_contour;
            } else if index == 0 {
                self.clear();
                return true;
            } else {
                contour.clear();
                any_empty = true;
            }
        }

        if any_empty {
            self.retain(|contour| !contour.is_empty());
        }

        any_simplified
    }
}

impl<I: IntNumber, D: OverlayEdgeData> VectorSimplify for Vec<DataVectorShape<I, D>> {
    #[inline]
    fn simplify_contour(&mut self) -> bool {
        let mut any_simplified = false;
        let mut any_empty = false;

        for shape in self.iter_mut() {
            if shape.is_simple() {
                continue;
            }
            any_simplified = true;
            if let Some(simple_shape) = shape.simplified() {
                *shape = simple_shape;
            } else {
                shape.clear();
                any_empty = true;
            }
        }

        if any_empty {
            self.retain(|shape| !shape.is_empty());
        }

        any_simplified
    }
}

impl<I: IntNumber, D: OverlayEdgeData> VectorSimpleContour for [DataVectorEdge<I, D>] {
    type Int = I;
    type Data = D;

    #[inline]
    fn is_simple(&self) -> bool {
        let count = self.len();
        if count < 3 {
            return false;
        }

        let mut prev = &self[count - 1];
        for edge in self.iter() {
            let curr = direction(edge);
            if curr.cross_product(direction(prev)) == I::Wide::ZERO && edge.data == prev.data {
                return false;
            }
            prev = edge;
        }

        true
    }

    fn simplified(&self) -> Option<DataVectorPath<I, D>> {
        if self.len() < 3 {
            return None;
        }

        let mut n = self.len();
        let mut nodes: Vec<Node> = vec![
            Node {
                next: 0,
                index: 0,
                prev: 0
            };
            n
        ];
        let mut validated: Vec<bool> = vec![false; n];

        let mut i0 = n - 2;
        let mut i1 = n - 1;
        for i2 in 0..n {
            nodes[i1] = Node {
                next: i2,
                index: i1,
                prev: i0,
            };
            i0 = i1;
            i1 = i2;
        }

        let mut first: usize = 0;
        let mut node = nodes[first];
        let mut i = 0;
        while i < n {
            if validated[node.index] {
                node = nodes[node.next];
                continue;
            }

            let p0 = self[node.prev].b;
            let p1 = self[node.index].b;
            let p2 = self[node.next].b;

            if (p1 - p0).cross_product(p2 - p1) == I::Wide::ZERO
                && self[node.index].data == self[node.next].data
            {
                n -= 1;
                if n < 3 {
                    return None;
                }

                // remove node
                nodes[node.prev].next = node.next;
                nodes[node.next].prev = node.prev;

                if node.index == first {
                    first = node.next
                }

                node = nodes[node.prev];

                if validated[node.prev] {
                    i -= 1;
                    validated[node.prev] = false
                }

                if validated[node.next] {
                    i -= 1;
                    validated[node.next] = false
                }

                if validated[node.index] {
                    i -= 1;
                    validated[node.index] = false
                }
            } else {
                validated[node.index] = true;
                i += 1;
                node = nodes[node.next];
            }
        }

        let mut buffer = vec![DataVectorEdge::new(0, IntPoint::ZERO, IntPoint::ZERO, self[0].data); n];
        node = nodes[first];

        let mut e0 = &self[node.index];
        for item in buffer.iter_mut().take(n) {
            node = nodes[node.next];
            let e1 = &self[node.index];
            item.a = e0.b;
            item.b = e1.b;
            item.fill = e1.fill;
            item.data = e1.data;

            e0 = e1;
        }

        Some(buffer)
    }
}

#[derive(Clone, Copy)]
struct Node {
    next: usize,
    index: usize,
    prev: usize,
}

impl<I: IntNumber, D: OverlayEdgeData> VectorSimpleShape for [DataVectorPath<I, D>] {
    type Int = I;
    type Data = D;

    #[inline]
    fn is_simple(&self) -> bool {
        self.iter().all(|contour| contour.is_simple())
    }

    fn simplified(&self) -> Option<DataVectorShape<I, D>> {
        let mut contours = Vec::with_capacity(self.len());
        for (i, contour) in self.iter().enumerate() {
            if contour.is_simple() {
                contours.push(contour.clone());
            } else if let Some(simple) = contour.simplified() {
                contours.push(simple);
            } else if i == 0 {
                return None;
            }
        }

        Some(contours)
    }
}

#[inline]
fn direction<I: IntNumber, D>(edge: &DataVectorEdge<I, D>) -> IntVector<I> {
    edge.b - edge.a
}

#[cfg(test)]
mod tests {
    use crate::core::edge_data::{EdgeDataMerge, OverlayEdgeData};
    use crate::vector::edge::DataVectorEdge;
    use crate::vector::simplify::{IntPoint, VectorSimplify};
    use alloc::vec;
    use i_float::int_pnt;

    #[derive(Clone, Copy, PartialEq)]
    enum TestData {
        A,
        B,
    }

    impl<C> OverlayEdgeData<C> for TestData
    where
        C: Copy + Send + Sync,
    {
        type Store = ();

        fn merge(ctx: EdgeDataMerge<C, Self>, _: &mut Self::Store) -> Self {
            if ctx.lhs_data == ctx.rhs_data {
                ctx.lhs_data
            } else {
                TestData::B
            }
        }
    }

    #[test]
    fn test_0() {
        #[rustfmt::skip]
        let mut contour = vec![
            DataVectorEdge::new(1, int_pnt!(0, -1), int_pnt!(0, -3), ()),
            DataVectorEdge::new(2, int_pnt!(0, -3), int_pnt!(1, -3), ()),
            DataVectorEdge::new(3, int_pnt!(1, -3), int_pnt!(3, -3), ()),
            DataVectorEdge::new(4, int_pnt!(3, -3), int_pnt!(3,  0), ()),
            DataVectorEdge::new(5, int_pnt!(3,  0), int_pnt!(0,  0), ()),
            DataVectorEdge::new(6, int_pnt!(0,  0), int_pnt!(0, -1), ()),
        ];

        let result = contour.simplify_contour();

        debug_assert!(result);
        debug_assert!(contour.len() == 4);
    }

    #[test]
    fn test_duplicate_points() {
        #[rustfmt::skip]
        let mut contour = vec![
            DataVectorEdge::new(1, int_pnt!(-1, 3), int_pnt!(-1, 1), ()),
            DataVectorEdge::new(2, int_pnt!(-1, 1), int_pnt!(-1, 1), ()),
            DataVectorEdge::new(3, int_pnt!(-1, 1), int_pnt!(-3, 1), ()),
            DataVectorEdge::new(4, int_pnt!(-3, 1), int_pnt!(-3, -2), ()),
            DataVectorEdge::new(5, int_pnt!(-3, -2), int_pnt!(3, -2), ()),
            DataVectorEdge::new(6, int_pnt!(3, -2), int_pnt!(3, 1), ()),
            DataVectorEdge::new(7, int_pnt!(3, 1), int_pnt!(3, 1), ()),
            DataVectorEdge::new(8, int_pnt!(3, 1), int_pnt!(1, 1), ()),
            DataVectorEdge::new(9, int_pnt!(1, 1), int_pnt!(1, 3), ()),
            DataVectorEdge::new(10, int_pnt!(1, 3), int_pnt!(-1, 3), ()),
        ];

        let result = contour.simplify_contour();

        debug_assert!(result);
        debug_assert!(contour.len() == 8);
    }

    #[test]
    fn test_tiny_segments() {
        #[rustfmt::skip]
        let mut contour = vec![
            DataVectorEdge::new(1, int_pnt!(0, 2), int_pnt!(-1, 1), ()),
            DataVectorEdge::new(2, int_pnt!(-1, 1), int_pnt!(-2, 0), ()),
            DataVectorEdge::new(3, int_pnt!(-2, 0), int_pnt!(0, -1), ()),
            DataVectorEdge::new(4, int_pnt!(0, -1), int_pnt!(2, 0), ()),
            DataVectorEdge::new(5, int_pnt!(2, 0), int_pnt!(1, 1), ()),
            DataVectorEdge::new(6, int_pnt!(1, 1), int_pnt!(0, 2), ()),
        ];

        let result = contour.simplify_contour();

        debug_assert!(result);
        debug_assert!(contour.len() == 4);
    }

    #[test]
    fn test_collinear_runs() {
        #[rustfmt::skip]
        let mut contour = vec![
            DataVectorEdge::new(1, int_pnt!(-2, -2), int_pnt!(0, -2), ()),
            DataVectorEdge::new(2, int_pnt!(0, -2), int_pnt!(2, -2), ()),
            DataVectorEdge::new(3, int_pnt!(2, -2), int_pnt!(2, 0), ()),
            DataVectorEdge::new(4, int_pnt!(2, 0), int_pnt!(2, 2), ()),
            DataVectorEdge::new(5, int_pnt!(2, 2), int_pnt!(0, 2), ()),
            DataVectorEdge::new(6, int_pnt!(0, 2), int_pnt!(-2, 2), ()),
            DataVectorEdge::new(7, int_pnt!(-2, 2), int_pnt!(-2, 0), ()),
            DataVectorEdge::new(8, int_pnt!(-2, 0), int_pnt!(-2, -2), ()),
        ];

        let result = contour.simplify_contour();

        debug_assert!(result);
        debug_assert!(contour.len() == 4);
    }

    #[test]
    fn test_collinear_same_data() {
        #[rustfmt::skip]
        let mut contour = vec![
            DataVectorEdge::new(1, int_pnt!(0, 0), int_pnt!(2, 0), TestData::A),
            DataVectorEdge::new(2, int_pnt!(2, 0), int_pnt!(4, 0), TestData::A),
            DataVectorEdge::new(3, int_pnt!(4, 0), int_pnt!(4, 4), TestData::A),
            DataVectorEdge::new(4, int_pnt!(4, 4), int_pnt!(0, 4), TestData::A),
            DataVectorEdge::new(5, int_pnt!(0, 4), int_pnt!(0, 0), TestData::A),
        ];

        let result = contour.simplify_contour();

        debug_assert!(result);
        debug_assert!(contour.len() == 4);
    }

    #[test]
    fn test_collinear_different_data() {
        #[rustfmt::skip]
        let mut contour = vec![
            DataVectorEdge::new(1, int_pnt!(0, 0), int_pnt!(2, 0), TestData::A),
            DataVectorEdge::new(2, int_pnt!(2, 0), int_pnt!(4, 0), TestData::B),
            DataVectorEdge::new(3, int_pnt!(4, 0), int_pnt!(4, 4), TestData::A),
            DataVectorEdge::new(4, int_pnt!(4, 4), int_pnt!(0, 4), TestData::A),
            DataVectorEdge::new(5, int_pnt!(0, 4), int_pnt!(0, 0), TestData::A),
        ];

        let result = contour.simplify_contour();

        debug_assert!(!result);
        debug_assert!(contour.len() == 5);
    }

    #[test]
    fn test_zero_area_path() {
        #[rustfmt::skip]
        let mut contour = vec![
            DataVectorEdge::new(1, int_pnt!(-3, 0), int_pnt!(0, 0), ()),
            DataVectorEdge::new(2, int_pnt!(0, 0), int_pnt!(3, 0), ()),
            DataVectorEdge::new(3, int_pnt!(3, 0), int_pnt!(-3, 0), ()),
        ];

        let result = contour.simplify_contour();

        debug_assert!(result);
        debug_assert!(contour.is_empty());
    }
}
