// The built-in derives on an enum in a sandboxed module load and run: PartialEq, PartialOrd, Ord
// and Hash read the discriminant, which (as in rustc) is a safe read through a reference, so the
// code they make is safe code the module checker accepts. Nothing of the module's own is let
// through: an `unsafe` block it writes is refused, beside the same derives.
use std::relax::Module;

const DERIVED: &str = r#"
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Mode { #[default] Off, Low(u8), High { level: u32 } }
struct Sum(u64);
impl core::hash::Hasher for Sum {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, b: &[u8]) { for x in b { self.0 = self.0.wrapping_mul(31).wrapping_add(*x as u64); } }
}
fn hash(m: Mode) -> u64 { let mut h = Sum(0); core::hash::Hash::hash(&m, &mut h); core::hash::Hasher::finish(&h) }
pub fn check() -> u32 {
    let (off, low, high) = (Mode::Off, Mode::Low(3), Mode::High { level: 9 });
    let eq = low == Mode::Low(3) && low != Mode::Low(4) && low != off && off == Mode::default();
    let ord = off < low && low < high && high.cmp(&low) == core::cmp::Ordering::Greater && Mode::Low(2) < low;
    let hashed = hash(low) == hash(Mode::Low(3)) && hash(low) != hash(Mode::Low(4)) && hash(off) != hash(low);
    let debug = format!("{:?}", high) == "High { level: 9 }";
    eq as u32 | (ord as u32) << 1 | (hashed as u32) << 2 | (debug as u32) << 3
}
"#;

#[test]
fn derives_on_an_enum() {
    let m = Module::load_source("derived_enum", "derived_enum.rs", DERIVED, &[]);
    if let Err(e) = &m {
        println!("out load error: {e}");
    }
    let bits = m.as_ref().ok().and_then(|m| m.get::<fn() -> u32>("check").ok()).map_or(0, |f| f());
    println!("out derived PartialEq: {}", bits & 1 != 0);
    println!("out derived PartialOrd, Ord: {}", bits & 2 != 0);
    println!("out derived Hash: {}", bits & 4 != 0);
    println!("out derived Debug: {}", bits & 8 != 0);
    let own = Module::load_source("own_unsafe", "own_unsafe.rs", "#[derive(PartialEq, PartialOrd, Hash)]\npub enum E { A, B(u8) }\npub fn f(e: &E) -> bool { unsafe { *e == E::A } }\n", &[]);
    println!("out its own unsafe refused: {}", own.is_err_and(|e| e.contains("`unsafe` is not allowed in a sandboxed module")));
}
