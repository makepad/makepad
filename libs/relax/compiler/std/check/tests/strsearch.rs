use relax_std_check::str as port;
use relax_std_check::str::pattern::{Pattern, Searcher, ReverseSearcher, SearchStep};

fn check(h: &str, n: &str) {
    assert_eq!(port::find(h, n), h.find(n), "{h:?} {n:?}");
    assert_eq!(port::rfind(h, n), h.rfind(n), "{h:?} {n:?}");
    assert_eq!(port::contains(h, n), h.contains(n));
    assert_eq!(port::match_indices(h, n).collect::<Vec<_>>(), h.match_indices(n).collect::<Vec<_>>());
    assert_eq!(port::rmatch_indices(h, n).collect::<Vec<_>>(), h.rmatch_indices(n).collect::<Vec<_>>());
    assert_eq!(port::split(h, n).collect::<Vec<_>>(), h.split(n).collect::<Vec<_>>());
    assert_eq!(port::rsplit(h, n).collect::<Vec<_>>(), h.rsplit(n).collect::<Vec<_>>());
    assert_eq!(port::split_terminator(h, n).collect::<Vec<_>>(), h.split_terminator(n).collect::<Vec<_>>());
    assert_eq!(port::rsplit_terminator(h, n).collect::<Vec<_>>(), h.rsplit_terminator(n).collect::<Vec<_>>());
    for count in 0..4 {
        assert_eq!(port::splitn(h, count, n).collect::<Vec<_>>(), h.splitn(count, n).collect::<Vec<_>>());
        assert_eq!(port::rsplitn(h, count, n).collect::<Vec<_>>(), h.rsplitn(count, n).collect::<Vec<_>>());
    }
    assert_eq!(relax_std_check::string::str_replace(h, n, "!").as_str(), h.replace(n, "!"));
    for reverse in [false, true] {
        let mut search = n.into_searcher(h);
        let mut intervals = Vec::new();
        let mut cursor = if reverse { h.len() } else { 0 };
        loop {
            let step = if reverse { search.next_back() } else { search.next() };
            let (a, b, matched) = match step {
                SearchStep::Match(a, b) => (a, b, true),
                SearchStep::Reject(a, b) => (a, b, false),
                SearchStep::Done => break,
            };
            assert!(h.is_char_boundary(a) && h.is_char_boundary(b));
            assert_eq!(if reverse { b } else { a }, cursor);
            cursor = if reverse { a } else { b };
            if matched { intervals.push((a, &h[a..b])); }
        }
        assert_eq!(cursor, if reverse { 0 } else { h.len() });
        let real = if reverse { h.rmatch_indices(n).collect::<Vec<_>>() } else { h.match_indices(n).collect::<Vec<_>>() };
        assert_eq!(intervals, real);
    }
}

fn rng(s: &mut u64) -> usize {
    *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17; *s as usize
}

#[test]
fn random_and_periodic() {
    let mut seed = 0x983abc12u64;
    for alphabet in [vec!['a', 'b', 'c'], vec!['a', 'é', 'ê', '🦀']] {
        for _ in 0..10000 {
            let len = rng(&mut seed) % 180;
            let h: String = (0..len).map(|_| alphabet[rng(&mut seed) % alphabet.len()]).collect();
            let len = rng(&mut seed) % 30;
            let n: String = (0..len).map(|_| alphabet[rng(&mut seed) % alphabet.len()]).collect();
            check(&h, &n);
            // Also force long successful matches, instead of relying on chance.
            let boundaries: Vec<usize> = h.char_indices().map(|(i, _)| i).chain(std::iter::once(h.len())).collect();
            let a = rng(&mut seed) % boundaries.len();
            let b = a + rng(&mut seed) % (boundaries.len() - a);
            check(&h, &h[boundaries[a]..boundaries[b]]);
            for c in &alphabet {
                assert_eq!(port::find(&h, *c), h.find(*c));
                assert_eq!(port::rfind(&h, *c), h.rfind(*c));
                assert_eq!(port::match_indices(&h, *c).collect::<Vec<_>>(), h.match_indices(*c).collect::<Vec<_>>());
                assert_eq!(port::rmatch_indices(&h, *c).collect::<Vec<_>>(), h.rmatch_indices(*c).collect::<Vec<_>>());
            }
        }
    }
    for unit in ["a", "abab", "acba", "éê", "🦀a"] {
        let h = format!("{}z{}", unit.repeat(100), unit.repeat(80));
        for n in [String::new(), unit.to_string(), unit.repeat(40), format!("{}z", unit.repeat(40)), unit.repeat(300)] {
            check(&h, &n);
        }
    }
    check("", ""); check("", "long");
}

