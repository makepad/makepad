# Third-party code in Relax's std (std)

- `smallvec/` is ported from smallvec 1.15.1 (https://github.com/servo/rust-smallvec),
  Copyright (c) 2018 The Servo Project Developers, MIT or Apache-2.0.
- `arrayvec/` is ported from arrayvec 0.7.6 (https://github.com/bluss/arrayvec),
  Copyright (c) Ulrik Sverdrup "bluss" 2015-2023, MIT or Apache-2.0.
- `core/fmt/float*` follows the structure of Rust core's flt2dec (dragon/grisu ideas),
  MIT or Apache-2.0, The Rust Project Developers.
- `core/str/` split/pattern state machines and slicing panic messages follow Rust core,
  MIT or Apache-2.0, The Rust Project Developers.
- `core/core_simd/` is portable-simd's core_simd (https://github.com/rust-lang/portable-simd, as vendored in
  rustc 1.97's library/portable-simd), and `std/std_float.rs` its std_float, pulled into core/std as rustc's
  library does; adapted for Relax (no f16 vectors, `simd_swizzle!` as macro_rules, per-lane transcendental
  functions). MIT or Apache-2.0, The Rust Project Developers.
