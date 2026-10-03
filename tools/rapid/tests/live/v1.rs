// Live-patch demo: the test hashes `kernel` over a range; edits swap `kernel`.
const STEPS: u32 = 1_000_000;

fn kernel(x: f64) -> f64 {
    (x * x).sqrt() - 2.0 * x
}

fn checksum() -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for i in 0..STEPS {
        let v = kernel(i as f64 * 0.001);
        h = (h ^ v.to_bits()).wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn run() {
    println!("checksum {:016x}", checksum());
}
