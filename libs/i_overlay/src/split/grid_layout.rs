use crate::geom::line_range::LineRange;
use crate::geom::x_segment::XSegment;
use crate::split::fragment::Fragment;
use alloc::vec;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::number::uint::UIntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::rect::IntRect;

pub(super) struct BorderVSegment<I: IntNumber> {
    pub(super) id: usize,
    pub(super) x: I,
    pub(super) y_range: LineRange<I>,
}

pub(super) struct FragmentBuffer<I: IntNumber> {
    pub(super) layout: GridLayout<I>,
    pub(super) groups: Vec<Vec<Fragment<I>>>,
    pub(super) on_border: Vec<BorderVSegment<I>>,
}

impl<I: IntNumber> FragmentBuffer<I> {
    #[inline]
    pub(super) fn new(layout: GridLayout<I>) -> Self {
        let n = layout.index(layout.max_x) + 1;
        Self {
            layout,
            groups: vec![Vec::new(); n],
            on_border: Vec::with_capacity(64),
        }
    }

    pub(super) fn init_fragment_buffer<It>(&mut self, iter: It)
    where
        It: Iterator<Item = XSegment<I>>,
    {
        let mut counts = vec![0; self.groups.len()];
        for s in iter {
            let i0 = self.layout.index(s.a.x);
            if s.a.x < s.b.x {
                let i1 = self.layout.index(s.b.x - I::ONE);
                for count in counts.iter_mut().take(i1).skip(i0) {
                    *count += 1;
                }
            } else {
                counts[i0] += 1;
            }
        }

        for (i, group) in self.groups.iter_mut().enumerate() {
            group.reserve(counts[i]);
        }
    }

    #[inline]
    fn insert(&mut self, fragment: Fragment<I>, bin_index: usize) {
        debug_assert!(bin_index < self.groups.len());
        // SAFETY: `bin_index` is produced by GridLayout::index(...) on coordinates clamped to
        // [min_x, max_x]. We sized `groups` to `index(max_x) + 1` in `new`, so
        // bin_index < groups.len() always holds (see callers). We take a unique &mut to the slot
        // and only push into the inner Vec<Fragment>, which cannot reallocate `self.groups`.
        unsafe { self.groups.get_unchecked_mut(bin_index) }.push(fragment);
    }

    #[inline]
    fn insert_vertical(&mut self, fragment: Fragment<I>, bin_index: usize) {
        let x = fragment.x_segment.a.x;
        if bin_index > 0 && x == self.layout.pos(bin_index) {
            self.on_border.push(BorderVSegment {
                id: fragment.index,
                y_range: fragment.x_segment.y_range(),
                x,
            });
        }
        self.insert(fragment, bin_index);
    }

    #[inline]
    pub(super) fn clear(&mut self) {
        for group in self.groups.iter_mut() {
            group.clear();
        }
        self.on_border.clear();
    }

    #[inline]
    pub(super) fn add_segment(&mut self, segment_index: usize, s: XSegment<I>) {
        if s.a.y == s.b.y {
            self.add_horizontal(segment_index, s);
            return;
        }

        let i0 = self.layout.index(s.a.x);
        if s.a.x == s.b.x {
            self.insert_vertical(Fragment::with_index_and_segment(segment_index, s), i0);
            return;
        }

        let i1 = self.layout.index(s.b.x - I::ONE);
        if i0 >= i1 {
            self.insert(Fragment::with_index_and_segment(segment_index, s), i0);
            return;
        }

        let x0 = s.a.x;
        let y0 = s.a.y;

        let mut prev_x = x0;
        let mut prev_y = y0;

        let is_inc = s.a.y <= s.b.y;

        let width = (s.b.x - s.a.x).to_uint();
        let height = (s.b.y.wide() - s.a.y.wide()).unsigned_abs();

        let log = (width * height).ilog2();
        let p = I::WideUInt::LAST_BIT_INDEX - log;
        let k = (height << p) / width;

        let mut w = (self.layout.pos(i0 + 1) - s.a.x).to_uint();
        let dw = I::WideUInt::ONE << self.layout.power;

        for i in i0..i1 {
            let h_min = (w * k) >> p;
            let mut h_max = h_min;

            let hw = height * w;
            if h_max * width < hw {
                h_max = (hw + width - I::WideUInt::ONE) / width;
            }

            let max_x = x0 + I::from_uint(w);

            let rect = if is_inc {
                let max_y = y0 + I::from_uint(h_max);
                let rect = IntRect {
                    min_x: prev_x,
                    max_x,
                    min_y: prev_y,
                    max_y,
                };
                prev_y = y0 + I::from_uint(h_min);
                rect
            } else {
                let min_y = y0 - I::from_uint(h_max);
                let rect = IntRect {
                    min_x: prev_x,
                    max_x,
                    min_y,
                    max_y: prev_y,
                };
                prev_y = y0 - I::from_uint(h_min);
                rect
            };

            prev_x = max_x;
            w += dw;

            self.insert(
                Fragment {
                    index: segment_index,
                    rect,
                    x_segment: s,
                },
                i,
            );
        }

        let rect = if is_inc {
            IntRect {
                min_x: prev_x,
                max_x: s.b.x,
                min_y: prev_y,
                max_y: s.b.y,
            }
        } else {
            IntRect {
                min_x: prev_x,
                max_x: s.b.x,
                min_y: s.b.y,
                max_y: prev_y,
            }
        };

        self.insert(
            Fragment {
                index: segment_index,
                rect,
                x_segment: s,
            },
            i1,
        );
    }

