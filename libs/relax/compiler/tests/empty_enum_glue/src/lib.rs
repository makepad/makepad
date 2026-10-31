use std::sync::Arc;
pub enum DevU32 {}
pub enum Rows { Host(Vec<usize>), Dev(Arc<DevU32>, usize) }
fn count(r: &Rows) -> usize { match r { Rows::Host(v) => v.len(), Rows::Dev(_, n) => *n } }
#[cfg_attr(test, test)]
pub
fn arc_of_empty_enum_drop_glue() {
    let r = Rows::Host(vec![1, 2, 3]);
    assert_eq!(count(&r), 3);
    drop(r);
    let v: Vec<Rows> = vec![Rows::Host(vec![]), Rows::Host(vec![4])];
    assert_eq!(v.iter().map(count).sum::<usize>(), 1);
    let o: Option<Arc<DevU32>> = None;
    drop(o);
}
