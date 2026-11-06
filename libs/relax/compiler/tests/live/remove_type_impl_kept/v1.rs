// A struct goes and its inherent impl stays: refused (E0412 at the impl's self type).
impl S {
    pub fn get(&self) -> u32 {
        self.0
    }
}

#[test]
fn run() {
    println!("out {}", 4);
}
