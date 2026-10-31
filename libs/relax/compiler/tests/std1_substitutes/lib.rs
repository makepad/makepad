#[cfg(test)]
mod tests {
    use arrayvec::ArrayString;
    use smallvec::{smallvec_inline, SmallVec};
    use std::borrow::{Borrow, BorrowMut};
    use std::fmt::Write;
    use std::hash::{Hash, Hasher};

    #[test]
    fn inline_const_arrays() {
        const LIST: SmallVec<[u8; 3]> = smallvec_inline![1, 2, 3,];
        const REPEAT: SmallVec<[u8; 4]> = smallvec_inline![7; 4];
        const EMPTY: SmallVec<[u8; 0]> = smallvec_inline![0; 0];
        const PREFIX: SmallVec<[u8; 4]> = unsafe { SmallVec::from_const_with_len_unchecked([1, 2, 3, 4], 2) };
        assert_eq!(LIST.as_slice(), &[1, 2, 3]);
        assert_eq!(REPEAT.as_slice(), &[7; 4]);
        assert!(EMPTY.is_empty());
        assert_eq!(PREFIX.as_slice(), &[1, 2]);
        assert!(!LIST.spilled());
        assert_eq!(LIST.capacity(), 3);
        assert_eq!(SmallVec::from_const([1, 2]), SmallVec::from_buf([1, 2]));
        let calls = std::cell::Cell::new(0);
        let next = || { calls.set(calls.get() + 1); calls.get() };
        let list = smallvec_inline![next(), next()];
        assert_eq!(calls.get(), 2);
        assert_eq!(list.as_slice(), &[1, 2]);
        let repeat = smallvec_inline![next(); 3];
        assert_eq!(calls.get(), 3);
        assert_eq!(repeat.as_slice(), &[3; 3]);
        let owned = smallvec_inline![String::from("a"), String::from("b")];
        assert_eq!(owned[0], "a");
        assert!(!owned.spilled());
        let zst = smallvec_inline![(); 3];
        assert_eq!(zst.len(), 3);
        assert!(!zst.spilled());
    }

    #[test]
    fn array_string_utf8_and_capacity() {
        const EMPTY: ArrayString<10> = ArrayString::new_const();
        let mut s = EMPTY;
        assert!(s.is_empty());
        assert_eq!(s.capacity(), 10);
        s.push('a');
        s.push('é');
        s.push('中');
        s.push('🦀');
        assert_eq!(s.len(), 10);
        assert!(s.is_full());
        assert_eq!(s.remaining_capacity(), 0);
        assert_eq!(s.as_str(), "aé中🦀");
        assert_eq!(s.try_push('x').unwrap_err().element(), 'x');
        assert_eq!(s.try_push_str("xy").unwrap_err().element(), "xy");
        assert_eq!(s.as_str(), "aé中🦀");
        assert!(s.try_push_str("").is_ok());
        assert_eq!(s.pop(), Some('🦀'));
        assert_eq!(s.remove(1), 'é');
        assert_eq!(s.as_str(), "a中");
        s.truncate(99);
        assert_eq!(s.len(), 4);
        s.truncate(1);
        assert_eq!(s.as_str(), "a");
        s.clear();
        assert_eq!(s.pop(), None);
        let mut short = ArrayString::<3>::from("ab").unwrap();
        assert_eq!(short.try_push('é').unwrap_err().element(), 'é');
        assert_eq!(short.as_str(), "ab");
        assert_eq!(short.try_push_str("cd").unwrap_err().element(), "cd");
        assert_eq!(short.as_str(), "ab");
        short.push('c');
        let zero = ArrayString::<0>::new();
        assert!(zero.is_empty() && zero.is_full());
        assert_eq!(ArrayString::<3>::zero_filled().as_bytes(), &[0; 3]);
        assert_eq!(ArrayString::from_byte_string(b"a\xc3\xa9").unwrap().as_str(), "aé");
        assert!(ArrayString::from_byte_string(&[0xff]).is_err());
    }

    #[test]
    fn array_string_traits() {
        let mut s: ArrayString<16> = "abc".parse().unwrap();
        let copied = s;
        assert_eq!(copied, s.clone());
        let mut cloned = ArrayString::<16>::default();
        cloned.clone_from(&s);
        assert_eq!(cloned, s);
        assert_eq!(format!("{} {:?}", s, s), "abc \"abc\"");
        assert!(s == *"abc" && *"abc" == s);
        assert!(s < *"abd" && *"abb" < s);
        assert!(s < ArrayString::<16>::from("abd").unwrap());
        assert_eq!(<ArrayString<16> as AsRef<str>>::as_ref(&s), "abc");
        assert_eq!(<ArrayString<16> as AsRef<std::path::Path>>::as_ref(&s), std::path::Path::new("abc"));
        assert_eq!(<ArrayString<16> as Borrow<str>>::borrow(&s), "abc");
        <ArrayString<16> as BorrowMut<str>>::borrow_mut(&mut s).make_ascii_uppercase();
        s.as_mut_str().make_ascii_lowercase();
        assert_eq!(s.as_str(), "abc");
        let mut h1 = std::collections::hash_map::DefaultHasher::new();
        let mut h2 = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut h1);
        s.as_str().hash(&mut h2);
        assert_eq!(h1.finish(), h2.finish());
        write!(&mut s, " {}", 42).unwrap();
        s.write_char('é').unwrap();
        assert_eq!(s.as_str(), "abc 42é");
        let formatted = ArrayString::<16>::try_from(format_args!("{}:{}", "x", 7)).unwrap();
        assert_eq!(formatted.as_str(), "x:7");
        assert!(ArrayString::<1>::try_from(format_args!("{}", "abc")).is_err());
        assert_eq!(ArrayString::<2>::try_from("abc").unwrap_err().element(), "abc");
        assert!("abc".parse::<ArrayString<2>>().is_err());
        let mut short = ArrayString::<1>::new();
        assert!(short.write_char('é').is_err());
        assert!(short.write_str("ab").is_err());
        assert!(short.is_empty());
        unsafe {
            s.as_mut_ptr().write(b'A');
            s.set_len(3);
            assert_eq!(s.as_ptr().read(), b'A');
        }
        assert_eq!(s.as_str(), "Abc");
    }

    #[test]
    #[should_panic]
    fn array_string_truncate_requires_char_boundary() {
        ArrayString::<2>::from("é").unwrap().truncate(1);
    }

    #[test]
    #[should_panic]
    fn array_string_remove_requires_char_boundary() {
        ArrayString::<2>::from("é").unwrap().remove(1);
    }

    #[test]
    #[should_panic]
    fn array_string_push_panics_when_full() {
        ArrayString::<0>::new().push('x');
    }
}
