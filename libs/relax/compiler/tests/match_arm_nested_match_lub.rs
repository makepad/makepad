// rustc --edition=2021 --test match_arm_nested_match_lub.rs
// makepad test match_arm_nested_match_lub.rs
// The arms of a match meet in one type, and an earlier arm may be coerced to a later arm's type
// (rustc coercion.rs try_find_coercion_lub: `&String` arms deref to a later `&str`). A later arm
// that is itself a match is typed on its own first: the earlier arms' type is a hint for it, not
// the type its own arms must coerce to. stylo's `CssUrl::to_css` (url/servo.rs) is the first test:
// it failed with "expected `&String`, found `&str`" at the inner match's first arm.
use std::sync::Arc;

struct Url {
    original: Option<Arc<String>>,
    resolved: Option<String>,
}

impl Url {
    fn text(&self) -> &str {
        let string = match self.original {
            Some(ref original) => &**original,
            None => match self.resolved {
                Some(ref url) => url.as_str(),
                None => "about:invalid",
            },
        };
        string
    }
}

fn nested_in_the_middle<'a>(a: &'a String, b: &'a str, pick: u8, inner: bool) -> &'a str {
    let s = match pick {
        0 => a,
        1 => match inner {
            true => b,
            false => "inner",
        },
        _ => a,
    };
    s
}

fn nested_twice<'a>(a: &'a Vec<u8>, pick: (u8, u8, u8)) -> usize {
    let s = match pick.0 {
        0 => a,
        _ => match pick.1 {
            0 => match pick.2 {
                0 => &a[1..],
                _ => &[7u8, 8, 9, 10][..],
            },
            _ => &a[..1],
        },
    };
    s.len()
}

fn nested_in_an_if<'a>(a: &'a String, c: bool, d: u8) -> &'a str {
    let s = if c {
        a
    } else {
        match d {
            0 => "zero",
            _ => a.as_str(),
        }
    };
    s
}

fn nested_if_in_a_match<'a>(a: &'a String, pick: u8, c: bool) -> &'a str {
    let s = match pick {
        0 => a,
        _ => if c { "yes" } else { a.as_str() },
    };
    s
}

// the hint still types what it can: the inner arms are integers of the first arm's type
fn hint_types_inner_literals(pick: u8, inner: bool) -> u64 {
    let n = match pick {
        0 => u64::MAX,
        _ => match inner {
            true => 1 << 40,
            false => 2,
        },
    };
    n
}

// an expected type from outside is still the type every arm coerces to, however deep
fn expected_type_reaches_inner_arms(pick: u8, inner: bool) -> Box<dyn Fn(u32) -> u32> {
    match pick {
        0 => Box::new(|x| x + 1),
        _ => match inner {
            true => Box::new(|x| x * 2),
            false => Box::new(|x| x - 1),
        },
    }
}

#[test]
fn earlier_arms_coerce_to_a_nested_match() {
    let u = Url { original: Some(Arc::new("abc".to_string())), resolved: None };
    assert_eq!(u.text(), "abc");
    let v = Url { original: None, resolved: Some("abcde".to_string()) };
    assert_eq!(v.text(), "abcde");
    let w = Url { original: None, resolved: None };
    assert_eq!(w.text(), "about:invalid");
}

#[test]
fn nesting_in_any_arm_and_any_depth() {
    let a = "owned".to_string();
    assert_eq!(nested_in_the_middle(&a, "b", 0, true), "owned");
    assert_eq!(nested_in_the_middle(&a, "b", 1, true), "b");
    assert_eq!(nested_in_the_middle(&a, "b", 1, false), "inner");
    assert_eq!(nested_in_the_middle(&a, "b", 2, false), "owned");
    let v = vec![1u8, 2, 3];
    assert_eq!(nested_twice(&v, (0, 0, 0)), 3);
    assert_eq!(nested_twice(&v, (1, 0, 0)), 2);
    assert_eq!(nested_twice(&v, (1, 0, 1)), 4);
    assert_eq!(nested_twice(&v, (1, 1, 0)), 1);
}

#[test]
fn if_and_match_nest_either_way() {
    let a = "owned".to_string();
    assert_eq!(nested_in_an_if(&a, true, 0), "owned");
    assert_eq!(nested_in_an_if(&a, false, 0), "zero");
    assert_eq!(nested_in_an_if(&a, false, 1), "owned");
    assert_eq!(nested_if_in_a_match(&a, 0, true), "owned");
    assert_eq!(nested_if_in_a_match(&a, 1, true), "yes");
    assert_eq!(nested_if_in_a_match(&a, 1, false), "owned");
}

#[test]
fn hints_and_expected_types_still_reach_the_inner_arms() {
    assert_eq!(hint_types_inner_literals(0, true), u64::MAX);
    assert_eq!(hint_types_inner_literals(1, true), 1 << 40);
    assert_eq!(hint_types_inner_literals(1, false), 2);
    assert_eq!(expected_type_reaches_inner_arms(0, true)(5), 6);
    assert_eq!(expected_type_reaches_inner_arms(1, true)(5), 10);
    assert_eq!(expected_type_reaches_inner_arms(1, false)(5), 4);
}
