use std::cell::Cell;
use std::rc::Rc;

struct Numbers {
    values: Vec<u32>,
}

impl Numbers {
    fn evens(&self) -> impl Iterator<Item = u32> + '_ {
        self.values.iter().filter(|x| **x % 2 == 0).copied()
    }
}

fn doubled<I: Iterator<Item = u32>>(i: I) -> impl Iterator<Item = u32> {
    i.map(|x| x * 2)
}

fn counter() -> impl Iterator<Item = u64> {
    (1..=4).map(|x| x * 10)
}

fn adder(n: i32) -> impl Fn(i32) -> i32 {
    move |x| x + n
}

fn shown(x: u32) -> impl std::fmt::Display {
    x + 1
}

trait Source {
    fn items(&self) -> impl Iterator<Item = u8>;
}

struct A;
struct B;

impl Source for A {
    fn items(&self) -> impl Iterator<Item = u8> {
        vec![1, 2, 3].into_iter()
    }
}

impl Source for B {
    fn items(&self) -> impl Iterator<Item = u8> {
        0..3
    }
}

fn total<S: Source>(s: &S) -> u32 {
    s.items().map(u32::from).sum()
}

fn names() -> impl Iterator<Item = impl std::fmt::Display> {
    vec!["a", "b"].into_iter()
}

struct Guard(Rc<Cell<u32>>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

fn make(c: Rc<Cell<u32>>) -> impl Fn() -> u32 {
    let g = Guard(c.clone());
    move || {
        let _ = &g;
        7
    }
}

#[test]
fn r1() {
    let a = Numbers { values: vec![1, 2, 3, 4] };
    let b = Numbers { values: vec![2, 5, 4, 7] };
    assert_eq!(a.evens().sum::<u32>(), 6);
    assert!(a.evens().eq(b.evens()));
}

#[test]
fn r2() {
    let values: Vec<u32> = doubled(vec![1, 2, 3].into_iter()).collect();
    assert_eq!(values, vec![2, 4, 6]);
}

#[test]
fn r3() {
    let mut sum: u64 = 0;
    for value in counter() {
        sum += value;
    }
    assert_eq!(sum, 100);
}

#[test]
fn r4() {
    let f = adder(5);
    assert_eq!(f(1), 6);
    assert_eq!(adder(2)(3), 5);
}

#[test]
fn r5() {
    assert_eq!(shown(4).to_string(), "5");
    assert_eq!(format!("{}", shown(1)), "2");
}

#[test]
fn r6() {
    assert_eq!(total(&A), 6);
    assert_eq!(total(&B), 3);
    assert_eq!(A.items().count(), 3);
}

#[test]
fn r7() {
    let values: Vec<String> = names().map(|n| n.to_string()).collect();
    assert_eq!(values, ["a", "b"]);
}

#[test]
fn r8() {
    let c = Rc::new(Cell::new(0));
    {
        let f = make(c.clone());
        assert_eq!(f(), 7);
        assert_eq!(c.get(), 0);
    }
    assert_eq!(c.get(), 1);
}

#[test]
fn r9() {
    let b: Box<dyn Iterator<Item = u32>> =
        Box::new(doubled(vec![1u32, 2].into_iter()));
    assert_eq!(b.sum::<u32>(), 6);
}

#[test]
fn r10() {
    let mut it = counter();
    assert_eq!(it.next(), Some(10));
    let rest: Vec<u64> = it.by_ref().take(2).collect();
    assert_eq!(rest, vec![20, 30]);
    assert_eq!(it.next(), Some(40));
}
