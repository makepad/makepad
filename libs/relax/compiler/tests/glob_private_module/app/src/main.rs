// `realj::json!` through `use globb::*`: `realj` is the extern crate, not globb's private module.
mod with_private_module {
    use globb::*;
    pub fn v() -> realj::Value {
        let _ = Marker;
        realj::json!(7)
    }
}

fn main() {
    println!("glob private module {}", with_private_module::v().0);
}
