//! Rapid std str vs real std: UTF-8 validation, iteration, the Pattern API and splits,
//! trimming, slicing panics, escapes, case conversion.

mod text_common;

use rapid_std_check::str as hs;
use rapid_std_check::str::pattern::CharPred;
use rapid_std_check::string as hstring;
use text_common::*;

// ---------------------------------------------------------------- UTF-8

fn random_bytes(rng: &mut Rng) -> Vec<u8> {
    const FRAGS: &[&[u8]] = &[
        b"a", b"\xc3\xa9", b"\xe2\x82\xac", b"\xf0\x9d\x84\x9e", b"\xc3", b"\xe2\x82", b"\xf0\x9d", b"\x80", b"\xbf",
        b"\xc0\x80", b"\xed\xa0\x80", b"\xf4\x90\x80\x80", b"\xf8", b"\xff", b"\xe0\x80\x80", b"\xf0\x80\x80\x80", b" ",
    ];
    let n = rng.below(7);
    let mut v = Vec::new();
    for _ in 0..n {
        v.extend_from_slice(FRAGS[rng.below(FRAGS.len())]);
    }
    v
}

#[test]
fn from_utf8_random() {
    let mut rng = Rng(9);
    for _ in 0..200_000 {
        let v = random_bytes(&mut rng);
        let m = hs::from_utf8(&v);
        let r = std::str::from_utf8(&v);
        match (m, r) {
            (Ok(a), Ok(b)) => assert_eq!(a, b),
            (Err(a), Err(b)) => {
                assert_eq!(a.valid_up_to(), b.valid_up_to(), "{:x?}", v);
                assert_eq!(a.error_len(), b.error_len(), "{:x?}", v);
                assert_eq!(hdisplay(&a), b.to_string());
            }
            _ => panic!("validity differs for {:x?}", v),
        }
        // chunks and lossy
        let mine: Vec<(String, Vec<u8>)> =
            hs::utf8_chunks(&v).map(|c| (c.valid().to_string(), c.invalid().to_vec())).collect();
        let real: Vec<(String, Vec<u8>)> = v.utf8_chunks().map(|c| (c.valid().to_string(), c.invalid().to_vec())).collect();
        assert_eq!(mine, real, "{:x?}", v);
        assert_eq!(hstring::lossy_string(&v), String::from_utf8_lossy(&v).to_string());
    }
}

// ---------------------------------------------------------------- iteration

#[test]
fn chars_bytes_indices() {
    let mut rng = Rng(3);
    for _ in 0..20_000 {
        let s = random_string(&mut rng, 8);
        assert_eq!(hs::chars(&s).collect::<Vec<_>>(), s.chars().collect::<Vec<_>>());
        assert_eq!(hs::chars(&s).rev().collect::<Vec<_>>(), s.chars().rev().collect::<Vec<_>>());
        assert_eq!(hs::chars(&s).count(), s.chars().count());
        assert_eq!(hs::chars(&s).last(), s.chars().last());
        assert_eq!(hs::char_indices(&s).collect::<Vec<_>>(), s.char_indices().collect::<Vec<_>>());
        assert_eq!(hs::char_indices(&s).rev().collect::<Vec<_>>(), s.char_indices().rev().collect::<Vec<_>>());
        assert_eq!(hs::bytes(&s).collect::<Vec<_>>(), s.bytes().collect::<Vec<_>>());
        assert_eq!(hs::bytes(&s).rev().collect::<Vec<_>>(), s.bytes().rev().collect::<Vec<_>>());
        assert_eq!(hs::encode_utf16(&s).collect::<Vec<_>>(), s.encode_utf16().collect::<Vec<_>>());
        assert_eq!(hs::chars(&s).size_hint(), s.chars().size_hint());
        assert_eq!(hs::encode_utf16(&s).size_hint(), s.encode_utf16().size_hint());
        // mixed front/back with as_str
        let mut a = hs::chars(&s);
        let mut b = s.chars();
        for k in 0..12 {
            let (x, y) = if (rng.next() >> 3) & 1 == 0 { (a.next(), b.next()) } else { (a.next_back(), b.next_back()) };
            assert_eq!(x, y, "{:?} step {}", s, k);
            assert_eq!(a.as_str(), b.as_str());
        }
        let mut a = hs::char_indices(&s);
        let mut b = s.char_indices();
        for _ in 0..6 {
            assert_eq!(a.next(), b.next());
            assert_eq!(a.offset(), b.offset());
        }
    }
}

