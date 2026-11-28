//~ none
// A repeat count is a `usize` body (rustc checks it as an anonymous constant and coerces its value
// to `usize`). Its literals are usize too when a binary operator's left operand takes no expected
// type: `[0i32; 32 + 1]` has a usize count, never an i32 one (the borrow checker's
// input/output check refused the count's i32 body as `usize==i32`).
fn len_of(data: &[u8]) -> usize {
    data.len()
}

fn main() {
    let codes = [0i32; 32 + 1];
    let image = len_of(&[127; 16 * 16 * 4]);
    let rows: Vec<[u8; 6]> = (1..=2).map(|c| [c; 2 * 3]).collect();
    let shifted = [0u16; 1 << 3];
    println!("{} {} {:?} {}", codes.len(), image, rows, shifted.len());
}
