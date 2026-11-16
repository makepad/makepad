// std-core differential test (core_diff.sh, agree.sh): Vec's allocator parameter, which rustc 1.100.0
// stabilized (relax issue 156). Vecs in allocators of our own that count and print what goes
// through them, by reference and by value: new_in, with_capacity_in and the growth, shrinking,
// try_reserve's errors (a capacity overflow, an allocator that refuses), clone and split_off, the
// drop order of elements, buffer and allocator (Vec and IntoIter, also when an element's drop
// unwinds), the raw and NonNull round trips (into_parts, from_parts, with or without an allocator),
// insert_mut, into_boxed_slice / into_vec and Box<[T], A>'s clone and into_iter,
// comparisons across allocators, and the layouts. drain, extract_if and splice take the global
// allocator only on stable (their iterators' allocator parameter is unstable): Splice, ported with
// this, on a Vec of the global allocator (shorter, longer, a replacement of unknown length, a tail
// that moves twice). "| " lines must equal rustc's.
use std::alloc::{AllocError, Allocator, Global, Layout, System};
use std::cell::Cell;
use std::mem::size_of;
use std::ptr::NonNull;

#[derive(Default)]
struct Counting {
    allocs: Cell<usize>,
    frees: Cell<usize>,
    live: Cell<usize>,
}

unsafe impl Allocator for Counting {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.allocs.set(self.allocs.get() + 1);
        self.live.set(self.live.get() + layout.size());
        Global.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        self.frees.set(self.frees.get() + 1);
        self.live.set(self.live.get() - layout.size());
        unsafe { Global.deallocate(ptr, layout) }
    }
}

impl Counting {
    fn line(&self, what: &str) {
        println!("| {what}: allocs {} frees {} live {}", self.allocs.get(), self.frees.get(), self.live.get());
    }
}

/// An allocator held by value that says when it frees and when it is dropped.
struct Named<'a>(&'a Counting, &'static str);

unsafe impl Allocator for Named<'_> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.0.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        println!("| {} frees {} bytes", self.1, layout.size());
        unsafe { self.0.deallocate(ptr, layout) }
    }
}

impl Drop for Named<'_> {
    fn drop(&mut self) {
        println!("| {} dropped", self.1);
    }
}

/// Refuses every block above `limit` bytes.
struct Limited {
    limit: usize,
}

unsafe impl Allocator for Limited {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if layout.size() > self.limit {
            println!("| limited refuses {} bytes", layout.size());
            return Err(AllocError);
        }
        Global.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { Global.deallocate(ptr, layout) }
    }
}

struct Loud(u32);

impl Drop for Loud {
    fn drop(&mut self) {
        println!("| drop {}", self.0);
    }
}

/// Unwinds out of its drop when it holds 0.
struct Bomb(u32);

impl Drop for Bomb {
    fn drop(&mut self) {
        if self.0 == 0 {
            panic!("bomb");
        }
        println!("| bomb {} dropped", self.0);
    }
}

fn ids(v: &[Loud]) -> Vec<u32> {
    v.iter().map(|l| l.0).collect()
}

#[test]
fn v01_growth_and_shrink() {
    let a = Counting::default();
    {
        let mut v: Vec<u32, &Counting> = Vec::new_in(&a);
        println!("| new_in cap {} len {}", v.capacity(), v.len());
        a.line("empty");
        for i in 0..9 {
            v.push(i);
            if v.len() == v.capacity() || i == 8 {
                println!("| len {} cap {}", v.len(), v.capacity());
            }
        }
        a.line("nine pushed");
        v.reserve_exact(20);
        println!("| reserve_exact cap {}", v.capacity());
        v.truncate(3);
        v.shrink_to(5);
        println!("| shrink_to 5 cap {}", v.capacity());
        v.shrink_to_fit();
        println!("| shrink_to_fit cap {} {:?}", v.capacity(), v);
        a.line("shrunk");
        v.clear();
        v.shrink_to_fit();
        println!("| cleared cap {}", v.capacity());
        a.line("emptied");
        let w: Vec<u64, &Counting> = Vec::with_capacity_in(6, &a);
        println!("| with_capacity_in cap {} same allocator {}", w.capacity(), std::ptr::eq(*w.allocator(), &a));
        let z: Vec<(), &Counting> = Vec::with_capacity_in(6, &a);
        println!("| zero-sized cap {}", z.capacity() == usize::MAX);
    }
    a.line("dropped");
}

