// A struct goes and its inherent impl stays: refused (E0412 at the impl's self type).
pub struct S(u32);
impl S {
    pub fn get(&self) -> u32 {
        self.0
    }
}

#[test]
fn run() {
    println!("out {}", S(4).get());
}
