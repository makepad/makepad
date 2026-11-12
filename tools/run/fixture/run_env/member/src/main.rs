fn main() {
    let bad = relax_run_env::check(false);
    if !bad.is_empty() {
        eprintln!("run_env: the program's environment differs from cargo run's: {bad:#?}");
        std::process::exit(1);
    }
    println!("run_env: program ok");
}
