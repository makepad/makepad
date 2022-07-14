cargo build -p webserver --release
sudo setcap 'cap_net_bind_service=+ep' target/release/webserver
target/release/webserver

