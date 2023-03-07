Makepad android quick build example for macos:

git clone https://github.com/makepad/makepad
cd makepad
git checkout 56cde36fa53d2d606a8638fe047178cdb483acb7
cargo run -p cargo-makepad --release -- android rustup-toolchain-install
cargo run -p cargo-makepad --release -- android install-sdk
cargo run -p cargo-makepad --release -- android run -p makepad-example-ironfish