    fn add_horizontal(&mut self, segment_index: usize, s: XSegment<I>) {
        let i0 = self.layout.index(s.a.x);
        let i1 = self.layout.index(s.b.x - I::ONE);

        let y = s.a.y;
        let mut x0 = s.a.x;
        let mut i = i0;
        while i < i1 {
            let x = self.layout.pos(i + 1);
            let rect = IntRect {
                min_x: x0,
                max_x: x,
                min_y: y,
                max_y: y,
            };
            self.insert(
                Fragment {
                    index: segment_index,
                    rect,
                    x_segment: s,
                },
                i,
            );

            x0 = x;
            i += 1;
        }

        let rect = IntRect {
            min_x: x0,
            max_x: s.b.x,
            min_y: y,
            max_y: y,
        };
        self.insert(
            Fragment {
                index: segment_index,
                rect,
                x_segment: s,
            },
            i1,
        );
    }

    #[cfg(debug_assertions)]
    pub(super) fn is_on_border_sorted(&self) -> bool {
        for w in self.on_border.windows(2) {
            if w[0].x > w[1].x {
                return false;
            }
        }
        true
    }
}

pub(super) struct GridLayout<I: IntNumber> {
    min_x: I,
    max_x: I,
    power: u32,
}

impl<I: IntNumber> GridLayout<I> {
    #[inline]
    pub(super) fn index(&self, x: I) -> usize {
        ((x.wide() - self.min_x.wide()) >> self.power).to_usize()
    }

    #[inline]
    pub(super) fn pos(&self, index: usize) -> I {
        I::from_usize(index << self.power) + self.min_x
    }

    pub(super) fn new<It>(iter: It, count: usize) -> Option<Self>
    where
        It: Iterator<Item = XSegment<I>>,
    {
        let mut iter = iter.peekable();
        let first = iter.peek()?;
        let min_x = first.a.x;
        let mut max_x = first.b.x;

        for s in iter {
            max_x = s.b.x.max(max_x);
        }

        let log = count.ilog2();
        let max_power = log >> 1;

        Self::with_min_max(min_x, max_x, max_power)
    }

