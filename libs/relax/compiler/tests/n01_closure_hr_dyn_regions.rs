// Closures whose arguments mention a higher-ranked trait object
// (`Rc<dyn Fn(&mut Cx) -> R>`, i.e. `dyn for<'a> Fn(&'a mut Cx) -> R`)
// borrow-check under their typeck root; the late-bound region of the object
// type is bound by the object, not a free region of the closure
// (makepad draw typed_styles::flush / queue_style_roots).
use std::rc::Rc;

struct Cx {
    redraws: u32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum RootResult {
    Applied,
    Busy,
    Expired,
}

type RootApply = Rc<dyn Fn(&mut Cx) -> RootResult>;

#[derive(Default)]
struct StyleRoots {
    roots: Vec<(u32, RootApply)>,
    pending: Vec<RootApply>,
}

fn flush(cx: &mut Cx, roots: &mut StyleRoots) {
    let pending = std::mem::take(&mut roots.pending);
    for root in pending {
        match root(cx) {
            RootResult::Applied => (),
            RootResult::Busy => {
                let pending = &mut roots.pending;
                if !pending.iter().any(|other| Rc::ptr_eq(other, &root)) {
                    pending.push(root);
                }
            }
            RootResult::Expired => roots.roots.retain(|(_, other)| !Rc::ptr_eq(other, &root)),
        }
    }
    if !roots.pending.is_empty() {
        cx.redraws += 1;
    }
}

fn queue_style_roots(cx: &mut Cx, roots: &mut StyleRoots) {
    for (_, root) in &roots.roots {
        if !roots.pending.iter().any(|pending| Rc::ptr_eq(pending, root)) {
            roots.pending.push(root.clone());
        }
    }
    cx.redraws += 1;
}

#[test]
fn closures_over_higher_ranked_objects() {
    let applied: RootApply = Rc::new(|cx: &mut Cx| {
        cx.redraws += 10;
        RootResult::Applied
    });
    let busy: RootApply = Rc::new(|_cx: &mut Cx| RootResult::Busy);
    let expired: RootApply = Rc::new(|_cx: &mut Cx| RootResult::Expired);
    let mut roots = StyleRoots::default();
    roots.roots.push((0, applied.clone()));
    roots.roots.push((1, busy.clone()));
    roots.roots.push((2, expired.clone()));
    let mut cx = Cx { redraws: 0 };
    queue_style_roots(&mut cx, &mut roots);
    queue_style_roots(&mut cx, &mut roots);
    assert_eq!(roots.pending.len(), 3);
    flush(&mut cx, &mut roots);
    assert_eq!(cx.redraws, 2 + 10 + 1);
    assert_eq!(roots.pending.len(), 1);
    assert!(Rc::ptr_eq(&roots.pending[0], &busy));
    assert_eq!(roots.roots.len(), 2);
    assert_eq!((roots.pending[0])(&mut cx), RootResult::Busy);
}
