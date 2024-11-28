use {super::*, proptest::prelude::*};

prop_compose! {
    fn vec_and_index
        ()
        (vec in prop::collection::vec(0..u8::MAX, 1..100))
        (index in 0..vec.len(), vec in Just(vec))
        -> (Vec<u8>, usize)
    {
        (vec, index)
    }
}

prop_compose! {
    fn vec_and_range
        ()
        ((vec, end) in vec_and_index())
        (start in 0..=end, (vec, end) in Just((vec, end)))
        -> (Vec<u8>, usize, usize)
    {
       (vec, start, end)
    }
}

proptest! {
    #[test]
    fn test_search_by(vec in prop::collection::vec(0u8..=255, 0..256), value: u16) {
        struct SumMetric;

        impl Metric<u8> for SumMetric {
            type Measure = u16;

            fn measure(item: &u8) -> Self::Measure {
                u16::from(*item)
            }
        }

        let btree_vec: BTreeVec<u8, SumMetric> = vec.as_slice().into();
        match btree_vec.search_by(|next_sum| value < next_sum) {
            Some((index, sum)) => {
                assert_eq!(sum, btree_vec.iter().take(index).map(|&item| u16::from(item)).sum::<u16>());
                assert!(value >= sum);
            }
            None => {
                assert!(value >= btree_vec.measure())
            }
        }
    }

    #[test]
    fn test_chunks(vec: Vec<u8>) {
        let btree_vec: BTreeVec<_> = vec.as_slice().into();
        assert_eq!(
            btree_vec.chunks().flat_map(|chunk| chunk.iter().cloned()).collect::<Vec<_>>(),
            vec
        );
    }

    #[test]
    fn test_iter(vec: Vec<u8>) {
        let btree_vec: BTreeVec<_> = vec.as_slice().into();
        assert_eq!(
            btree_vec.iter().cloned().collect::<Vec<_>>(),
            vec.iter().cloned().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn test_iter_rev(vec: Vec<u8>) {
        let btree_vec: BTreeVec<_> = vec.as_slice().into();
        assert_eq!(
            btree_vec.iter_rev().cloned().collect::<Vec<_>>(),
            vec.iter().rev().cloned().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn test_push_front(vec: Vec<u8>, item: u8) {
        let mut btree_vec: BTreeVec<_> = vec.as_slice().into();
        btree_vec.push_front(item);
        btree_vec.assert_valid();
        let mut vec = vec;
        vec.insert(0, item);
        assert_eq!(btree_vec.to_vec(), vec)
    }

    #[test]
    fn test_push_back(vec: Vec<u8>, item: u8) {
        let mut btree_vec: BTreeVec<_> = vec.as_slice().into();
        btree_vec.push_front(item);
        btree_vec.assert_valid();
        let mut vec = vec;
        vec.insert(0, item);
        assert_eq!(btree_vec.to_vec(), vec)
    }

    #[test]
    fn test_insert((vec, index) in vec_and_index(), item: u8) {
        let mut btree_vec: BTreeVec<_> = vec.as_slice().into();
        btree_vec.insert(index, item);
        btree_vec.assert_valid();
        let mut vec = vec;
        vec.insert(index, item);
        assert_eq!(btree_vec.to_vec(), vec)
    }

    #[test]
    fn test_prepend(vec_0: Vec<u8>, vec_1: Vec<u8>) {
        let btree_vec_0: BTreeVec<_> = vec_0.as_slice().into();
        let btree_vec_1: BTreeVec<_>  = vec_1.as_slice().into();
        let mut btree_vec = btree_vec_0.clone();
        btree_vec.prepend(btree_vec_1);
        btree_vec.assert_valid();
        let mut vec = vec_1.clone();
        vec.append(&mut vec_0.clone());
        assert_eq!(btree_vec.to_vec(), vec)
    }

    #[test]
    fn test_append(vec_0: Vec<u8>, vec_1: Vec<u8>) {
        let btree_vec_0: BTreeVec<_> = vec_0.as_slice().into();
        let btree_vec_1: BTreeVec<_>  = vec_1.as_slice().into();
        let mut btree_vec = btree_vec_0.clone();
        btree_vec.append(btree_vec_1);
        btree_vec.assert_valid();
        let mut vec = vec_0.clone();
        vec.append(&mut vec_1.clone());
        assert_eq!(btree_vec.to_vec(), vec)
    }

    #[test]
    fn test_pop_front(mut vec: Vec<u8>) {
        let mut btree_vec: BTreeVec<_> = vec.as_slice().into();
        let btree_item = btree_vec.pop_front();
        let vec_item = vec.drain(0..1.min(vec.len())).next();
        assert_eq!(btree_vec.to_vec(), vec);
        assert_eq!(btree_item, vec_item);
    }

    #[test]
    fn test_pop_back(mut vec: Vec<u8>) {
        let mut btree_vec: BTreeVec<_> = vec.as_slice().into();
        let btree_item = btree_vec.pop_back();
        let vec_item = vec.pop();
        assert_eq!(btree_vec.to_vec(), vec);
        assert_eq!(btree_item, vec_item);
    }

    #[test]
    fn test_remove((mut vec, index) in vec_and_index()) {
        let mut btree_vec: BTreeVec<_> = vec.as_slice().into();
        let btree_item = btree_vec.remove(index);
        let vec_item = vec.remove(index);
        assert_eq!(btree_vec.to_vec(), vec);
        assert_eq!(btree_item, vec_item);
    }

    #[test]
    fn test_remove_from((mut vec, start) in vec_and_index()) {
        let mut btree_vec: BTreeVec<_> = vec.as_slice().into();
        btree_vec.remove_from(start);
        vec.truncate(start);
        assert_eq!(btree_vec.to_vec(), vec);
    }

    #[test]
    fn test_remove_to((mut vec, end) in vec_and_index()) {
        let mut btree_vec: BTreeVec<_> = vec.as_slice().into();
        btree_vec.remove_to(end);
        vec.drain(..end);
        assert_eq!(btree_vec.to_vec(), vec);
    }

    #[test]
    fn test_replace_range((vec_0, start, end) in vec_and_range(), vec_1: Vec<u8>) {
        let btree_vec_0: BTreeVec<_> = vec_0.as_slice().into();
        let btree_vec_1: BTreeVec<_> = vec_1.as_slice().into();
        let mut btree_vec = btree_vec_0.clone();
        btree_vec.replace_range(start, end, btree_vec_1);
        btree_vec.assert_valid();
        let mut vec = vec_0.clone();
        vec.splice(start..end, vec_1.iter().copied());
        assert_eq!(btree_vec.to_vec(), vec)
    }

    #[test]
    fn test_split_off((mut vec, index) in vec_and_index()) {
        let mut btree_vec: BTreeVec<_> = vec.as_slice().into();
        let other_btree_vec = btree_vec.split_off(index);
        let other_vec = vec.split_off(index);
        assert_eq!(btree_vec.to_vec(), vec);
        assert_eq!(other_btree_vec.to_vec(), other_vec);
    }
}
