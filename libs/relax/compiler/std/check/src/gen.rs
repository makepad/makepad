//! Generators for checked-in Relax std sources whose content is derived mechanically:
//! `relax-std-gen num <core dir>` (integer impls), `relax-std-gen unicode <core dir>`
//! (char property/case tables, derived from this rustc's std so behaviour matches it),
//! `relax-std-gen fmt <core dir>` (integer formatting impls).
//! `relax-std-gen grisu <core dir>` (cached powers for float formatting).
//! `relax-std-gen dec2flt <core dir>` (exact powers of five for float parsing).
//! Run from libs/relax/compiler/std/check: cargo run --release --bin relax-std-gen -- <what> ../core

mod gen_dec2flt;
mod gen_fmt;
mod gen_grisu;
mod gen_num;
mod gen_tuple;
mod gen_unicode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: relax-std-gen num|unicode|fmt|tuple|dec2flt|grisu <core dir>");
        std::process::exit(2);
    }
    match args[1].as_str() {
        "dec2flt" => gen_dec2flt::run(&args[2]),
        "grisu" => gen_grisu::run(&args[2]),
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
