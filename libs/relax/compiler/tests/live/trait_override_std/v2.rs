// An Iterator impl gets and loses `size_hint`: std's `ExactSizeIterator::len`, which inlined the
// default, is rebuilt and calls the override.
pub struct Counter(u32);
impl Iterator for Counter {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.0 == 0 {
            None
        } else {
            self.0 -= 1;
            Some(self.0)
        }
    }
}
impl ExactSizeIterator for Counter {}

#[test]
fn run() {
    let c = Counter(4);
    println!("out {} {:?}", c.len(), Counter(3).size_hint());
}
