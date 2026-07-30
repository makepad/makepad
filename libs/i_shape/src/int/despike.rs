use crate::int::shape::{IntContour, IntShape, IntShapes};
use alloc::vec;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;

/// A trait for removing spike artifacts from polygon contours.
pub trait DeSpike {
    /// Removes spikes from the contour in-place.
    ///
    /// # Returns
    ///
    /// - `true` if spikes were found and removed.
    /// - `false` if the contour was already clean.
    fn remove_spikes(&mut self) -> bool;
}

pub trait DeSpikeContour<I: IntNumber> {
    /// Checks whether the contour has no spikes.
    ///
    /// A contour with no spikes is considered clean and valid
    /// for most geometric operations.
    ///
    /// # Returns
    ///
    /// - `true` if the contour has no spike patterns.
    /// - `false` if any spike-like edge reversal is detected.
    fn has_no_spikes(&self) -> bool;

    /// Returns a copy of the contour with spikes removed.
    ///
    /// # Returns
    ///
    /// - `Some(IntContour)` if a valid, despiked contour can be produced.
    /// - `None` if the contour is degenerate after spike removal.
    fn despiked_contour(&self) -> Option<IntContour<I>>;
}

pub trait DeSpikeShape<I: IntNumber> {
    /// Checks whether the shape has no spikes.
    ///
    /// A contour with no spikes is considered clean and valid
    /// for most geometric operations.
    ///
    /// # Returns
    ///
    /// - `true` if the contour has no spike patterns.
    /// - `false` if any spike-like edge reversal is detected.
    fn has_no_spikes(&self) -> bool;

    /// Returns an optional simplified version of the shape.
    ///
    /// # Returns
    ///
    /// - `Some(IntShape)` containing the simplified shape if simplification is possible.
    /// - `None` if the shape is degenerate or empty.
    fn despiked_shape(&self) -> Option<IntShape<I>>;
}

pub trait DeSpikeShapes<I: IntNumber> {
    /// Checks whether the shapes have no spikes.
    ///
    /// A contour with no spikes is considered clean and valid
    /// for most geometric operations.
    ///
    /// # Returns
    ///
    /// - `true` if the contour has no spike patterns.
    /// - `false` if any spike-like edge reversal is detected.
    fn has_no_spikes(&self) -> bool;

    /// Returns an optional simplified version of the collection.
    ///
    /// # Returns
    ///
    /// - `IntShapes` the simplified shapes.
    fn despiked_shapes(&self) -> IntShapes<I>;
}

impl<I: IntNumber> DeSpike for IntContour<I> {
    fn remove_spikes(&mut self) -> bool {
        if self.has_no_spikes() {
            return false;
        }
        if let Some(contour) = self.despiked_contour() {
            *self = contour;
        } else {
            self.clear()
        }
        true
    }
}

impl<I: IntNumber> DeSpikeContour<I> for IntContour<I> {
    fn has_no_spikes(&self) -> bool {
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
            let cross = vi.cross_product(v0);
            let dot = vi.dot_product(v0);
            if cross == I::Wide::ZERO && dot < I::Wide::ZERO {
                return false;
            }
            v0 = vi;
            p0 = pi;
        }

        true
    }

    fn despiked_contour(&self) -> Option<IntContour<I>> {
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

            let p0 = self[node.prev];
            let p1 = self[node.index];
            let p2 = self[node.next];

            let v10 = p1 - p0;
            let v21 = p2 - p1;
            let cross = v10.cross_product(v21);
            let dot = v10.dot_product(v21);

            if cross == I::Wide::ZERO && dot < I::Wide::ZERO {
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

        let mut buffer = vec![IntPoint::<I>::ZERO; n];
        node = nodes[first];

        for item in buffer.iter_mut().take(n) {
            *item = self[node.index];
            node = nodes[node.next];
        }

        Some(buffer)
    }
}