// ---------------------------------------------------------------- patterns

/// Runs every pattern-taking operation for one (haystack, pattern) pair with both the
/// Rapid free fn and real std, through a macro-free closure table.
fn check_pattern_ops<'a>(s: &'a str, label: &str, m: &dyn Fn(&'a str) -> Vec<String>, r: &dyn Fn(&'a str) -> Vec<String>) {
    assert_eq!(m(s), r(s), "{} on {:?}", label, s);
}

fn v<'a, I: Iterator<Item = &'a str>>(it: I) -> Vec<String> {
    it.map(|x| x.to_string()).collect()
}

fn vi<'a, I: Iterator<Item = (usize, &'a str)>>(it: I) -> Vec<String> {
    it.map(|(i, x)| format!("{}:{}", i, x)).collect()
}

fn o(x: Option<&str>) -> Vec<String> {
    match x {
        Some(s) => vec![s.to_string()],
        None => vec!["<none>".to_string()],
    }
}

fn opair(x: Option<(&str, &str)>) -> Vec<String> {
    match x {
        Some((a, b)) => vec![a.to_string(), b.to_string()],
        None => vec!["<none>".to_string()],
    }
}

fn ou(x: Option<usize>) -> Vec<String> {
    vec![format!("{:?}", x)]
}

fn b(x: bool) -> Vec<String> {
    vec![format!("{}", x)]
}

const NEEDLES: &[&str] = &["", "a", "aa", "ab", "é", "€𝄞", "\n", " ", "Σ", "ba", "xyz"];
const CHARS: &[char] = &['a', 'é', '€', '𝄞', ' ', '\n', 'Σ', 'q'];

#[test]
fn str_patterns() {
    let mut rng = Rng(11);
    for _ in 0..4_000 {
        let s = random_string(&mut rng, 7);
        let s = s.as_str();
        for &n in NEEDLES {
            check_pattern_ops(s, "split", &|s| v(hs::split(s, n)), &|s| v(s.split(n)));
            check_pattern_ops(s, "rsplit", &|s| v(hs::rsplit(s, n)), &|s| v(s.rsplit(n)));
            check_pattern_ops(s, "split_terminator", &|s| v(hs::split_terminator(s, n)), &|s| v(s.split_terminator(n)));
            check_pattern_ops(s, "rsplit_terminator", &|s| v(hs::rsplit_terminator(s, n)), &|s| v(s.rsplit_terminator(n)));
            check_pattern_ops(s, "split_inclusive", &|s| v(hs::split_inclusive(s, n)), &|s| v(s.split_inclusive(n)));
            for k in 0..4 {
                check_pattern_ops(s, "splitn", &|s| v(hs::splitn(s, k, n)), &|s| v(s.splitn(k, n)));
                check_pattern_ops(s, "rsplitn", &|s| v(hs::rsplitn(s, k, n)), &|s| v(s.rsplitn(k, n)));
            }
            check_pattern_ops(s, "matches", &|s| v(hs::matches(s, n)), &|s| v(s.matches(n)));
            check_pattern_ops(s, "rmatches", &|s| v(hs::rmatches(s, n)), &|s| v(s.rmatches(n)));
            check_pattern_ops(s, "match_indices", &|s| vi(hs::match_indices(s, n)), &|s| vi(s.match_indices(n)));
            check_pattern_ops(s, "rmatch_indices", &|s| vi(hs::rmatch_indices(s, n)), &|s| vi(s.rmatch_indices(n)));
            check_pattern_ops(s, "find", &|s| ou(hs::find(s, n)), &|s| ou(s.find(n)));
            check_pattern_ops(s, "rfind", &|s| ou(hs::rfind(s, n)), &|s| ou(s.rfind(n)));
            check_pattern_ops(s, "contains", &|s| b(hs::contains(s, n)), &|s| b(s.contains(n)));
            check_pattern_ops(s, "starts_with", &|s| b(hs::starts_with(s, n)), &|s| b(s.starts_with(n)));
            check_pattern_ops(s, "ends_with", &|s| b(hs::ends_with(s, n)), &|s| b(s.ends_with(n)));
            check_pattern_ops(s, "strip_prefix", &|s| o(hs::strip_prefix(s, n)), &|s| o(s.strip_prefix(n)));
            check_pattern_ops(s, "strip_suffix", &|s| o(hs::strip_suffix(s, n)), &|s| o(s.strip_suffix(n)));
            check_pattern_ops(s, "trim_start_matches", &|s| vec![hs::trim_start_matches(s, n).to_string()], &|s| {
                vec![s.trim_start_matches(n).to_string()]
            });
            check_pattern_ops(s, "trim_end_matches", &|s| vec![hs::trim_end_matches(s, n).to_string()], &|s| {
                vec![s.trim_end_matches(n).to_string()]
            });
            check_pattern_ops(s, "split_once", &|s| opair(hs::split_once(s, n)), &|s| opair(s.split_once(n)));
            check_pattern_ops(s, "rsplit_once", &|s| opair(hs::rsplit_once(s, n)), &|s| opair(s.rsplit_once(n)));
            check_pattern_ops(s, "replace", &|s| vec![hstring::str_replace(s, n, "<>").as_str().to_string()], &|s| {
                vec![s.replace(n, "<>")]
            });
            for k in 0..3 {
                check_pattern_ops(s, "replacen", &|s| vec![hstring::str_replacen(s, n, "_", k).as_str().to_string()], &|s| {
                    vec![s.replacen(n, "_", k)]
                });
            }
            // &&str and &String forms
            let nn: &&str = &n;
            assert_eq!(v(hs::split(s, nn)), v(s.split(n)));
            let ns = hstring::String::from(n);
            assert_eq!(v(hs::split(s, &ns)), v(s.split(n)));
        }
    }
}

