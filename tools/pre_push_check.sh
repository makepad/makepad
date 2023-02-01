rustup update
rustup target add x86_64-pc-windows-gnu
rustup target add x86_64-pc-windows-msvc
rustup target add wasm32-unknown-unknown
rustup target add x86_64-unknown-linux-gnu
rustup target add x86_64-apple-darwin
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin

rustup target add x86_64-pc-windows-gnu --toolchain nightly
rustup target add x86_64-pc-windows-msvc --toolchain nightly
rustup target add wasm32-unknown-unknown --toolchain nightly
rustup target add x86_64-unknown-linux-gnu --toolchain nightly
rustup target add x86_64-apple-darwin --toolchain nightly
rustup target add x86_64-apple-darwin --toolchain nightly
rustup target add aarch64-apple-darwin --toolchain nightly

echo "Checking Windows GNU stable"
cargo check -p makepad-example-ironfish --release --target=x86_64-pc-windows-gnu
echo "Checking Windows MSVC stable"
cargo check -p makepad-example-ironfish --release --target=x86_64-pc-windows-msvc
echo "Checking Linux X11 stable"
cargo check -p makepad-example-ironfish --release --target=x86_64-unknown-linux-gnu
echo "Checking Apple macos intel stable"
cargo check -p makepad-example-ironfish --release --target=x86_64-apple-darwin
echo "Checking Apple macos arm stable"
cargo check -p makepad-example-ironfish --release --target=aarch64-apple-darwin
echo "Checking Wasm stable"
cargo check -p makepad-example-ironfish --release --target=wasm32-unknown-unknown


echo "Checking Windows GNU nightly"
MAKEPAD=lines cargo +nightly check -p makepad-example-ironfish --release --target=x86_64-pc-windows-gnu
echo "Checking Windows MSVC nightly"
MAKEPAD=lines cargo +nightly check -p makepad-example-ironfish --release --target=x86_64-pc-windows-msvc
echo "Checking Linux X11 nightly"
MAKEPAD=lines cargo +nightly check -p makepad-example-ironfish --release --target=x86_64-unknown-linux-gnu
echo "Checking Apple macos intel nightly"
MAKEPAD=lines cargo +nightly check -p makepad-example-ironfish --release --target=x86_64-apple-darwin
echo "Checking Apple macos  intel nightly"
MAKEPAD=lines cargo +nightly check -p makepad-example-ironfish --release --target=aarch64-apple-darwin
echo "Checking Wasm nightly"
MAKEPAD=lines cargo +nightly check -p makepad-example-ironfish --release --target=wasm32-unknown-unknown
