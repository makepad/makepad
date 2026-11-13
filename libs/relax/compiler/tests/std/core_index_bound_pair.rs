// Indexing a slice or a str by `(Bound<usize>, Bound<usize>)` (SliceIndex since 1.53): `get` and the
// index over a grid of pairs, past both ends and out of order, with each failure's panic message.
// A str had no such index (E0277), and a slice's failures named other bounds than core's
// (`slice_index_fail`: a start past the length, else an end past it, else the order, else the end).
use std::ops::Bound::{self, Excluded, Included, Unbounded};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn pairs() -> Vec<(Bound<usize>, Bound<usize>)> {
    let starts = [Unbounded, Included(0), Included(2), Included(5), Included(6), Excluded(0), Excluded(2), Excluded(4), Excluded(5), Excluded(usize::MAX)];
    let ends = [Unbounded, Included(1), Included(4), Included(5), Included(usize::MAX), Excluded(0), Excluded(3), Excluded(5), Excluded(6)];
    starts.iter().flat_map(|s| ends.iter().map(move |e| (*s, *e))).collect()
}

fn outcome(f: impl FnOnce() -> String) -> String {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(e) => format!("panics: {}", e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()),
    }
}

#[test]
fn a01_slices() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut v = [1u8, 2, 3, 4, 5];
    for p in pairs() {
        let got = v.get(p).map(|s| s.to_vec());
        let at = outcome(|| format!("{:?}", &v[p]));
        let mutated = outcome(|| {
            let s = &mut v[p];
            s.reverse();
            format!("{:?}", s)
        });
        println!("| slice {:?}: get {:?} index {} mut {}", p, got, at, mutated);
        v = [1, 2, 3, 4, 5];
    }
}

#[test]
fn a02_strs() {
    std::panic::set_hook(Box::new(|_| {}));
    let s = "hello";
    let mut owned = String::from("hello");
    for p in pairs() {
        let at = outcome(|| s[p].to_string());
        let upper = outcome(|| {
            let part = &mut owned[p];
            part.make_ascii_uppercase();
            part.to_string()
        });
        println!("| str {:?}: get {:?} get_mut {:?} index {} mut {}", p, s.get(p), owned.clone().get_mut(p).map(|x| x.to_string()), at, upper);
        owned = String::from("hello");
    }
}

#[test]
fn a03_char_boundaries() {
    std::panic::set_hook(Box::new(|_| {}));
    let s = "aé€b";
    for p in [(Included(1), Excluded(3)), (Included(2), Unbounded), (Excluded(0), Included(6)), (Unbounded, Excluded(4))] {
        println!("| {:?}: get {:?} index {}", p, s.get(p), outcome(|| s[p].to_string()));
    }
}
