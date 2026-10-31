extern crate core;

use std::ffi::{OsStr, OsString};
use std::iter::FusedIterator;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

fn assert_exact<I: ExactSizeIterator>(iter: &I, len: usize) {
    assert_eq!(iter.len(), len);
    assert_eq!(iter.size_hint(), (len, Some(len)));
}

fn assert_exhausted<I: ExactSizeIterator + DoubleEndedIterator + FusedIterator>(iter: &mut I) {
    assert_exact(iter, 0);
    assert!(iter.next().is_none());
    assert!(iter.next_back().is_none());
    assert!(iter.next().is_none());
    assert_exact(iter, 0);
}

#[test]
fn ascii_escape_all_bytes() {
    for byte in 0u16..=255 {
        let byte = byte as u8;
        let expected = match byte {
            b'\t' => String::from("\\t"),
            b'\r' => String::from("\\r"),
            b'\n' => String::from("\\n"),
            b'\'' => String::from("\\'"),
            b'"' => String::from("\\\""),
            b'\\' => String::from("\\\\"),
            0x20..=0x7e => (byte as char).to_string(),
            _ => format!("\\x{:02x}", byte),
        };
        let escaped: std::ascii::EscapeDefault = std::ascii::escape_default(byte);
        let core_escaped: core::ascii::EscapeDefault = core::ascii::escape_default(byte);
        assert_exact(&escaped, expected.len());
        assert_eq!(escaped.to_string(), expected);
        assert_eq!(escaped.clone().collect::<Vec<u8>>(), expected.as_bytes());
        assert_eq!(core_escaped.collect::<Vec<u8>>(), expected.as_bytes());
        assert_eq!(format!("b'{}'", escaped), format!("b'{}'", expected));
        assert_eq!(format!("{:?}", escaped), "EscapeDefault { .. }");
    }
}

#[test]
fn ascii_escape_remaining_display_and_ends() {
    let mut escaped = std::ascii::escape_default(0x9d);
    assert_eq!(escaped.next(), Some(b'\\'));
    assert_eq!(escaped.next_back(), Some(b'd'));
    assert_exact(&escaped, 2);
    assert_eq!(escaped.to_string(), "x9");
    assert_eq!(escaped.clone().rev().collect::<Vec<u8>>(), b"9x");
    assert_eq!(escaped.nth(1), Some(b'9'));
    assert_exhausted(&mut escaped);
    assert_eq!(escaped.to_string(), "");

    let mut escaped = std::ascii::escape_default(b'\n');
    assert_eq!(escaped.nth_back(usize::MAX), None);
    assert_exhausted(&mut escaped);
}

#[cfg(unix)]
#[test]
fn unix_parent_id_matches_getppid() {
    unsafe extern "C" {
        fn getppid() -> std::os::raw::c_int;
    }
    let parent_id: fn() -> u32 = std::os::unix::process::parent_id;
    assert_eq!(parent_id(), unsafe { getppid() as u32 });
}

#[test]
fn option_iterators() {
    let mut value = Some(17);
    assert_eq!(value.iter().len(), 1);
    let mut iter = value.iter();
    assert_exact(&iter, 1);
    assert_eq!(iter.next_back(), Some(&17));
    assert_exhausted(&mut iter);
    let mut iter = value.iter();
    assert_eq!(iter.next(), Some(&17));
    assert_exhausted(&mut iter);

    let mut iter = value.iter_mut();
    assert_exact(&iter, 1);
    *iter.next_back().unwrap() += 1;
    assert_exhausted(&mut iter);
    let mut iter = value.into_iter();
    assert_exact(&iter, 1);
    assert_eq!(iter.next_back(), Some(18));
    assert_exhausted(&mut iter);

    let mut empty: Option<i32> = None;
    assert_exhausted(&mut empty.iter());
    assert_exhausted(&mut empty.iter_mut());
    assert_exhausted(&mut empty.into_iter());
}

#[test]
fn result_iterators() {
    let mut value: Result<i32, ()> = Ok(21);
    let mut iter = value.iter();
    assert_exact(&iter, 1);
    assert_eq!(iter.next_back(), Some(&21));
    assert_exhausted(&mut iter);
    let mut iter = value.iter_mut();
    assert_exact(&iter, 1);
    *iter.next_back().unwrap() += 1;
    assert_exhausted(&mut iter);
    let mut iter = value.into_iter();
    assert_exact(&iter, 1);
    assert_eq!(iter.next_back(), Some(22));
    assert_exhausted(&mut iter);

    let mut empty: Result<i32, ()> = Err(());
    assert_exhausted(&mut empty.iter());
    assert_exhausted(&mut empty.iter_mut());
    assert_exhausted(&mut empty.into_iter());
}

