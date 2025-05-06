
rustup target remove x86_64-pc-windows-gnu &>/dev/null    
rustup target remove x86_64-pc-windows-msvc &>/dev/null   
rustup target remove wasm32-unknown-unknown &>/dev/null   
rustup target remove x86_64-unknown-linux-gnu  &>/dev/null
rustup target remove x86_64-apple-darwin  &>/dev/null 
rustup target remove x86_64-apple-darwin  &>/dev/null 
rustup target remove aarch64-apple-darwin  &>/dev/null
rustup target remove aarch64-linux-android &>/dev/null

rustup target add x86_64-pc-windows-gnu &>/dev/null
rustup target add x86_64-pc-windows-msvc &>/dev/null
rustup target add wasm32-unknown-unknown &>/dev/null
rustup target add x86_64-unknown-linux-gnu  &>/dev/null
rustup target add x86_64-apple-darwin  &>/dev/null
rustup target add x86_64-apple-darwin  &>/dev/null
rustup target add aarch64-apple-darwin  &>/dev/null
rustup target add aarch64-linux-android &>/dev/null

rustup target add x86_64-pc-windows-gnu --toolchain nightly &>/dev/null
rustup target add x86_64-pc-windows-msvc --toolchain nightly &>/dev/null
rustup target add wasm32-unknown-unknown --toolchain nightly &>/dev/null
rustup target add x86_64-unknown-linux-gnu --toolchain nightly &>/dev/null
rustup target add x86_64-apple-darwin --toolchain nightly &>/dev/null
rustup target add x86_64-apple-darwin --toolchain nightly &>/dev/null
rustup target add aarch64-apple-darwin --toolchain nightly &>/dev/null
rustup target add aarch64-linux-android --toolchain nightly &>/dev/null