#[test]
fn byte_search_all_alignments() {
    use relax_std_check::slice::memchr::{memchr, memrchr};
    let mut seed = 17;
    let mut bytes = vec![0u8; 400];
    for b in &mut bytes { *b = rng(&mut seed) as u8; }
    for start in 0..32 {
        for len in 0..256 {
            let text = &bytes[start..start + len];
            for x in 0..=255 {
                assert_eq!(memchr(x, text), text.iter().position(|b| *b == x));
                assert_eq!(memrchr(x, text), text.iter().rposition(|b| *b == x));
            }
        }
    }
}

#[test]
fn rejection_steps_and_independent_cursors() {
    use SearchStep::{Done, Match, Reject};
    // Reference Two-Way skips, including the adjustment to UTF-8 boundaries.
    for (h, n) in [("zzzzababzzzz", "abab"), ("xxxxabcdxxxx", "abcd"), ("ééababéé", "abab")] {
        let mut s = n.into_searcher(h);
        for expected in [Reject(0, 4), Match(4, 8), Reject(8, 12), Done, Done] {
            assert_eq!(s.next(), expected);
        }
        for expected in [Reject(8, 12), Match(4, 8), Reject(0, 4), Done, Done] {
            assert_eq!(s.next_back(), expected);
        }
    }
    let mut s = "abab".into_searcher("zzababx");
    assert_eq!(s.next(), Reject(0, 1));
    assert_eq!(s.next_match(), Some((2, 6)));
    assert_eq!(s.next(), Reject(6, 7));
    assert_eq!(s.next(), Done);
    let mut s = "".into_searcher("éa");
    for expected in [Match(0, 0), Reject(0, 2), Match(2, 2), Reject(2, 3), Match(3, 3), Done] {
        assert_eq!(s.next(), expected);
    }
    assert_eq!(s.next_back(), Done);
    let mut s = "".into_searcher("");
    assert_eq!(s.next(), Match(0, 0));
    assert_eq!(s.next_back(), Match(0, 0));
    assert_eq!(s.next(), Done);
    assert_eq!(s.next_back(), Done);
}

#[test]
#[ignore]
fn quick_timing() {
    use std::hint::black_box;
    use std::time::Instant;
    for (label, h, n) in [
        ("prose", format!("{}needle", "some ordinary text with spaces ".repeat(2000)), "needle".to_string()),
        ("periodic", format!("{}b", "a".repeat(64000)), format!("{}b", "a".repeat(80))),
        ("short", "small haystack with a needle".to_string(), "needle".to_string()),
    ] {
        let mut real_best = u128::MAX;
        let mut port_best = u128::MAX;
        for _ in 0..5 {
            let start = Instant::now();
            for _ in 0..2000 { black_box(black_box(h.as_str()).find(black_box(n.as_str()))); }
            real_best = real_best.min(start.elapsed().as_nanos());
            let start = Instant::now();
            for _ in 0..2000 { black_box(port::find(black_box(h.as_str()), black_box(n.as_str()))); }
            port_best = port_best.min(start.elapsed().as_nanos());
        }
        println!("{label}: real {} ns/find, port {} ns/find", real_best / 2000, port_best / 2000);
    }
}
