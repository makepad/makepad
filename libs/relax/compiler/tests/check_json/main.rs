#![no_std]
struct Widget;
use crate::Widgte;
fn bad(_: Widgte) {}
macro_rules! broken { () => { fn inside(_: Missing) {} }; }
broken!();
fn main() {}
macro_rules! greeting { () => {} }
greetign!();
