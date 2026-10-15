// Big values built through temporaries (an array filled in a loop, a struct literal around it,
// Box::new of the struct) are built in place and big slots with disjoint lifetimes share
// stack (opt::forward_slot_dests_x, color_slots): Amp's 0.9 MB frames overflowed 2 MiB worker
// stacks. The values must come out the same: rustc prints the same hash, 12467953869637839889.
struct Big {
    a: [u32; 40000],
    n: usize,
}
impl Big {
    fn new(k: u32) -> Big {
        let mut a = [0u32; 40000];
        for i in 0..40000 {
            a[i] = i as u32 ^ k;
        }
        Big { a, n: k as usize }
    }
}
#[inline(never)]
fn mk(k: u32) -> Box<Big> {
    Box::new(Big::new(k))
}
#[inline(never)]
fn twice() -> u64 {
    let a = mk(3);
    let b = Box::new(Big::new(5));
    let c = Box::new(Big { a: [7; 40000], n: 9 });
    let mut h = 0u64;
    for i in 0..40000 {
        h = h.wrapping_mul(31) ^ a.a[i] as u64 ^ (b.a[i] as u64) << 7 ^ (c.a[i] as u64) << 13;
    }
    h ^ (a.n + b.n + c.n) as u64
}
#[test]
fn big_value_frames() {
    let h = twice();
    println!("hash {}", h);
    assert_eq!(h, 12467953869637839889);
}
