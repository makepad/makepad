NIGHTLY=${1:-nightly}
rustup update
rustup toolchain install "$NIGHTLY"
rustup target add wasm32-unknown-unknown --toolchain "$NIGHTLY"
rustup target add wasm32-unknown-unknown --toolchain stable
rustup component add rust-src --toolchain stable
rustup component add rust-src --toolchain "$NIGHTLY"
echo "----- Building stable builds -----"
cargo +stable build -p fun_audio
cargo +stable build -p fractal_zoom
cargo +stable build -p makepad_studio
echo "----- Building nightly builds -----"
cargo +"$NIGHTLY" build -F nightly -p fun_audio
cargo +"$NIGHTLY" build -F nightly -p fractal_zoom
cargo +"$NIGHTLY" build -F nightly -p makepad_studio
echo "----- Building makepad studio normal -----"
./tools/build_wasm_normal.sh makepad_studio "$NIGHTLY"
echo "----- Building fractal zoom threaded -----"
./tools/build_wasm_thread.sh fractal_zoom "$NIGHTLY"
echo "----- Building fractal zoom SIMD -----"
./tools/build_wasm_simd.sh fractal_zoom "$NIGHTLY"
echo "----- Building fun audio threaded -----"
./tools/build_wasm_thread.sh fun_audio "$NIGHTLY"

