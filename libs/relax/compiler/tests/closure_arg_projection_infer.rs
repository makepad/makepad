struct S(u32);
fn offer(out: &mut Vec<S>, s: Option<S>) { if let Some(s) = s { out.push(s) } }
fn offer_reach<T>(out: &mut Vec<S>, ranked: impl IntoIterator<Item = (f64, T)>, tol: f64, mut chip: impl FnMut(T) -> Option<S>) {
    for (d, item) in ranked { if d > tol { break; } offer(out, chip(item)); }
}
fn ranked(found: Vec<(f64, usize, Vec<u32>)>) -> impl Iterator<Item = (f64, (usize, Vec<u32>))> {
    found.into_iter().map(|(d, p, s)| (d, (p, s)))
}
fn chip(scheme: &[u32], label: String) -> Option<S> { Some(S(scheme.len() as u32 + label.len() as u32)) }
fn main() {
    let mut out = Vec::new();
    offer_reach(&mut out, ranked(vec![(1.0, 0, vec![1, 2])]), 2.0, |(_, scheme)| chip(&scheme, String::new()));
    let named: Vec<(f64, (String, Vec<u32>))> = vec![(0.5, ("a".to_string(), vec![3]))];
    offer_reach(&mut out, named, 2.0, |(label, scheme)| chip(&scheme, label));
    assert_eq!(out.len(), 2);
    println!("{}", out.len());
}

#[test]
fn closure_arg_projection_infer() {
    main();
}
