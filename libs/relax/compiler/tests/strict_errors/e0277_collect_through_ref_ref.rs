//~ E0277
//~ help: a value of type `Vec<usize>` cannot be built from an iterator over elements of type `u64`
// A u64 field collected into a Vec<usize> through `&&T` (`iter()` over a `Vec<&T>` gives the closure
// `&&T`): rustc rejects it, as no u64 -> usize FromIterator exists. The closure's output is the
// field's type, whatever the reference depth. The message names the item type normalized,
// as rustc's does, not `<Map<..> as Iterator>::Item`.
struct Cost { instructions: u64 }
fn main() {
    let all = vec![Cost { instructions: 3 }, Cost { instructions: 1 }];
    let ticks: Vec<_> = all.iter().rev().take(2).collect();
    let instr: Vec<usize> = ticks.iter().map(|t| t.instructions).collect();
    println!("{}", instr.len());
}
