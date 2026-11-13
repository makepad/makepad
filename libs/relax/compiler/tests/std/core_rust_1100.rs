// std-core differential test (core_diff.sh, agree.sh): what rustc 1.100.0 stabilized (relax issue 156). The `!`
// type with its impls, layouts and the never-type fallback of a diverging closure, and the library items
// bool::toggle, mem::DropGuard, mem::conjure_zst, trim_prefix/trim_suffix (str, slice, Path), UnsafeCell's
// access methods, Box::take, Box/Rc/Arc::map, Once's Default, Vec::from_fn and Vec/slice == Cow<[U]>.
use std::borrow::Cow;
use std::cell::{RefCell, UnsafeCell};
use std::convert::Infallible;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::mem::{self, DropGuard};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Once};

trait Which {
    fn which() -> &'static str;
}
impl Which for () {
    fn which() -> &'static str {
        "()"
    }
}
impl Which for ! {
    fn which() -> &'static str {
        "!"
    }
}

fn returned_by<R: Which>(_f: impl FnOnce() -> R) -> &'static str {
    R::which()
}

fn never_bounds<T>() -> bool
where
    T: Clone + Copy + PartialEq + Eq + PartialOrd + Ord + Hash + Debug + Display + std::error::Error,
    T: Send + Sync + 'static + AsRef<str> + AsMut<[u8]> + std::ops::Not<Output = !>,
    std::num::TryFromIntError: From<T>,
{
    true
}

fn count(s: &str) -> Result<usize, !> {
    Ok(s.len())
}

struct Loud(u32);
impl Drop for Loud {
    fn drop(&mut self) {
        println!("| drop {}", self.0);
    }
}

#[derive(Debug)]
struct Unit;

#[test]
fn a01_never_type() {
    // a closure whose body only diverges returns `!` (every edition since 1.100.0)
    println!("| fallback {} unit {}", returned_by(|| panic!("not called")), returned_by(|| ()));
    println!("| bounds {}", never_bounds::<!>());
    let Ok(n) = count("abc");
    println!("| irrefutable {}", n);
    println!("| alias {}", std::any::TypeId::of::<Infallible>() == std::any::TypeId::of::<!>());
    println!("| names {} {}", std::any::type_name::<Infallible>(), std::any::type_name::<fn() -> !>());
    println!("| sizes {} {} {}", mem::size_of::<!>(), mem::size_of::<Option<!>>(), mem::size_of::<Result<u32, !>>());
    let widened: Result<u32, !> = u32::try_from(5u8);
    println!("| try_from {:?}", widened);
    let r: Result<u8, std::num::TryFromIntError> = Ok::<u8, !>(3).map_err(Into::into);
    println!("| from never {:?}", r);
}

#[test]
fn a02_bool_toggle() {
    const FLIPPED: bool = {
        let mut b = true;
        b.toggle();
        b
    };
    let mut b = false;
    b.toggle();
    print!("| toggle {}", b);
    b.toggle();
    println!(" {} const {}", b, FLIPPED);
}

#[test]
fn a03_drop_guard() {
    let log = RefCell::new(Vec::new());
    {
        let mut g = DropGuard::new(1u32, |v| log.borrow_mut().push(format!("ran on {}", v)));
        *g += 1;
        println!("| deref {} debug {:?}", *g, g);
    }
    println!("| log {:?}", log.borrow());
    let g = DropGuard::new(String::from("kept"), |s| log.borrow_mut().push(s));
    let s = DropGuard::dismiss(g);
    println!("| dismissed {} log {}", s, log.borrow().len());
}

#[test]
fn a04_conjure_zst() {
    let u: Unit = unsafe { mem::conjure_zst() };
    let t: () = unsafe { mem::conjure_zst::<()>() };
    println!("| zst {:?} {:?}", u, t);
    let caught = std::panic::catch_unwind(|| unsafe { mem::conjure_zst::<u16>() });
    let msg = caught.unwrap_err();
    println!("| not zst {:?}", msg.downcast_ref::<String>());
}

#[test]
fn a05_trim() {
    println!("| str {} {} {}", "lib.rs".trim_suffix(".rs"), "lib".trim_suffix(".rs"), "xxa".trim_prefix('x'));
    println!("| str fn {}", "123abc".trim_prefix(|c: char| c.is_ascii_digit()));
    let v = [1, 2, 3, 4];
    println!("| slice {:?} {:?} {:?} {:?}", v.trim_prefix(&[1, 2]), v.trim_prefix(&[9]), v.trim_suffix(&[3, 4]), v.trim_suffix(&[]));
    let p = Path::new("/usr/lib/x");
    println!("| path {} {}", p.trim_prefix("/usr").display(), p.trim_prefix("/opt").display());
}

#[test]
fn a06_unsafe_cell() {
    const SUM: u32 = {
        let c = UnsafeCell::new(3u32);
        unsafe { c.replace(4) + *c.as_ref_unchecked() }
    };
    let c = UnsafeCell::new(5u32);
    unsafe {
        let old = c.replace(7);
        print!("| cell {} {}", old, *c.as_ref_unchecked());
        *c.as_mut_unchecked() += 1;
        println!(" {} const {}", *c.as_ref_unchecked(), SUM);
    }
}

#[test]
fn a07_map_and_take() {
    let b = Box::map(Box::new(5u32), |x| x as f32 * 1.5);
    let wide = Box::map(Box::new(1u8), |x| [x; 3]);
    println!("| box {} {:?}", b, wide);
    let (v, uninit) = Box::take(Box::new(String::from("t")));
    println!("| take {}", Box::write(uninit, v + "!"));
    let shared = Rc::new(String::from("shared"));
    let keep = shared.clone();
    let len = Rc::map(shared, |s| s.len());
    println!("| rc {} {} strong {}", Rc::map(Rc::new(2u64), |x| x * 10), len, Rc::strong_count(&keep));
    let loud = Rc::map(Rc::new(Loud(1)), |l| {
        println!("| rc f {}", l.0);
        Loud(l.0 + 1)
    });
    println!("| rc loud {}", loud.0);
    drop(loud);
    let a = Arc::new(vec![1, 2]);
    let a2 = a.clone();
    println!("| arc {} {} strong {}", Arc::map(Arc::new(3u32), |x| x + 1), Arc::map(a, |v| v.len()), Arc::strong_count(&a2));
    let loud = Arc::map(Arc::new(Loud(10)), |l| {
        println!("| arc f {}", l.0);
        Loud(l.0 + 1)
    });
    println!("| arc loud {}", loud.0);
    drop(loud);
    let moved = Box::map(Box::new(Loud(20)), |l| l.0 * 2);
    println!("| box loud {}", moved);
}

#[test]
fn a08_once_from_fn_cow() {
    let once: Once = Default::default();
    once.call_once(|| println!("| once ran"));
    println!("| once done {}", once.is_completed());
    println!("| from_fn {:?}", Vec::from_fn(5, |i| i * i));
    let borrowed: Cow<'_, [u8]> = Cow::Borrowed(&[1, 2]);
    let owned: Cow<'_, [u8]> = Cow::Owned(vec![3]);
    let v = vec![1u8, 2];
    let mut arr = [1u8, 2];
    let m: &mut [u8] = &mut arr;
    println!("| cow {} {} {} {} {}", v == borrowed, &v[..] == borrowed, m == borrowed, vec![3u8] == owned, v == owned);
}
