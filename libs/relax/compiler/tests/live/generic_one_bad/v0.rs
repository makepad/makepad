// G4 / 3.5: a generic fn edited so its body is only valid for some of its live instances.
// rustc rejects `x + x` for a T without Add at the definition; checking per instance would
// accept the u8 and u32 instances and fail on W. Either way the whole save must be refused and
// all three instances keep running the old code.
#[derive(Clone, Copy)]
struct W(u8);
impl From<W> for u64 {
    fn from(w: W) -> u64 { w.0 as u64 * 100 }
}

fn bump<T: Copy + Into<u64>>(x: T) -> u64 {
    x.into() + 1
}

#[test]
fn run() {
    println!("out {} {} {}", bump(3u8), bump(70000u32), bump(W(2)));
}
