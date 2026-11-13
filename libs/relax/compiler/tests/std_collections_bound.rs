// relax issue 118: `std::collections::Bound`, std's old path of `std::ops::Bound` (deprecated since
// 1.26, still exported), did not resolve: E0432 (datasketches imports it this way).
#[allow(deprecated)]
use std::collections::Bound;
use std::collections::BTreeMap;
use std::ops::RangeBounds;

#[test]
fn the_old_bound_path_names_ops_bound() {
    let b: Bound<u32> = Bound::Included(3);
    assert_eq!(format!("{b:?}"), "Included(3)");
    let same: std::ops::Bound<u32> = b;
    assert_eq!(same, std::ops::Bound::Included(3));
    let r = (Bound::Excluded(1), Bound::Unbounded);
    assert!(!r.contains(&1) && r.contains(&2) && r.contains(&u32::MAX));
    let m: BTreeMap<u32, char> = [(1, 'a'), (2, 'b'), (3, 'c')].into_iter().collect();
    let got: Vec<char> = m.range((Bound::Excluded(1), Bound::Included(3))).map(|(_, c)| *c).collect();
    assert_eq!(got, ['b', 'c']);
}
