// G1: consts are folded into their users; an edit of a const must recompile every reader,
// through other consts (B = A * 2) and array lengths.
const STEPS: u32 = 2000;
const A: u64 = 3;
const B: u64 = A * 2;

fn kernel(x: u64) -> u64 {
    x * B + 1
}

fn checksum() -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for i in 0..STEPS {
        h = (h ^ kernel(i as u64)).wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn run() {
    println!("out steps {} b {} sum {:016x}", STEPS, B, checksum());
}
