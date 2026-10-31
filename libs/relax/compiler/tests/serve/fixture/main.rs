#![no_std]
/// A documented counter.
#[doc = "Used by the editor fixture."]
pub struct Counter {
    /// Current count.
    pub value: i32,
}
impl Counter {
    /// Read the count.
    pub fn read(&self) -> i32 { self.value }
}
fn broken(_: Missing) {}
fn helper(n: i32) -> i32 { n + 1 }
fn main() {
    let local = Counter { value: 3 };
    let field = local.value;
    let call = local.read();
    let _result = helper(field + call);
}
