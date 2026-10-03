// Fixed tier-0 benchmark kernels (beside libs/math deterministic.rs tests).
// Each test does a fixed amount of work and checks a checksum also produced by rustc.

#[derive(Clone, Copy)]
struct V2 {
    x: f32,
    y: f32,
}

impl V2 {
    fn new(x: f32, y: f32) -> V2 {
        V2 { x, y }
    }
    fn add(self, o: V2) -> V2 {
        V2 { x: self.x + o.x, y: self.y + o.y }
    }
    fn scale(self, s: f32) -> V2 {
        V2 { x: self.x * s, y: self.y * s }
    }
    fn dot(self, o: V2) -> f32 {
        self.x * o.x + self.y * o.y
    }
}

/// live_id-style string hash over a byte slice
fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    let mut x = seed;
    let mut i = 0;
    while i < bytes.len() {
        x = x.wrapping_add(bytes[i] as u64);
        x ^= x >> 32;
        x = x.wrapping_mul(0xd6e8_feb8_6659_fd93);
        x ^= x >> 32;
        i += 1;
    }
    x & 0x0000_3fff_ffff_ffff
}

/// particle step: small structs by value
fn vec_step(p: &mut [V2; 64], v: &mut [V2; 64], dt: f32) -> f32 {
    let g = V2::new(0.0, -9.81);
    let mut e = 0.0f32;
    for i in 0..64 {
        v[i] = v[i].add(g.scale(dt));
        p[i] = p[i].add(v[i].scale(dt));
        if p[i].y < 0.0 {
            p[i].y = -p[i].y;
            v[i].y = -v[i].y * 0.9;
        }
        e += v[i].dot(v[i]);
    }
    e
}

/// insertion sort of a fixed array (bounds-checked indexing in nested loops)
fn sort_ints(a: &mut [i32; 256]) {
    let mut i = 1;
    while i < 256 {
        let k = a[i];
        let mut j = i;
        while j > 0 && a[j - 1] > k {
            a[j] = a[j - 1];
            j -= 1;
        }
        a[j] = k;
        i += 1;
    }
}

#[test]
fn bench_hash() {
    let text = b"the quick brown fox jumps over the lazy dog; widgets, draw, platform, script";
    let mut h = 0u64;
    for r in 0..200000u64 {
        h ^= hash_bytes(r, text);
    }
    assert_eq!(h, 45583272550279);
}

#[test]
fn bench_vec2() {
    let mut p = [V2::new(0.0, 0.0); 64];
    let mut v = [V2::new(0.0, 0.0); 64];
    for i in 0..64 {
        p[i] = V2::new(i as f32, 10.0 + i as f32 * 0.5);
        v[i] = V2::new(1.0, 0.0);
    }
    let mut e = 0.0f32;
    for _ in 0..40000 {
        e = vec_step(&mut p, &mut v, 0.001);
    }
    assert!(e.to_bits() == 1136748848, "e = {} bits {}", e, e.to_bits());
}

#[test]
fn bench_sort() {
    let mut sum = 0i64;
    for r in 0..2000 {
        let mut a = [0i32; 256];
        let mut s = r as u32 * 2654435761u32;
        for i in 0..256 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            a[i] = (s >> 8) as i32 % 10000;
        }
        sort_ints(&mut a);
        sum += a[0] as i64 + a[128] as i64 + a[255] as i64;
    }
    assert_eq!(sum, 30041520);
}
