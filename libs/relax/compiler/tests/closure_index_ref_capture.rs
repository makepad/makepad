// An inner closure that indexes with the outer closure's `&` parameter takes it reborrowed: the
// index operand of an overloaded index is coerced to the impl's index type, so `files[name]` passes
// `&*name` and the closure captures `*name`, which outlives the outer call, not the local `name`
// (relax issue 77: a false E0373 in check, and a run that read the dead local and faulted).
use std::collections::BTreeMap;
use std::ops::Index;

struct Tables {
    roles: BTreeMap<String, Vec<usize>>,
    files: BTreeMap<String, Vec<u32>>,
}

fn picked<'a>(t: &'a Tables) -> Vec<&'a u32> {
    t.roles.iter().flat_map(|(name, columns)| columns.iter().filter_map(|c| t.files[name].get(*c))).collect()
}

struct Named(Vec<(String, u32)>);

impl Index<&str> for Named {
    type Output = u32;
    fn index(&self, key: &str) -> &u32 {
        &self.0.iter().find(|(k, _)| k == key).unwrap().1
    }
}

fn scaled<'a>(n: &'a Named, keys: &'a [(&'a str, Vec<u32>)]) -> Vec<u32> {
    keys.iter().flat_map(|(key, factors)| factors.iter().map(|f| n[key] * f)).collect()
}

#[test]
fn an_inner_closure_indexes_with_the_outer_closures_reference() {
    let t = Tables {
        roles: BTreeMap::from([("a".to_string(), vec![0, 2]), ("b".to_string(), vec![1, 0])]),
        files: BTreeMap::from([("a".to_string(), vec![10, 20]), ("b".to_string(), vec![30, 40])]),
    };
    assert_eq!(picked(&t), [&10, &40, &30]);
    let n = Named(vec![("x".to_string(), 3), ("y".to_string(), 5)]);
    let keys = [("y", vec![1, 2]), ("x", vec![10])];
    assert_eq!(scaled(&n, &keys), [5, 10, 30]);
}
