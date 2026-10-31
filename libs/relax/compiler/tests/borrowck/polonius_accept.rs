// Polonius (relaxed Rust A2): NLL rejects these (stable rustc: E0499 / E0502 / E0506), Polonius accepts
// them: the borrow returned on one path is not live on the other. Relaxed crates check clean; strict
// crates keep NLL's errors. main() runs every case (the behaviour is plain Rust's).
use std::collections::{BTreeMap, HashMap};

// NLL problem case #3 (rust-lang/rust#21906): get-or-insert through a match
fn get_default<'r>(map: &'r mut HashMap<u32, String>, key: u32) -> &'r mut String {
    match map.get_mut(&key) {
        Some(value) => value,
        None => {
            map.insert(key, String::new());
            map.get_mut(&key).unwrap()
        }
    }
}

// conditional early return of the borrow
fn get_or_insert<'a>(map: &'a mut HashMap<String, Vec<u32>>, key: &str) -> &'a mut Vec<u32> {
    if let Some(v) = map.get_mut(key) {
        return v;
    }
    map.insert(key.to_string(), Vec::new());
    map.get_mut(key).unwrap()
}

// a shared lookup returned on the hit path, a mutation on the miss path
fn cached<'a>(map: &'a mut BTreeMap<u64, u64>, n: u64) -> &'a u64 {
    if let Some(v) = map.get(&n) {
        return v;
    }
    map.insert(n, n * n);
    map.get(&n).unwrap()
}

struct Interner { names: HashMap<u32, String>, misses: u32 }
impl Interner {
    // the map is a field of `self`
    fn name(&mut self, id: u32) -> &mut String {
        if let Some(s) = self.names.get_mut(&id) {
            return s;
        }
        self.misses += 1;
        self.names.insert(id, format!("#{id}"));
        self.names.get_mut(&id).unwrap()
    }
}

// not a map lookup: no exact rewrite, the acceptance is reported
fn first_positive(v: &mut Vec<i32>) -> &mut i32 {
    if let Some(x) = v.iter_mut().find(|x| **x > 0) {
        return x;
    }
    v.push(1);
    v.last_mut().unwrap()
}

// the returned borrow is conditional on a test of the borrowed value
fn last_positive_or_push(v: &mut Vec<i32>) -> &mut i32 {
    if let Some(x) = v.last_mut() {
        if *x > 0 {
            return x;
        }
    }
    v.push(7);
    v.last_mut().unwrap()
}

// rust-lang/rust#46859 (Decoder): a loop that returns a borrow from one iteration and borrows again
// in the next
struct Reader { data: Vec<u8>, pos: usize }
impl Reader {
    fn fill(&mut self) -> &[u8] { self.pos += 1; &self.data[..self.pos] }
    fn word(&mut self) -> &[u8] {
        loop {
            let buf = self.fill();
            if let Some(w) = word_end(buf) {
                return w;
            }
        }
    }
}
fn word_end(b: &[u8]) -> Option<&[u8]> { if b.last() == Some(&b' ') { Some(&b[..b.len() - 1]) } else { None } }

fn main() {
    let mut m = HashMap::new();
    get_default(&mut m, 1).push('a');
    get_default(&mut m, 1).push('b');
    assert_eq!(m[&1], "ab");
    let mut g = HashMap::new();
    get_or_insert(&mut g, "k").push(3);
    get_or_insert(&mut g, "k").push(4);
    assert_eq!(g["k"], [3, 4]);
    let mut c = BTreeMap::new();
    assert_eq!(*cached(&mut c, 9), 81);
    assert_eq!(*cached(&mut c, 9), 81);
    let mut i = Interner { names: HashMap::new(), misses: 0 };
    i.name(5).push('!');
    assert_eq!(i.name(5), "#5!");
    assert_eq!(i.misses, 1);
    let mut v = vec![-1, 2];
    *first_positive(&mut v) += 10;
    assert_eq!(v, [-1, 12]);
    let mut w = vec![-3];
    assert_eq!(*last_positive_or_push(&mut w), 7);
    let mut r = Reader { data: b"abc def".to_vec(), pos: 0 };
    assert_eq!(r.word(), b"abc");
    println!("polonius_accept ok");
}
