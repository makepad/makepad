// Program stacks (relax_base::stack::on_program_stack): a recursion and a stack buffer far beyond
// a native thread's 2-8 MiB fit on a JIT program thread's stack. Relax only (program_stack.sh):
// a rustc build of this overflows.
fn down(n: u64) -> u64 {
    let pad = [n; 64];
    if n == 0 {
        return 0;
    }
    down(n - 1) + std::hint::black_box(pad)[7] % 2
}

#[inline(never)]
fn big_buffer(len_mb: usize) -> u64 {
    let mut buf = [0u8; 256 << 20];
    let n = (len_mb << 20).min(buf.len());
    for i in (0..n).step_by(4096) {
        buf[i] = (i >> 12) as u8;
    }
    std::hint::black_box(&mut buf);
    buf.iter().step_by(4096).map(|&b| b as u64).sum()
}

#[test]
fn deep_recursion() {
    // ~600 bytes a level, a million levels: ~600 MiB of stack
    println!("D deep {}", down(1_000_000));
}

#[test]
fn big_stack_buffer() {
    println!("D buffer {}", big_buffer(256));
}

#[test]
fn spawned_thread_too() {
    let h = std::thread::spawn(|| down(500_000));
    println!("D thread {}", h.join().unwrap());
}
