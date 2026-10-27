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
- `core/libm/` (web targets only, cfg(relax_web)) is ported from the Rust port of musl libm in rustc 1.97.1's
  library/compiler-builtins/libm (https://github.com/rust-lang/compiler-builtins), the math library rustc's
  wasm32-unknown-unknown std uses; the musl / FreeBSD msun / Sun Microsystems notices stay in each file and
  `core/libm/LICENSE.txt` is the upstream licence (MIT). Native targets keep the platform C math library.

- `std/sys/dlmalloc/` is the web-only port of dlmalloc 0.2.13
  (https://github.com/alexcrichton/dlmalloc-rs), Copyright Alex Crichton, MIT OR Apache-2.0.
  Crate SHA256: `9f5b01c17f85ee988d832c40e549a64bd89ab2c9f8d8a613bdf5122ae507e294`.
