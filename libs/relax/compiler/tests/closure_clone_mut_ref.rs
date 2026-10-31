// A closure is Clone when every capture is (Reference types/closure.md "Other traits"), and its
// captures are what rustc's capture analysis makes of the places it uses (RFC 2229): a closure
// that only reads through a `&mut T` variable captures the place behind it by shared reference
// (`&T`, Clone), never the `&mut T` itself (a Clone shim for `&mut Scene` was refused, Stage
// 10-30). A `move` closure takes the `&mut T` by value and is not Clone (rustc rejects a clone).
#[derive(Debug, PartialEq)]
struct Scene {
    items: Vec<u32>,
    name: String,
}
impl Scene {
    fn count(&self) -> usize {
        self.items.len()
    }
}
fn take(s: &Scene) -> usize {
    s.items.iter().map(|x| *x as usize).sum()
}
fn twice<F: Fn() -> usize + Clone>(f: F) -> (usize, usize) {
    let g = f.clone();
    (f(), g())
}
#[test]
fn reads_through_mut_ref_clone() {
    let mut owned = Scene { items: vec![1, 2, 3], name: "s".into() };
    let scene: &mut Scene = &mut owned;
    // a field read: captures `(*scene).items` by shared reference
    assert_eq!(twice(|| scene.items.len()), (3, 3));
    // a `&self` method: captures `*scene` by shared reference
    assert_eq!(twice(|| scene.count()), (3, 3));
    // a `&mut T` passed where `&T` is expected: a shared reborrow of `*scene`
    assert_eq!(twice(|| take(scene)), (6, 6));
    assert_eq!(twice(|| take(&*scene) + scene.name.len()), (7, 7));
    // nested: the inner closure's capture goes through the outer one's
    assert_eq!(twice(|| twice(|| scene.count()).0), (3, 3));
    // the variable is usable mutably again once the closures are gone
    scene.items.push(4);
    assert_eq!(owned.items, vec![1, 2, 3, 4]);
}
#[test]
fn reads_through_mut_ref_in_struct_clone() {
    struct Ctx<'a> {
        scene: &'a mut Scene,
    }
    let mut owned = Scene { items: vec![5], name: "x".into() };
    let ctx = Ctx { scene: &mut owned };
    // through a field of type `&mut Scene`: captures `(*ctx.scene).items` by shared reference
    assert_eq!(twice(|| ctx.scene.items[0] as usize), (5, 5));
    let opt: Option<usize> = Some(1);
    let cloned = opt.map(|k| k + ctx.scene.count());
    assert_eq!(cloned, Some(2));
}
