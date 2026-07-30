use crate::int::shape::{IntContour, IntShape, IntShapes};
use alloc::vec;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;

/// A trait that provides methods for simplifying complex geometrical structures.
pub trait Simplify {
    /// Simplifies the structure in-place if it is not already simple.
    ///
    /// # Returns
    ///
    /// - `true` if the structure was simplified successfully.
    /// - `false` if the structure was already simple and no modification was made.
    fn simplify_contour(&mut self) -> bool;
}

/// A trait for determining if a contour is simple and for obtaining a simplified version.
pub trait SimpleContour<I: IntNumber> {
    /// Checks if the contour is already simple, meaning it has no self-intersections
    /// and meets the minimum complexity required.
    ///
    /// # Returns
    ///
    /// - `true` if the contour is simple.
    /// - `false` if the contour is complex or does not meet simplicity criteria.
    fn is_simple(&self) -> bool;

    /// Returns an optional simplified version of the contour.
    ///
    /// # Returns
    ///
    /// - `Some(IntContour)` containing the simplified contour if simplification is possible.
    /// - `None` if the contour is degenerate or empty.
    fn simplified(&self) -> Option<IntContour<I>>;
}

/// A trait for determining if a shape, composed of multiple contours, is simple,
/// and for obtaining a simplified version.
pub trait SimpleShape<I: IntNumber> {
    /// Checks if the shape is simple, meaning all its contours are simple.
    ///
    /// # Returns
    ///
    /// - `true` if all contours in the shape are simple.
    /// - `false` if any contour is complex.
    fn is_simple(&self) -> bool;

    /// Returns an optional simplified version of the shape.
    ///
    /// # Returns
    ///
    /// - `Some(IntShape)` containing the simplified shape if simplification is possible.
    /// - `None` if the shape is degenerate or empty.
    fn simplified(&self) -> Option<IntShape<I>>;
}

/// A trait for determining if a collection of shapes is simple, and for obtaining
/// a simplified version of the entire collection.
pub trait SimpleShapes<I: IntNumber> {
    /// Checks if all shapes in the collection are simple.
    ///
    /// # Returns
    ///
    /// - `true` if all shapes are simple.
    /// - `false` if any shape in the collection is complex.
    fn is_simple(&self) -> bool;

    /// Returns an optional simplified version of the collection.
    ///
    /// # Returns
    ///
    /// - `IntShapes` the simplified shapes.
    fn simplified(&self) -> IntShapes<I>;
}

impl<I: IntNumber> Simplify for IntContour<I> {
    #[inline]
    fn simplify_contour(&mut self) -> bool {
        if self.is_simple() {
            return false;
        }
        if let Some(contour) = self.simplified() {
            self.clear();
            self.extend(contour);
        } else {
            self.clear()
        }
        true
    }
}

impl<I: IntNumber> Simplify for IntShape<I> {
    fn simplify_contour(&mut self) -> bool {
        let mut any_simplified = false;
        let mut any_empty = false;

        for (index, contour) in self.iter_mut().enumerate() {
            if contour.is_simple() {
                continue;
            }
            any_simplified = true;

            if let Some(simple_contour) = contour.simplified() {
                contour.clear();
                contour.extend(simple_contour);
            } else if index == 0 {
                // early out main contour is empty
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

impl<I: IntNumber> Simplify for IntShapes<I> {
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
            self.retain(|contour| !contour.is_empty());
        }

        any_simplified
    }
}
impl<I: IntNumber> SimpleShape<I> for [IntContour<I>] {
    #[inline]
    fn is_simple(&self) -> bool {
        for contour in self.iter() {
            if !contour.is_simple() {
                return false;
            }
        }
        true
    }

    fn simplified(&self) -> Option<IntShape<I>> {
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

impl<I: IntNumber> SimpleShapes<I> for [IntShape<I>] {
    #[inline]
    fn is_simple(&self) -> bool {
        for shape in self.iter() {
            if !shape.is_simple() {
                return false;
            }
        }
        true
    }

    fn simplified(&self) -> IntShapes<I> {
        let mut shapes = Vec::with_capacity(self.len());
        for shape in self.iter() {
            if shape.is_simple() {
                shapes.push(shape.clone());
            } else if let Some(simple) = shape.simplified() {
                shapes.push(simple);
            }
        }

        shapes
    }
}

impl<I: IntNumber> SimpleContour<I> for [IntPoint<I>] {
    fn is_simple(&self) -> bool {
        let count = self.len();

        if count < 3 {
            return false;
        }

        let mut p0 = self[count - 2];
        let p1 = self[count - 1];

        let mut v0 = p1 - p0;
        p0 = p1;

        for &pi in self.iter() {
            let vi = pi - p0;
            let prod = vi.cross_product(v0);
            if prod == I::Wide::ZERO {
                return false;
            }
            v0 = vi;
            p0 = pi;
        }

        true
    }

    #[inline]
    fn simplified(&self) -> Option<IntContour<I>> {
        ContourSimplifier::default().simplify_contour(self)
    }
}

#[derive(Default)]
pub struct ContourSimplifier {
    nodes: Vec<Node>,
    validated: Vec<bool>,
}

impl ContourSimplifier {
    pub fn simplify_contour<I: IntNumber>(&mut self, contour: &[IntPoint<I>]) -> Option<IntContour<I>> {
        let mut n = contour.len();

        if n < 3 {
            return None;
        }

        self.validated.clear();
        self.validated.resize(n, false);

        self.nodes.clear();
        self.nodes.reserve(n);

        let mut prev = n - 1;
        let mut next = 1;
        let last = n - 1;
        #[allow(clippy::explicit_counter_loop)]
        for index in 0..last {
            self.nodes.push(Node { next, index, prev });
            prev = index;
            next += 1;
        }
        self.nodes.push(Node {
            next: 0,
            index: last,
            prev,
        });

        let mut first: usize = 0;
        let mut node = self.nodes[first];
        let mut i = 0;
        while i < n {
            if self.validated[node.index] {
                node = self.nodes[node.next];
                continue;
            }

            let p0 = contour[node.prev];
            let p1 = contour[node.index];
            let p2 = contour[node.next];

            if (p1 - p0).cross_product(p2 - p1) == I::Wide::ZERO {
                n -= 1;
                if n < 3 {
                    return None;
                }

                // remove node
                self.nodes[node.prev].next = node.next;
                self.nodes[node.next].prev = node.prev;

                if node.index == first {
                    first = node.next
                }

                node = self.nodes[node.prev];

                if self.validated[node.prev] {
                    i -= 1;
                    self.validated[node.prev] = false
                }

                if self.validated[node.next] {
                    i -= 1;
                    self.validated[node.next] = false
                }

                if self.validated[node.index] {
                    i -= 1;
                    self.validated[node.index] = false
                }
            } else {
                self.validated[node.index] = true;
                i += 1;
                node = self.nodes[node.next];
            }
        }

        let mut buffer = vec![IntPoint::<I>::ZERO; n];
        node = self.nodes[first];

        for item in buffer.iter_mut().take(n) {
            *item = contour[node.index];
            node = self.nodes[node.next];
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
