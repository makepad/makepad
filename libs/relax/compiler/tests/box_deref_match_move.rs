// `match *b` / `if let P = *b` of a Box local moving fields out of the contents: the moves are
// move paths in `b` (rustc move_paths/builder.rs: a Box deref is a move path), so `b`'s drop
// drops only what was not moved and frees the allocation (elaborate_drop.rs
// open_drop_for_box_contents) -- not the whole contents a second time
use std::cell::Cell;
thread_local! { static DROPS: Cell<u32> = Cell::new(0); }
struct D(u32);
impl Drop for D { fn drop(&mut self) { DROPS.with(|d| d.set(d.get() + 1)); } }
fn drops() -> u32 { DROPS.with(|d| d.get()) }
fn reset() { DROPS.with(|d| d.set(0)); }
enum E { A { x: D, y: D }, B(D) }
fn early(q: Box<E>) -> Option<(D, D)> { if let E::A { x, y } = *q { return Some((x, y)); } None }
fn part(q: Box<E>) -> u32 { if let E::A { x, .. } = *q { return x.0; } 0 }
fn arms(q: Box<E>) -> u32 { match *q { E::A { x, y } => x.0 + y.0, E::B(d) => d.0 } }
fn taken(mut o: Option<Box<E>>) -> u32 { let q = o.take().unwrap(); if let E::A { x, y } = *q { return x.0 + y.0; } 7 }
fn twice(q: Box<Box<E>>) -> u32 { match **q { E::A { x, .. } => x.0, E::B(_) => 9 } }
#[test] fn early_return() { reset(); drop(early(Box::new(E::A { x: D(1), y: D(2) }))); assert_eq!(drops(), 2); }
#[test] fn no_match_drops_all() { reset(); assert!(early(Box::new(E::B(D(1)))).is_none()); assert_eq!(drops(), 1); }
#[test] fn rest_dropped_once() { reset(); assert_eq!(part(Box::new(E::A { x: D(1), y: D(2) })), 1); assert_eq!(drops(), 2); }
#[test] fn match_arms() { reset(); assert_eq!(arms(Box::new(E::A { x: D(1), y: D(2) })), 3); assert_eq!(drops(), 2); }
#[test] fn match_arms_b() { reset(); assert_eq!(arms(Box::new(E::B(D(4)))), 4); assert_eq!(drops(), 1); }
#[test] fn take_unwrap() { reset(); assert_eq!(taken(Some(Box::new(E::A { x: D(1), y: D(2) }))), 3); assert_eq!(drops(), 2); }
#[test] fn box_box() { reset(); assert_eq!(twice(Box::new(Box::new(E::A { x: D(1), y: D(2) }))), 1); assert_eq!(drops(), 2); }

// the shape that crashed Scope's parser (code_graph TypeRef / Path / PathSegment): a qualified
// self `<T as Tr>::` taken out of a path and its boxed parts moved into a new value
struct Segment { generics: Vec<Ty> }
struct Path { segments: Vec<Segment>, qself: Option<Box<Ty>> }
enum Ty {
    Path { path: Path, args: Vec<Ty> },
    Assoc { self_type: Box<Ty>, trait_type: Option<Box<Ty>> },
    Infer,
}
fn ty() -> Ty { Ty::Path { path: Path { segments: vec![Segment { generics: vec![Ty::Infer] }], qself: None }, args: Vec::new() } }
fn unpack(mut path: Path) -> Ty {
    if path.qself.is_some() {
        let qself = path.qself.take().unwrap();
        if let Ty::Assoc { self_type, trait_type } = *qself {
            return Ty::Assoc { self_type, trait_type };
        }
    }
    Ty::Path { path, args: Vec::new() }
}
#[test]
fn qualified_type_drop() {
    let owner = Path { segments: Vec::new(), qself: Some(Box::new(Ty::Assoc { self_type: Box::new(ty()), trait_type: Some(Box::new(ty())) })) };
    let t = unpack(owner);
    assert!(matches!(&t, Ty::Assoc { trait_type: Some(_), .. }));
    drop(t);
}
