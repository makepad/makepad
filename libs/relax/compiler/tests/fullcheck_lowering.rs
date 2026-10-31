// Valid Rust regressions for machine-code lowering: unsized pattern places and
// comparisons that must dispatch through PartialOrd with borrowed operands.

struct Context<'a> {
    params: &'a [f32],
}

fn parameters(cx: &Context<'_>) -> (f32, f32, f32, f32, f32) {
    let [cutoff, res, env_mod, _, accent, slide, ..] = *cx.params else {
        unreachable!()
    };
    (cutoff, res, env_mod, accent, slide)
}

fn ends_let(s: &[i32]) -> Option<(i32, usize, i32)> {
    let [first, ref middle @ .., last] = *s else {
        return None;
    };
    Some((first, middle.len(), last))
}

fn ends_match(s: &[i32]) -> Option<(i32, usize, i32)> {
    match *s {
        [first, ref middle @ .., last] => Some((first, middle.len(), last)),
        _ => None,
    }
}

fn ends_if(s: &[i32]) -> Option<(i32, usize, i32)> {
    if let [first, ref middle @ .., last] = *s {
        Some((first, middle.len(), last))
    } else {
        None
    }
}

#[test]
fn dereferenced_slice_patterns() {
    assert_eq!(parameters(&Context { params: &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0] }), (1.0, 2.0, 3.0, 5.0, 6.0));
    assert_eq!(parameters(&Context { params: &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0] }), (1.0, 2.0, 3.0, 5.0, 6.0));
    for s in [&[][..], &[10][..], &[10, 20][..], &[10, 20, 30, 40][..]] {
        let expected = if s.len() < 2 { None } else { Some((s[0], s.len() - 2, s[s.len() - 1])) };
        assert_eq!(ends_let(s), expected);
        assert_eq!(ends_match(s), expected);
        assert_eq!(ends_if(s), expected);
    }
}

fn exact_pair(s: &[i32]) -> Option<i32> {
    let [a, b] = *s else { return None };
    Some(a + b)
}

fn change_ends(s: &mut [i32]) -> bool {
    let [ref mut first, ref mut middle @ .., ref mut last] = *s else { return false };
    *first += 1;
    *last += 2;
    if !middle.is_empty() {
        middle[0] += 3;
    }
    true
}

#[test]
fn slice_exact_length_and_mutable_bindings() {
    assert_eq!(exact_pair(&[]), None);
    assert_eq!(exact_pair(&[1]), None);
    assert_eq!(exact_pair(&[1, 2]), Some(3));
    assert_eq!(exact_pair(&[1, 2, 3]), None);
    let mut a = [10, 20, 30];
    assert!(change_ends(&mut a));
    assert_eq!(a, [11, 23, 32]);
    assert!(!change_ends(&mut a[..1]));
    assert_eq!(a, [11, 23, 32]);
}

#[derive(PartialEq, PartialOrd)]
struct VertexId(u32);

// Deliberately not Copy: ordering borrows rather than consumes either operand.
#[derive(PartialEq, PartialOrd)]
struct EdgeKey(VertexId, VertexId);

fn edge(a: u32, b: u32) -> EdgeKey {
    EdgeKey(VertexId(a), VertexId(b))
}

#[test]
fn derived_ordering_borrows_operands() {
    let a = edge(1, 9);
    let b = edge(2, 0);
    assert!(a < b);
    assert!(a <= b);
    assert!(b > a);
    assert!(b >= a);
    assert!(a <= a);
    assert!(a >= a);
    assert!(!(a < a));
    assert!(!(a > a));
    assert!(&a < &b);
    assert!(&&b >= &&a);
    assert!(a.0.0 == 1 && b.0.0 == 2);
}

#[test]
fn aggregate_and_slice_ordering() {
    let a = (edge(1, 2), 9);
    let b = (edge(1, 3), 0);
    assert!(a < b && a <= b && b > a && b >= a);
    assert!(&a < &b);
    let a = [edge(1, 2), edge(3, 4)];
    let b = [edge(1, 2), edge(3, 5)];
    assert!(a < b && a <= b && b > a && b >= a);
    assert!(&a < &b);
    let sa = &a[..];
    let sb = &b[..];
    assert!(sa < sb && sa <= sb && sb > sa && sb >= sa);
    assert!(*sa < *sb && *sa <= *sb && *sb > *sa && *sb >= *sa);
    assert!(sa[..1] < sa[..]);
    assert!(&&sa < &&sb);
    // Arrays are not restricted to the former primitive fallback's 64 elements.
    let a = [1u32; 65];
    let mut b = a;
    b[64] = 2;
    assert!(a < b);
}

#[test]
fn ordering_inside_closures() {
    let key = edge(2, 3);
    assert!(Some(edge(2, 4)).is_some_and(|p| p >= key));
    assert!(!Some(edge(1, 9)).is_some_and(|p| p >= key));
    assert!(!None::<EdgeKey>.is_some_and(|p| p >= key));
    let compare = |p: EdgeKey| (p < key, p <= key, p > key, p >= key);
    assert_eq!(compare(edge(2, 3)), (false, true, false, true));
    assert_eq!(compare(edge(2, 2)), (true, true, false, false));
    assert_eq!(compare(edge(3, 0)), (false, false, true, true));
    assert!(Some((edge(2, 4), 0)).is_some_and(|p| p > (edge(2, 3), 9)));
    assert!(key.1.0 == 3);
}

#[derive(PartialEq, PartialOrd)]
struct FloatKey(f32);

#[test]
fn unordered_values_stay_unordered() {
    let a = FloatKey(f32::NAN);
    let b = FloatKey(1.0);
    assert!(!(a < b) && !(a <= b) && !(a > b) && !(a >= b));
    let a = (a, 0);
    let b = (b, 1);
    assert!(!(a < b) && !(a <= b) && !(a > b) && !(a >= b));
}

#[derive(PartialEq)]
struct ReverseKey(i32);

impl PartialOrd for ReverseKey {
    fn partial_cmp(&self, _other: &Self) -> Option<std::cmp::Ordering> {
        // Operators must call the overridden methods below.
        panic!("ordering operator bypassed its trait method")
    }

    fn lt(&self, other: &Self) -> bool { self.0 > other.0 }
    fn le(&self, other: &Self) -> bool { self.0 >= other.0 }
    fn gt(&self, other: &Self) -> bool { self.0 < other.0 }
    fn ge(&self, other: &Self) -> bool { self.0 <= other.0 }
}

#[test]
fn custom_ordering_methods() {
    let a = ReverseKey(2);
    let b = ReverseKey(1);
    assert!(a < b && a <= b && b > a && b >= a);
}
