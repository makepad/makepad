use alloc::vec::Vec;
use core::cmp::{self, Ordering};
use core::ops::RangeInclusive;

use ttf_parser::GlyphId;

/// A set of glyphs.
///
/// Performs best when the glyphs are in consecutive ranges.
#[derive(Clone, Debug)]
pub struct GlyphSet {
    ranges: Vec<RangeInclusive<GlyphId>>,
}

impl GlyphSet {
    /// Create a new glyph set builder.
    pub fn builder() -> GlyphSetBuilder {
        GlyphSetBuilder { ranges: Vec::new() }
    }

    /// Check whether the glyph is contained in the set.
    pub fn contains(&self, glyph: GlyphId) -> bool {
        self.ranges.binary_search_by(|range| {
            if glyph < *range.start() {
                Ordering::Greater
            } else if glyph <= *range.end() {
                Ordering::Equal
            } else {
                Ordering::Less
            }
        }).is_ok()
    }
}

/// A builder for a [`GlyphSet`].
#[derive(Clone, Debug)]
pub struct GlyphSetBuilder {
    ranges: Vec<RangeInclusive<GlyphId>>,
}

impl GlyphSetBuilder {
    /// Insert a single glyph.
    pub fn insert(&mut self, glyph: GlyphId) {
        self.ranges.push(glyph..=glyph);
    }

    /// Insert a range of glyphs.
    pub fn insert_range(&mut self, range: RangeInclusive<GlyphId>) {
        self.ranges.push(range);
    }

    /// Finish the set building.
    pub fn finish(self) -> GlyphSet {
        let mut ranges = self.ranges;

        // Sort because we want to use binary search in `GlyphSet::contains`.
        ranges.sort_by_key(|range| *range.start());

        // The visited and merged ranges are in `ranges[..=left]` and the
        // unvisited ranges in `ranges[right..]`.
        let mut left = 0;
        let mut right = 1;

        // Merge touching and overlapping adjacent ranges.
        //
        // The cloning is cheap, it's just needed because `RangeInclusive<T>`
        // does not implement `Copy`.
        while let Some(next) = ranges.get(right).cloned() {
            right += 1;

            if let Some(prev) = ranges.get_mut(left) {
                // Detect whether the ranges can be merged.
                //
                // We add one to `prev.end` because we want to merge touching ranges
                // like `1..=3` and `4..=5`. We have to be careful with overflow,
                // hence `saturating_add`.
                if next.start().0 <= prev.end().0.saturating_add(1) {
                    *prev = *prev.start()..=cmp::max(*prev.end(), *next.end());
                    continue;
                }
            }

            left += 1;
            ranges[left] = next.clone();
        }

        // Can't overflow because `left < ranges.len() <= isize::MAX`.
        ranges.truncate(left + 1);

        GlyphSet { ranges }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_empty() {
        assert!(GlyphSet::builder().finish().ranges.is_empty());
    }

    #[test]
    fn test_contains() {
        let mut builder = GlyphSet::builder();
        builder.insert(GlyphId(1));
        builder.insert_range(GlyphId(3)..=GlyphId(5));
        let set = builder.finish();
        assert!(set.contains(GlyphId(1)));
        assert!(!set.contains(GlyphId(2)));
        assert!(set.contains(GlyphId(3)));
        assert!(set.contains(GlyphId(4)));
        assert!(set.contains(GlyphId(5)));
        assert!(!set.contains(GlyphId(6)));
    }

    #[test]
    fn test_merge_ranges() {
        let mut builder = GlyphSet::builder();
        builder.insert(GlyphId(0));
        builder.insert_range(GlyphId(2)..=GlyphId(6));
        builder.insert_range(GlyphId(3)..=GlyphId(7));
        builder.insert_range(GlyphId(9)..=GlyphId(10));
        builder.insert(GlyphId(9));
        builder.insert_range(GlyphId(18)..=GlyphId(21));
        builder.insert_range(GlyphId(11)..=GlyphId(14));
        assert_eq!(builder.finish().ranges, vec![
            GlyphId(0)..=GlyphId(0),
            GlyphId(2)..=GlyphId(7),
            GlyphId(9)..=GlyphId(14),
            GlyphId(18)..=GlyphId(21),
        ])
    }

    #[test]
    fn test_merge_ranges_at_numeric_boundaries() {
        let mut builder = GlyphSet::builder();
        builder.insert_range(GlyphId(3)..=GlyphId(u16::MAX));
        builder.insert(GlyphId(u16::MAX - 1));
        builder.insert(GlyphId(2));
        assert_eq!(builder.finish().ranges, vec![GlyphId(2)..=GlyphId(u16::MAX)]);
    }
}
