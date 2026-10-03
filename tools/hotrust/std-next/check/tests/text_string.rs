//! HotRust std String vs real String: random operation sequences, panics, conversions.

mod text_common;

use hotrust_std_check::string::String as HString;
use text_common::*;

fn same(m: &HString, r: &String, what: &str) {
    assert_eq!(m.as_str(), r.as_str(), "after {}", what);
    assert_eq!(m.len(), r.len());
    assert_eq!(m.is_empty(), r.is_empty());
}

#[test]
fn random_ops() {
    let mut rng = Rng(21);
    for round in 0..3_000 {
        let init = random_string(&mut rng, 4);
        let mut m = HString::from(init.as_str());
        let mut r = init.clone();
        for step in 0..20 {
            let op = rng.below(16);
            let len = r.len();
            let i = rng.below(len + 2);
            let j = rng.below(len + 2);
            let piece = PIECES[rng.below(PIECES.len())];
            let ch = piece.chars().next().unwrap();
            let what = format!("round {} step {} op {} i {} j {} on {:?}", round, step, op, i, j, r);
            match op {
                0 => {
                    m.push(ch);
                    r.push(ch);
                }
                1 => {
                    m.push_str(piece);
                    r.push_str(piece);
                }
                2 => assert_eq!(m.pop(), r.pop()),
                3 => {
                    let a = panic_msg(std::panic::AssertUnwindSafe(|| m.insert(i, ch)));
                    let b = panic_msg(std::panic::AssertUnwindSafe(|| r.insert(i, ch)));
                    assert_eq!(a, b, "{}", what);
                }
                4 => {
                    let a = panic_msg(std::panic::AssertUnwindSafe(|| m.insert_str(i, piece)));
                    let b = panic_msg(std::panic::AssertUnwindSafe(|| r.insert_str(i, piece)));
                    assert_eq!(a, b, "{}", what);
                }
                5 => {
                    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.remove(i)));
                    let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.remove(i)));
                    match (a, b) {
                        (Ok(x), Ok(y)) => assert_eq!(x, y),
                        (Err(x), Err(y)) => assert_eq!(msg(x), msg(y), "{}", what),
                        _ => panic!("remove differs: {}", what),
                    }
                }
                6 => {
                    let a = panic_msg(std::panic::AssertUnwindSafe(|| m.truncate(i)));
                    let b = panic_msg(std::panic::AssertUnwindSafe(|| r.truncate(i)));
                    assert_eq!(a, b, "{}", what);
                }
                7 => {
                    let keep = ch;
                    m.retain(|c| c != keep);
                    r.retain(|c| c != keep);
                }
                8 => {
                    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.drain(i..j).collect::<String>()));
                    let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.drain(i..j).collect::<String>()));
                    match (a, b) {
                        (Ok(x), Ok(y)) => assert_eq!(x, y),
                        (Err(x), Err(y)) => assert_eq!(msg(x), msg(y), "{}", what),
                        _ => panic!("drain differs: {}", what),
                    }
                }
                9 => {
                    // partially consumed drain, from the back
                    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut d = m.drain(..=i);
                        d.next_back()
                    }));
                    let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut d = r.drain(..=i);
                        d.next_back()
                    }));
                    match (a, b) {
                        (Ok(x), Ok(y)) => assert_eq!(x, y),
                        (Err(x), Err(y)) => assert_eq!(msg(x), msg(y), "{}", what),
                        _ => panic!("drain back differs: {}", what),
                    }
                }
                10 => {
                    let a = panic_msg(std::panic::AssertUnwindSafe(|| m.replace_range(i..j, piece)));
                    let b = panic_msg(std::panic::AssertUnwindSafe(|| r.replace_range(i..j, piece)));
                    assert_eq!(a, b, "{}", what);
                }
                11 => {
                    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.split_off(i)));
                    let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.split_off(i)));
                    match (a, b) {
                        (Ok(x), Ok(y)) => assert_eq!(x.as_str(), y.as_str()),
                        (Err(x), Err(y)) => assert_eq!(msg(x), msg(y), "{}", what),
                        _ => panic!("split_off differs: {}", what),
                    }
                }
                12 => {
                    let a = panic_msg(std::panic::AssertUnwindSafe(|| m.extend_from_within(i..j)));
                    let b = panic_msg(std::panic::AssertUnwindSafe(|| r.extend_from_within(i..j)));
                    assert_eq!(a, b, "{}", what);
                }
                13 => {
                    m += piece;
                    r += piece;
                }
                14 => {
                    m.extend(piece.chars());
                    r.extend(piece.chars());
                }
                _ => {
                    let a = panic_msg(std::panic::AssertUnwindSafe(|| m[i..j].to_string()));
                    let b = panic_msg(std::panic::AssertUnwindSafe(|| r[i..j].to_string()));
                    assert_eq!(a, b, "{}", what);
                }
            }
            same(&m, &r, &what);
        }
        let mc = m.clone();
        assert_eq!(mc.as_str(), r.as_str());
        assert_eq!(hdebug(&m), format!("{:?}", r));
        assert_eq!(hdisplay(&m), r.to_string());
    }
}

