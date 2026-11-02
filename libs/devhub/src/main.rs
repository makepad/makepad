//! The devhub command line and the master (`serve`): its own binary, so a change here never
//! rebuilds the JIT. `makepad devhub <words>` forwards here through cargo.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(makepad_devhub::cli::main(&args));
}
