NIGHTLY=${2:-nightly}
#CARGO_TARGET_DIR=target/wasm32-thread 
RUSTFLAGS="-C codegen-units=1 -C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--export=__stack_pointer -C opt-level=z" cargo +"$NIGHTLY" build -p $1 --target=wasm32-unknown-unknown --release -F nightly -Z build-std=panic_abort,std
cargo run -p wasm_strip --release -- target/wasm32-unknown-unknown/release/$1.wasm
#cargo run -p brotli_check --release -- target/wasm32-unknown-unknown/release/$1.wasm
