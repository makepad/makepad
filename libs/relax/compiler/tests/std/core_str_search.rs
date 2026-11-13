// String and byte searching across random, periodic, Unicode and aligned inputs.
// Differential file: `| ` lines equal under rustc --test and makepad test.

struct D(u64, u32);
impl D {
    fn new() -> D {
        D(0xcbf29ce484222325, 0)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn add<T: std::fmt::Debug + ?Sized>(&mut self, value: &T) {
        use std::fmt::Write;
        write!(self, "{:?};", value).unwrap();
        self.1 += 1;
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}
impl std::fmt::Write for D {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

fn check(h: &str, n: &str, d: &mut D) {
    d.add(&h.find(n));
    d.add(&h.rfind(n));
    d.add(&h.contains(n));
    d.add(&h.match_indices(n).collect::<Vec<_>>());
    d.add(&h.rmatch_indices(n).collect::<Vec<_>>());
    d.add(&h.split(n).collect::<Vec<_>>());
    d.add(&h.rsplit(n).collect::<Vec<_>>());
    d.add(&h.split_terminator(n).collect::<Vec<_>>());
    d.add(&h.rsplit_terminator(n).collect::<Vec<_>>());
    for count in 0..4 {
        d.add(&h.splitn(count, n).collect::<Vec<_>>());
        d.add(&h.rsplitn(count, n).collect::<Vec<_>>());
    }
    d.add(&h.replace(n, "!"));
    for matches in [h.match_indices(n).collect::<Vec<_>>(), h.rmatch_indices(n).collect()] {
        for &(start, matched) in &matches {
            assert!(h.is_char_boundary(start) && h.is_char_boundary(start + matched.len()));
        }
        d.add(&matches);
    }
}

fn rng(s: &mut u64) -> usize {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s as usize
}

#[test]
fn a01_random_and_periodic() {
    let mut seed = 0x983abc12u64;
    for alphabet in [vec!['a', 'b', 'c'], vec!['a', 'é', 'ê', '🦀']] {
        let mut d = D::new();
        for _ in 0..1_000 {
            let len = rng(&mut seed) % 180;
            let h: String = (0..len).map(|_| alphabet[rng(&mut seed) % alphabet.len()]).collect();
            let len = rng(&mut seed) % 30;
            let n: String = (0..len).map(|_| alphabet[rng(&mut seed) % alphabet.len()]).collect();
            check(&h, &n, &mut d);
            let boundaries: Vec<_> = h.char_indices().map(|(i, _)| i)
                .chain(std::iter::once(h.len())).collect();
            let a = rng(&mut seed) % boundaries.len();
            let b = a + rng(&mut seed) % (boundaries.len() - a);
            check(&h, &h[boundaries[a]..boundaries[b]], &mut d);
            for &c in &alphabet {
                d.add(&h.find(c));
                d.add(&h.rfind(c));
                d.add(&h.match_indices(c).collect::<Vec<_>>());
                d.add(&h.rmatch_indices(c).collect::<Vec<_>>());
            }
        }
        d.line(&format!("search random alphabet {}", alphabet.len()));
    }
    let mut d = D::new();
    for unit in ["a", "abab", "acba", "éê", "🦀a"] {
        let h = format!("{}z{}", unit.repeat(100), unit.repeat(80));
        for n in [
            String::new(), unit.to_string(), unit.repeat(40),
            format!("{}z", unit.repeat(40)), unit.repeat(300),
        ] {
            check(&h, &n, &mut d);
        }
    }
    check("", "", &mut d);
    check("", "long", &mut d);
    d.line("search periodic");
}

fn position(d: &mut D, value: Option<usize>) {
    match value {
        None => d.bytes(&[0]),
        Some(value) => {
            d.bytes(&[1]);
            d.bytes(&value.to_le_bytes());
        }
    }
    d.1 += 1;
}

#[test]
fn a02_byte_search_all_alignments() {
    let mut seed = 17;
    let mut bytes = vec![0u8; 400];
    for b in &mut bytes {
        *b = rng(&mut seed) as u8;
    }
    let mut d = D::new();
    for start in 0..32 {
        for len in 0..256 {
            let text = &bytes[start..start + len];
            for x in 0..=255 {
                position(&mut d, text.iter().position(|b| *b == x));
                position(&mut d, text.iter().rposition(|b| *b == x));
            }
        }
    }
    d.line("search bytes all alignments");
}

#[test]
fn a03_rejection_steps_and_independent_cursors() {
    let mut d = D::new();
    for (h, n) in [("zzzzababzzzz", "abab"), ("xxxxabcdxxxx", "abcd"), ("ééababéé", "abab")] {
        let mut forward = h.match_indices(n);
        let mut backward = h.rmatch_indices(n);
        assert_eq!(forward.next(), Some((4, n)));
        assert_eq!(forward.next(), None);
        assert_eq!(forward.next(), None);
        assert_eq!(backward.next(), Some((4, n)));
        assert_eq!(backward.next(), None);
        assert_eq!(backward.next(), None);
        check(h, n, &mut d);
    }
    let mut matches = "zzababx".match_indices("abab");
    assert_eq!(matches.next(), Some((2, "abab")));
    assert_eq!(matches.next(), None);
    assert_eq!("éa".match_indices("").collect::<Vec<_>>(), vec![(0, ""), (2, ""), (3, "")]);
    let mut forward = "".match_indices("");
    let mut backward = "".rmatch_indices("");
    assert_eq!(forward.next(), Some((0, "")));
    assert_eq!(backward.next(), Some((0, "")));
    assert_eq!(forward.next(), None);
    assert_eq!(backward.next(), None);
    check("zzababx", "abab", &mut d);
    check("éa", "", &mut d);
    check("", "", &mut d);
    d.line("search cursor cases");
}

