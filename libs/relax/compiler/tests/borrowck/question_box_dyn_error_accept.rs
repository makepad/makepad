// epaint text/font.rs FontFace::new: `?` on a Result<_, Box<dyn Error>> inside a function that
// returns Box<dyn Error>. Box<dyn Error> is not an Error (library/alloc/src/boxed.rs:2420 is
// `impl<E: Error> Error for Box<E>`), so `From<Box<dyn Error>> for Box<dyn Error>` is the
// reflexive impl alone.
use std::error::Error;
struct Cell(u32);
fn try_new<F: FnOnce(&u32) -> Result<u32, E>, E>(o: u32, f: F) -> Result<Cell, E> { Ok(Cell(f(&o)?)) }
fn parse(s: &str) -> Result<u32, std::num::ParseIntError> { s.parse() }
fn make(s: &str) -> Result<Cell, Box<dyn Error>> {
    let c = try_new(1, |_o| {
        let v = parse(s)?;
        Ok::<u32, Box<dyn Error>>(v)
    })?;
    Ok(c)
}
fn same() -> Result<u32, Box<dyn Error>> {
    let c = Ok::<u32, Box<dyn Error>>(1)?;
    Ok(c)
}
fn main() { println!("{} {}", make("5").unwrap().0, same().unwrap()); }
