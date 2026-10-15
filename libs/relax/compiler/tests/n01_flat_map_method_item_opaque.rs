// Stage search.rs:2822 (asset store build_candidate_sql): `flat_map(TermGroup::all).enumerate()` where `all` is a method
// item returning `impl Iterator`. rustc accepts; the tip's old selector failed method dispatch (E0599 enumerate /
// NoSolution panic in tcx/selection.rs). Same family as Score's filter_map/flat_map and the FlatMap/zip instances.
struct TermGroup { exact: Vec<String>, expansion: Vec<String> }
impl TermGroup {
    fn all(&self) -> impl Iterator<Item = &String> { self.exact.iter().chain(self.expansion.iter()) }
}

#[test]
fn flat_map_method_item_returning_opaque_then_enumerate() {
    let groups = vec![TermGroup { exact: vec![String::from("exact")], expansion: vec![String::from("expanded")] }];
    let terms: Vec<(usize, &String)> = groups.iter().flat_map(TermGroup::all).enumerate().collect();
    assert_eq!(terms.len(), 2);
    assert_eq!(terms[1].1, "expanded");
}

#[test]
fn filter_map_and_zip_over_method_item_opaque() {
    let groups = vec![TermGroup { exact: vec![String::from("a")], expansion: vec![] }, TermGroup { exact: vec![], expansion: vec![String::from("b")] }];
    let firsts: Vec<&String> = groups.iter().filter_map(|g| g.all().next()).collect();
    assert_eq!(firsts.len(), 2);
    let pairs: Vec<(&String, usize)> = groups.iter().flat_map(TermGroup::all).zip(0..).collect();
    assert_eq!(pairs[1].1, 1);
}
