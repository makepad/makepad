CARGO_TARGET_DIR=target/wasm32-thread RUSTFLAGS="-C codegen-units=1 -C target-feature=+atomics,+bulk-memory,+mutable-globals -C 
link-arg=--export=__stack_pointer -C opt-level=z" cargo +nightly-2022-07-09 build -p $1 --target=wasm32-unknown-unknown --release -Z build-std=panic_abort,std
cargo +nightly-2022-07-09 run -p wasm_strip --release -- target/wasm32-thread/wasm32-unknown-unknown/release/$1.wasm
#cargo run -p brotli_check --release -- target/wasm32-unknown-unknown/release/$1.wasm
