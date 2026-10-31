fn f(c: &[u8]) -> u32 { if c == b"." { 1 } else if c == b".." { 2 } else { 0 } }
#[test] fn t() { assert!(f(b".") + f(b"..") + f(b"x") == 3); }
