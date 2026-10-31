// Prints "out tick <n> <value>" every 20 ms; run.py edits this file while it runs.
mod ease;
use ease::{ease, Ease};

pub struct Pad {
    pub a: u64,
}

fn value(p: &Pad) -> u64 {
    p.a + ease(&Ease::Pow { begin: 2, end: 1 }, 1u32) as u64
}

fn main() {
    let t0 = std::time::Instant::now();
    let pad = Pad { a: 1 };
    let mut n: u64 = 0;
    loop {
        let v = value(&pad);
        println!("out tick {} {}", n, v);
        if v == 999 || t0.elapsed().as_secs() > 60 {
            break;
        }
        n += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
