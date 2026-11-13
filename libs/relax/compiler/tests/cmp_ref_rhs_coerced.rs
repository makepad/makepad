// relax issue 87: `a < b` with `a: &T` and `b: &&T`. rustc gives the right operand a fresh type,
// the one `PartialOrd<&B> for &A` impl fixes it as `&T`, and `b` deref-coerces to it: the values
// compare. Relax borrowed `b` itself and compared the pointer it holds with the value on the left
// (`&1 < &&0` was true for integers; floats and chars came out right only by chance).
use std::collections::HashMap;

#[inline(never)]
fn lt(a: &usize, b: &&usize) -> bool {
    a < b
}

#[test]
fn a_reference_compares_with_a_reference_to_one_by_value() {
    let (one, zero): (&usize, &usize) = (&1, &0);
    let zero_ref: &&usize = &zero;
    assert_eq!([one < zero_ref, one > zero_ref, one <= zero_ref, one >= zero_ref], [false, true, false, true], "&usize against &&usize");
    let (a, b): (&u32, &u32) = (&1, &0);
    let (x, y): (&i64, &i64) = (&-1, &2);
    let (f, g): (&f64, &f64) = (&1.0, &0.5);
    let (c, d): (&char, &char) = (&'b', &'a');
    assert_eq!([a < &b, x < &y, f < &g, c > &d], [false, true, false, true], "u32, i64, f64 and char");
    let by_value = (0..1000usize).filter(|n| n % 7 < n % 5).count();
    assert_eq!((0..1000usize).filter(|n| lt(&(n % 7), &&(n % 5))).count(), by_value, "a call that is not inlined");
    // HashMap::iter hands the closure `&&usize`, `get` gives `&usize` (the reported shape)
    let mine: HashMap<u32, usize> = [(1, 3), (2, 1), (3, 4)].into_iter().collect();
    let other: HashMap<u32, usize> = [(1, 5), (2, 0), (3, 4)].into_iter().collect();
    let mut above: Vec<u32> = mine.iter().filter(|(k, a)| other.get(*k).is_some_and(|b| b < a)).map(|(k, _)| *k).collect();
    above.sort();
    assert_eq!(above, [2], "only key 2 has a smaller value in `other`");
}
