RUSTFLAGS="-C opt-level=z -C panic=abort -C codegen-units=1" cargo build -p makepad_studio --release --target=wasm32-unknown-unknown
cargo run -p wasm_strip --release -- target/wasm32-unknown-unknown/release/makepad_studio.wasm

