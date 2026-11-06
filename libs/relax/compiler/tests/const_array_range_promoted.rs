// `CONST[..n]` of a const array borrows the promoted constant (rustc promote_consts: the index's
// `&CONST` is a borrow of a constant), so the sub-slice is 'static and outlives the fn or closure
// that took it. Fab's coral rows test (`GAPS[..1].iter()` returned from a flat_map closure) read
// a dead stack copy and faulted in `format!`.
const PLATES: [(&str, &str); 3] = [("cube", "c-text"), ("bench", "b-text"), ("vase", "v-text")];
const CUBE_TURNED: &str = "t-text";
const GAPS: [&str; 2] = ["2", "0.8"];
const WHERE: [(&str, &str); 2] = [("bed", "1"), ("all", "0")];
const NUMS: [u64; 4] = [10, 20, 30, 40];

#[inline(never)]
fn churn(n: usize) -> usize {
    let big = [n as u8 ^ 0x5a; 16384];
    std::hint::black_box(&big).iter().map(|&b| b as usize).sum()
}

#[inline(never)]
fn first_gaps() -> std::slice::Iter<'static, &'static str> {
    GAPS[..1].iter()
}

#[inline(never)]
fn middle() -> &'static [u64] {
    &NUMS[1..3]
}

#[test]
fn a_range_of_a_const_array_is_static() {
    let it = first_gaps();
    let m = middle();
    churn(3);
    assert_eq!(it.copied().collect::<Vec<_>>(), ["2"]);
    assert_eq!(m, [20, 30]);
}

#[test]
fn rows_from_a_flat_map_over_const_ranges() {
    let plates = PLATES.iter().chain(&[("cube turned", CUBE_TURNED)]);
    let rows = plates.filter(|p| p.0 == "cube" || p.0 == "cube turned").flat_map(|p| GAPS[..1].iter().map(move |g| (p, g))).flat_map(|pg| WHERE[..].iter().map(move |w| (pg, w)));
    let mut out = Vec::new();
    for (((plate, text), gap), (place, bed)) in rows {
        churn(out.len());
        out.push(format!("{plate} {place} at {gap} mm {text} {bed}"));
    }
    assert_eq!(out, ["cube bed at 2 mm c-text 1", "cube all at 2 mm c-text 0", "cube turned bed at 2 mm t-text 1", "cube turned all at 2 mm t-text 0"]);
}

#[test]
fn a_mutable_range_is_a_copy() {
    let mut a = NUMS;
    a[..2].reverse();
    assert_eq!(a, [20, 10, 30, 40]);
    assert_eq!(NUMS, [10, 20, 30, 40]);
}
