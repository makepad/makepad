//~ E0658
//~ help: attributes on expressions are experimental
// A macro_rules defined in the crate whose expansion puts an attribute on an expression in
// expression position (a `let` initializer, an operand, a match arm without braces, a
// parenthesized expression, an array's repeated element): rustc gates the attribute's tokens in
// the definition as it gates the crate's own text. Relax had skipped all expansion text.
macro_rules! five { () => { #[cfg(all())] 5u32 } }
fn f(o: Option<u32>) -> u32 {
    let x = five!();
    let y = x + (five!()) + [five!(); 2][1];
    match o { Some(_) => five!(), None => y }
}
fn main() { f(None); }
