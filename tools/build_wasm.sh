RUSTFLAGS="-C linker-plugin-lto -C embed-bitcode=yes -C lto=yes -C 
codegen-units=1 -C opt-level=z" cargo build -p $1 --target=wasm32-unknown-unknown --release 
cargo run -p wasm_strip --release -- target/wasm32-unknown-unknown/release/$1.wasm
cargo run -p brotli_check --release -- target/wasm32-unknown-unknown/release/$1.wasm
