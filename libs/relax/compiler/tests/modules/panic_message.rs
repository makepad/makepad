// What a module's panic says reaches the host: the guard's error (and a budget's Fault) is the
// panic's message, for `panic!`, `unwrap`, `expect` and `assert!` as it already was for a
// run-time check (an index out of bounds).
use std::relax::{guard, Budget, BudgetError, Module};

const SOURCE: &str = r#"
pub fn boom(n: u32) -> u32 { if n > 0 { panic!("boom {}", n) } n }
pub fn none(n: u32) -> u32 { let x: Option<u32> = if n > 0 { None } else { Some(1) }; x.unwrap() }
pub fn wanted(n: u32) -> u32 { let x: Result<u32, &str> = if n > 0 { Err("bad") } else { Ok(1) }; x.expect("wanted") }
pub fn checked(n: u32) -> u32 { assert!(n == 0, "n was {}", n); n }
pub fn index(n: usize) -> u8 { let v = [1u8, 2, 3]; v[n] }
"#;

#[test]
fn panic_message() {
    let m = Module::load_source("talker", "talker.rs", SOURCE, &[]).unwrap();
    let boom: fn(u32) -> u32 = m.get("boom").unwrap();
    let none: fn(u32) -> u32 = m.get("none").unwrap();
    let wanted: fn(u32) -> u32 = m.get("wanted").unwrap();
    let checked: fn(u32) -> u32 = m.get("checked").unwrap();
    let index: fn(usize) -> u8 = m.get("index").unwrap();
    println!("out panic!: {:?}", guard(0, || boom(7)));
    println!("out unwrap: {:?}", guard(0, || none(1)));
    println!("out expect: {:?}", guard(0, || wanted(1)));
    println!("out assert!: {:?}", guard(0, || checked(3)));
    println!("out index: {:?}", guard(0, || index(9)));
    let mut budget = Budget::new(1_000_000, 16 << 20);
    let fault = match m.call_with(&mut budget, || boom(2)) { Err(BudgetError::Fault(e)) => e, _ => String::new() };
    println!("out under a budget: {:?}", fault);
    println!("out no panic: {:?}", guard(0, || boom(0) + none(0) + wanted(0)));
    m.unload();
}
