// rustc_const_eval/src/check_consts/qualifs.rs (Qualif::in_rvalue): NeedsDrop and HasMutInterior are
// judged on the value an rvalue builds, not on every value of its type. An aggregate has the qualif
// when its ADT has it inherently (its own `Drop` impl; `UnsafeCell`) or when an operand has it. So
// `&Phase::Choose` is promoted although another variant of `Phase` owns a `Box`: once returned inside
// a struct it pointed at the helper's dead stack slot.
use std::sync::atomic::{AtomicUsize, Ordering};

#[allow(dead_code)]
#[derive(Debug, PartialEq)]
enum Phase {
    Loading,
    Choose,
    Review(Box<u64>),
    Failed(String),
    Big([u64; 48]),
}

struct Holder<'a> {
    phase: &'a Phase,
    n: u32,
}

#[derive(Debug, PartialEq)]
struct Pair {
    a: Option<Vec<u8>>,
    b: u32,
}

#[inline(never)]
fn holder<'a>() -> Holder<'a> {
    Holder { phase: &Phase::Choose, n: 7 }
}
#[inline(never)]
fn unit() -> &'static Phase {
    &Phase::Loading
}
#[inline(never)]
fn none() -> &'static Option<String> {
    &None
}
#[inline(never)]
fn nested() -> &'static Option<Phase> {
    &Some(Phase::Choose)
}
#[inline(never)]
fn braced() -> &'static Pair {
    &Pair { a: None, b: 9 }
}
#[inline(never)]
fn tuple() -> &'static (Option<Box<u8>>, u8) {
    &(None, 3)
}
#[inline(never)]
fn array() -> &'static [Option<String>; 2] {
    &[None, None]
}
// Interior mutability is a property of the value too: `None` holds no `Cell`.
#[inline(never)]
fn cell() -> &'static Option<std::cell::Cell<u8>> {
    &None
}
#[inline(never)]
fn overwrite_stack() -> usize {
    let values = [std::hint::black_box(usize::MAX); 256];
    std::hint::black_box(values)[63]
}

#[test]
fn a_value_without_a_destructor_is_promoted_whatever_its_type_owns() {
    let held = holder();
    let (unit, none, nested, braced, tuple, array, cell) = (unit(), none(), nested(), braced(), tuple(), array(), cell());
    assert_eq!(overwrite_stack(), usize::MAX);
    assert_eq!(*held.phase, Phase::Choose);
    assert_eq!(held.n, 7);
    assert_eq!(*unit, Phase::Loading);
    assert_eq!(*none, None);
    assert_eq!(*nested, Some(Phase::Choose));
    assert_eq!(*braced, Pair { a: None, b: 9 });
    assert_eq!(*tuple, (None, 3));
    assert_eq!(*array, [None, None]);
    assert!(cell.is_none());
}

static DROPS: AtomicUsize = AtomicUsize::new(0);

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
}
struct Noisy {
    #[allow(dead_code)]
    n: u8,
}
impl Drop for Noisy {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
}
#[allow(dead_code)]
enum WithGuard {
    Empty,
    Full(Guard),
}

// A value that does hold a destructor is a temporary as before: it is dropped where its scope ends.
#[test]
fn a_value_with_a_destructor_is_not_promoted() {
    let drops = || DROPS.load(Ordering::Relaxed);
    {
        let _guard = &Guard;
        assert_eq!(drops(), 0);
    }
    assert_eq!(drops(), 1);
    {
        let _noisy = &Noisy { n: 1 };
    }
    assert_eq!(drops(), 2);
    {
        let _empty = &WithGuard::Empty;
    }
    assert_eq!(drops(), 2);
    {
        let _full = &WithGuard::Full(Guard);
        assert_eq!(drops(), 2);
    }
    assert_eq!(drops(), 3);
}
