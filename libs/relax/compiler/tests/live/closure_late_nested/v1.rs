// LIVEICE: an edit above `later` shifts the expression ids of its closures (later is not
// rebuilt). Its `or_else` closure first runs (and is built) after the edit, on another thread;
// the closures nested in it (`find`, `map`) were never laid out before: their environment
// layout must come from the version that made them, not from later's current body
// ("side table: no entry" ICE in the user's Amp). v1 adds `first_end` and calls it
// (refuse_then_nested_closure: the same after a refused edit).
use std::cell::Cell;

thread_local! {
    static RUNS: Cell<u32> = Cell::new(0);
}

fn first_end(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in text.bytes().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn edited(text: &str) -> usize {
    first_end(text).unwrap_or(0)
}

fn later(n: u32, text: &str) -> u32 {
    let base = n * 10;
    let first: Option<u32> = if n >= 3 { None } else { Some(1) };
    first
        .or_else(|| text.split(',').find(|p| p.trim().parse::<u32>().is_ok()).map(|p| p.trim().parse::<u32>().unwrap_or(0) + base))
        .unwrap_or(0)
}

#[test]
fn run() {
    let n = RUNS.with(|r| { r.set(r.get() + 1); r.get() });
    let h = std::thread::spawn(move || later(n, "a, 7, b"));
    println!("out {} {}", edited("{a}{b}"), h.join().unwrap());
}