#[test]
fn char_patterns() {
    let mut rng = Rng(12);
    for _ in 0..6_000 {
        let s = random_string(&mut rng, 7);
        let s = s.as_str();
        for &c in CHARS {
            assert_eq!(v(hs::split(s, c)), v(s.split(c)), "split {:?} {:?}", s, c);
            assert_eq!(v(hs::split(s, c).rev()), v(s.split(c).rev()), "split rev {:?} {:?}", s, c);
            assert_eq!(v(hs::rsplit(s, c)), v(s.rsplit(c)));
            assert_eq!(v(hs::split_terminator(s, c)), v(s.split_terminator(c)));
            assert_eq!(v(hs::split_terminator(s, c).rev()), v(s.split_terminator(c).rev()));
            assert_eq!(v(hs::rsplit_terminator(s, c)), v(s.rsplit_terminator(c)));
            assert_eq!(v(hs::split_inclusive(s, c)), v(s.split_inclusive(c)));
            assert_eq!(v(hs::split_inclusive(s, c).rev()), v(s.split_inclusive(c).rev()));
            for k in 0..4 {
                assert_eq!(v(hs::splitn(s, k, c)), v(s.splitn(k, c)));
                assert_eq!(v(hs::rsplitn(s, k, c)), v(s.rsplitn(k, c)));
            }
            assert_eq!(v(hs::matches(s, c)), v(s.matches(c)));
            assert_eq!(v(hs::matches(s, c).rev()), v(s.matches(c).rev()));
            assert_eq!(vi(hs::match_indices(s, c)), vi(s.match_indices(c)));
            assert_eq!(vi(hs::rmatch_indices(s, c)), vi(s.rmatch_indices(c)));
            assert_eq!(hs::find(s, c), s.find(c));
            assert_eq!(hs::rfind(s, c), s.rfind(c));
            assert_eq!(hs::contains(s, c), s.contains(c));
            assert_eq!(hs::starts_with(s, c), s.starts_with(c));
            assert_eq!(hs::ends_with(s, c), s.ends_with(c));
            assert_eq!(hs::strip_prefix(s, c), s.strip_prefix(c));
            assert_eq!(hs::strip_suffix(s, c), s.strip_suffix(c));
            assert_eq!(hs::trim_matches(s, c), s.trim_matches(c));
            assert_eq!(hs::trim_start_matches(s, c), s.trim_start_matches(c));
            assert_eq!(hs::trim_end_matches(s, c), s.trim_end_matches(c));
            assert_eq!(hs::split_once(s, c), s.split_once(c));
            assert_eq!(hs::rsplit_once(s, c), s.rsplit_once(c));
            assert_eq!(hstring::str_replace(s, c, "-").as_str(), s.replace(c, "-"));
            // mixed next / next_back on a double-ended split
            let mut a = hs::split(s, c);
            let mut bb = s.split(c);
            for _ in 0..6 {
                if rng.next() & 1 == 0 {
                    assert_eq!(a.next(), bb.next(), "{:?} {:?}", s, c);
                } else {
                    assert_eq!(a.next_back(), bb.next_back(), "{:?} {:?}", s, c);
                }
            }
            let mut a = hs::split_terminator(s, c);
            let mut bb = s.split_terminator(c);
            for _ in 0..6 {
                if rng.next() & 1 == 0 {
                    assert_eq!(a.next(), bb.next());
                } else {
                    assert_eq!(a.next_back(), bb.next_back());
                }
            }
        }
        // char slices, char arrays, predicates
        let set: &[char] = &['a', 'é', ' '];
        assert_eq!(v(hs::split(s, set)), v(s.split(set)));
        assert_eq!(v(hs::split(s, set).rev()), v(s.split(set).rev()));
        assert_eq!(hs::trim_matches(s, set), s.trim_matches(set));
        assert_eq!(hs::find(s, ['€', '\n']), s.find(['€', '\n']));
        assert_eq!(v(hs::split(s, ['€', '\n'])), v(s.split(['€', '\n'])));
        assert_eq!(hs::trim_end_matches(s, &['a', 'b']), s.trim_end_matches(&['a', 'b']));
        assert_eq!(v(hs::split(s, CharPred(|c: char| c.is_whitespace()))), v(s.split(|c: char| c.is_whitespace())));
        assert_eq!(
            v(hs::rsplit(s, CharPred(|c: char| c == 'a' || c == 'Σ'))),
            v(s.rsplit(|c: char| c == 'a' || c == 'Σ'))
        );
        assert_eq!(hs::trim_matches(s, CharPred(|c: char| !c.is_alphabetic())), s.trim_matches(|c: char| !c.is_alphabetic()));
        assert_eq!(hs::rfind(s, CharPred(|c: char| c > 'z')), s.rfind(|c: char| c > 'z'));
    }
}

