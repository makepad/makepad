use {super::*, proptest::prelude::*, std::ops::Range};

fn arbitrary_range_vec() -> impl Strategy<Value = Vec<Range<u16>>> {
    prop::collection::vec(
        (1..256u16)
            .prop_flat_map(|len| (Just(len), ..u16::MAX - len))
            .prop_map(|(len, start)| start..start + len),
        ..16,
    )
}

proptest! {
    #[test]
    fn difference(ranges_0 in arbitrary_range_vec(), ranges_1 in arbitrary_range_vec()) {
        let range_set_0: RangeSet<_> = ranges_0.iter().cloned().collect();
        let range_set_1: RangeSet<_> = ranges_1.iter().cloned().collect();
        let range_set: RangeSet<_> = range_set_0.difference(&range_set_1).collect();
        assert!(range_set.iter().flat_map(|range| range.clone()).all(|value| {
            let range = value..value + 1;
            range_set_0.contains(&range) && !range_set_1.contains(&range)
        }));
        assert!(range_set_0.iter().flat_map(|range| range.clone()).all(|value| {
            let range = value..value + 1;
            range_set.contains(&range) || range_set_1.contains(&range)
        }));
        assert!(range_set_1.iter().flat_map(|range| range.clone()).all(|value| {
            let range = value..value + 1;
            !range_set.contains(&range)
        }));
    }

    #[test]
    fn intersection(ranges_0 in arbitrary_range_vec(), ranges_1 in arbitrary_range_vec()) {
        let range_set_0: RangeSet<_> = ranges_0.iter().cloned().collect();
        let range_set_1: RangeSet<_> = ranges_1.iter().cloned().collect();
        let range_set: RangeSet<_> = range_set_0.intersection(&range_set_1).collect();
        assert!(range_set.iter().all(|range| {
            range_set_0.contains(range) && range_set_1.contains(range)
        }));
        assert!(range_set_0.iter().flat_map(|range| range.clone()).all(|value| {
            let range = value..value + 1;
            range_set.contains(&range) || !range_set_1.contains(&range)
        }));
        assert!(range_set_1.iter().flat_map(|range| range.clone()).all(|value| {
            let range = value..value + 1;
            range_set.contains(&range) || !range_set_0.contains(&range)
        }));
    }

    #[test]
    fn symmetric_difference(ranges_0 in arbitrary_range_vec(), ranges_1 in arbitrary_range_vec()) {
        let range_set_0: RangeSet<_> = ranges_0.iter().cloned().collect();
        let range_set_1: RangeSet<_> = ranges_1.iter().cloned().collect();
        let range_set: RangeSet<_> = range_set_0.symmetric_difference(&range_set_1).collect();
        assert!(range_set.iter().flat_map(|range| range.clone()).all(|value| {
            let range = value..value + 1;
            range_set_0.contains(&range) != range_set_1.contains(&range)
        }));
        assert!(range_set_0.iter().flat_map(|range| range.clone()).all(|value| {
            let range = value..value + 1;
            range_set.contains(&range) || range_set_1.contains(&range)
        }));
        assert!(range_set_1.iter().flat_map(|range| range.clone()).all(|value| {
            let range = value..value + 1;
            range_set.contains(&range) || range_set_0.contains(&range)
        }));
    }

    #[test]
    fn union(ranges_0 in arbitrary_range_vec(), ranges_1 in arbitrary_range_vec()) {
        let range_set_0: RangeSet<_> = ranges_0.iter().cloned().collect();
        let range_set_1: RangeSet<_> = ranges_1.iter().cloned().collect();
        let range_set: RangeSet<_> = range_set_0.union(&range_set_1).collect();
        assert!(range_set.iter().flat_map(|range| range.clone()).all(|value| {
            let range = value..value + 1;
            range_set_0.contains(&range) || range_set_1.contains(&range)
        }));
        assert!(range_set_0.iter().all(|range| range_set.contains(range)));
        assert!(range_set_1.iter().all(|range| range_set.contains(range)));
    }

    #[test]
    fn insert(ranges in arbitrary_range_vec()) {
        let mut range_set = RangeSet::new();
        for range in &ranges {
            range_set.insert(range.clone());
        }
        assert!(ranges.iter().all(|range| range_set.contains(range)));
    }
}
