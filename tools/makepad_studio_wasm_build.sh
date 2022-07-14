RUSTFLAGS="-C opt-level=z -C panic=abort -C codegen-units=1" cargo build -p makepad_studio --release --target=wasm32-unknown-unknown
