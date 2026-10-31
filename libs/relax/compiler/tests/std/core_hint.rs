// core::hint::select_unpredictable: picks like `if`, and drops the value not picked.
use std::cell::Cell;

thread_local! {
    static DROPS: Cell<u32> = Cell::new(0);
}

struct D(u32);

impl Drop for D {
    fn drop(&mut self) {
        DROPS.with(|d| d.set(d.get() * 10 + self.0));
    }
}

#[test]
fn a01_select_unpredictable() {
    for c in [true, false] {
        println!("| select {} -> {}", c, std::hint::select_unpredictable(c, 1u8, 2u8));
        DROPS.with(|d| d.set(0));
        let picked = std::hint::select_unpredictable(c, D(1), D(2));
        let dropped = DROPS.with(|d| d.get());
        println!("| picked {} dropped {}", picked.0, dropped);
    }
}
