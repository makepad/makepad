git pull
cargo run -p builder -- index .
cargo run -p builder -- build . makepad_wasm small
echo "restart the webserver"
