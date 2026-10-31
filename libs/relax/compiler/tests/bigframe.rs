// Big stack frames and big copies: locals of 80 KB and 1 MB (stack probes touch every
// page), aggregates of several KB passed and returned by value (copy loops), deep
// recursion with mid-sized frames. Output matches rustc.

#[derive(Clone, Copy)]
struct Blob {
    head: u64,
    data: [u32; 1024],
    tail: u8,
}

fn make_blob(seed: u32) -> Blob {
    let mut b = Blob { head: seed as u64 * 3, data: [0; 1024], tail: (seed & 0xff) as u8 };
    let mut i = 0;
    while i < 1024 {
        b.data[i] = seed.wrapping_mul(2654435761).wrapping_add(i as u32 * 7);
        i += 1;
    }
    b
}

fn sum_blob(b: Blob) -> u64 {
    let mut s = b.head + b.tail as u64;
    let mut i = 0;
    while i < 1024 {
        s = s.wrapping_mul(31).wrapping_add(b.data[i] as u64);
        i += 1;
    }
    s
}

fn big_local(n: usize) -> u64 {
    let mut a = [0u64; 10000];
    let mut i = 0;
    while i < 10000 {
        a[i] = (i * n) as u64 ^ 0x5555;
        i += 1;
    }
    let mut s = 0u64;
    i = 0;
    while i < 10000 {
        s = s.wrapping_add(a[(i * 7919) % 10000]);
        i += 1;
    }
    s
}

fn huge_local() -> u64 {
    let mut a = [1u8; 1 << 20];
    a[12345] = 7;
    a[(1 << 20) - 1] = 9;
    let mut s = 0u64;
    let mut i = 0;
    while i < (1 << 20) {
        s += a[i] as u64;
        i += 4096;
    }
    s + a[12345] as u64 + a[(1 << 20) - 1] as u64
}

fn recurse(depth: u32, acc: u64) -> u64 {
    let mut pad = [0u64; 64];
    pad[(depth % 64) as usize] = acc;
    if depth == 0 {
        return pad[0] + 1;
    }
    recurse(depth - 1, acc.wrapping_mul(6364136223846793005).wrapping_add(depth as u64)) ^ pad[(depth % 64) as usize]
}

#[test]
fn big_copies() {
    let b = make_blob(17);
    let c = b;
    let mut d = c;
    d.data[1000] = 5;
    println!("blob {} {} {}", sum_blob(b), sum_blob(c), sum_blob(d));
}

#[test]
fn big_frames() {
    println!("big_local {} huge_local {}", big_local(3), huge_local());
    println!("recurse {}", recurse(2000, 1));
}
