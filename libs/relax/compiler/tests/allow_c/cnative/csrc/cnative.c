#include <stdint.h>
#include <stddef.h>
int32_t cn_add(int32_t a, int32_t b) { return a + b; }
uint64_t cn_hash(const uint8_t *p, size_t n) {
    uint64_t h = 0xcbf29ce484222325ull;
    for (size_t i = 0; i < n; i++) { h ^= p[i]; h *= 0x100000001b3ull; }
    return h;
}
typedef int32_t (*cn_cb)(int32_t);
int32_t cn_apply(cn_cb f, int32_t x) { return f(x) * 2; }

/* Kept out of the dynamic symbol table, as `-fvisibility=hidden` builds (ring's whole C core) and
 * `.private_extern` assembly are: the JIT must still reach it through the library's symbol table. */
__attribute__((visibility("hidden"))) int cn_hidden_mul(int a, int b) { return a * b; }
