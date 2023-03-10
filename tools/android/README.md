Makepad android quick build example for Macos, Linux-x64 and Windows:

git clone https://github.com/makepad/makepad

cd makepad

git checkout 9e5f93839ead80335c726739b779ce773718a2b2

cargo run -p cargo-makepad --release -- android toolchain-install

cargo run -p cargo-makepad --release -- android run -p makepad-example-ironfish