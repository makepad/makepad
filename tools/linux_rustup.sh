rustup update
rustup target add wasm32-unknown-unknown
# Try to install deps for Debian based if fails try to install on Fedora based
sudo apt install libxcursor-dev libx11-dev libgl1-mesa-dev || sudo dnf install libXcursor-devel libX11-devel mesa-libGL-devel
