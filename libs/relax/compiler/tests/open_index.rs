// An index whose type is still an inference variable when the indexing is checked
// (`tree[p]`, p from an Option assigned later in the loop) is fixed by the type's single Index
// impl, not assumed usize (rustc lookup_indexing: `T: Index<?I>` with one candidate). Before:
// "cannot index `&mut Tree`" -- pulldown-cmark firstpass.rs:2262 (splash example, Vector tab).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Ix(usize);
struct Tree { v: Vec<u32> }
impl std::ops::Index<Ix> for Tree { type Output = u32; fn index(&self, i: Ix) -> &u32 { &self.v[i.0] } }
impl std::ops::IndexMut<Ix> for Tree { fn index_mut(&mut self, i: Ix) -> &mut u32 { &mut self.v[i.0] } }
fn f(tree: &mut Tree) -> u32 {
    let mut prev = None;
    let mut sum = 0;
    for k in 0..3 {
        if let Some(p) = prev {
            tree[p] += 1;
            sum += tree[p];
        }
        prev = Some(Ix(k));
    }
    sum
}
#[test] fn open_index_selects_impl() { let mut t = Tree { v: vec![1, 2, 3] }; assert_eq!(f(&mut t), 5); assert_eq!(t.v, vec![2, 3, 3]); }
fn g(v: &mut Vec<u32>) -> u32 {
    let mut last = None;
    for k in 0..v.len() {
        if let Some(i) = last {
            v[i] += 10;
        }
        last = Some(k);
    }
    v.iter().sum()
}
#[test] fn open_index_on_vec() { let mut v = vec![1, 2, 3]; assert_eq!(g(&mut v), 26); }
