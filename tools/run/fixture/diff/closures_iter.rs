// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

fn make_adder(n: i32) -> impl Fn(i32) -> i32 { move |x| x + n }
fn make_boxed(k: i32) -> Box<dyn FnMut(i32) -> i32> { let mut st = 0; Box::new(move |x| { st += x * k; st }) }
fn apply<F: FnOnce() -> String>(f: F) -> String { f() }
fn work() {
    let add = make_adder(5);
    let mut b = make_boxed(3);
    let r: Vec<i32> = (0..10).map(|x| b(add(x))).collect();
    d!("{:?}", r);
    let s = String::from("moved");
    d!("{}", apply(move || s + "!"));
    let v: Vec<u32> = (1..200).collect();
    d!("{}", v.iter().zip(v.iter().rev()).map(|(a, b)| (a * b) as u64).filter(|x| x % 3 != 0).sum::<u64>());
    d!("{:?}", v.windows(3).map(|w| w[0] + w[2] - w[1]).take(5).collect::<Vec<_>>());
    d!("{:?}", v.chunks(40).map(|c| c.iter().sum::<u32>()).collect::<Vec<_>>());
    d!("{:?}", v.iter().step_by(37).flat_map(|&x| [x, x * 2]).collect::<Vec<_>>());
    d!("{:?}", v.iter().rev().skip(3).take_while(|&&x| x > 150).fold(0u32, |a, x| a * 3 + x));
    let mut words = vec!["pear", "fig", "apple", "kiwi", "banana", "date"];
    words.sort_by(|a, b| a.len().cmp(&b.len()).then(b.cmp(a)));
    d!("{:?} {:?}", words, words.iter().position(|w| w.starts_with('k')));
    let (ev, od): (Vec<u32>, Vec<u32>) = v.iter().take(12).partition(|&&x| x % 2 == 0);
    d!("{:?} {:?} {:?} {:?}", ev, od, v.iter().max_by_key(|&&x| (x * 7) % 11), v.iter().min_by(|a, b| (*a % 9).cmp(&(*b % 9))));
    let mut fs: Vec<Box<dyn Fn(f64) -> f64>> = Vec::new();
    for i in 1..5 { let k = i as f64; fs.push(Box::new(move |x| x * k + 0.5)); }
    d!("{}", fs.iter().fold(1.0, |a, f| f(a)));
    let mut counter = 0; let mut inc = || { counter += 2; counter }; inc(); inc();
    d!("{}", counter);
    let o: Option<u32> = v.iter().copied().find(|x| x * x > 5000); d!("{:?} {:?}", o, "a,b,,c".split(',').collect::<Vec<_>>());
    let r: Result<Vec<i32>, _> = "1 2 x 4".split(' ').map(|s| s.parse::<i32>()).collect(); d!("{:?}", r.is_err());
}
