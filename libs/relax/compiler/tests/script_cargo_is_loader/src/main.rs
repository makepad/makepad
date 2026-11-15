fn main() {
    println!("{}", env!("SCRIPT_CARGO"));
    println!("{}", env!("SCRIPT_RUSTC"));
    println!("{}", env!("SCRIPT_RUSTC_VERSION"));
    println!("{}", env!("SCRIPT_RUSTC_CFG"));
}
