cls
set RUSTFLAGS=-Ctarget-feature=+avx,+avx2,+sse3,+fma
cargo run --release --manifest-path=example\Cargo.toml
