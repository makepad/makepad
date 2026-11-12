#include <stdint.h>
#include <stddef.h>
#if defined(__linux__)
#define CN_HIDDEN __attribute__((visibility("hidden")))
#else
#define CN_HIDDEN
#endif
CN_HIDDEN int32_t cn_offset = 7;
CN_HIDDEN int32_t cn_add(int32_t a, int32_t b) { return a + b + cn_offset; }
uint64_t cn_hash(const uint8_t *p, size_t n) {
    uint64_t h = 0xcbf29ce484222325ull;
    for (size_t i = 0; i < n; i++) { h ^= p[i]; h *= 0x100000001b3ull; }
    return h;
}
typedef int32_t (*cn_cb)(int32_t);
int32_t cn_apply(cn_cb f, int32_t x) { return f(x) * 2; }
// C calling back into Rust by name (#1387): its own crate's `#[no_mangle]` fn, as blake3's NEON
// code calls blake3_compress_in_place_portable.
extern int32_t cn_rust_square(int32_t x);
int32_t cn_square_plus(int32_t x) { return cn_rust_square(x) + 1; }
