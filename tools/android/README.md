Makepad android quick build example for Macos, Linux-x64 and Windows.
Please plug an android device set to developer mode into your PC via USB. The toolchain install is completely local and does not change things in your path. After compiling/running you have to give the app some rights otherwise it cant access midi e.d. 

git clone https://github.com/makepad/makepad

cd makepad

git checkout e086a4b61c7bc9ebfc87b50cbbac199dc12ddf1c

cargo run -p cargo-makepad --release -- android toolchain-install

cargo run -p cargo-makepad --release -- android run -p makepad-example-ironfish
