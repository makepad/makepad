RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--export=__stack_pointer" cargo build -p layout_example --target=wasm32-unknown-unknown -Z build-std=panic_abort,std
