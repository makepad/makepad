// Boot batch 7 (counter from MetalCx::new to the first frame): or-pattern bindings (SILENT
// MISCOMPILE: never bound), byte-string patterns against &[u8; N] (SILENT: compared pointers),
// slice comparisons (SILENT: `a[..] == b[..]` was false), fn items / pointers are Clone, calls
// through a `*const dyn Fn` place, match arms coerce to the expected type, closures under
// `&mut dyn FnMut` deduce their signature, `str::to_owned` with a glob-imported `str` module,
// generic type aliases without args infer them, literal args select the only fitting impl
// (`Fid::from(9)`), single-variant enums clone, `f64 as u128`, `rchunks_mut`, `utf8_chunks`,
// `std::backtrace`. rustc: all pass.
use std::sync::Arc;

enum Me {
    A { name: u64, args: Vec<(u8, String)> },
    B { name: u64, extra: [u64; 3], args: Vec<(u8, String)> },
    C,
}

fn push(m: &mut Me, x: u8) {
    match m {
        Me::A { args, .. } | Me::B { args, .. } => args.push((x, "s".to_string())),
        Me::C => {}
    }
}

fn arg_count(m: &Me) -> usize {
    match m {
        Me::A { args, .. } | Me::B { args, .. } => args.len(),
        Me::C => 0,
    }
}

#[test]
fn or_pattern_bindings() {
    let mut a = Me::A { name: 1, args: Vec::new() };
    let mut b = Me::B { name: 2, extra: [7, 8, 9], args: Vec::new() };
    push(&mut a, 1);
    push(&mut b, 2);
    push(&mut b, 3);
    assert!(arg_count(&a) == 1 && arg_count(&b) == 2 && arg_count(&Me::C) == 0);
    if let Me::A { name, .. } | Me::B { name, .. } = &b {
        assert!(*name == 2);
    }
    if let Me::B { extra, .. } = &b {
        assert!(extra[2] == 9);
    }
    let v = [Some(3u32), None];
    let n: u32 = v.iter().map(|o| match o { Some(x @ 1..=5) | Some(x) => *x, None => 0 }).sum();
    assert!(n == 3);
}

fn tag_kind(tag: [u8; 4]) -> u32 {
    match &tag {
        b"bdat" => 1,
        b"head" => 2,
        _ => 0,
    }
}

