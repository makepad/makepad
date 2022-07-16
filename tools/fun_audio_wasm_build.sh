RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--export=__stack_pointer" cargo build -p fun_audio --target=wasm32-unknown-unknown -Z build-std=panic_abort,std --release
cargo run -p wasm_strip --release -- target/wasm32-unknown-unknown/release/fun_audio.wasm