    fn with_min_max(min_x: I, max_x: I, max_power: u32) -> Option<Self> {
        let dx = max_x.wide() - min_x.wide();
        if dx < I::Wide::FOUR {
            return None;
        }
        let log = dx.ilog2();
        let power = if log > max_power { log - max_power } else { 1 };

        Some(Self { min_x, max_x, power })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::useless_vec)]

    use crate::geom::x_segment::XSegment;
    use crate::split::grid_layout::vec;
    use crate::split::grid_layout::{FragmentBuffer, GridLayout};
    use i_float::int::point::IntPoint;
    use i_float::int::rect::IntRect;
    use i_float::triangle::Triangle;
    use rand::RngExt;

    #[test]
    fn test_0() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 0 },
            b: IntPoint { x: 6, y: 3 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 0,
                max_y: 1,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 0,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: 1,
                max_y: 3,
            },
        );

        validate_rect(&segment, &buffer.groups[0][0].rect);
        validate_rect(&segment, &buffer.groups[1][0].rect);
        validate_rect(&segment, &buffer.groups[2][0].rect);
    }

    #[test]
    fn test_0_inv() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 3 },
            b: IntPoint { x: 6, y: 0 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 2,
                max_y: 3,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 1,
                max_y: 3,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: 0,
                max_y: 2,
            },
        );

        validate_rect(&segment, &buffer.groups[0][0].rect);
        validate_rect(&segment, &buffer.groups[1][0].rect);
        validate_rect(&segment, &buffer.groups[2][0].rect);
    }

    #[test]
    fn test_1() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 1 },
            b: IntPoint { x: 6, y: 4 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 1,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 1,
                max_y: 3,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: 2,
                max_y: 4,
            },
        );

        validate_rect(&segment, &buffer.groups[0][0].rect);
        validate_rect(&segment, &buffer.groups[1][0].rect);
        validate_rect(&segment, &buffer.groups[2][0].rect);
    }

    #[test]
    fn test_1_inv() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 4 },
            b: IntPoint { x: 6, y: 1 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 3,
                max_y: 4,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 2,
                max_y: 4,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: 1,
                max_y: 3,
            },
        );

        validate_rect(&segment, &buffer.groups[0][0].rect);
        validate_rect(&segment, &buffer.groups[1][0].rect);
        validate_rect(&segment, &buffer.groups[2][0].rect);
    }

    #[test]
    fn test_2() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: -1 },
            b: IntPoint { x: 6, y: 2 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: -1,
                max_y: 0,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: -1,
                max_y: 1,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: 0,
                max_y: 2,
            },
        );

        validate_rect(&segment, &buffer.groups[0][0].rect);
        validate_rect(&segment, &buffer.groups[1][0].rect);
        validate_rect(&segment, &buffer.groups[2][0].rect);
    }

    #[test]
    fn test_2_inv() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 2 },
            b: IntPoint { x: 6, y: -1 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 1,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 0,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: -1,
                max_y: 1,
            },
        );

        validate_rect(&segment, &buffer.groups[0][0].rect);
        validate_rect(&segment, &buffer.groups[1][0].rect);
        validate_rect(&segment, &buffer.groups[2][0].rect);
    }

    #[test]
    fn test_3() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 0 },
            b: IntPoint { x: 6, y: 1 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 0,
                max_y: 1,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 0,
                max_y: 1,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: 0,
                max_y: 1,
            },
        );

        validate_rect(&segment, &buffer.groups[0][0].rect);
        validate_rect(&segment, &buffer.groups[1][0].rect);
        validate_rect(&segment, &buffer.groups[2][0].rect);
    }

    #[test]
    fn test_3_inv() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 1 },
            b: IntPoint { x: 6, y: 0 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 0,
                max_y: 1,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 0,
                max_y: 1,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: 0,
                max_y: 1,
            },
        );

        validate_rect(&segment, &buffer.groups[0][0].rect);
        validate_rect(&segment, &buffer.groups[1][0].rect);
        validate_rect(&segment, &buffer.groups[2][0].rect);
    }

    #[test]
    fn test_4() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 0, y: 0 },
            b: IntPoint { x: 5, y: 3 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 0,
                max_x: 2,
                min_y: 0,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 1,
                max_y: 3,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 5,
                min_y: 2,
                max_y: 3,
            },
        );

        validate_rect(&segment, &buffer.groups[0][0].rect);
        validate_rect(&segment, &buffer.groups[1][0].rect);
        validate_rect(&segment, &buffer.groups[2][0].rect);
    }

    #[test]
    fn test_5() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 0 },
            b: IntPoint { x: 4, y: 5 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 2);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 0,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 1,
                max_y: 5,
            },
        );
    }

    #[test]
    fn test_6() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 0, y: 0 },
            b: IntPoint { x: 6, y: 6 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 0,
                max_x: 2,
                min_y: 0,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 2,
                max_y: 4,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: 4,
                max_y: 6,
            },
        );
    }

    #[test]
    fn test_7() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 1 },
            b: IntPoint { x: 5, y: 5 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 1,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 2,
                max_y: 4,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 5,
                min_y: 4,
                max_y: 5,
            },
        );
    }

    #[test]
    fn test_7_inv() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 5 },
            b: IntPoint { x: 5, y: 1 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 3);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 2,
                min_y: 4,
                max_y: 5,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 2,
                max_y: 4,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 5,
                min_y: 1,
                max_y: 2,
            },
        );
    }

    #[test]
    fn test_8() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 0, y: 0 },
            b: IntPoint { x: 7, y: 0 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 4);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 0,
                max_x: 2,
                min_y: 0,
                max_y: 0,
            },
        );
        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 0,
                max_y: 0,
            },
        );
        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: 4,
                max_x: 6,
                min_y: 0,
                max_y: 0,
            },
        );
        rect_compare(
            &buffer.groups[3][0].rect,
            &IntRect {
                min_x: 6,
                max_x: 7,
                min_y: 0,
                max_y: 0,
            },
        );
    }

    #[test]
    fn test_9() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 1 },
            b: IntPoint { x: 1, y: 9 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 1);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 1,
                min_y: 1,
                max_y: 9,
            },
        );
    }

    #[test]
    fn test_9_inv() {
        let layout = GridLayout {
            min_x: 0,
            max_x: 12,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: 1, y: 9 },
            b: IntPoint { x: 1, y: 1 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 7);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 1);

        rect_compare(
            &buffer.groups[0][0].rect,
            &IntRect {
                min_x: 1,
                max_x: 1,
                min_y: 1,
                max_y: 9,
            },
        );
    }

    #[test]
    fn test_10() {
        let layout = GridLayout {
            min_x: -1_000_000,
            max_x: 1_000_000,
            power: 10,
        };

        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint {
                x: -100_000,
                y: -100_000,
            },
            b: IntPoint {
                x: 100_000,
                y: 100_000,
            },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        for fragment in buffer.groups.iter().flatten() {
            let rect = &fragment.rect;
            let p0 = IntPoint::new(rect.min_x, rect.min_y);
            let p1 = IntPoint::new(rect.max_x, rect.min_y);
            let p2 = IntPoint::new(rect.min_x, rect.max_y);
            let p3 = IntPoint::new(rect.max_x, rect.max_y);

            assert!(Triangle::is_cw_or_line(p0, segment.a, segment.b));
            assert!(Triangle::is_cw_or_line(p1, segment.a, segment.b));
            assert!(Triangle::is_cw_or_line(p2, segment.b, segment.a));
            assert!(Triangle::is_cw_or_line(p3, segment.b, segment.a));
        }
    }

    #[test]
    fn test_11() {
        let layout = GridLayout {
            min_x: -10,
            max_x: 10,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: -6, y: 0 },
            b: IntPoint { x: 4, y: 2 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 11);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 5);

        validate_rect(&segment, &buffer.groups[2][0].rect);
        validate_rect(&segment, &buffer.groups[3][0].rect);
        validate_rect(&segment, &buffer.groups[4][0].rect);
        validate_rect(&segment, &buffer.groups[5][0].rect);
        validate_rect(&segment, &buffer.groups[6][0].rect);

        rect_compare(
            &buffer.groups[2][0].rect,
            &IntRect {
                min_x: -6,
                max_x: -4,
                min_y: 0,
                max_y: 1,
            },
        );
        rect_compare(
            &buffer.groups[3][0].rect,
            &IntRect {
                min_x: -4,
                max_x: -2,
                min_y: 0,
                max_y: 1,
            },
        );
        rect_compare(
            &buffer.groups[4][0].rect,
            &IntRect {
                min_x: -2,
                max_x: 0,
                min_y: 0,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[5][0].rect,
            &IntRect {
                min_x: 0,
                max_x: 2,
                min_y: 1,
                max_y: 2,
            },
        );
        rect_compare(
            &buffer.groups[6][0].rect,
            &IntRect {
                min_x: 2,
                max_x: 4,
                min_y: 1,
                max_y: 2,
            },
        );
    }

    #[test]
    fn test_12() {
        let layout = GridLayout {
            min_x: -10,
            max_x: 10,
            power: 1,
        };
        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: -8, y: -10 },
            b: IntPoint { x: -8, y: -9 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        assert_eq!(buffer.groups.len(), 11);
        assert_eq!(buffer.groups.iter().fold(0usize, |s, it| s + it.len()), 1);

        rect_compare(
            &buffer.groups[1][0].rect,
            &IntRect {
                min_x: -8,
                max_x: -8,
                min_y: -10,
                max_y: -9,
            },
        );
    }

    #[test]
    fn test_13() {
        let layout = GridLayout {
            min_x: -100_000,
            max_x: 100_000,
            power: 10,
        };

        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: -83143, y: 65289 },
            b: IntPoint { x: 45253, y: -76778 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        for fragment in buffer.groups.iter().flatten() {
            validate_rect(&segment, &fragment.rect);
        }
    }

    #[test]
    fn test_14() {
        let layout = GridLayout {
            min_x: -100_000,
            max_x: 100_000,
            power: 10,
        };

        let mut buffer = FragmentBuffer::new(layout);

        let segment = XSegment {
            a: IntPoint { x: -78454, y: -40819 },
            b: IntPoint { x: 47599, y: -57780 },
        };
        let segments = vec![segment];
        buffer.init_fragment_buffer(segments.iter().copied());

        buffer.add_segment(0, segment);

        for fragment in buffer.groups.iter().flatten() {
            validate_rect(&segment, &fragment.rect);
        }
    }

    #[test]
    fn test_loop_range_0() {
        let min_x = -10;
        let max_x = 10;
        let min_y = -10;
        let max_y = 10;

        let layout = GridLayout {
            min_x,
            max_x,
            power: 1,
        };

        let mut buffer = FragmentBuffer::new(layout);

        let mut a = IntPoint::new(min_x, min_y - 1);

        while let Some(ai) = next_point(a, min_y, max_x, max_y) {
            a = ai;
            let mut b = a;
            while let Some(bi) = next_point(b, min_y, max_x, max_y) {
                b = bi;

                let segment = XSegment { a, b };

                let segments = vec![segment];
                buffer.init_fragment_buffer(segments.iter().copied());

                buffer.add_segment(0, segment);

                for fragment in buffer.groups.iter().flatten() {
                    validate_rect(&segment, &fragment.rect);
                }

                buffer.clear();
            }
        }
    }

    #[test]
    fn test_loop_range_1() {
        let min_x = -20;
        let max_x = 20;
        let min_y = -20;
        let max_y = 20;

        let layout = GridLayout {
            min_x,
            max_x,
            power: 2,
        };

        let mut buffer = FragmentBuffer::new(layout);

        let mut a = IntPoint::new(min_x, min_y - 1);

        while let Some(ai) = next_point(a, min_y, max_x, max_y) {
            a = ai;
            let mut b = a;
            while let Some(bi) = next_point(b, min_y, max_x, max_y) {
                b = bi;

                let segment = XSegment { a, b };

                let segments = vec![segment];
                buffer.init_fragment_buffer(segments.iter().copied());

                buffer.add_segment(0, segment);

                for fragment in buffer.groups.iter().flatten() {
                    validate_rect(&segment, &fragment.rect);
                }

                buffer.clear();
            }
        }
    }

    #[test]
    fn test_loop_range_2() {
        let min_x = -20;
        let max_x = 20;
        let min_y = -20;
        let max_y = 20;

        let layout = GridLayout {
            min_x,
            max_x,
            power: 3,
        };

        let mut buffer = FragmentBuffer::new(layout);

        let mut a = IntPoint::new(min_x, min_y - 1);

        while let Some(ai) = next_point(a, min_y, max_x, max_y) {
            a = ai;
            let mut b = a;
            while let Some(bi) = next_point(b, min_y, max_x, max_y) {
                b = bi;

                let segment = XSegment { a, b };

                let segments = vec![segment];
                buffer.init_fragment_buffer(segments.iter().copied());

                buffer.add_segment(0, segment);

                for fragment in buffer.groups.iter().flatten() {
                    validate_rect(&segment, &fragment.rect);
                }

                buffer.clear();
            }
        }
    }

    #[test]
    fn test_loop_range_3() {
        let min_x = -20;
        let max_x = 20;
        let min_y = -20;
        let max_y = 20;

        let layout = GridLayout {
            min_x,
            max_x,
            power: 4,
        };

        let mut buffer = FragmentBuffer::new(layout);

        let mut a = IntPoint::new(min_x, min_y - 1);

        while let Some(ai) = next_point(a, min_y, max_x, max_y) {
            a = ai;
            let mut b = a;
            while let Some(bi) = next_point(b, min_y, max_x, max_y) {
                b = bi;

                let segment = XSegment { a, b };

                let segments = vec![segment];
                buffer.init_fragment_buffer(segments.iter().copied());

                buffer.add_segment(0, segment);

                for fragment in buffer.groups.iter().flatten() {
                    validate_rect(&segment, &fragment.rect);
                }

                buffer.clear();
            }
        }
    }

    #[test]
    fn test_random_0() {
        let layout = GridLayout {
            min_x: -100,
            max_x: 100,
            power: 4,
        };

        let range = layout.min_x..=layout.max_x;
        let mut buffer = FragmentBuffer::new(layout);

        let mut rng = rand::rng();

        for _ in 0..100_000 {
            let x0 = rng.random_range(range.clone());
            let y0 = rng.random_range(range.clone());
            let x1 = rng.random_range(range.clone());
            let y1 = rng.random_range(range.clone());

            let a = IntPoint::new(x0, y0);
            let b = IntPoint::new(x1, y1);

            if a == b {
                continue;
            }
            let segment = if a < b {
                XSegment { a, b }
            } else {
                XSegment { a: b, b: a }
            };

            let segments = vec![segment];
            buffer.init_fragment_buffer(segments.iter().copied());

            buffer.add_segment(0, segment);

            for fragment in buffer.groups.iter().flatten() {
                validate_rect(&segment, &fragment.rect);
            }

            buffer.clear();
        }
    }

    #[test]
    fn test_random_1() {
        let layout = GridLayout {
            min_x: -100_000,
            max_x: 100_000,
            power: 10,
        };

        let range = layout.min_x..=layout.max_x;
        let mut buffer = FragmentBuffer::new(layout);

        let mut rng = rand::rng();

        for _ in 0..10_000 {
            let x0 = rng.random_range(range.clone());
            let y0 = rng.random_range(range.clone());
            let x1 = rng.random_range(range.clone());
            let y1 = rng.random_range(range.clone());

            let a = IntPoint::new(x0, y0);
            let b = IntPoint::new(x1, y1);

            let segment = if a <= b {
                XSegment { a, b }
            } else {
                XSegment { a: b, b: a }
            };

            let segments = vec![segment];
            buffer.init_fragment_buffer(segments.iter().copied());

            buffer.add_segment(0, segment);

            for fragment in buffer.groups.iter().flatten() {
                validate_rect(&segment, &fragment.rect);
            }

            buffer.clear();
        }
    }

    #[inline]
    fn rect_compare(a: &IntRect<i32>, b: &IntRect<i32>) {
        assert_eq!(a.min_x, b.min_x);
        assert_eq!(a.max_x, b.max_x);
        assert_eq!(a.min_y, b.min_y);
        assert_eq!(a.max_y, b.max_y);
    }

    #[inline]
    fn validate_rect(segment: &XSegment<i32>, rect: &IntRect<i32>) {
        let p0 = IntPoint::new(rect.min_x, rect.min_y);
        let p1 = IntPoint::new(rect.max_x, rect.min_y);
        let p2 = IntPoint::new(rect.min_x, rect.max_y);
        let p3 = IntPoint::new(rect.max_x, rect.max_y);

        let a0 = Triangle::area_two(p0, segment.a, segment.b);
        let a1 = Triangle::area_two(p1, segment.a, segment.b);
        let a2 = Triangle::area_two(p2, segment.b, segment.a);
        let a3 = Triangle::area_two(p3, segment.b, segment.a);

        assert!(a0 <= 0);
        assert!(a1 <= 0);
        assert!(a2 <= 0);
        assert!(a3 <= 0);
    }

    #[inline]
    fn next_point(point: IntPoint, min_y: i32, max_x: i32, max_y: i32) -> Option<IntPoint> {
        let mut x = point.x;
        let mut y = point.y + 1;
        if y <= max_y {
            return Some(IntPoint::new(x, y));
        }
        y = min_y;
        x += 1;
        if x <= max_x {
            return Some(IntPoint::new(x, y));
        }

        None
    }
}