#[test]
fn byte_string_patterns_on_arrays() {
    assert!(tag_kind(*b"bdat") == 1 && tag_kind(*b"head") == 2 && tag_kind(*b"zzzz") == 0);
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
struct Id(u64);

#[test]
fn slice_comparisons() {
    let a = [Id(1), Id(2)];
    let b = vec![Id(1), Id(2)];
    assert!(a[..] == b[..] && [1u8, 2][..] == b"\x01\x02"[..]);
    assert!(&a[..1] < &b[..] && a[..].cmp(&b[..]) == std::cmp::Ordering::Equal);
    let s1: &[Id] = &a;
    assert!(*s1 == b[..]);
}

struct Vm(u32);
fn inc(vm: &mut Vm) {
    vm.0 += 1
}
fn add10(vm: &mut Vm) {
    vm.0 += 10
}

#[test]
fn fn_pointers_clone_and_dyn_places() {
    let fs: [fn(&mut Vm); 2] = [inc, add10];
    let mut v: Vec<fn(&mut Vm)> = Vec::new();
    v.extend_from_slice(&fs);
    let w = v.clone();
    let mut vm = Vm(0);
    for f in &w {
        f(&mut vm)
    }
    let item = inc;
    (item.clone())(&mut vm);
    assert!(vm.0 == 12);
    let mut getters: Vec<Box<dyn Fn(&mut Vm, u64) -> u64>> = Vec::new();
    getters.push(Box::new(|vm: &mut Vm, x: u64| {
        vm.0 += 1;
        x * 2
    }));
    let p: *const dyn Fn(&mut Vm, u64) -> u64 = &*getters[0] as *const _;
    assert!(unsafe { (*p)(&mut vm, 21) } == 42 && vm.0 == 13);
}

#[derive(Clone)]
pub struct Inner(pub Arc<String>);
pub enum Str2 {
    Rc(Inner),
    Owned(String),
}
impl Str2 {
    pub fn as_ref(&self) -> &str {
        match self {
            Self::Rc(rc) => &rc.0,
            Self::Owned(s) => s,
        }
    }
}

struct P;
impl P {
    fn children(&self, visit: &mut dyn FnMut(u64, Id)) -> bool {
        visit(1, Id(0));
        visit(2, Id(5));
        true
    }
}

#[test]
fn coercions_and_closure_hints() {
    let a = Str2::Rc(Inner(Arc::new("ab".to_string())));
    assert!(a.as_ref() == "ab" && Str2::Owned("c".to_string()).as_ref() == "c");
    let mut found = Vec::new();
    let ok = P.children(&mut |name, child| {
        if child.0 != 0 {
            found.push((name, child));
        }
    });
    assert!(ok && found.len() == 1 && found[0].0 == 2);
}

mod reexport {
    pub use std::*;
}
#[allow(unused_imports)]
use reexport::*;

pub type MySet<V> = std::collections::HashSet<V>;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Fid(u64);
impl From<u64> for Fid {
    fn from(v: u64) -> Self {
        Fid(v)
    }
}
impl From<&str> for Fid {
    fn from(v: &str) -> Self {
        Fid(v.len() as u64)
    }
}
fn take(f: Fid) -> u64 {
    f.0
}

#[derive(Clone)]
enum OneState {
    Drawing,
}

#[test]
fn paths_aliases_and_literals() {
    let o: Option<&str> = Some("x");
    assert!(o.map(str::to_owned).unwrap() == "x");
    let mut s: MySet<String> = MySet::new();
    s.insert("a".to_string());
    let d: MySet<u32> = MySet::default();
    assert!(s.len() == 1 && d.is_empty());
    assert!(take(7.into()) == 7 && take("abc".into()) == 3 && Fid::from(9).0 == 9);
    let st = Some(OneState::Drawing).clone();
    assert!(matches!(st, Some(OneState::Drawing)));
    let v = vec![10u32, 20];
    assert!(v.get(1) == Some(&20));
}

#[test]
fn std_surface() {
    let x = 1.5e6f64;
    assert!((x * 2.0) as u128 == 3_000_000 && (-7.9f32) as i128 == -7 && f64::NAN as u128 == 0);
    assert!(1e40f64 as u128 == u128::MAX && -1.0f64 as u128 == 0);
    let mut v = vec![1u32, 2, 3, 4, 5];
    for c in v.rchunks_mut(2) {
        c[0] += 10;
    }
    assert!(v == [11u32, 12, 3, 14, 5]);
    let chunks: Vec<&str> = b"ab\xffc".utf8_chunks().map(|c| c.valid()).collect();
    assert!(chunks == ["ab", "c"]);
    let bt = std::backtrace::Backtrace::disabled();
    assert!(bt.status() == std::backtrace::BacktraceStatus::Disabled);
}

#[derive(Clone, Copy)]
pub enum TableIndex {
    A = 0,
    B = 1,
}
impl<T> std::ops::Index<TableIndex> for [T] {
    type Output = T;
    fn index(&self, i: TableIndex) -> &T {
        &self[i as usize]
    }
}
impl<T> std::ops::IndexMut<TableIndex> for [T] {
    fn index_mut(&mut self, i: TableIndex) -> &mut T {
        &mut self[i as usize]
    }
}

// rustybuzz ot_map.rs: an array indexed by a program type through `impl Index<TableIndex> for
// [T]` (rustc lookup_indexing unsizes the array); it was typed as slicing
#[test]
fn custom_index_on_arrays() {
    let mut chosen = [None; 2];
    for ti in [TableIndex::A, TableIndex::B] {
        chosen[ti] = Some(7u32);
    }
    assert!(chosen[TableIndex::B] == Some(7) && chosen[0] == Some(7));
}

// type params are implicitly Sized (Reference special-types-and-traits.md Sized): the blanket
// `impl<T, U: TryFrom<T>> TryInto<U> for T` is no candidate for a `[u8]` receiver, autoref
// reaches `&[u8]` and `impl TryFrom<&[T]> for [T; N]` (byteorder read_u32)
#[test]
fn implicit_sized_blanket_params() {
    let a: [u8; 2] = [7u8, 8][..].try_into().unwrap();
    let b = <[u8; 4]>::try_from(&[1u8, 0, 0, 0, 9][..4]).unwrap();
    assert!(a[1] == 8 && u32::from_le_bytes(b) == 1);
}