#[test]
fn v02_try_reserve_errors() {
    let a = Counting::default();
    let mut v: Vec<u64, &Counting> = Vec::new_in(&a);
    match v.try_reserve(usize::MAX) {
        Ok(()) => println!("| overflow reserved?"),
        Err(e) => println!("| overflow: {e}"),
    }
    let mut l: Vec<u8, Limited> = Vec::new_in(Limited { limit: 16 });
    println!("| small reserve {:?}", l.try_reserve(10).is_ok());
    l.extend_from_slice(b"0123456789");
    match l.try_reserve(100) {
        Ok(()) => println!("| big reserved?"),
        Err(e) => println!("| refused: {e}"),
    }
    match l.try_reserve_exact(40) {
        Ok(()) => println!("| big exact reserved?"),
        Err(e) => println!("| refused exact: {e}"),
    }
    println!("| still {:?} cap {}", std::str::from_utf8(&l).unwrap(), l.capacity());
    v.push(1);
    a.line("one pushed");
}

#[test]
fn v03_drop_order() {
    let a = Counting::default();
    {
        let mut v = Vec::new_in(Named(&a, "vec"));
        v.push(Loud(1));
        v.push(Loud(2));
        v.push(Loud(3));
        println!("| dropping a vec of {}", v.len());
    }
    a.line("vec dropped");
    {
        let mut v = Vec::with_capacity_in(4, Named(&a, "iter"));
        v.extend([Loud(4), Loud(5), Loud(6)]);
        let mut it = v.into_iter();
        let first = it.next().unwrap();
        println!("| took {} left {}", first.0, it.len());
        drop(first);
        println!("| dropping the iterator");
    }
    a.line("iter dropped");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut v = Vec::new_in(Named(&a, "bombs"));
        v.push(Bomb(1));
        v.push(Bomb(0));
        v.push(Bomb(2));
    }));
    println!("| vec drop unwound {}", r.is_err());
    a.line("after the bombs");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut v = Vec::new_in(Named(&a, "iter bombs"));
        v.push(Bomb(3));
        v.push(Bomb(0));
        v.push(Bomb(4));
        let mut it = v.into_iter();
        drop(it.next());
    }));
    println!("| iter drop unwound {}", r.is_err());
    a.line("after the iter bombs");
}

#[test]
fn v04_clone_split_append() {
    let a = Counting::default();
    {
        let mut v: Vec<u16, &Counting> = Vec::new_in(&a);
        v.extend(1..=6u16);
        let c = v.clone();
        a.line("cloned");
        let tail = v.split_off(4);
        println!("| split {:?} {:?} same allocator {}", v, tail, std::ptr::eq(*tail.allocator(), &a));
        let mut other: Vec<u16, &Counting> = Vec::new_in(&a);
        other.push(9);
        v.append(&mut other);
        println!("| appended {:?} other {:?}", v, other);
        let mut d = c.clone();
        d.clone_from(&v);
        println!("| clone_from {:?}", d);
        a.line("before drops");
    }
    a.line("dropped");
}

#[test]
fn v05_drain_extract_splice() {
    {
        let mut v: Vec<Loud> = Vec::new();
        v.extend((0..8).map(Loud));
        {
            let mut d = v.drain(1..4);
            let x = d.next().unwrap();
            println!("| drained {} rest {}", x.0, d.len());
        }
        println!("| after drain {:?}", ids(&v));
        let odd: Vec<Loud> = v.extract_if(.., |l| l.0 % 2 == 1).collect();
        println!("| extracted {:?} kept {:?}", ids(&odd), ids(&v));
        drop(odd);
        let removed: Vec<u32> = v.splice(1..2, [Loud(10), Loud(11), Loud(12)]).map(|l| l.0).collect();
        println!("| splice longer removed {:?} now {:?}", removed, ids(&v));
        let removed: Vec<u32> = v.splice(0..3, [Loud(20)]).map(|l| l.0).collect();
        println!("| splice shorter removed {:?} now {:?}", removed, ids(&v));
        v.shrink_to_fit();
        // an unknown length (a filter's lower bound is 0): collected, the tail moves once, the
        // buffer grows
        v.splice(1..1, (30..34).map(Loud).filter(|l| l.0 != 31));
        println!("| splice unknown now {:?} cap {}", ids(&v), v.capacity());
        // a lower bound that falls short: the tail moves for it, then again for the rest
        v.splice(2..3, [Loud(50), Loud(51)].into_iter().chain((52..55).map(Loud).filter(|l| l.0 != 0)));
        println!("| splice short hint now {:?}", ids(&v));
        let n = v.len();
        let mut s = v.splice(n.., [Loud(40), Loud(41)]);
        println!("| splice at the end yields {:?}", s.next().map(|l| l.0));
        drop(s);
        println!("| now {:?} cap {}", ids(&v), v.capacity());
        let s = v.splice(0..2, [Loud(60)]);
        println!("| splice dropped untouched");
        drop(s);
        println!("| now {:?}", ids(&v));
    }
    println!("| dropped");
}

