cargo build -p makepad-web-server --release
sudo setcap 'cap_net_bind_service=+ep' target/release/makepad-web-server
target/release/makepad-web-server

