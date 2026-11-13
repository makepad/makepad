#include <stdint.h>
#include <stddef.h>
#if defined(__linux__) || defined(__APPLE__)
#define CN_HIDDEN __attribute__((visibility("hidden")))
#else
#define CN_HIDDEN
#endif
CN_HIDDEN int32_t cn_offset = CN_OFFSET;
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
// Names a system library the host process already has loaded defines too (macOS libSystem's
// getprogname, glibc's gnu_get_libc_version): a static link binds the program's own definitions,
// so these are what the program calls (relax issue 78: a bundled library ran as the system's).
const char *getprogname(void) { return "cnative"; }
const char *gnu_get_libc_version(void) { return "cnative"; }
