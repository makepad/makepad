// A generic impl loses a generic method's override, then another's, and gets both back: the impl's
// instances map to the trait's (impl args, then the method's).
use std::sync::atomic::{AtomicU32, Ordering};
static PICKS: AtomicU32 = AtomicU32::new(0);
pub trait Named {
    fn name(&self) -> u32 {
        1
    }
    fn pick<U: Copy>(&self, u: U) -> U {
        u
    }
}
pub struct W<T>(T);
impl<T: Copy + Into<u32>> Named for W<T> {
    fn name(&self) -> u32 {
        let v: u32 = self.0.into();
        v * 3
    }
}
fn show<T: Named>(t: &T) -> u32 {
    t.name() + t.pick(10u32)
}

#[test]
fn run() {
    let p0 = PICKS.load(Ordering::Relaxed);
    println!("out {} {} {} {}", W(3u8).name(), show(&W(4u16)), W(5u32).pick(7i64), W(2u8).pick('x'));
    println!("out picks {}", PICKS.load(Ordering::Relaxed) - p0);
}
