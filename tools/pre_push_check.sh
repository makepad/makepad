#echo "Updating rust"
#rustup update &>/dev/null
#rustup target add x86_64-pc-windows-gnu &>/dev/null
#rustup target add x86_64-pc-windows-msvc &>/dev/null
#rustup target add wasm32-unknown-unknown &>/dev/null
#rustup target add x86_64-unknown-linux-gnu  &>/dev/null
#rustup target add x86_64-apple-darwin  &>/dev/null
#rustup target add x86_64-apple-darwin  &>/dev/null
#rustup target add aarch64-apple-darwin  &>/dev/null
#rustup target add aarch64-linux-android &>/dev/null

#rustup target add x86_64-pc-windows-gnu --toolchain nightly &>/dev/null
#rustup target add x86_64-pc-windows-msvc --toolchain nightly &>/dev/null
#rustup target add wasm32-unknown-unknown --toolchain nightly &>/dev/null
#rustup target add x86_64-unknown-linux-gnu --toolchain nightly &>/dev/null
#rustup target add x86_64-apple-darwin --toolchain nightly &>/dev/null
#rustup target add x86_64-apple-darwin --toolchain nightly &>/dev/null
#rustup target add aarch64-apple-darwin --toolchain nightly &>/dev/null
#rustup target add aarch64-linux-android --toolchain nightly &>/dev/null

echo "Checking all examples"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-fractal-zoom --release --message-format=json
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --message-format=json
MAKEPAD=lines cargo +nightly check -q -p makepad-example-numbers --release --message-format=json
MAKEPAD=lines cargo +nightly check -q -p makepad-example-simple --release --message-format=json
MAKEPAD=lines cargo +nightly check -q -p makepad-example-chatgpt --release --message-format=json
MAKEPAD=lines cargo +nightly check -q -p makepad-example-news-feed --release --message-format=json
MAKEPAD=lines cargo +nightly check -q -p makepad-example-sdxl --release --message-format=json
MAKEPAD=lines cargo +nightly check -q -p makepad-studio --release --message-format=json

echo "Checking Windows GNU stable"
cargo +stable check -q -p makepad-example-ironfish --release --target=x86_64-pc-windows-gnu --message-format=json
echo "Checking Windows MSVC stable"
cargo +stable check -q -p makepad-example-ironfish --release --target=x86_64-pc-windows-msvc --message-format=json
echo "Checking Linux X11 stable"
cargo +stable check -q -p makepad-example-ironfish --release --target=x86_64-unknown-linux-gnu --message-format=json
echo "Checking Linux Direct stable"
MAKEPAD=linux_direct cargo +stable check -q -p makepad-example-ironfish --release --target=x86_64-unknown-linux-gnu --message-format=json
echo "Checking Apple macos intel stable"
cargo +stable check -q -p makepad-example-ironfish --release --target=x86_64-apple-darwin --message-format=json
echo "Checking Apple macos arm stable"
cargo +stable check -q -p makepad-example-ironfish --release --target=aarch64-apple-darwin --message-format=json
echo "Checking Wasm stable"
cargo +stable check -q -p makepad-example-ironfish --release --target=wasm32-unknown-unknown --message-format=json
echo "Checking android stable"
cargo +stable check --lib -q -p makepad-example-ironfish --release --target=aarch64-linux-android --message-format=json

echo "Checking Windows GNU nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=x86_64-pc-windows-gnu --message-format=json
echo "Checking Windows MSVC nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=x86_64-pc-windows-msvc --message-format=json
echo "Checking Linux X11 nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=x86_64-unknown-linux-gnu --message-format=json
echo "Checking Linux Direct stable"
MAKEPAD=lines,linux_direct cargo +nightly check -q -p makepad-example-ironfish --release --target=x86_64-unknown-linux-gnu --message-format=json
echo "Checking Apple macos intel nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=x86_64-apple-darwin --message-format=json
echo "Checking Apple macos  intel nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=aarch64-apple-darwin --message-format=json
echo "Checking Wasm nightly"
MAKEPAD=lines cargo +nightly check -q -p makepad-example-ironfish --release --target=wasm32-unknown-unknown --message-format=json
echo "Checking android nightly"
MAKEPAD=lines cargo +nightly check --lib -q -p makepad-example-ironfish --release --target=aarch64-linux-android --message-format=json
