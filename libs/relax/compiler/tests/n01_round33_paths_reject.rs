// typeck-audit 1.9 / 1.30: rustc rejects both (E0277, E0747). Checked as a package.
#![allow(dead_code)]
trait Tr { const C: u8; }
fn unbounded<T>() -> u8 { <T as Tr>::C }
struct V<T>(T);
fn kind_mismatch(_: V<3>) {}
