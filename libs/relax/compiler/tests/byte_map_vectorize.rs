// Differential regression for loops.rs::vectorize_byte_maps. Run with rustc -O --test
// --edition 2021, and Relax at O1 and MAKEPAD_RUN_O3=1 MAKEPAD_RUN_O3_CALLS=100.
// Repeated sweeps keep calling every kernel while asynchronous O3 is installed.
// Hash the entire allocation (including untouched prefixes, suffixes and zip tails).

#[inline(never)]
fn add_pairs(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((x, y), out) in a.iter().zip(b.iter()).zip(dst.iter_mut()) {
        *out = x.wrapping_add(*y);
    }
}

#[inline(never)]
fn add_nested(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((out, x), y) in dst.iter_mut().zip(a.iter()).zip(b.iter()) {
        *out = x.wrapping_add(*y);
    }
}

#[inline(never)]
fn sub_pairs(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((x, y), out) in a.iter().zip(b.iter()).zip(dst.iter_mut()) {
        *out = x.wrapping_sub(*y);
    }
}

#[inline(never)]
fn sub_nested(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((out, x), y) in dst.iter_mut().zip(a.iter()).zip(b.iter()) {
        *out = x.wrapping_sub(*y);
    }
}

#[inline(never)]
fn and_pairs(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((x, y), out) in a.iter().zip(b.iter()).zip(dst.iter_mut()) {
        *out = *x & *y;
    }
}

#[inline(never)]
fn and_nested(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((out, x), y) in dst.iter_mut().zip(a.iter()).zip(b.iter()) {
        *out = *x & *y;
    }
}

#[inline(never)]
fn or_pairs(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((x, y), out) in a.iter().zip(b.iter()).zip(dst.iter_mut()) {
        *out = *x | *y;
    }
}

#[inline(never)]
fn or_nested(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((out, x), y) in dst.iter_mut().zip(a.iter()).zip(b.iter()) {
        *out = *x | *y;
    }
}

#[inline(never)]
fn xor_pairs(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((x, y), out) in a.iter().zip(b.iter()).zip(dst.iter_mut()) {
        *out = *x ^ *y;
    }
}

#[inline(never)]
fn xor_nested(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((out, x), y) in dst.iter_mut().zip(a.iter()).zip(b.iter()) {
        *out = *x ^ *y;
    }
}

#[inline(never)]
fn up(cur: &mut [u8], prev: &[u8]) {
    for (out, x) in cur.iter_mut().zip(prev.iter()) {
        *out = out.wrapping_add(*x);
    }
}

#[inline(never)]
fn sub_filter(cur: &mut [u8], bpp: usize) {
    for i in bpp..cur.len() {
        cur[i] = cur[i].wrapping_add(cur[i - bpp]);
    }
}

#[inline(never)]
fn copy_zip(dst: &mut [u8], src: &[u8]) {
    for (out, x) in dst.iter_mut().zip(src.iter()) {
        *out = *x;
    }
}

#[inline(never)]
fn constants(dst: &mut [u8], a: &[u8], b: &[u8]) {
    for ((out, x), y) in dst.iter_mut().zip(a.iter()).zip(b.iter()) {
        *out = (x.wrapping_add(*y) ^ 0xa5).wrapping_sub(37) | 3;
    }
}

#[inline(never)]
fn live_out(dst: &mut [u8], a: &[u8], b: &[u8]) -> u8 {
    let mut last = 0x69;
    for ((out, x), y) in dst.iter_mut().zip(a.iter()).zip(b.iter()) {
        last = x.wrapping_sub(*y);
        *out = last;
    }
    last
}

// Iterators yield raw pointers, never overlapping Rust references. All pointers
// stay in the one live allocation, even for empty ranges and the final increment.
struct Ptrs {
    ptr: *mut u8,
    left: usize,
}
impl Iterator for Ptrs {
    type Item = *mut u8;
    #[inline(always)]
    fn next(&mut self) -> Option<*mut u8> {
        if self.left == 0 {
            None
        } else {
            let p = self.ptr;
            self.ptr = unsafe { p.add(1) };
            self.left -= 1;
            Some(p)
        }
    }
}

#[inline(never)]
unsafe fn copy_index(dst: *mut u8, src: *mut u8, n: usize) {
    for i in 0..n {
        *dst.add(i) = *src.add(i);
    }
}

#[inline(never)]
unsafe fn copy_ptrs(dst: *mut u8, src: *mut u8, n: usize) {
    for (out, x) in (Ptrs { ptr: dst, left: n }).zip(Ptrs { ptr: src, left: n }) {
        *out = *x;
    }
}

#[inline(never)]
unsafe fn add_index(dst: *mut u8, src: *mut u8, n: usize) {
    for i in 0..n {
        *dst.add(i) = (*dst.add(i)).wrapping_add(*src.add(i));
    }
}

#[inline(never)]
unsafe fn add_ptrs(dst: *mut u8, src: *mut u8, n: usize) {
    for (out, x) in (Ptrs { ptr: dst, left: n }).zip(Ptrs { ptr: src, left: n }) {
        *out = (*out).wrapping_add(*x);
    }
}

