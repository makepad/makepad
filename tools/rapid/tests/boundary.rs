// Proxy for the VM boundary cost: a property read done as an extern "C" call into the
// host (here libc `labs` as a stand-in for a generated C entry point) versus the same
// value read inline (the "VM intrinsic" path: a load from a repr(C) slot).
extern "C" {
    fn labs(x: i64) -> i64;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    tag: u64,
    value: i64,
}

fn read_inline(slots: &[Slot; 64], i: usize) -> i64 {
    if slots[i].tag == 3 {
        slots[i].value
    } else {
        0
    }
}

#[test]
fn call_through() {
    let mut s = 0i64;
    for i in 0..10_000_000i64 {
        s = s.wrapping_add(unsafe { labs(i - 5_000_000) });
    }
    println!("call-through sum {}", s);
}

#[test]
fn inline_intrinsic() {
    let mut slots = [Slot { tag: 3, value: 0 }; 64];
    for i in 0..64 {
        slots[i].value = i as i64;
    }
    let mut s = 0i64;
    for i in 0..10_000_000usize {
        s = s.wrapping_add(read_inline(&slots, i & 63));
    }
    println!("inline sum {}", s);
}