#[test]
fn v06_raw_round_trip_and_boxes() {
    let a = Counting::default();
    {
        let mut v: Vec<u32, &Counting> = Vec::with_capacity_in(5, &a);
        v.extend([7, 8, 9]);
        let (p, len, cap, alloc) = v.into_raw_parts_with_allocator();
        println!("| raw len {} cap {}", len, cap);
        let v = unsafe { Vec::from_raw_parts_in(p, len, cap, alloc) };
        let (p, len, cap, alloc) = v.into_parts_with_allocator();
        println!("| parts len {} cap {}", len, cap);
        let mut v = unsafe { Vec::from_parts_in(p, len, cap, alloc) };
        *v.insert_mut(1, 4) += 10;
        println!("| insert_mut {:?}", v);
        v.remove(1);
        let b: Box<[u32], &Counting> = v.into_boxed_slice();
        a.line("boxed");
        let c = b.clone();
        let back = b.into_vec();
        println!("| into_vec {:?} cap {}", back, back.capacity());
        let sum: u32 = IntoIterator::into_iter(c).sum();
        println!("| box into_iter sum {}", sum);
        let arr: [u32; 3] = back.try_into().unwrap();
        println!("| array {:?}", arr);
        let short: Vec<u32, &Counting> = Vec::from(Box::new_in([1u32, 2], &a) as Box<[u32], &Counting>);
        let err: Result<[u32; 3], _> = short.try_into();
        println!("| wrong length gives back {:?}", err.map_err(|v| v.len()));
        let flat: Vec<u8, &Counting> = {
            let mut pairs = Vec::new_in(&a);
            pairs.push([1u8, 2]);
            pairs.push([3, 4]);
            pairs.into_flattened()
        };
        println!("| flattened {:?} cap {}", flat, flat.capacity());
    }
    a.line("dropped");
    // KNOWN_GAPS 8 (now closed): a boxed slice in an allocator of its own
    let b: Box<[u8], System> = Box::new_in([1u8, 2, 3], System);
    let c = b.clone();
    let v = b.into_vec();
    let n: u32 = IntoIterator::into_iter(c).map(u32::from).sum();
    println!("| system {} {}", v.len(), n);
    // the global allocator's parts (box_vec_non_null)
    let g = vec![5u16, 6];
    let (p, len, cap) = g.into_parts();
    let g = unsafe { Vec::from_parts(p.cast::<i16>(), len, cap) };
    println!("| global parts {:?} cap {}", g, g.capacity());
}

#[test]
fn v07_comparisons_and_layouts() {
    let a = Counting::default();
    let mut x: Vec<i32, &Counting> = Vec::new_in(&a);
    x.extend([1, 2, 3]);
    let y: Vec<i32> = vec![1, 2, 3];
    let z: Vec<i32, System> = {
        let mut z = Vec::new_in(System);
        z.extend_from_slice(&[1, 2, 4]);
        z
    };
    println!("| eq across allocators {} {} {}", x == y, y == z, x == [1, 2, 3]);
    println!("| cmp across allocators {:?} {:?}", x.partial_cmp(&z), y.partial_cmp(&x));
    println!("| slice eq {} {}", x[..] == y, &y[..] == x);
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    x.hash(&mut h1);
    y.hash(&mut h2);
    println!("| same hash {}", h1.finish() == h2.finish());
    fn send<T: Send>(_: &T) -> bool {
        true
    }
    println!("| a System vec is Send {}", send(&z));
    println!("| Vec<u8> {} Option {}", size_of::<Vec<u8>>(), size_of::<Option<Vec<u8>>>());
    println!("| Vec<u8, &A> {} Option {} System {}", size_of::<Vec<u8, &Counting>>(), size_of::<Option<Vec<u8, &Counting>>>(), size_of::<Vec<u8, System>>());
}
