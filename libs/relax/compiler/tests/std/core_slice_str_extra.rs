// std-core differential test (core_diff.sh): Box<str>, chunks, utf8_chunks, slice Ord/Eq.
#[test]
fn a01() {
    let s = String::from("héllo");
    let b: Box<str> = s.into_boxed_str();
    println!("| {} {}", b, b.len());
    let t = str::into_string(b); // method form: F-STD17 (core_r05)
    println!("| {}", t);
    let x = [1i16, 2, 3, 4, 5];
    let l: &[i16; 2] = x.last_chunk().unwrap();
    println!("| {:?} {:?}", l, x.split_first_chunk::<2>().map(|(a, r)| (a[1], r.len())));
    for c in b"ab\xffcd".utf8_chunks() { println!("| [{}] {}", c.valid(), c.invalid().len()); }
    let bs: Box<str> = Box::from("zz");
    println!("| {}", bs);
}
#[test]
fn a02_slice_cmp() {
    let a: &[u8] = b"abc"; let b: &[u8] = b"abd";
    println!("| {:?} {} {} {}", a.cmp(b), a < b, a == b, a.to_vec() == b.to_vec());
    let mut v = vec![b"zz".to_vec(), b"a".to_vec()];
    v.sort();
    println!("| {:?}", v);
    let c = std::ffi::CString::new("hi").unwrap();
    let d = std::ffi::CString::new("ho").unwrap();
    println!("| {:?}", c.as_c_str().cmp(d.as_c_str()));
}
