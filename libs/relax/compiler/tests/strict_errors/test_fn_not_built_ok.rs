//~ none
// a #[test] fn is not an item outside a test build (rustc's harness removes it before name
// resolution): its body may name what only the test configuration has (brotli-decompressor
// huffman/tests.rs: `#[cfg(test)]` covers only the next `use`, the #[test] fns name HuffmanCode)
mod inner {
    #[cfg(test)]

    use super::*;
    #[test]
    fn only_in_tests() {
        let _x = Helper { v: 1 };
    }
}
#[cfg(test)]
struct Helper { v: u32 }
fn main() {
    println!("ok");
}
