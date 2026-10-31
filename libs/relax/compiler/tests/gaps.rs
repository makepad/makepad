// cheap gaps (coord.md): impl candidates filtered by trait args, literals through comparisons,
// dyn supertrait impls, other traits' methods on dyn through impls, trait paths select impls
#[test]
fn g1_sum_ref_items() {
    let v = vec![1u32, 2, 3];
    let s: u32 = v.iter().sum();
    assert_eq!(s, 6);
    let s2 = v.iter().sum::<u32>();
    assert_eq!(s2, 6);
}

#[test]
fn g2_sum_literal_range() {
    let s: u64 = (1..=10).sum();
    assert_eq!(s, 55);
}

#[test]
fn g3_collect_option_literal() {
    let o: Option<Vec<u32>> = (1..4).map(|x| Some(x)).collect();
    assert_eq!(o, Some(vec![1, 2, 3]));
}

#[test]
fn g4_vec_eq_array_literal() {
    let v: Vec<u16> = vec![5, 6];
    assert!(v == [5, 6]);
}

#[test]
fn g5_box_dyn_error_debug() {
    let e: Box<dyn std::error::Error + Send + Sync> = Box::from("bad");
    let d: &dyn std::fmt::Debug = &e;
    assert!(format!("{:?}", d).contains("bad"));
}

#[test]
fn g6_dyn_error_to_string() {
    let e: Box<dyn std::error::Error> = Box::from("oops");
    let r: &dyn std::error::Error = &*e;
    assert_eq!(r.to_string(), "oops");
}

#[test]
fn g7_atomic_from() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let a = AtomicUsize::from(5);
    assert_eq!(a.load(Ordering::Relaxed), 5);
}
#[test]
fn g8_u32_times_duration() {
    use std::time::Duration;
    let d = 3u32 * Duration::from_millis(10);
    assert_eq!(d, Duration::from_millis(30));
    let d = 2 * Duration::from_secs(1);
    assert_eq!(d, Duration::from_secs(2));
    let n: u32 = 4;
    let d = n * Duration::from_millis(5) + Duration::from_millis(1);
    assert_eq!(d.as_millis(), 21);
}
