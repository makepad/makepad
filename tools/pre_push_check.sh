rustup target add x86_64-pc-windows-gnu
rustup target add x86_64-pc-windows-msvc
rustup target add wasm32-unknown-unknown
rustup target add x86_64-unknown-linux-gnu
rustup target add x86_64-apple-darwin
cd ..
echo "Checking Windows GNU"
cargo check --release --target=x86_64-pc-windows-gnu
echo "Checking Windows MSVC"
cargo check --release --target=x86_64-pc-windows-msvc
echo "Checking Linux"
cargo check --release --target=x86_64-unknown-linux-gnu
echo "Checking Apple"
cargo check --release --target=x86_64-apple-darwin
echo "Checking Wasm"
cargo check --release --target=wasm32-unknown-unknown --manifest-path="./makepad/wasm/Cargo.toml"
