# Usage: tools/llvm_lines.sh <package> [binary]
CARGO_PROFILE_RELEASE_LTO=fat cargo llvm-lines --release -p "${1:?package}" --bin "${2:-$1}"

