// Corrupts the system heap so glibc's malloc aborts with its arena locked (tests/abort_heap/run.sh).
fn main() {
    println!("started");
    let t = std::thread::spawn(|| {
        const N: usize = 120_000;
        let mut kept = Vec::new();
        for _ in 0..8 {
            let v: Vec<u8> = Vec::with_capacity(N);
            // glibc: the chunk after an N-byte block (N a multiple of 16) starts N bytes past it, its
            // size word 8 bytes later. A size past anything the heap holds: the next block taken
            // from the heap's top aborts ("malloc(): corrupted top size").
            unsafe { std::ptr::write_unaligned((v.as_ptr() as usize + N + 8) as *mut u64, u64::MAX & !0xf) };
            kept.push(v);
        }
        kept.len()
    });
    println!("not aborted: {}", t.join().unwrap_or(0));
    std::process::exit(3);
}
