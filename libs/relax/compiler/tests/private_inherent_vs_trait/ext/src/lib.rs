pub struct W(pub u32);
impl W {
    fn apply(&self, _: u32) {}
    const K: u32 = 1;
    fn make() -> u32 { 7 }
}
pub trait Apply {
    const K: u32;
    fn apply(&mut self, v: u32);
    fn make() -> u32;
}
impl Apply for W {
    const K: u32 = 2;
    fn apply(&mut self, v: u32) { self.0 = v }
    fn make() -> u32 { 9 }
}
