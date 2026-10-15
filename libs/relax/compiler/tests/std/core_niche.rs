// std-core differential test (core_diff.sh): pointer niches and Option ownership.
use std::cell::Cell;
use std::collections::VecDeque;
use std::mem::size_of;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

#[test]
fn a01_sizes() {
    println!("| Box<u32> {}", size_of::<Box<u32>>());
    println!("| Option<Box<u32>> {}", size_of::<Option<Box<u32>>>());
    println!("| Box<[u8]> {}", size_of::<Box<[u8]>>());
    println!("| Option<Box<[u8]>> {}", size_of::<Option<Box<[u8]>>>());
    println!("| Box<dyn Fn()> {}", size_of::<Box<dyn Fn()>>());
    println!("| Option<Box<dyn Fn()>> {}", size_of::<Option<Box<dyn Fn()>>>());
    println!("| Vec<u8> {}", size_of::<Vec<u8>>());
    println!("| Option<Vec<u8>> {}", size_of::<Option<Vec<u8>>>());
    println!("| String {}", size_of::<String>());
    println!("| Option<String> {}", size_of::<Option<String>>());
    println!("| Rc<u8> {}", size_of::<Rc<u8>>());
    println!("| Option<Rc<u8>> {}", size_of::<Option<Rc<u8>>>());
    println!("| Arc<str> {}", size_of::<Arc<str>>());
    println!("| Option<Arc<str>> {}", size_of::<Option<Arc<str>>>());
    println!("| NonNull<u8> {}", size_of::<NonNull<u8>>());
    println!("| Option<NonNull<u8>> {}", size_of::<Option<NonNull<u8>>>());
    println!("| VecDeque<u8> {}", size_of::<VecDeque<u8>>());
    println!("| Option<VecDeque<u8>> {}", size_of::<Option<VecDeque<u8>>>());
}

#[test]
fn a02_options() {
    let mut value: Option<Box<u32>> = None;
    println!("| box none {}", value.is_none());
    value = Some(Box::new(42));
    println!("| box {} {}", value.take().map(|x| *x).unwrap(), value.is_none());
    let mut value: Option<Box<[u8]>> = None;
    println!("| slice none {}", value.is_none());
    value = Some(vec![3, 5, 7].into_boxed_slice());
    println!("| slice {} {}", value.take().map(|x| x.len()).unwrap(), value.is_none());
    let mut value: Option<Vec<u8>> = None;
    println!("| vec none {}", value.is_none());
    value = Some(vec![11, 13]);
    println!("| vec {} {}", value.take().map(|x| x.len()).unwrap(), value.is_none());
    let mut value: Option<String> = None;
    println!("| string none {}", value.is_none());
    value = Some(String::from("niche"));
    println!("| string {} {}", value.take().map(|x| x.len()).unwrap(), value.is_none());
    let mut value: Option<Rc<u8>> = None;
    println!("| rc none {}", value.is_none());
    value = Some(Rc::new(17));
    println!("| rc {} {}", value.take().map(|x| *x).unwrap(), value.is_none());
    let mut value: Option<Arc<str>> = None;
    println!("| arc none {}", value.is_none());
    value = Some(Arc::from("shared"));
    println!("| arc {} {}", value.take().map(|x| x.len()).unwrap(), value.is_none());
    let mut value: Option<VecDeque<u8>> = None;
    println!("| deque none {}", value.is_none());
    value = Some(VecDeque::from(vec![23, 29]));
    println!("| deque {} {}", value.take().map(|x| x.len()).unwrap(), value.is_none());
    let calls = Cell::new(0);
    let f: Box<dyn Fn()> = Box::new(|| calls.set(calls.get() + 1));
    let mut f = Some(f);
    f.take().unwrap()();
    println!("| closure {} {}", calls.get(), f.is_none());
    let mut byte = 19u8;
    let mut p = Some(NonNull::from(&mut byte));
    println!("| nonnull {} {}", p.take().map(|x| unsafe { *x.as_ptr() }).unwrap(), p.is_none());
    println!("| empty {} {} {} {}", Some(Vec::<u8>::new()).is_some(), Some(Vec::<()>::new()).is_some(), Some(VecDeque::<u8>::new()).is_some(), Some(VecDeque::<()>::new()).is_some());
    let mut weak = Some(std::rc::Weak::<u8>::new());
    println!("| weak {} {}", weak.take().unwrap().upgrade().is_none(), weak.is_none());
    let mut weak = Some(std::sync::Weak::<u8>::new());
    println!("| arc weak {} {}", weak.take().unwrap().upgrade().is_none(), weak.is_none());
}

struct Count<'a>(&'a Cell<usize>);
impl Drop for Count<'_> {
    fn drop(&mut self) { self.0.set(self.0.get() + 1); }
}

#[test]
fn a03_drops() {
    let count = Cell::new(0);
    let mut b = Some(Box::new(Count(&count)));
    drop(b.take());
    println!("| drop box {} {}", count.get(), b.is_none());
    let mut s = Some(vec![Count(&count), Count(&count)].into_boxed_slice());
    drop(s.take());
    println!("| drop slice {} {}", count.get(), s.is_none());
    count.set(0);
    let captured = Count(&count);
    let f: Box<dyn Fn()> = Box::new(move || { let _ = &captured; });
    let mut f = Some(f);
    drop(f.take());
    println!("| drop closure {} {}", count.get(), f.is_none());
    let mut v = Some(vec![Count(&count), Count(&count)]);
    drop(v.take().map(|x| x.into_iter()).unwrap());
    println!("| drop vec {} {}", count.get(), v.is_none());
    let mut r = Some(Rc::new(Count(&count)));
    let copy = r.as_ref().unwrap().clone();
    drop(r.take());
    println!("| drop rc shared {}", count.get());
    drop(copy);
    let mut a = Some(Arc::new(Count(&count)));
    drop(a.take());
    println!("| drop rc arc {} {} {}", count.get(), r.is_none(), a.is_none());
    let mut d = Some(VecDeque::from(vec![Count(&count), Count(&count)]));
    drop(d.take());
    println!("| drop deque {} {}", count.get(), d.is_none());
}

#[test]
fn a04_fat_nonnull() {
    let mut bytes = [2u8, 4, 6];
    let mut p = NonNull::from_mut(&mut bytes[..]);
    println!("| fat {} {}", unsafe { p.as_ref().len() }, unsafe { (&*p.as_ptr()).len() });
    unsafe { p.as_mut()[1] = 9; }
    let q = NonNull::from_ref(&bytes[..2]);
    println!("| metadata {} {} {}", p == q, p.addr() == q.addr(), unsafe { *p.cast::<u8>().as_ptr() });
    println!("| raw metadata {}", p.as_ptr() == q.as_ptr());
    println!("| fat null {}", NonNull::new(std::ptr::slice_from_raw_parts_mut(std::ptr::null_mut::<u8>(), 2)).is_none());
    println!("| fat new {} {}", NonNull::new(p.as_ptr()).is_some(), unsafe { NonNull::new_unchecked(p.as_ptr()).as_ref()[1] });
}