#[test]
fn whitespace_lines_trim() {
    let mut rng = Rng(13);
    for _ in 0..30_000 {
        let s = random_string(&mut rng, 9);
        let s = s.as_str();
        assert_eq!(v(hs::lines(s)), v(s.lines()), "{:?}", s);
        assert_eq!(v(hs::lines(s).rev()), v(s.lines().rev()));
        assert_eq!(v(hs::split_whitespace(s)), v(s.split_whitespace()));
        assert_eq!(v(hs::split_whitespace(s).rev()), v(s.split_whitespace().rev()));
        assert_eq!(v(hs::split_ascii_whitespace(s)), v(s.split_ascii_whitespace()));
        assert_eq!(v(hs::split_ascii_whitespace(s).rev()), v(s.split_ascii_whitespace().rev()));
        assert_eq!(hs::trim(s), s.trim());
        assert_eq!(hs::trim_start(s), s.trim_start());
        assert_eq!(hs::trim_end(s), s.trim_end());
        assert_eq!(hs::trim_ascii(s), s.trim_ascii());
        assert_eq!(hs::trim_ascii_start(s), s.trim_ascii_start());
        assert_eq!(hs::trim_ascii_end(s), s.trim_ascii_end());
    }
}

// ---------------------------------------------------------------- slicing

#[test]
fn slicing_and_panics() {
    let mut rng = Rng(14);
    for _ in 0..3_000 {
        let s = random_string(&mut rng, 5);
        let s = s.as_str();
        let len = s.len();
        for i in 0..=len + 1 {
            assert_eq!(hs::is_char_boundary(s, i), s.is_char_boundary(i));
            assert_eq!(hs::floor_char_boundary(s, i), s.floor_char_boundary(i));
            assert_eq!(hs::ceil_char_boundary(s, i), s.ceil_char_boundary(i));
            assert_eq!(hs::split_at_checked(s, i), s.split_at_checked(i));
            assert_eq!(panic_msg(|| hs::split_at(s, i)), panic_msg(|| s.split_at(i)), "split_at {:?} {}", s, i);
            assert_eq!(hs::get(s, ..i), s.get(..i));
            assert_eq!(hs::get(s, i..), s.get(i..));
            assert_eq!(hs::get(s, ..=i), s.get(..=i));
            assert_eq!(panic_msg(|| hs::index(s, ..i)), panic_msg(|| &s[..i]), "..{} of {:?}", i, s);
            assert_eq!(panic_msg(|| hs::index(s, i..)), panic_msg(|| &s[i..]));
            assert_eq!(panic_msg(|| hs::index(s, ..=i)), panic_msg(|| &s[..=i]));
            for j in 0..=len + 1 {
                assert_eq!(hs::get(s, i..j), s.get(i..j));
                assert_eq!(hs::get(s, i..=j), s.get(i..=j));
                assert_eq!(panic_msg(|| hs::index(s, i..j)), panic_msg(|| &s[i..j]), "{}..{} of {:?}", i, j, s);
                assert_eq!(panic_msg(|| hs::index(s, i..=j)), panic_msg(|| &s[i..=j]), "{}..={} of {:?}", i, j, s);
            }
        }
        assert_eq!(panic_msg(|| hs::index(s, 0..=usize::MAX)), panic_msg(|| &s[0..=usize::MAX]));
        // exhausted inclusive range
        let mut r = 0..=0usize;
        r.next();
        assert_eq!(hs::get(s, r.clone()), s.get(r.clone()));
        assert_eq!(panic_msg(|| hs::index(s, r.clone())), panic_msg(|| &s[r.clone()]));
    }
}

