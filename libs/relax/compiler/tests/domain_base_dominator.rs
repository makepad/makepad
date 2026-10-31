// A function whose only domain-static uses sit in two branches below an if/else: the block that reads the
// domain base is their common dominator, which holds no instruction of its own; it must still run the read
// (x64 skips empty jump blocks). Output matches rustc.
use std::hint::black_box;

static TABLE_A: [u32; 4] = [1, 2, 3, 4];
static TABLE_B: [u32; 4] = [10, 20, 30, 40];

fn pick(mode: u32, i: usize) -> u32 {
    let mut s = 0;
    if mode & 1 == 0 {
        if mode & 2 == 0 {
            s += TABLE_A[i & 3];
        } else {
            s += TABLE_B[i & 3];
        }
    } else if mode & 4 == 0 {
        s += TABLE_B[(i + 1) & 3] * 2;
    } else {
        s += TABLE_A[(i + 2) & 3] * 3;
    }
    s
}

fn run() {
    let mut t = 0u64;
    for r in 0..4000u32 {
        t += pick(black_box(r % 8), black_box(r as usize)) as u64;
    }
    println!("{t}");
}

#[test]
fn base_read_in_empty_dominator() {
    run();
}

fn main() {
    run();
}
