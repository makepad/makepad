// Code the program has not run yet, a round at a time: each first call is a lazy compile, which
// the compiler's owner serves while the background coherence check runs.
fn step<const N: usize>() -> usize {
    N / N
}

fn main() {
    let mut steps = 0;
    macro_rules! rounds {
        ($($n:literal)*) => {
            $(
                steps += step::<$n>();
                std::thread::sleep(std::time::Duration::from_millis(25));
            )*
        };
    }
    rounds!(1 2 3 4 5 6 7 8);
    println!("steps {steps} state {}", cdl_state::state());
}
