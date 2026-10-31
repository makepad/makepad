// make_mut / From<Box<T>> on struct-tailed DSTs (CStr, OsStr, Path): their CloneToUninit is a byte
// copy of size_of_val(self). Under Relax this needs size_of_val of a struct whose last field is
// unsized (CStr { inner: [c_char] }), which Relax currently reports as 0 (frontend item, coord.md
// 2026-10-04 std-core): this file is the repro and differs from rustc until that is fixed.
use std::ffi::{CStr, OsStr};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

#[test]
fn a01_size_of_val_struct_tail() {
    let b: Box<CStr> = Box::from(c"abc");
    println!("| cstr size_of_val {} align {}", std::mem::size_of_val(&*b), std::mem::align_of_val(&*b));
    println!("| path size_of_val {}", std::mem::size_of_val(Path::new("/a/b")));
}

#[test]
fn a02_make_mut_struct_tail() {
    let mut c: Rc<CStr> = Rc::from(Box::<CStr>::from(c"abc"));
    let c2 = c.clone();
    let _ = Rc::make_mut(&mut c);
    println!("| cstr {:?} {:?} {}", c, c2, Rc::ptr_eq(&c, &c2));
    let mut o: Arc<OsStr> = Arc::from(Box::<OsStr>::from(OsStr::new("os")));
    let o2 = o.clone();
    let _ = Arc::make_mut(&mut o);
    println!("| osstr {:?} {:?}", o, o2);
    let mut pth: Rc<Path> = Rc::from(Box::<Path>::from(Path::new("/a/b")));
    let p2 = pth.clone();
    let _ = Rc::make_mut(&mut pth);
    println!("| path {:?} {:?}", pth, p2);
}
