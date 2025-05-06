C bindings to the Rust's `ttf-parser` library.

Provides access only to most common methods.

## Build

```sh
cargo build --release
```

This will produce a dynamic library that can be located at `target/release`.
Be sure to strip it.

## Run tests

```sh
cargo build
gcc test.c -g -o test -L./target/debug/ -lttfparser
env LD_LIBRARY_PATH=./target/debug/ ./test
```

## Safety

- The library doesn't use `unsafe` (except the bindings itself).
- All data access is bound checked.
- No heap allocations, so crash due to OOM is not possible.
- Technically, should use less than 64KiB of stack in worst case scenario.
- All methods are thread-safe.
- All recursive methods have a depth limit.
- Most of arithmetic operations are checked.
- Most of numeric casts are checked.
- Rust panics will be caught at the FFI boundary.

## Header generation

The `ttfparser.h` is generated via [cbindgen](https://github.com/eqrion/cbindgen)
and then manually edited a bit.
