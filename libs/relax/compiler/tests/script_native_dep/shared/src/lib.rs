// Plain Rust that a native package's build script and the program both use: the native build
// compiles its host unit for the script and only stubs its program-side unit.
pub fn offset() -> i32 {
    1
}

pub fn scale() -> i32 {
    10
}
