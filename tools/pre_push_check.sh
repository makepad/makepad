echo "Updating rust"
rustup update &>/dev/null
rustup target add x86_64-pc-windows-gnu &>/dev/null
rustup target add x86_64-pc-windows-msvc &>/dev/null
rustup target add wasm32-unknown-unknown &>/dev/null
rustup target add x86_64-unknown-linux-gnu  &>/dev/null
rustup target add x86_64-apple-darwin  &>/dev/null
rustup target add x86_64-apple-darwin  &>/dev/null
rustup target add aarch64-apple-darwin  &>/dev/null

rustup target add x86_64-pc-windows-gnu --toolchain nightly  &>/dev/null
rustup target add x86_64-pc-windows-msvc --toolchain nightly  &>/dev/null
rustup target add wasm32-unknown-unknown --toolchain nightly  &>/dev/null
rustup target add x86_64-unknown-linux-gnu --toolchain nightly  &>/dev/null
rustup target add x86_64-apple-darwin --toolchain nightly  &>/dev/null
rustup target add x86_64-apple-darwin --toolchain nightly  &>/dev/null
rustup target add aarch64-apple-darwin --toolchain nightly  &>/dev/null

echo "Checking Windows GNU stable"
cargo check -q -p makepad-example-ironfish --release --target=x86_64-pc-windows-gnu
echo "Checking Windows MSVC stable"
cargo check -q -p makepad-example-ironfish --release --target=x86_64-pc-windows-msvc
echo "Checking Linux X11 stable"
cargo check -q -p makepad-example-ironfish --release --target=x86_64-unknown-linux-gnu
echo "Checking Apple macos intel stable"
cargo check -q -p makepad-example-ironfish --release --target=x86_64-apple-darwin
echo "Checking Apple macos arm stable"
cargo check -q -p makepad-example-ironfish --release --target=aarch64-apple-darwin
echo "Checking Wasm stable"
cargo check -q -p makepad-example-ironfish --release --target=wasm32-unknown-unknown


echo "Checking Windows GNU nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=x86_64-pc-windows-gnu
echo "Checking Windows MSVC nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=x86_64-pc-windows-msvc
echo "Checking Linux X11 nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=x86_64-unknown-linux-gnu
echo "Checking Apple macos intel nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=x86_64-apple-darwin
echo "Checking Apple macos  intel nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=aarch64-apple-darwin
echo "Checking Wasm nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=wasm32-unknown-unknown
