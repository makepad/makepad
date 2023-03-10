Makepad android quick build example for Macos, Linux-x64 and Windows:

git clone https://github.com/makepad/makepad

cd makepad

git checkout db3c2a6c98f108a47be7f0a29a8ba244f3e6a68e

cargo run -p cargo-makepad --release -- android toolchain-install

cargo run -p cargo-makepad --release -- android run -p makepad-example-ironfish