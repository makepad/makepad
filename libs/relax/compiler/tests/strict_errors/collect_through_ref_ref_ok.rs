//~ none
// accepted by rustc: a field read through `&&T` (`iter()` over a `Vec<&T>` built by an inferred
// `Vec<_>` collect) collects into a Vec of the field's type, and a `&dyn Fn(&T)` closure takes `&&T`
// by deref coercion.
struct Cost { instructions: u64, us: u64 }
fn main() {
    let all = vec![Cost { instructions: 3, us: 10 }, Cost { instructions: 1, us: 30 }];
    let ticks: Vec<_> = all.iter().rev().take(2).collect();
    let mean = |f: &dyn Fn(&Cost) -> f64| if ticks.is_empty() { 0.0 } else { ticks.iter().map(|t| f(t)).sum::<f64>() / ticks.len() as f64 };
    let mut instr: Vec<u64> = ticks.iter().map(|t| t.instructions).collect();
    instr.sort();
    let p95 = instr.get(((instr.len().max(1) - 1) as f64 * 0.95) as usize).copied().unwrap_or(0);
    println!("{} {} {}", instr[0], p95, mean(&|t| t.us as f64));
}
