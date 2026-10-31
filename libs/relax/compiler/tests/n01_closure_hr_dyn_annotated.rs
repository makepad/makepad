// Closures whose *annotated* parameters mention a higher-ranked trait object
// (`|p: &Rc<dyn Fn(&mut Cx) -> u32>|`, `|p: &Box<dyn for<'a> Fn(&'a u8)>|`,
// with an explicit `'_`). The supplied signature merges with the expected one
// into the closure's one signature: the expected input region (the
// iterator's, free) is the closure's, the annotation is ascribed onto it, and
// the object type's own late-bound region is bound by the object.
use std::rc::Rc;

struct Cx {
    redraws: u32,
}

type RootApply = Rc<dyn Fn(&mut Cx) -> u32>;

fn queue(roots: &Vec<RootApply>, root: &RootApply) -> bool {
    roots.iter().any(|pending: &Rc<dyn Fn(&mut Cx) -> u32>| Rc::ptr_eq(pending, root))
}

fn queue_anon(roots: &Vec<RootApply>, root: &RootApply) -> bool {
    roots.iter().any(|pending: &'_ Rc<dyn Fn(&'_ mut Cx) -> u32>| Rc::ptr_eq(pending, root))
}

fn retain_others(roots: &mut Vec<(u32, RootApply)>, root: &RootApply) {
    roots.retain(|(_, other): &'_ (u32, RootApply)| !Rc::ptr_eq(other, root));
}

fn named<'x>(roots: &'x Vec<RootApply>, root: &RootApply) -> bool {
    roots.iter().any(|p: &'x RootApply| Rc::ptr_eq(p, root))
}

fn any_of<'i, F: FnMut(&'i RootApply) -> bool>(roots: &'i [RootApply], mut f: F) -> bool {
    for r in roots {
        if f(r) {
            return true;
        }
    }
    false
}

fn named_generic<'x>(roots: &'x [RootApply], root: &RootApply) -> bool {
    any_of(roots, |p: &'x Rc<dyn Fn(&mut Cx) -> u32>| Rc::ptr_eq(p, root))
}

fn each<F: FnMut(&RootApply)>(roots: &[RootApply], mut f: F) {
    for r in roots {
        f(r)
    }
}

fn hits(roots: &[RootApply], root: &RootApply) -> u32 {
    let mut hits = 0;
    each(roots, |p: &RootApply| {
        if Rc::ptr_eq(p, root) {
            hits += 1
        }
    });
    each(roots, |p: &'_ RootApply| {
        if Rc::ptr_eq(p, root) {
            hits += 1
        }
    });
    hits
}

fn apply(roots: &[RootApply], cx: &mut Cx) -> u32 {
    let mut total = 0;
    each(roots, |p: &Rc<dyn Fn(&mut Cx) -> u32>| total += p(cx));
    total
}

type Hook = Box<dyn for<'a> Fn(&'a u8) -> u32>;

fn has(hooks: &Vec<Hook>, hook: &Hook) -> bool {
    hooks.iter().any(|p: &Box<dyn for<'a> Fn(&'a u8) -> u32>| std::ptr::eq(p, hook))
}

fn position(hooks: &[Hook], hook: &Hook) -> Option<usize> {
    hooks.iter().position(|p: &Box<dyn Fn(&u8) -> u32>| std::ptr::eq(p, hook))
}

fn find<'v>(hooks: &'v [Hook], hook: &Hook) -> Option<&'v Hook> {
    hooks.iter().find(|p: &&Box<dyn Fn(&u8) -> u32>| std::ptr::eq(*p, hook))
}

fn call_all(hooks: &[Hook], x: &u8) -> u32 {
    let mut n = 0;
    hooks.iter().for_each(|p: &'_ Box<dyn for<'a> Fn(&'a u8) -> u32 + '_>| n += p(x));
    n
}

#[test]
fn annotated_closures_over_higher_ranked_objects() {
    let a: RootApply = Rc::new(|cx: &mut Cx| {
        cx.redraws += 1;
        10
    });
    let b: RootApply = Rc::new(|_cx: &mut Cx| 20);
    let c: RootApply = Rc::new(|_cx: &mut Cx| 30);
    let roots = vec![a.clone(), b.clone()];
    assert!(queue(&roots, &a));
    assert!(!queue(&roots, &c));
    assert!(queue_anon(&roots, &b));
    assert!(!queue_anon(&roots, &c));
    assert!(named(&roots, &b));
    assert!(!named(&roots, &c));
    assert!(named_generic(&roots, &a));
    assert!(!named_generic(&roots, &c));
    assert_eq!(hits(&roots, &a), 2);
    assert_eq!(hits(&roots, &c), 0);
    let mut cx = Cx { redraws: 0 };
    assert_eq!(apply(&roots, &mut cx), 30);
    assert_eq!(cx.redraws, 1);
    let mut pairs = vec![(0, a.clone()), (1, b.clone()), (2, a.clone())];
    retain_others(&mut pairs, &a);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, 1);

    let hooks: Vec<Hook> = vec![Box::new(|x: &u8| *x as u32), Box::new(|x: &u8| *x as u32 * 2)];
    let other: Hook = Box::new(|_x: &u8| 0);
    assert!(has(&hooks, &hooks[1]));
    assert!(!has(&hooks, &other));
    assert_eq!(position(&hooks, &hooks[1]), Some(1));
    assert_eq!(position(&hooks, &other), None);
    assert!(find(&hooks, &hooks[0]).is_some());
    assert!(find(&hooks, &other).is_none());
    assert_eq!(call_all(&hooks, &7), 21);
}
