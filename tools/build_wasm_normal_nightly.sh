RUSTFLAGS="-C linker-plugin-lto -C embed-bitcode=yes -C 
codegen-units=1 -C opt-level=z" cargo +nightly build -p $1 --target=wasm32-unknown-unknown --release 
cargo +nightly run -p wasm_strip --release -- target/wasm32-unknown-unknown/release/$1.wasm
