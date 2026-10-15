// Stack slot colouring must not share a slot whose address a call stored away. deflate's
// radix_sort_symbols (miniz): `hist.iter().enumerate().take(n)` built by a call that is not
// inlined keeps `&hist` in its result; the loop then reads `hist` only through that adapter
// while it fills `offsets` -- colouring took `hist` as dead after the call and gave `offsets`
// the same stack slot (wrong deflate checksum in ~1-2% of x64 runs, whenever the adapter was not
// inlined). rustc prints the same lines.
struct Rows<'a> {
    h: &'a [[u64; 256]; 2],
    i: usize,
    n: usize,
}

#[inline(never)]
fn rows<'a>(h: &'a [[u64; 256]; 2], n: usize) -> Rows<'a> {
    Rows { h, i: 0, n }
}

impl<'a> Iterator for Rows<'a> {
    type Item = (usize, &'a [u64; 256]);
    #[inline(never)]
    fn next(&mut self) -> Option<Self::Item> {
        if self.i >= self.n {
            return None;
        }
        self.i += 1;
        Some((self.i - 1, &self.h[self.i - 1]))
    }
}

fn sort(keys: &[u16]) -> Vec<u16> {
    let mut hist = [[0u64; 256]; 2];
    for k in keys {
        hist[0][(k & 0xff) as usize] += 1;
        hist[1][(k >> 8) as usize] += 1;
    }
    let mut cur = keys.to_vec();
    let mut new = vec![0u16; keys.len()];
    // through a function pointer: never inlined, whatever the inliner decides
    let make: fn(&[[u64; 256]; 2], usize) -> Rows = std::hint::black_box(rows);
    for (pass, h) in make(&hist, 2) {
        let mut offsets = [0u64; 256];
        let mut o = 0;
        for i in 0..256 {
            offsets[i] = o;
            o += h[i];
        }
        for k in cur.iter() {
            let j = ((k >> (pass * 8)) & 0xff) as usize;
            new[offsets[j] as usize] = *k;
            offsets[j] += 1;
        }
        std::mem::swap(&mut cur, &mut new);
    }
    cur
}

#[test]
fn slot_color_escape() {
    let keys: Vec<u16> = (0..600u32).map(|i| ((i * 7919 + 13) % 65521) as u16).collect();
    let s = sort(&keys);
    let mut want = keys.clone();
    want.sort();
    println!("| sorted {} first {:?} last {:?}", s == want, &s[..4], &s[s.len() - 4..]);
    assert_eq!(s, want);
}
