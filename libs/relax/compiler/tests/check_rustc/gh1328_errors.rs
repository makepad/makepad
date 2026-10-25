// GitHub #1328 (the false dead_code of the report): rustc's analysis stops after an error of the
// required analyses (passes.rs), so check_mod_deathness never runs. Items used only from a body
// that failed its type check are not dead; not even a truly dead item is reported.
const SH_QUAD: usize = 0;
struct S { f: u32, g: u32 }
impl S { fn helper(&self) -> u32 { self.f } }
fn used_by_bad(s: &S) -> u32 { s.g }
fn bad(s: &S) -> u32 {
    let x: u32 = "no";
    s.helper() + x + SH_QUAD as u32 + used_by_bad(s)
}
fn truly_dead() {}
#[expect(dead_code)]
fn expected_dead() {}
fn main() { let s = S { f: 1, g: 2 }; println!("{}", bad(&s)); }