// Cell has u8's layout and permits aliased shared references. These are the
// slice-iterator shape of the raw-pointer kernels: they exercise the SIMD alias
// guard without ever constructing overlapping &mut [u8] / &[u8] references.
#[inline(never)]
unsafe fn copy_cells(dst: *mut u8, src: *mut u8, n: usize) {
    let dst = std::slice::from_raw_parts(dst as *const std::cell::Cell<u8>, n);
    let src = std::slice::from_raw_parts(src as *const std::cell::Cell<u8>, n);
    for (out, x) in dst.iter().zip(src.iter()) {
        out.set(x.get());
    }
}

#[inline(never)]
unsafe fn add_cells(dst: *mut u8, src: *mut u8, n: usize) {
    let dst = std::slice::from_raw_parts(dst as *const std::cell::Cell<u8>, n);
    let src = std::slice::from_raw_parts(src as *const std::cell::Cell<u8>, n);
    for (out, x) in dst.iter().zip(src.iter()) {
        out.set(out.get().wrapping_add(x.get()));
    }
}

#[inline(never)]
fn input(n: usize, seed: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for i in 0..n {
        bytes.push((i.wrapping_mul(197) ^ (i >> 3) ^ seed.wrapping_mul(113)) as u8);
    }
    bytes
}

#[inline(never)]
fn hash(mut h: u64, bytes: &[u8]) -> u64 {
    for x in bytes {
        h = (h ^ *x as u64).wrapping_mul(0x100000001b3);
    }
    h
}

// Keep the full case in a separate, sufficiently large function: Relax can inline
// small kernels even with inline(never). Calling this 7,008 times ensures that the
// installed O3 body (including its inlined byte maps) is actually entered.
#[inline(never)]
fn exercise_case(mut h: u64, rep: usize, n: usize, off: usize) -> u64 {
    let a = input(n + 64, rep + 1);
    let b = input(n + 64, rep + 19);
    // Every stream is shortest in turn, including empty/nonempty zips.
    // Rotating independent offsets exercises unaligned loads and stores.
    let ao = (off + 5) % 16;
    let bo = (off + 11) % 16;
    for short in 0..4usize {
        let dn = n + if short == 1 || short == 2 { 7 } else { 0 };
        let an = n + if short == 0 || short == 2 { 5 } else { 0 };
        let bn = n + if short == 0 || short == 1 { 3 } else { 0 };
        let aa = &a[ao..ao + an];
        let bb = &b[bo..bo + bn];
        let mut out = input(n + 64, rep + 37);
        add_pairs(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        add_nested(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        sub_pairs(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        sub_nested(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        and_pairs(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        and_nested(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        or_pairs(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        or_nested(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        xor_pairs(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        xor_nested(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        copy_zip(&mut out[off..off + dn], aa);
        h = hash(h, &out);
        constants(&mut out[off..off + dn], aa, bb);
        h = hash(h, &out);
        let last = live_out(&mut out[off..off + dn], aa, bb);
        h = hash(h ^ last as u64, &out);
        up(&mut out[off..off + dn], aa);
        h = hash(h, &out);
    }
    for bpp in [1usize, 2, 3, 4, 6, 8] {
        let mut out = input(n + 64, rep + bpp);
        sub_filter(&mut out[off..off + n], bpp);
        h = hash(h, &out);
    }
    // These are forward scalar loops, not memmove: dst > src at a
    // distance below 16 must see bytes written earlier in this loop.
    // Include exact alias, both directions, and distances 15/16/17.
    for distance in 0..21usize {
        for direction in 0..2usize {
            let dst = off + if direction == 0 { distance } else { 0 };
            let src = off + if direction == 0 { 0 } else { distance };
            for kernel in 0..6usize {
                let mut out = input(n + 64, rep + 53);
                let p = out.as_mut_ptr();
                unsafe {
                    if kernel == 0 {
                        copy_index(p.add(dst), p.add(src), n);
                    }
                    if kernel == 1 {
                        copy_ptrs(p.add(dst), p.add(src), n);
                    }
                    if kernel == 2 {
                        add_index(p.add(dst), p.add(src), n);
                    }
                    if kernel == 3 {
                        add_ptrs(p.add(dst), p.add(src), n);
                    }
                    if kernel == 4 {
                        copy_cells(p.add(dst), p.add(src), n);
                    }
                    if kernel == 5 {
                        add_cells(p.add(dst), p.add(src), n);
                    }
                }
                h = hash(h, &out);
            }
        }
    }
    h
}

#[test]
fn byte_map_vectorize() {
    let mut h = 0xcbf29ce484222325u64;
    for rep in 0..6usize {
        for size in 0..73usize {
            let n = if size == 71 {
                1000
            } else if size == 72 {
                4099
            } else {
                size
            };
            for off in 0..16usize {
                h = exercise_case(h, rep, n, off);
            }
        }
        println!("| byte_map_vectorize {} {:016x}", rep, h);
    }
    // Optimized rustc oracle, identical on x86_64 and aarch64.
    assert_eq!(h, 0x8ac0df74dbe3aa71);
}
