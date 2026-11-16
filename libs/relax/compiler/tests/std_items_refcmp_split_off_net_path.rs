//! std items a large app's dependency graph uses, against rustc's std: comparisons through
//! references, bool's bit operators by reference, pointer provenance and alignment, `Cow<str>`
//! sums, slice `split_off`, set and deque iterator impls, `ToSocketAddrs`' iterator types,
//! address comparisons both ways, path iterators as paths, aarch64 feature names and NEON
//! pairwise, table, unzip and zip lanes, and `CommandExt::uid`.
use std::borrow::Cow;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// An `Ord::cmp` written as `self < other` on references, with a `lt` of its own and a
/// `partial_cmp` that is `cmp`: comparing references forwards every method, as core does, or
/// `cmp` recursed until the stack overflowed.
#[test]
fn reference_comparisons_keep_the_types_own_methods() {
    #[derive(Clone, Copy, PartialEq, Debug)]
    struct Ordered(f64);
    impl Eq for Ordered {}
    impl PartialOrd for Ordered {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
        fn lt(&self, other: &Self) -> bool {
            !self.ge(other)
        }
        fn le(&self, other: &Self) -> bool {
            other.ge(self)
        }
        fn gt(&self, other: &Self) -> bool {
            !other.ge(self)
        }
        fn ge(&self, other: &Self) -> bool {
            self.0.is_nan() | (self.0 >= other.0)
        }
    }
    impl Ord for Ordered {
        fn cmp(&self, other: &Self) -> Ordering {
            if self < other {
                Ordering::Less
            } else if self > other {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
    }
    let (a, b, nan) = (Ordered(1.0), Ordered(2.0), Ordered(f64::NAN));
    assert_eq!(a.cmp(&b), Ordering::Less);
    assert_eq!(nan.cmp(&b), Ordering::Greater);
    let mut v = vec![b, nan, a];
    v.sort();
    assert_eq!((v[0].0, v[1].0), (1.0, 2.0));
    assert!(v[2].0.is_nan());
    assert!(&mut a.clone() < &mut b.clone() && &a <= &b && &b >= &a && !(&a > &b));

    struct Never;
    impl PartialEq for Never {
        fn eq(&self, _: &Self) -> bool {
            true
        }
        fn ne(&self, _: &Self) -> bool {
            true
        }
    }
    assert!(&Never != &Never, "a reference's `ne` is the type's own");
}

#[test]
fn bool_bit_operators_by_reference() {
    fn all<T>(a: T, b: T) -> (T, T, T)
    where
        for<'a> T: std::ops::BitAnd<&'a T, Output = T> + std::ops::BitOr<&'a T, Output = T> + std::ops::BitXor<&'a T, Output = T> + Copy,
    {
        (a & &b, a | &b, a ^ &b)
    }
    assert_eq!(all(true, false), (false, true, true));
    let (t, f) = (&true, &false);
    assert_eq!((t & f, t | false, true ^ t), (false, true, false));
    let mut m = true;
    m &= &false;
    m |= &true;
    m ^= &true;
    assert!(!m);
}

#[test]
fn pointer_provenance_alignment_and_cow_sums() {
    let mut x = 7u64;
    let p: *mut u64 = &mut x;
    assert_eq!(p.expose_provenance(), p as usize);
    assert_eq!((p as *const u64).expose_provenance(), p as usize);
    let nn = std::ptr::NonNull::new(p).unwrap();
    assert_eq!(nn.align_offset(8), 0);
    let joined = Cow::Borrowed("a") + Cow::Borrowed("b");
    assert_eq!(joined, "ab");
    let mut c = Cow::Borrowed("");
    c += "x";
    assert!(matches!(c, Cow::Borrowed("x")), "an empty Cow takes the borrowed str as it is");
    c += Cow::Owned("y".to_string());
    assert_eq!(c, "xy");
}

#[test]
fn slice_split_off() {
    let mut s: &[i32] = &[1, 2, 3, 4, 5, 6];
    assert_eq!(s.split_off(..2), Some(&[1, 2][..]));
    assert_eq!(s.split_off(3..), Some(&[6][..]));
    assert_eq!(s.split_off(..=0), Some(&[3][..]));
    assert_eq!(s.split_off_first(), Some(&4));
    assert_eq!(s.split_off_last(), Some(&5));
    assert!(s.is_empty());
    assert_eq!(s.split_off(..1), None);
    assert_eq!(s.split_off_first(), None);
    let mut a = [1, 2, 3, 4, 5];
    let mut m: &mut [i32] = &mut a;
    m.split_off_mut(..1).unwrap()[0] = 10;
    *m.split_off_first_mut().unwrap() = 20;
    *m.split_off_last_mut().unwrap() = 50;
    m.split_off_mut(1..).unwrap()[0] = 40;
    assert_eq!(m.len(), 1);
    assert!(m.split_off_mut(..9).is_none());
    assert_eq!(a, [10, 20, 3, 40, 50]);
    let mut v = vec![1u8, 2, 3];
    let r = v.as_mut_ptr_range();
    assert_eq!(unsafe { r.end.offset_from(r.start) }, 3);
    unsafe { *r.start = 7 };
    assert_eq!(v[0], 7);
}

#[test]
fn iterator_types_and_impls() {
    use std::collections::{BTreeSet, VecDeque};
    use std::net::{SocketAddr, ToSocketAddrs};
    let addr: SocketAddr = "127.0.0.1:80".parse().unwrap();
    let one: std::option::IntoIter<SocketAddr> = addr.to_socket_addrs().unwrap();
    assert_eq!(one.collect::<Vec<_>>(), [addr]);
    let list = [addr, addr];
    let many: std::iter::Cloned<std::slice::Iter<'_, SocketAddr>> = (&list[..]).to_socket_addrs().unwrap();
    assert_eq!(many.count(), 2);
    let deque: VecDeque<u8> = [1, 2].into_iter().collect();
    let cycled: Vec<u8> = deque.into_iter().cycle().take(5).collect();
    assert_eq!(cycled, [1, 2, 1, 2, 1]);
    let (a, b): (BTreeSet<u8>, BTreeSet<u8>) = ([1, 2, 3].into(), [2, 3, 4].into());
    assert_eq!(a.union(&b).clone().count(), 4);
    assert_eq!(a.intersection(&b).clone().count(), 2);
    assert_eq!(a.difference(&b).clone().count(), 1);
    assert_eq!(a.symmetric_difference(&b).clone().count(), 2);
    let shown = [format!("{:?}", a.union(&b)), format!("{:?}", a.intersection(&b)), format!("{:?}", a.difference(&b)), format!("{:?}", a.symmetric_difference(&b))];
    assert!(shown.iter().all(|s| s.contains('2') || s.contains('1')), "{:?}", shown);
    assert!(format!("{:?}", a.clone().into_iter()).contains('1'));
    assert!(format!("{:?}", VecDeque::from([5u8]).into_iter()).contains('5'));
}

#[test]
fn ip_addresses_compare_with_ip_addr_both_ways() {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let v4 = Ipv4Addr::new(127, 0, 0, 1);
    let v6 = Ipv6Addr::LOCALHOST;
    let (any4, any6) = (IpAddr::V4(v4), IpAddr::V6(v6));
    assert!(v4 == any4 && any4 == v4 && v6 == any6 && any6 == v6);
    assert!(v4 != any6 && v6 != any4);
    assert!(v4 < any6 && any6 > v4 && v6 > any4 && any4 < v6);
    assert_eq!(v6.partial_cmp(&any6), Some(Ordering::Equal));
}

#[test]
fn components_and_iter_as_path() {
    let mut rest = Path::new("a/b/c").components();
    rest.next();
    let mut p = PathBuf::from("x");
    p.push(rest.clone());
    assert_eq!(p, Path::new("x/b/c"));
    let os: &std::ffi::OsStr = rest.as_ref();
    assert_eq!(os, "b/c");
    let mut iter = Path::new("a/b").iter();
    iter.next();
    let tail: &Path = iter.as_ref();
    assert_eq!(tail, Path::new("b"));
    let tail_os: &std::ffi::OsStr = iter.as_ref();
    assert_eq!(tail_os, "b");
}

/// This process's own user: setuid to it needs no privilege, and the child starts through the
/// fork path (posix_spawn cannot change the child's user).
#[cfg(unix)]
#[test]
fn command_ext_uid() {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::process::CommandExt;
    let file = std::env::temp_dir().join(format!("relax-uid-{}", std::process::id()));
    std::fs::write(&file, b"x").unwrap();
    let uid = std::fs::metadata(&file).unwrap().uid();
    let gid = std::fs::metadata(&file).unwrap().gid();
    std::fs::remove_file(&file).unwrap();
    assert!(std::process::Command::new("true").uid(uid).gid(gid).status().unwrap().success());
}

#[cfg(target_arch = "aarch64")]
#[test]
fn aarch64_feature_names() {
    assert!(std::arch::is_aarch64_feature_detected!("neon"));
    let _ = (std::arch::is_aarch64_feature_detected!("paca"), std::arch::is_aarch64_feature_detected!("sve2"));
    let _ = (std::arch::is_aarch64_feature_detected!("bti"), std::arch::is_aarch64_feature_detected!("mte"));
    // the second word of the cache: a feature past bit 63 does not disturb the first ones
    assert_eq!(std::arch::is_aarch64_feature_detected!("neon"), std::arch::is_aarch64_feature_detected!("asimd"));
}

/// Pairwise min/max, two-register tables, unzips and zips (the NEON paths of base64 and hex codecs).
#[cfg(target_arch = "aarch64")]
#[test]
fn neon_pairwise_tables_unzip_zip() {
    use std::arch::aarch64::*;
    unsafe {
        let a: uint8x8_t = std::mem::transmute([1u8, 2, 3, 4, 5, 6, 7, 8]);
        let b: uint8x8_t = std::mem::transmute([9u8, 10, 11, 12, 13, 14, 15, 16]);
        let at: [u8; 8] = std::mem::transmute(vpmin_u8(a, b));
        assert_eq!(at, [1, 3, 5, 7, 9, 11, 13, 15]);
        let m: [u32; 2] = std::mem::transmute(vpmax_u32(std::mem::transmute([3u32, 9]), std::mem::transmute([7u32, 2])));
        assert_eq!(m, [9, 7]);
        let u1: [u8; 8] = std::mem::transmute(vuzp1_u8(a, b));
        assert_eq!(u1, [1, 3, 5, 7, 9, 11, 13, 15]);
        let uz = vuzp_u8(a, b);
        let u2: [u8; 8] = std::mem::transmute(uz.1);
        assert_eq!(u2, [2, 4, 6, 8, 10, 12, 14, 16]);
        let t2: [u8; 8] = std::mem::transmute(vtbl2_u8(uint8x8x2_t(a, b), std::mem::transmute([0u8, 8, 15, 16, 7, 3, 255, 9])));
        assert_eq!(t2, [1, 9, 16, 0, 8, 4, 0, 10]);
        let q: uint8x16_t = std::mem::transmute(*b"0123456789abcdef");
        let r: uint8x16_t = std::mem::transmute(*b"ghijklmnopqrstuv");
        let tq: [u8; 16] = std::mem::transmute(vqtbl2q_u8(uint8x16x2_t(q, r), std::mem::transmute([0u8, 16, 31, 32, 15, 17, 200, 1, 2, 3, 4, 5, 6, 7, 8, 9])));
        assert_eq!(&tq, &[b'0', b'g', b'v', 0, b'f', b'h', 0, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9']);
        let uq = vuzpq_u8(q, r);
        let (e, o): ([u8; 16], [u8; 16]) = (std::mem::transmute(uq.0), std::mem::transmute(uq.1));
        assert_eq!(&e, b"02468acegikmoqsu");
        assert_eq!(&o, b"13579bdfhjlnprtv");
        let z = vzipq_u8(q, r);
        let (lo, hi): ([u8; 16], [u8; 16]) = (std::mem::transmute(z.0), std::mem::transmute(z.1));
        assert_eq!(&lo, b"0g1h2i3j4k5l6m7n");
        assert_eq!(&hi, b"8o9paqbrcsdteufv");
        let z16 = vzipq_u16(std::mem::transmute([0u16, 1, 2, 3, 4, 5, 6, 7]), std::mem::transmute([10u16, 11, 12, 13, 14, 15, 16, 17]));
        let (l16, h16): ([u16; 8], [u16; 8]) = (std::mem::transmute(z16.0), std::mem::transmute(z16.1));
        assert_eq!((l16, h16), ([0, 10, 1, 11, 2, 12, 3, 13], [4, 14, 5, 15, 6, 16, 7, 17]));
        let z32 = vzipq_u32(std::mem::transmute([0u32, 1, 2, 3]), std::mem::transmute([10u32, 11, 12, 13]));
        let (l32, h32): ([u32; 4], [u32; 4]) = (std::mem::transmute(z32.0), std::mem::transmute(z32.1));
        assert_eq!((l32, h32), ([0, 10, 1, 11], [2, 12, 3, 13]));
    }
}