fn msg(e: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = e.downcast_ref::<&str>() {
        s.to_string()
    } else {
        "<?>".to_string()
    }
}

#[test]
fn conversions() {
    let mut rng = Rng(22);
    for _ in 0..20_000 {
        let n = rng.below(6);
        let bytes: Vec<u8> = (0..n).map(|_| [b'a', 0xc3, 0xa9, 0xe2, 0x82, 0xac, 0xff, 0x80][rng.below(8)]).collect();
        let m = HString::from_utf8(bytes.clone());
        let r = String::from_utf8(bytes.clone());
        match (m, r) {
            (Ok(a), Ok(b)) => assert_eq!(a.as_str(), b.as_str()),
            (Err(a), Err(b)) => {
                assert_eq!(a.utf8_error().valid_up_to(), b.utf8_error().valid_up_to());
                assert_eq!(a.utf8_error().error_len(), b.utf8_error().error_len());
                assert_eq!(hdisplay(&a), b.to_string());
                assert_eq!(a.as_bytes(), b.as_bytes());
                assert_eq!(a.into_utf8_lossy().as_str(), String::from_utf8_lossy(&bytes));
            }
            _ => panic!("{:x?}", bytes),
        }
        let k = rng.below(5);
        let units: Vec<u16> = (0..k).map(|_| [0x41u16, 0xd834, 0xdd1e, 0xdc00, 0xe9][rng.below(5)]).collect();
        let m = HString::from_utf16(&units);
        let r = String::from_utf16(&units);
        match (m, r) {
            (Ok(a), Ok(b)) => assert_eq!(a.as_str(), b.as_str()),
            (Err(a), Err(b)) => assert_eq!(hdisplay(&a), b.to_string()),
            _ => panic!("{:x?}", units),
        }
        assert_eq!(HString::from_utf16_lossy(&units).as_str(), String::from_utf16_lossy(&units));
    }
}

#[test]
fn traits_and_iterators() {
    let parts = ["ab", "é", "", "𝄞z"];
    let m: HString = parts.iter().cloned().collect();
    assert_eq!(m.as_str(), parts.concat());
    let m: HString = "héllo".chars().collect();
    assert_eq!(m.as_str(), "héllo");
    let m: HString = ['x', 'ÿ'].iter().collect();
    assert_eq!(m.as_str(), "xÿ");
    let owned: Vec<HString> = parts.iter().map(|p| HString::from(*p)).collect();
    let m: HString = owned.into_iter().collect();
    assert_eq!(m.as_str(), parts.concat());
    let m = HString::from("ab") + "cd";
    assert_eq!(m.as_str(), "abcd");
    assert!(m == *"abcd");
    assert!(m == "abcd");
    assert!(*"abcd" == m);
    assert!(HString::from("a") < HString::from("b"));
    assert_eq!(HString::from("é").cmp(&HString::from("e")), "é".cmp("e"));
    assert_eq!(HString::from('€').as_str(), "€");
    let v: Vec<u8> = HString::from("hi").into();
    assert_eq!(v, b"hi");
    assert_eq!(hdebug(&HString::from("a\"b\n")), format!("{:?}", "a\"b\n"));
    let mut w = HString::new();
    hotrust_std_check::fmt::Write::write_str(&mut w, "x").unwrap();
    hotrust_std_check::fmt::Write::write_char(&mut w, 'é').unwrap();
    assert_eq!(w.as_str(), "xé");
    let mut s = HString::from("abc");
    s.as_mut_str().make_ascii_uppercase();
    assert_eq!(s.as_str(), "ABC");
    assert_eq!(&s[1..], "BC");
    assert_eq!(s.as_str().parse::<u32>().is_err(), true);
    let d = s.drain(..1);
    assert_eq!(hdebug(&d), format!("{:?}", String::from("ABC").drain(..1)));
}
