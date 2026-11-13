// What the program was given: its first argument and two environment values, as the OS gave them.
fn main() {
    let arg = std::env::args_os().nth(1);
    let value = std::env::var_os("NON_UNICODE_VALUE");
    let recorded = std::env::var_os("MAKEPAD_NON_UNICODE");
    println!("{arg:?} {value:?} {recorded:?}");
}