#[test]
fn other_exact_size_iterators() {
    let calls = std::cell::Cell::new(0);
    let mut once = std::iter::once_with(|| {
        calls.set(calls.get() + 1);
        7
    });
    assert_exact(&once, 1);
    assert_eq!(calls.get(), 0);
    assert_eq!(once.next_back(), Some(7));
    assert_exhausted(&mut once);
    assert_eq!(calls.get(), 1);

    let mut inspected = (0..3).inspect(|_| calls.set(calls.get() + 1));
    assert_exact(&inspected, 3);
    assert_eq!(calls.get(), 1);
    assert_eq!(inspected.next(), Some(0));
    assert_eq!(inspected.next_back(), Some(2));
    assert_exact(&inspected, 1);
    assert_eq!(inspected.next(), Some(1));
    assert_exhausted(&mut inspected);
    assert_eq!(calls.get(), 4);

    let mut map = std::collections::BTreeMap::new();
    map.insert(1, 10);
    map.insert(2, 20);
    let mut keys = map.clone().into_keys();
    assert_exact(&keys, 2);
    assert_eq!(keys.next_back(), Some(2));
    assert_exact(&keys, 1);
    assert_eq!(keys.next(), Some(1));
    assert_exhausted(&mut keys);
    let mut values = map.into_values();
    assert_exact(&values, 2);
    assert_eq!(values.next(), Some(10));
    assert_exact(&values, 1);
    assert_eq!(values.next_back(), Some(20));
    assert_exhausted(&mut values);
}

fn arc_raw_roundtrip<T: ?Sized>(value: Arc<T>) -> Arc<T> {
    let raw = Arc::into_raw(value.clone());
    unsafe {
        Arc::increment_strong_count(raw);
        assert_eq!(Arc::strong_count(&value), 3);
        Arc::decrement_strong_count(raw);
        let restored = Arc::from_raw(raw);
        assert!(Arc::ptr_eq(&value, &restored));
        assert_eq!(Arc::strong_count(&restored), 2);
        restored
    }
}

fn rc_raw_roundtrip<T: ?Sized>(value: Rc<T>) -> Rc<T> {
    let raw = Rc::into_raw(value.clone());
    unsafe {
        Rc::increment_strong_count(raw);
        assert_eq!(Rc::strong_count(&value), 3);
        Rc::decrement_strong_count(raw);
        let restored = Rc::from_raw(raw);
        assert!(Rc::ptr_eq(&value, &restored));
        assert_eq!(Rc::strong_count(&restored), 2);
        restored
    }
}

fn check_shared_path(path: &Path) {
    let arc = arc_raw_roundtrip(Arc::<Path>::from(path));
    let rc = rc_raw_roundtrip(Rc::<Path>::from(path));
    assert_eq!(&*arc, path);
    assert_eq!(&*rc, path);
    assert_eq!(Arc::strong_count(&arc), 1);
    assert_eq!(Rc::strong_count(&rc), 1);
    assert_eq!(&*Arc::<Path>::from(PathBuf::from(path)), path);
    assert_eq!(&*Rc::<Path>::from(PathBuf::from(path)), path);

    let os = path.as_os_str();
    assert_eq!(&*arc_raw_roundtrip(Arc::<OsStr>::from(os)), os);
    assert_eq!(&*rc_raw_roundtrip(Rc::<OsStr>::from(os)), os);
    assert_eq!(&*Arc::<OsStr>::from(OsString::from(os)), os);
    assert_eq!(&*Rc::<OsStr>::from(OsString::from(os)), os);
}

#[test]
fn shared_unsized_paths_and_raw_pointers() {
    check_shared_path(Path::new("dir/café.txt"));
    check_shared_path(Path::new(""));
    assert_eq!(&*arc_raw_roundtrip(Arc::<str>::from("abc")), "abc");
    assert_eq!(&*rc_raw_roundtrip(Rc::<str>::from("abc")), "abc");
    assert_eq!(&*arc_raw_roundtrip(Arc::<[u8]>::from(&b"abc"[..])), b"abc");
    assert_eq!(&*rc_raw_roundtrip(Rc::<[u8]>::from(&b"abc"[..])), b"abc");
}

#[cfg(unix)]
#[test]
fn shared_paths_preserve_non_utf8_bytes() {
    use std::os::unix::ffi::OsStrExt;
    check_shared_path(Path::new(OsStr::from_bytes(b"dir/\xff\0")));
}

#[test]
fn float_next_up_down() {
    assert_eq!(1.0f32.next_down(), f32::from_bits(1.0f32.to_bits() - 1));
    assert_eq!(1.0f32.next_up(), f32::from_bits(1.0f32.to_bits() + 1));
    assert_eq!(0.0f32.next_up(), f32::from_bits(1));
    assert_eq!(0.0f32.next_down(), -f32::from_bits(1));
    assert_eq!((-1.0f64).next_up(), f64::from_bits((-1.0f64).to_bits() - 1));
    assert_eq!(f64::INFINITY.next_up(), f64::INFINITY);
    assert_eq!(f64::NEG_INFINITY.next_down(), f64::NEG_INFINITY);
    assert!(f32::NAN.next_up().is_nan());
}
