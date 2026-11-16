// Destructuring assignment coerces each part to its assignee's type, as a plain assignment does:
// rustc lowers `(a, b) = rhs` to `{ let (lhs0, lhs1) = rhs; a = lhs0; b = lhs1; }`, and each
// `a = lhs0` is a coercion site. Deref coercions (`&Box<T>`, `&Arc<T>`, `&String`, `&Vec<T>` to
// the target), unsizing (to `&dyn Trait`, `&[T]`) and `&mut` to `&`, in tuple, array, tuple
// struct and struct assignees. The whole right side is evaluated before the first part is
// coerced. Relax reported E0308 for the deref coercions (relax issue 231).
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::fmt::Display;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

#[test]
fn a01_deref_through_smart_pointers() {
    let x = Box::new(1u32);
    let y = Arc::new(2u32);
    let z = Rc::new(3u32);
    let (a, b, c): (&u32, &u32, &u32);
    (a, b, c) = (&x, &y, &z);
    println!("| {a} {b} {c}");
}

#[test]
fn a02_string_and_vec_to_slices() {
    let owned = String::from("hi");
    let v = vec![1, 2, 3];
    let r = Rc::new(String::from("rc"));
    let mut out: &str = "";
    let s: &[i32];
    let t: &str;
    println!("| before {out:?}");
    (out, s) = (&owned, &v);
    [t] = [&r];
    println!("| {out} {s:?} {t}");
}

#[test]
fn a03_unsizing() {
    let x = 5u8;
    let arr = [7u16, 8, 9];
    let d: &dyn Display;
    let e: &dyn Display;
    let sl: &[u16];
    (d, (e, sl)) = (&x, (&"str", &arr));
    println!("| {d} {e} {sl:?} {}", sl.len());
}

struct Two<'a>(&'a str, &'a [u8]);
struct Named<'a> {
    text: &'a str,
    num: &'a u64,
}

#[test]
fn a04_struct_assignees() {
    let s = String::from("tuple struct");
    let bytes = vec![1u8, 2];
    let boxed = Box::new(64u64);
    let (mut t, mut b): (&str, &[u8]) = ("", &[]);
    let (mut text, mut num): (&str, &u64) = ("", &0);
    println!("| {t:?} {b:?} {text:?} {num}");
    Two(t, b) = Two(&s, &bytes);
    Named { text, num } = Named { text: &s[6..], num: &boxed };
    println!("| {t} {b:?} {text} {num}");
}

#[test]
fn a05_mut_to_shared_and_reborrow() {
    let mut a = 1u32;
    let mut b = vec![2u32];
    let (x, y): (&u32, &mut Vec<u32>);
    (x, y) = (&mut a, &mut b);
    y.push(3);
    println!("| {x} {y:?}");
}

/// A Deref that says when it runs: the coercions run after the whole right side, part by part.
struct Loud(u32, &'static str);
impl Deref for Loud {
    type Target = u32;
    fn deref(&self) -> &u32 {
        println!("| deref {}", self.1);
        &self.0
    }
}

#[test]
fn a06_order_of_coercions() {
    let (p, q) = (Loud(10, "p"), Loud(20, "q"));
    let (a, b): (&u32, &u32);
    (a, b) = (
        {
            println!("| part 0");
            &p
        },
        {
            println!("| part 1");
            &q
        },
    );
    println!("| {a} {b}");
}

fn pair<'a>(x: &'a Box<i64>, y: &'a String) -> (&'a Box<i64>, &'a String) {
    (x, y)
}

#[test]
fn a07_from_a_call_and_in_a_loop() {
    let xs = [Box::new(-1i64), Box::new(-2)];
    let ys = [String::from("one"), String::from("two")];
    let mut n: &i64 = &0;
    let mut s: &str = "";
    for i in 0..2 {
        (n, s) = pair(&xs[i], &ys[i]);
        println!("| {i}: {n} {s}");
    }
    println!("| last {n} {s}");
}
