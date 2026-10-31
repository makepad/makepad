pub mod s {
    // The shim UnsafeCell is not rustc's lang item; see README and probe log.
    pub use std::{cell::RefCell, rc::Rc};
    pub type RcVec<T> = std::vec::Vec<T>;
    // Entire hashset_string row uses real std in B: shim String lacks Hash.
    pub use std::collections::HashSet;
    pub type SetVec<T> = std::vec::Vec<T>;
    pub fn set_key(i: u64) -> std::string::String {
        format!("{} {}", i, i * 3)
    }
    pub use std::collections::{BTreeMap, HashMap};
    pub use std::sync::mpsc::channel;
    pub use std::{string::String, vec::Vec};
    pub fn sort<T: Ord>(v: &mut [T]) {
        v.sort();
    }
    pub fn sort_unstable<T: Ord>(v: &mut [T]) {
        v.sort_unstable();
    }
    pub fn sort_key<T, K: Ord, F: FnMut(&T) -> K>(v: &mut [T], f: F) {
        v.sort_by_key(f);
    }
    pub fn search(v: &[u64], x: u64) -> usize {
        v.binary_search(&x).unwrap()
    }
    pub fn find(v: &str, p: &str) -> usize {
        v.find(p).unwrap()
    }
    pub fn find_char(v: &str, p: char) -> usize {
        v.find(p).unwrap()
    }
    pub fn fields(v: &str) -> usize {
        v.split(',').count() + v.lines().count()
    }
    pub fn parse_u32(v: &str) -> u32 {
        v.parse().unwrap()
    }
    pub fn parse_i64(v: &str) -> i64 {
        v.parse().unwrap()
    }
    pub fn parse_f64(v: &str) -> f64 {
        v.parse().unwrap()
    }
    pub fn ints(i: u64) -> String {
        format!("{} {}", i, i * 3)
    }
    pub fn float_fixed(f: f64) -> String {
        format!("{:.3}", f)
    }
    pub fn float_short(f: f64) -> String {
        format!("{}", f)
    }
    pub fn write_ints(s: &mut String, i: u64) {
        use std::fmt::Write;
        write!(s, "{} {}", i, i * 3).unwrap();
    }
}
