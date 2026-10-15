// typeck-audit 1.14: rustc rejects each (E0277 Copy for a count > 1 or a parameter count,
// E0282 for an inferred count). Checked as a package with `makepad check`.
#![allow(dead_code)]
fn two() { let _s = [String::new(); 2]; }
fn param<const N: usize>() { let _s = [String::new(); N]; }
fn unknown() { let _x = [0u8; _]; }
