/// `entry!(f)` defines `main` calling `f` (the shape of makepad's `app_main!`).
#[macro_export]
macro_rules! entry {
    ($f:ident) => {
        fn main() {
            println!("bin-lib ok {}", $f());
        }
    };
}
