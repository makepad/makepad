// A fn item whose fn returns `impl Trait`, passed as a closure: `<fn item as FnOnce<A>>::Output`
// is the opaque type, and codegen (PostAnalysis) must normalize it to the hidden type, as it
// does for the closure signatures it is related to (Stage's asset store search:
// `groups.iter().flat_map(TermGroup::all).enumerate()` -> E0599, and a segfault through
// `.map(upto).flatten()`), and in type checking `Output == impl Iterator` failed once the
// opaque's hidden type was known.
struct TermGroup {
    exact: Vec<String>,
    expansion: Vec<String>,
}

impl TermGroup {
    fn all(&self) -> impl Iterator<Item = &String> {
        self.exact.iter().chain(self.expansion.iter())
    }
}

fn upto(n: u32) -> impl Iterator<Item = u32> {
    0..n
}

fn sql(groups: &[TermGroup]) -> String {
    let mut sql = String::new();
    for (i, t) in groups.iter().flat_map(TermGroup::all).enumerate() {
        if i > 0 {
            sql.push_str(" UNION ALL ");
        }
        sql.push_str(t);
    }
    sql
}

#[test]
fn flat_map_of_rpit_method() {
    let g = vec![TermGroup { exact: vec!["a".into()], expansion: vec!["b".into()] }, TermGroup { exact: vec!["c".into()], expansion: vec![] }];
    // `all` compiled (its hidden type known) before `sql` is checked
    assert_eq!(g[0].all().count(), 2);
    assert_eq!(sql(&g), "a UNION ALL b UNION ALL c");
}

#[test]
fn map_of_rpit_fn_then_closure() {
    assert_eq!((1..4u32).map(upto).map(|i| i.count()).sum::<usize>(), 6);
    assert_eq!((1..4u32).map(upto).flatten().enumerate().map(|(i, x)| i + x as usize).sum::<usize>(), 19);
}
