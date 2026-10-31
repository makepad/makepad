// A big value built in a local and returned is built in the caller's result buffer
// (opt::forward_slot_dests_x, Func::sret), also when a panic unwinds out of the half-built
// value: its cleanup drops the fields already written, from the buffer. rustc prints the same
// lines and hash: 5150696096613278025.
use std::cell::Cell;
thread_local! { static DROPS: Cell<u32> = const { Cell::new(0) }; }
struct Noisy(u32);
impl Drop for Noisy {
    fn drop(&mut self) {
        DROPS.with(|d| d.set(d.get() + self.0));
    }
}
struct Big {
    table: [u16; 40000],
    head: Noisy,
    tail: Noisy,
}
#[inline(never)]
fn make(k: u32, fail: bool) -> Big {
    let mut table = [0u16; 40000];
    for i in 0..40000 {
        table[i] = (i as u32 * 7 ^ k) as u16;
    }
    let head = Noisy(1);
    if fail {
        panic!("half built");
    }
    Big { table, head, tail: Noisy(10) }
}
#[inline(never)]
fn boxed(k: u32) -> Box<Big> {
    Box::new(make(k, false))
}
#[test]
fn sret_in_place() {
    let mut h = 0u64;
    let b = make(3, false);
    let c = boxed(5);
    for i in (0..40000).step_by(97) {
        h = h.wrapping_mul(31) ^ b.table[i] as u64 ^ (c.table[i] as u64) << 17;
    }
    h ^= (b.head.0 + b.tail.0 + c.head.0 + c.tail.0) as u64;
    drop(b);
    drop(c);
    let r = std::panic::catch_unwind(|| make(9, true).table[1]);
    let drops = DROPS.with(|d| d.get());
    println!("hash {} err {} drops {}", h, r.is_err(), drops);
    assert_eq!(drops, 23);
    assert_eq!(h, 5150696096613278025);
}
