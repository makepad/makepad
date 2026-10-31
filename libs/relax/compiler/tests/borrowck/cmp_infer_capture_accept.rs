// Scope code_graph prepared_graph/builder.rs:1145 (resolve_type): `out`'s element type is still a type
// variable when `*p == path` is checked, so the comparison is `<?T as PartialEq<Vec<String>>>::eq(&*p, &path)`
// (rustc op.rs:41, check_overloaded_binop) and the inner closure borrows `path`; rustc accepts. Typing it as a
// built-in comparison made the capture consume `path`: a false E0382 under MAKEPAD_RUN_BORROWCK_ALL.
fn dedup(items: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut push = |path: Vec<String>| {
        if path.is_empty() || out.len() >= 8 { return; }
        if !out.iter().any(|p| *p == path) { out.push(path); }
    };
    for i in items { push(i); }
    out
}

fn small(xs: Vec<u32>) -> usize {
    // the same shape over a scalar: after inference the comparison is built in again (writeback)
    let mut seen = Vec::new();
    for x in xs { if !seen.iter().any(|s| *s == x) { seen.push(x); } }
    seen.len()
}

fn main() {
    let v = dedup(vec![vec!["a".into()], vec!["a".into()], vec!["b".into()]]);
    assert_eq!(v.len(), 2);
    assert_eq!(small(vec![1, 2, 1, 3]), 3);
    println!("cmp_infer_capture ok");
}
