// Scalar SHA-256 reference formerly in Relax core::arch. Test code only.
// (dead_code allowed by the including module)

pub fn _mm_sha256msg1_epu32(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let w = [a[0], a[1], a[2], a[3], b[0]];
    let mut r = [0u32; 4];
    let mut i = 0;
    while i < 4 {
        let x = w[i + 1];
        let s0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
        r[i] = w[i].wrapping_add(s0);
        i += 1;
    }
    r
}

pub fn _mm_sha256msg2_epu32(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let mut w = [b[2], b[3], 0, 0, 0, 0];
    let mut i = 0;
    while i < 4 {
        let x = w[i];
        let s1 = x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10);
        w[i + 2] = a[i].wrapping_add(s1);
        i += 1;
    }
    [w[2], w[3], w[4], w[5]]
}

pub fn _mm_sha256rnds2_epu32(a: [u32; 4], b: [u32; 4], k: [u32; 4]) -> [u32; 4] {
    let mut sa = b[3];
    let mut sb = b[2];
    let mut sc = a[3];
    let mut sd = a[2];
    let mut se = b[1];
    let mut sf = b[0];
    let mut sg = a[1];
    let mut sh = a[0];
    let mut i = 0;
    while i < 2 {
        let ch = (se & sf) ^ (!se & sg);
        let maj = (sa & sb) ^ (sa & sc) ^ (sb & sc);
        let s0 = sa.rotate_right(2) ^ sa.rotate_right(13) ^ sa.rotate_right(22);
        let s1 = se.rotate_right(6) ^ se.rotate_right(11) ^ se.rotate_right(25);
        let t1 = sh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(k[i]);
        let t2 = s0.wrapping_add(maj);
        sh = sg;
        sg = sf;
        sf = se;
        se = sd.wrapping_add(t1);
        sd = sc;
        sc = sb;
        sb = sa;
        sa = t1.wrapping_add(t2);
        i += 1;
    }
    [sf, se, sb, sa]
}

fn sha256_hash(x: [u32; 4], y: [u32; 4], w: [u32; 4], part1: bool) -> [u32; 4] {
    let (mut x, mut y) = (x, y);
    let mut e = 0;
    while e < 4 {
        let chs = (y[0] & y[1]) ^ (!y[0] & y[2]);
        let maj = (x[0] & x[1]) | ((x[0] | x[1]) & x[2]);
        let sigma1 = y[0].rotate_right(6) ^ y[0].rotate_right(11) ^ y[0].rotate_right(25);
        let sigma0 = x[0].rotate_right(2) ^ x[0].rotate_right(13) ^ x[0].rotate_right(22);
        let t1 = y[3].wrapping_add(sigma1).wrapping_add(chs).wrapping_add(w[e]);
        let new_e = t1.wrapping_add(x[3]);
        let new_a = t1.wrapping_add(sigma0).wrapping_add(maj);
        // ROL(Y:X, 32): X's top element moves into Y's bottom, Y's top into X's bottom
        x = [new_a, x[0], x[1], x[2]];
        y = [new_e, y[0], y[1], y[2]];
        e += 1;
    }
    if part1 { x } else { y }
}

pub fn vsha256hq_u32(hash_abcd: [u32; 4], hash_efgh: [u32; 4], wk: [u32; 4]) -> [u32; 4] {
    sha256_hash(hash_abcd, hash_efgh, wk, true)
}

pub fn vsha256h2q_u32(hash_efgh: [u32; 4], hash_abcd: [u32; 4], wk: [u32; 4]) -> [u32; 4] {
    sha256_hash(hash_abcd, hash_efgh, wk, false)
}

pub fn vsha256su0q_u32(w0_3: [u32; 4], w4_7: [u32; 4]) -> [u32; 4] {
    let t = [w0_3[1], w0_3[2], w0_3[3], w4_7[0]];
    let mut r = [0u32; 4];
    let mut e = 0;
    while e < 4 {
        let elt = t[e].rotate_right(7) ^ t[e].rotate_right(18) ^ (t[e] >> 3);
        r[e] = elt.wrapping_add(w0_3[e]);
        e += 1;
    }
    r
}

pub fn vsha256su1q_u32(tw0_3: [u32; 4], w8_11: [u32; 4], w12_15: [u32; 4]) -> [u32; 4] {
    let t0 = [w8_11[1], w8_11[2], w8_11[3], w12_15[0]];
    let sig = |v: u32| v.rotate_right(17) ^ v.rotate_right(19) ^ (v >> 10);
    let mut r = [0u32; 4];
    let t1 = [w12_15[2], w12_15[3]];
    let mut e = 0;
    while e < 2 {
        r[e] = sig(t1[e]).wrapping_add(tw0_3[e]).wrapping_add(t0[e]);
        e += 1;
    }
    while e < 4 {
        r[e] = sig(r[e - 2]).wrapping_add(tw0_3[e]).wrapping_add(t0[e]);
        e += 1;
    }
    r
}
