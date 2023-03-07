Makepad android quick build example for macos:

git clone https://github.com/makepad/makepad

cd makepad

git checkout 970d19e1f4ecfb9a3df47a9df02b61346bbad3b4

cargo run -p cargo-makepad --release -- android rustup-toolchain-install

cargo run -p cargo-makepad --release -- android install-sdk

cargo run -p cargo-makepad --release -- android run -p makepad-example-ironfish
