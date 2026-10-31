// typeck-audit 1.19: rustc rejects this (E0080, the static's initializer panics); `makepad run`
// must not run past it (exit 101), not print with a null static.
static TABLE: [u8; 2] = [1, 2];
static BAD: u8 = { let i = TABLE.len(); TABLE[i] };
fn main() { println!("ran past: {}", BAD); }
