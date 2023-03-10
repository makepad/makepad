Makepad android quick build example for Macos, Linux-x64 and Windows.
Please plug an android device set to developer mode into your PC via USB. The toolchain install is completely local and does not change things in your path:

git clone https://github.com/makepad/makepad

cd makepad

git checkout db3c2a6c98f108a47be7f0a29a8ba244f3e6a68e

cargo run -p cargo-makepad --release -- android toolchain-install

cargo run -p cargo-makepad --release -- android run -p makepad-example-ironfish
