//! Relax's built-in arrayvec vs arrayvec 0.7.6.

use arrayvec as ra;
use relax_std_ext_check::hr_arrayvec as ha;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
}

fn panic_message<F: FnOnce() + std::panic::UnwindSafe>(f: F) -> String {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(f);
    std::panic::set_hook(prev);
    match r {
        Ok(()) => "no panic".to_string(),
        Err(e) => {
            if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "?".to_string()
            }
        }
    }
}

#[test]
fn random_ops() {
    let mut rng = Rng(77);
    for round in 0..500 {
        let mut h: ha::ArrayVec<u32, 6> = ha::ArrayVec::new();
        let mut r: ra::ArrayVec<u32, 6> = ra::ArrayVec::new();
        for step in 0..80 {
            let x = rng.below(50) as u32;
            match rng.below(14) {
                0..=3 => assert_eq!(h.try_push(x).is_ok(), r.try_push(x).is_ok()),
                4 => assert_eq!(h.pop(), r.pop()),
                5 => {
                    let i = rng.below(r.len() as u64 + 1) as usize;
                    assert_eq!(h.try_insert(i, x).is_ok(), r.try_insert(i, x).is_ok());
                }
                6 => {
                    let i = rng.below(r.len() as u64 + 2) as usize;
                    assert_eq!(h.pop_at(i), r.pop_at(i));
                }
                7 => {
                    let i = rng.below(r.len() as u64 + 2) as usize;
                    assert_eq!(h.swap_pop(i), r.swap_pop(i));
                }
                8 => {
                    h.retain(|v| *v % 4 != 1);
                    r.retain(|v| *v % 4 != 1);
                }
                9 => {
                    let a = rng.below(r.len() as u64 + 1) as usize;
                    let b = a + rng.below((r.len() - a) as u64 + 1) as usize;
                    let hd: Vec<u32> = h.drain(a..b).collect();
                    let rd: Vec<u32> = r.drain(a..b).collect();
                    assert_eq!(hd, rd);
                }
                10 => {
                    let s = [x, x + 1];
                    assert_eq!(h.try_extend_from_slice(&s).is_ok(), r.try_extend_from_slice(&s).is_ok());
                }
                11 => {
                    let n = rng.below(7) as usize;
                    h.truncate(n);
                    r.truncate(n);
                }
                12 => assert_eq!(h.take().as_slice(), r.take().as_slice()),
                _ => {
                    h.clear();
                    r.clear();
                }
            }
            assert_eq!(h.as_slice(), r.as_slice(), "round {} step {}", round, step);
            assert_eq!((h.len(), h.is_full(), h.remaining_capacity()), (r.len(), r.is_full(), r.remaining_capacity()));
        }
        let hi: Vec<u32> = h.clone().into_iter().rev().collect();
        let ri: Vec<u32> = r.clone().into_iter().rev().collect();
        assert_eq!(hi, ri);
        assert_eq!(h.clone().into_inner().is_ok(), r.clone().into_inner().is_ok());
        assert_eq!(format!("{:?}", h), format!("{:?}", r));
    }
}

#[test]
fn conversions_and_errors() {
    let h = ha::ArrayVec::from([1u8, 2, 3]);
    let r = ra::ArrayVec::from([1u8, 2, 3]);
    assert_eq!(h.as_slice(), r.as_slice());
    let h: Result<ha::ArrayVec<u8, 2>, _> = ha::ArrayVec::try_from(&[1u8, 2, 3][..]);
    let r: Result<ra::ArrayVec<u8, 2>, _> = ra::ArrayVec::try_from(&[1u8, 2, 3][..]);
    assert_eq!(format!("{:?}", h.err().unwrap()), format!("{:?}", r.err().unwrap()));
    let mut h: ha::ArrayVec<u8, 1> = ha::ArrayVec::new();
    let mut r: ra::ArrayVec<u8, 1> = ra::ArrayVec::new();
    h.push(1);
    r.push(1);
    let he = h.try_push(2).unwrap_err();
    let re = r.try_push(2).unwrap_err();
    assert_eq!(format!("{} / {:?}", he, he), format!("{} / {:?}", re, re));
    assert_eq!(he.element(), re.element());
    let hc: ha::ArrayVec<u32, 5> = (0..4).collect();
    let rc: ra::ArrayVec<u32, 5> = (0..4).collect();
    assert_eq!(hc.as_slice(), rc.as_slice());
    assert_eq!(hc == hc.clone(), rc == rc.clone());
    assert_eq!(hc.partial_cmp(&hc), rc.partial_cmp(&rc));
}

#[test]
fn panics_match() {
    let pairs = [
        (
            panic_message(|| { let mut v: ha::ArrayVec<u8, 1> = ha::ArrayVec::new(); v.push(1); v.push(2); }),
            panic_message(|| { let mut v: ra::ArrayVec<u8, 1> = ra::ArrayVec::new(); v.push(1); v.push(2); }),
        ),
        (
            panic_message(|| { let mut v: ha::ArrayVec<u8, 3> = ha::ArrayVec::new(); v.insert(2, 0); }),
            panic_message(|| { let mut v: ra::ArrayVec<u8, 3> = ra::ArrayVec::new(); v.insert(2, 0); }),
        ),
        (
            panic_message(|| { let mut v: ha::ArrayVec<u8, 3> = ha::ArrayVec::new(); v.remove(0); }),
            panic_message(|| { let mut v: ra::ArrayVec<u8, 3> = ra::ArrayVec::new(); v.remove(0); }),
        ),
        (
            panic_message(|| { let mut v: ha::ArrayVec<u8, 3> = ha::ArrayVec::new(); v.swap_remove(1); }),
            panic_message(|| { let mut v: ra::ArrayVec<u8, 3> = ra::ArrayVec::new(); v.swap_remove(1); }),
        ),
        (
            panic_message(|| { let _v: ha::ArrayVec<u8, 2> = (0..5).collect(); }),
            panic_message(|| { let _v: ra::ArrayVec<u8, 2> = (0..5).collect(); }),
        ),
    ];
    for (h, r) in pairs.iter() {
        assert_eq!(h, r);
    }
}