impl<I: IntNumber> DeSpike for IntShape<I> {
    fn remove_spikes(&mut self) -> bool {
        let mut any_simplified = false;
        let mut any_empty = false;

        for (index, contour) in self.iter_mut().enumerate() {
            if contour.has_no_spikes() {
                continue;
            }
            any_simplified = true;

            if let Some(simple_contour) = contour.despiked_contour() {
                *contour = simple_contour;
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

impl<I: IntNumber> DeSpikeShape<I> for IntShape<I> {
    fn has_no_spikes(&self) -> bool {
        for contour in self.iter() {
            if !contour.has_no_spikes() {
                return false;
            }
        }
        true
    }

    fn despiked_shape(&self) -> Option<IntShape<I>> {
        let mut contours = Vec::with_capacity(self.len());
        for (i, contour) in self.iter().enumerate() {
            if contour.has_no_spikes() {
                contours.push(contour.clone());
            } else if let Some(simple) = contour.despiked_contour() {
                contours.push(simple);
            } else if i == 0 {
                return None;
            }
        }

        Some(contours)
    }
}

impl<I: IntNumber> DeSpike for IntShapes<I> {
    fn remove_spikes(&mut self) -> bool {
        let mut any_simplified = false;
        let mut any_empty = false;

        for shape in self.iter_mut() {
            if shape.has_no_spikes() {
                continue;
            }
            any_simplified = true;
            if let Some(simple_shape) = shape.despiked_shape() {
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

impl<I: IntNumber> DeSpikeShapes<I> for IntShapes<I> {
    fn has_no_spikes(&self) -> bool {
        for shape in self.iter() {
            if !shape.has_no_spikes() {
                return false;
            }
        }
        true
    }

    fn despiked_shapes(&self) -> IntShapes<I> {
        let mut shapes = Vec::with_capacity(self.len());
        for shape in self.iter() {
            if shape.has_no_spikes() {
                shapes.push(shape.clone());
            } else if let Some(simple) = shape.despiked_shape() {
                shapes.push(simple);
            }
        }

        shapes
    }
}

#[derive(Clone, Copy)]
struct Node {
    next: usize,
    index: usize,
    prev: usize,
}

#[cfg(test)]
mod tests {
    use crate::int::despike::DeSpike;
    use crate::int_path;

    #[test]
    fn test_0() {
        let mut contour = int_path![[0, 0], [1, 0], [1, 1], [0, 1],];

        let modified = contour.remove_spikes();

        assert_eq!(contour.len(), 4);
        assert_eq!(modified, false);
    }

    #[test]
    fn test_1() {
        let mut contour = int_path![[0, -1], [0, 1], [1, 1], [1, 0], [0, 0],];

        let modified = contour.remove_spikes();

        assert_eq!(contour.len(), 4);
        assert_eq!(modified, true);
    }

    #[test]
    fn test_2() {
        let mut contour = int_path![[0, -1], [0, 1], [1, 1], [1, 0], [0, 0],];

        let modified = contour.remove_spikes();

        assert_eq!(contour.len(), 4);
        assert_eq!(modified, true);
    }

    #[test]
    fn test_3() {
        let mut contour = int_path![[0, 0], [0, 2], [1, 2], [3, 2], [4, 2], [2, 2], [2, 0],];

        let modified = contour.remove_spikes();

        assert_eq!(contour.len(), 5);
        assert_eq!(modified, true);
    }

    #[test]
    fn test_4() {
        let mut contour = int_path![[0, 0], [0, 2], [1, 2], [4, 2], [3, 2], [2, 2], [2, 0],];

        let modified = contour.remove_spikes();

        assert_eq!(contour.len(), 5);
        assert_eq!(modified, true);
    }

    #[test]
    fn test_5() {
        let mut contour = int_path![
            [-10, 10],
            [-10, 0],
            [-10, -10],
            [0, -10],
            [10, -10],
            [10, 0],
            [10, 10],
            [0, 10],
        ];

        let modified = contour.remove_spikes();

        assert_eq!(contour.len(), 8);
        assert_eq!(modified, false);
    }
}
