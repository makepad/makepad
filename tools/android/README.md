Makepad android quick build example for macos:

git clone https://github.com/makepad/makepad

cd makepad

git checkout 994ba35ed838e23ecd6c09a18410aaa74c142c8f

cargo run -p cargo-makepad --release -- android rustup-toolchain-install

cargo run -p cargo-makepad --release -- android install-sdk

cargo run -p cargo-makepad --release -- android run -p makepad-example-ironfish
