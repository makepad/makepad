//! The program behind makepad inspect's answer eval (tools/run/fixture/inspect_eval.sh): a small
//! "inventory sync" with known faults an agent must find through `makepad inspect` alone.
//! - main: `score_items` is the hot fn (~30% of a core: busy ~30 ms, then sleeps ~70 ms);
//!   `render_label` allocates and frees many small strings per tick (churn, nothing kept);
//!   `remember_batch` keeps 32 KB every tick in `HISTORY` (grows ~320 KB/s, never freed).
//! - threads `ledger` and `audit` take the locks `STOCK` and `ORDERS` in opposite order: they
//!   deadlock after ~2 s.
//! - thread `importer` panics in `parse_record` (index 6 of a 4-field record) once EVAL_SECS/60
//!   seconds (at least 5) have passed (parks under MAKEPAD_RUN_PANIC_PARK).
//! - thread `uploader` waits on a channel whose sender (main) never sends: blocked, not deadlocked.

use std::sync::Mutex;
use std::time::{Duration, Instant};

static STOCK: Mutex<u64> = Mutex::new(0);
static ORDERS: Mutex<u64> = Mutex::new(0);
static HISTORY: Mutex<Vec<Box<[u8]>>> = Mutex::new(Vec::new());

#[inline(never)]
fn score_items(items: &[u64], rounds: u32) -> u64 {
    let mut acc = 0xcbf2_9ce4_8422_2325u64;
    for r in 0..rounds {
        for &x in items {
            acc ^= x.wrapping_add(r as u64);
            acc = acc.wrapping_mul(0x100_0000_01b3).rotate_left(5);
        }
    }
    acc
}

#[inline(never)]
fn render_label(i: u64) -> usize {
    let mut n = 0;
    for k in 0..200 {
        let s = format!("item-{}-{}-{:x}", i, k, i.wrapping_mul(k));
        n += s.len();
    }
    n
}

#[inline(never)]
fn remember_batch(tick: u64) {
    let batch = vec![(tick & 0xff) as u8; 32 << 10].into_boxed_slice();
    HISTORY.lock().unwrap().push(batch);
}

#[inline(never)]
fn post_ledger(n: u64) {
    let mut s = STOCK.lock().unwrap();
    *s += n;
    std::thread::sleep(Duration::from_millis(50));
    let mut o = ORDERS.lock().unwrap();
    *o += 1;
}

#[inline(never)]
fn run_audit(n: u64) {
    let mut o = ORDERS.lock().unwrap();
    *o += n;
    std::thread::sleep(Duration::from_millis(50));
    let mut s = STOCK.lock().unwrap();
    *s += 1;
}

#[inline(never)]
fn parse_record(line: &str, want: usize) -> u64 {
    let fields: Vec<&str> = line.split(',').collect();
    let idx = fields.len() - 1 + want;
    fields[idx].parse().unwrap_or(0)
}

fn main() {
    let secs: u64 = std::env::var("EVAL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
    let start = Instant::now();
    std::thread::Builder::new()
        .name("ledger".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            for i in 0.. {
                post_ledger(i);
            }
        })
        .unwrap();
    std::thread::Builder::new()
        .name("audit".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            for i in 0.. {
                run_audit(i);
            }
        })
        .unwrap();
    let panic_after = Duration::from_secs((secs / 60).max(5));
    std::thread::Builder::new()
        .name("importer".into())
        .spawn(move || {
            let mut sum = 0u64;
            loop {
                let want = if start.elapsed() > panic_after { 3 } else { 0 };
                sum = sum.wrapping_add(parse_record("12,7,99,4", want.min(0)));
                if want > 0 {
                    sum = sum.wrapping_add(parse_record("12,7,99,4", want));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::Builder::new()
        .name("uploader".into())
        .spawn(move || {
            while let Ok(v) = rx.recv() {
                std::hint::black_box(v);
            }
        })
        .unwrap();
    let items: Vec<u64> = (0..4096u64).map(|i| i.wrapping_mul(2654435761)).collect();
    let mut total = 0u64;
    let mut tick = 0u64;
    while start.elapsed() < Duration::from_secs(secs) {
        let busy = Instant::now();
        while busy.elapsed() < Duration::from_millis(30) {
            total = total.wrapping_add(score_items(&items, 8));
        }
        total = total.wrapping_add(render_label(tick) as u64);
        remember_batch(tick);
        tick += 1;
        std::thread::sleep(Duration::from_millis(70));
    }
    drop(tx);
    println!("total {}", total);
}
