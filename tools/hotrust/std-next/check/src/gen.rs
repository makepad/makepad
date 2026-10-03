//! Generators for checked-in HotRust std sources whose content is derived mechanically:
//! `hotrust-std-gen num <core dir>` (integer impls), `hotrust-std-gen unicode <core dir>`
//! (char property/case tables, derived from this rustc's std so behaviour matches it),
//! `hotrust-std-gen fmt <core dir>` (integer formatting impls).
//! Run from tools/hotrust/std-next/check: cargo run --release --bin hotrust-std-gen -- <what> ../core

mod gen_fmt;
mod gen_num;
mod gen_tuple;
mod gen_unicode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: hotrust-std-gen num|unicode <core dir>");
        std::process::exit(2);
    }
    match args[1].as_str() {
        "fmt" => gen_fmt::run(&args[2]),
        "num" => gen_num::run(&args[2]),
        "tuple" => gen_tuple::run(&args[2]),
        "unicode" => gen_unicode::run(&args[2]),
        _ => {
            eprintln!("unknown generator {}", args[1]);
            std::process::exit(2);
        }
    }
}