// ---------------------------------------------------------------- escapes, Debug, case

#[test]
fn escapes_and_debug() {
    let mut rng = Rng(15);
    let extra = ["\0", "\"", "\\", "\u{7f}", "\u{200b}", "\u{e000}", "\u{feff}", "\u{301}a", "a\u{301}", "\u{ad}", "\u{85}"];
    for i in 0..30_000 {
        let mut s = random_string(&mut rng, 6);
        if i % 3 == 0 {
            s.push_str(extra[rng.below(extra.len())]);
        }
        let s = s.as_str();
        assert_eq!(hs::escape_debug(s).collect::<String>(), s.escape_debug().to_string(), "{:?}", s);
        assert_eq!(hdisplay(&hs::escape_debug(s)), s.escape_debug().to_string());
        assert_eq!(hs::escape_default(s).collect::<String>(), s.escape_default().to_string());
        assert_eq!(hs::escape_unicode(s).collect::<String>(), s.escape_unicode().to_string());
        assert_eq!(hdebug(&s), format!("{:?}", s), "Debug {:?}", s);
        assert_eq!(hstring::str_to_lowercase(s).as_str(), s.to_lowercase(), "lower {:?}", s);
        assert_eq!(hstring::str_to_uppercase(s).as_str(), s.to_uppercase(), "upper {:?}", s);
        assert_eq!(hstring::str_to_ascii_lowercase(s).as_str(), s.to_ascii_lowercase());
        assert_eq!(hstring::str_to_ascii_uppercase(s).as_str(), s.to_ascii_uppercase());
        assert_eq!(hs::is_ascii(s), s.is_ascii());
        let t = s.to_ascii_uppercase();
        assert_eq!(hs::eq_ignore_ascii_case(s, &t), s.eq_ignore_ascii_case(&t));
        assert_eq!(hstring::str_repeat(s, 3).as_str(), s.repeat(3));
        assert_eq!(hs::cmp_str(s, &t), s.cmp(&t));
    }
}

#[test]
fn final_sigma() {
    let cases = [
        "Σ", "AΣ", "ΑΣ", "ΑΣ ", "ΑΣΑ", "Α'Σ", "ΑΣ'", "ΑΣ'Α", "Α\u{301}Σ", "ΑΣ\u{301}", "ΣΑ", " Σ", "Α.Σ", "Α:Σ", "ΑΣ.",
        "ΌΣΟΣ ΣΟΦΟΣ", "\u{1f88}Σ", "1Σ", "ǅΣ", "ﬀΣ",
    ];
    for s in cases {
        assert_eq!(hstring::str_to_lowercase(s).as_str(), s.to_lowercase(), "{:?}", s);
    }
    // every char before/after a sigma
    for cp in 0..0x30000u32 {
        if let Some(c) = char::from_u32(cp) {
            let a = format!("{}Σ", c);
            assert_eq!(hstring::str_to_lowercase(&a).as_str(), a.to_lowercase(), "{:?}", a);
            let b = format!("AΣ{}", c);
            assert_eq!(hstring::str_to_lowercase(&b).as_str(), b.to_lowercase(), "{:?}", b);
            let d = format!("A{}Σ", c);
            assert_eq!(hstring::str_to_lowercase(&d).as_str(), d.to_lowercase(), "{:?}", d);
        }
    }
}

#[test]
fn parse_bool() {
    for s in ["true", "false", "True", "", "1", "true "] {
        let m = hs::parse::<bool>(s);
        let r = s.parse::<bool>();
        match (m, r) {
            (Ok(a), Ok(b)) => assert_eq!(a, b),
            (Err(a), Err(b)) => assert_eq!(hdisplay(&a), b.to_string()),
            _ => panic!("{:?}", s),
        }
    }
}
