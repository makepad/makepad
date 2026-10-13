#[path = "../src/prelude_real.rs"]
mod prelude;
use prelude::s;
#[path = "../src/benches.rs"]
mod benches;
fn main() {
    benches::run();
}
