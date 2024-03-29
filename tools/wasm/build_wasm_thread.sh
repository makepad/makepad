#CARGO_TARGET_DIR=target/wasm32-thread 
MAKEPAD=lines RUSTFLAGS="-C codegen-units=1 -C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--export=__stack_pointer -C opt-level=z" cargo +nightly build $1 $2 --target=wasm32-unknown-unknown --release -Z build-std=panic_abort,std
#cargo run -p wasm_strip --release -- target/wasm32-unknown-unknown/release/$1.wasm
#cargo run -p brotli_check --release -- target/wasm32-unknown-unknown/release/$1.wasm
