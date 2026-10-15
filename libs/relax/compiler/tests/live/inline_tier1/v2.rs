// 3.4: a patched leaf that O3 (and O1) inlined: every slot that inlined it is rebuilt at
// both tiers, and a O3 build in flight from the old source is dropped. MAKEPAD_RUN_O3_CALLS=1
// makes `drive` hot at once; each run sleeps so its O3 build installs before the next edit.
// v2 also edits the leaf reached through a devirtualised dyn call.
trait Op {
    fn apply(&self, x: u64) -> u64;
}
struct Add3;
impl Op for Add3 {
    fn apply(&self, x: u64) -> u64 {
        x + 5
    }
}

fn mix(x: u64, k: u64) -> u64 {
    (x ^ k).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 1
}

fn drive(n: u64, op: &dyn Op) -> u64 {
    let mut acc = 1;
    for i in 0..n {
        acc = op.apply(mix(acc, i)).rotate_left(7);
    }
    acc
}

#[test]
fn run() {
    let op = Add3;
    for _ in 0..20 {
        drive(1000, &op);
    }
    println!("out {:016x}", drive(100_000, &op));
    std::thread::sleep(std::time::Duration::from_millis(200));
}
