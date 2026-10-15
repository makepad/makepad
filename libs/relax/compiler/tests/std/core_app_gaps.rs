// std items the app sweep found missing: Default for Arc<[T]> / Rc<[T]> / Arc<str> / Rc<str>,
// PathBuf: From<&T: AsRef<OsStr>> (so `(&path_buf).into()`), Cow<Path> conversions, and BTreeMap
// lookups through Borrow<[T]> (Vec<String> keys looked up by &[String]).
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Default, Debug)]
struct Mesh {
    uv: Arc<[[f32; 2]]>,
    idx: Arc<[u32]>,
    name: Rc<str>,
    data: Rc<[u8]>,
}

#[test]
fn a01_default_slices() {
    let m = Mesh::default();
    println!("| mesh {:?} {} {} {} {}", m, m.uv.len(), m.idx.is_empty(), m.name.len(), m.data.len());
    let a: Arc<str> = Default::default();
    println!("| arc str {:?} {}", a, a.is_empty());
}

#[test]
fn a02_pathbuf_from_refs() {
    let base = PathBuf::from("/tmp/a");
    let p1: PathBuf = (&base).into();
    let p2 = PathBuf::from(&base);
    let s = String::from("x/y");
    let p3: PathBuf = (&s).into();
    let p4 = PathBuf::from("lit");
    let p5 = PathBuf::from(Path::new("p"));
    let p6 = PathBuf::from(std::ffi::OsStr::new("os"));
    let os = std::ffi::OsString::from("oss");
    let p7 = PathBuf::from(&os);
    println!("| {:?} {:?} {:?} {:?} {:?} {:?} {:?}", p1, p2, p3, p4, p5, p6, p7);
    let c: Cow<Path> = (&base).into();
    let back: PathBuf = c.clone().into();
    println!("| cow {:?} borrowed {} back {:?}", c, matches!(c, Cow::Borrowed(_)), back);
}

#[test]
fn a03_btree_borrow_slices() {
    let mut m: BTreeMap<Vec<String>, u32> = BTreeMap::new();
    m.insert(vec![String::from("inbox")], 1);
    m.insert(vec![String::from("inbox"), String::from("work")], 2);
    let key: Vec<String> = vec![String::from("inbox"), String::from("work")];
    let slice: &[String] = &key[..];
    println!("| get {:?} {:?} contains {}", m.get(slice), m.get(&key[..1]), m.contains_key(slice));
    let mut s: BTreeMap<String, u8> = BTreeMap::new();
    s.insert(String::from("k"), 7);
    println!("| str key {:?}", s.get("k"));
}

#[test]
fn a04_btree_slice_keys_more() {
    let mut m: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
    for (i, k) in [&b"a"[..], b"ab", b"b", b"ba", b"c"].iter().enumerate() {
        m.insert(k.to_vec(), i as u32);
    }
    let lo: &[u8] = b"ab";
    let hi: &[u8] = b"c";
    let r: Vec<u32> = m.range::<[u8], _>((std::ops::Bound::Included(lo), std::ops::Bound::Excluded(hi))).map(|(_, v)| *v).collect();
    println!("| range {:?}", r);
    let tail = m.split_off(&b"b"[..]);
    println!("| split {:?} {:?}", m.keys().collect::<Vec<_>>(), tail.keys().collect::<Vec<_>>());
    println!("| remove {:?} {:?}", m.remove(&b"ab"[..]), m.get(&b"a"[..]));
}

#[test]
fn a05_slice_keys_hash_and_set() {
    let mut h: std::collections::HashMap<Vec<String>, u8> = std::collections::HashMap::new();
    h.insert(vec![String::from("x")], 1);
    let k: &[String] = &[String::from("x")];
    println!("| hash get {:?} contains {}", h.get(k), h.contains_key(k));
    println!("| hash remove {:?}", h.remove(k));
    let mut s: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
    s.insert(b"ab".to_vec());
    s.insert(b"b".to_vec());
    let r: Vec<&Vec<u8>> = s.range::<[u8], _>((std::ops::Bound::Included(&b"a"[..]), std::ops::Bound::Unbounded)).collect();
    println!("| set range {:?} contains {}", r, s.contains(&b"b"[..]));
}
