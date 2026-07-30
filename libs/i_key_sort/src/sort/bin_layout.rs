use crate::sort::key::{KeyFn, SortKey};
use crate::sort::min_max::MinMax;

pub(crate) const BIN_SORT_MIN: usize = 64;
pub(crate) const MAX_BINS_POWER: u32 = 8;
pub(crate) const MAX_BINS_COUNT: usize = 1 << MAX_BINS_POWER;

#[derive(Debug, Clone)]
pub(crate) struct BinLayout<K> {
    pub(crate) min_key: K,
    pub(crate) max_key: K,
    pub(crate) power: usize,
    bin_width_is_one: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct LayoutConstraints {
    pub(crate) max_split_count: usize,
}

impl Default for LayoutConstraints {
    #[inline]
    fn default() -> Self {
        Self {
            max_split_count: MAX_BINS_COUNT,
        }
    }
}

impl<K: SortKey> BinLayout<K> {
    #[inline(always)]
    pub(super) fn bin_width_is_one(&self) -> bool {
        self.bin_width_is_one
    }

    #[inline(always)]
    pub fn index(&self, value: K) -> usize {
        debug_assert!(value >= self.min_key, "value must be >= min_key");
        value.shifted_distance(self.min_key, self.power)
    }

    #[inline(always)]
    pub fn count(&self) -> usize {
        self.index(self.max_key) + 1
    }

    pub(crate) fn with_constraints(
        min_key: K,
        max_key: K,
        constraints: LayoutConstraints,
    ) -> BinLayout<K> {
        // `max_split_count` is a sizing hint. Cap it at half of `usize::MAX`
        let max_split_count = constraints.max_split_count.clamp(1, usize::MAX / 2);
        let distance = max_key.shifted_distance(min_key, 0);
        let power = if distance < max_split_count {
            0
        } else {
            max_key
                .distance_bits(min_key)
                .saturating_sub(max_split_count.ilog2() as usize)
        };

        Self {
            min_key,
            max_key,
            power,
            bin_width_is_one: power == 0,
        }
    }

    #[inline(always)]
    pub fn with_keys<T, F: KeyFn<T, K>>(array: &[T], key: F) -> Option<Self> {
        if array.is_empty() {
            return None;
        }

        let (min_key, max_key) = array.min_max(key);

        if min_key == max_key {
            return None;
        }

        Some(Self::with_constraints(min_key, max_key, Default::default()))
    }
}

#[cfg(test)]
mod tests {
    use crate::sort::bin_layout::{BinLayout, LayoutConstraints, MAX_BINS_COUNT};

    #[test]
    fn test_0() {
        let layout = BinLayout::<i32>::with_constraints(0i32, 3i32, Default::default());
        assert_eq!(layout.power, 0);
    }

    #[test]
    fn test_1() {
        let layout = BinLayout::<i32>::with_constraints(0, 255, Default::default());

        assert_eq!(layout.power, 0);
    }

    #[test]
    fn test_2() {
        let layout = BinLayout::<i32>::with_constraints(0, 256, Default::default());

        assert_eq!(layout.power, 1);
    }

    #[test]
    fn test_i64_full_range() {
        let layout = BinLayout::<i64>::with_constraints(i64::MIN, i64::MAX, Default::default());

        assert_eq!(layout.power, 56);
        assert_eq!(layout.index(i64::MIN), 0);
        assert_eq!(layout.index(i64::MAX), MAX_BINS_COUNT - 1);
        assert_eq!(layout.count(), MAX_BINS_COUNT);
    }

    #[test]
    fn test_max_split_count_is_limited_to_usize() {
        let layout = BinLayout::<usize>::with_constraints(
            usize::MIN,
            usize::MAX,
            LayoutConstraints {
                max_split_count: usize::MAX,
            },
        );

        assert_eq!(layout.power, 2);
        assert_eq!(layout.count(), (usize::MAX >> 2) + 1);
    }

    #[test]
    fn test_non_power_of_two_max_split_count() {
        let constraints = LayoutConstraints {
            max_split_count: 160,
        };

        let layout = BinLayout::<i64>::with_constraints(0, 159, constraints);
        assert_eq!(layout.power, 0);
        assert_eq!(layout.count(), 160);

        let layout = BinLayout::<i64>::with_constraints(0, 40_000, constraints);
        assert_eq!(layout.power, 9);
        assert_eq!(layout.count(), 79);
    }

    #[test]
    fn test_zero_max_split_count() {
        let layout = BinLayout::<i64>::with_constraints(
            i64::MIN,
            i64::MAX,
            LayoutConstraints { max_split_count: 0 },
        );

        assert_eq!(layout.count(), 1);
    }
}
