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
    pub use relax_std_check::collections::{BTreeMap, HashMap};
    use relax_std_check::{
        fmt,
        fmt::rt::{Argument, Count, Placeholder, ALIGN_UNKNOWN},
    };
    pub use relax_std_check::{string::String, vec::Vec};
    pub use relax_std_os_check::sync::mpsc::channel;
    pub fn sort<T: Ord>(v: &mut [T]) {
        relax_std_check::slice::sort::merge_sort(v, &mut |a, b| a < b);
    }
    pub fn sort_unstable<T: Ord>(v: &mut [T]) {
        relax_std_check::slice::sort::quicksort(v, &mut |a, b| a < b);
    }
    pub fn sort_key<T, K: Ord, F: FnMut(&T) -> K>(v: &mut [T], mut f: F) {
        relax_std_check::slice::sort::merge_sort(v, &mut |a, b| f(a) < f(b));
    }
    pub fn search(v: &[u64], x: u64) -> usize {
        relax_std_check::slice::binary_search_by(v, |a| a.cmp(&x)).unwrap()
    }
    pub fn find(v: &str, p: &str) -> usize {
        relax_std_check::str::find(v, p).unwrap()
    }
    pub fn find_char(v: &str, p: char) -> usize {
        relax_std_check::str::find(v, p).unwrap()
    }
    pub fn fields(v: &str) -> usize {
        relax_std_check::str::split(v, ',').count() + relax_std_check::str::lines(v).count()
    }
    pub fn parse_u32(v: &str) -> u32 {
        relax_std_check::num::parse::parse_unsigned(v.as_bytes(), 10, u32::MAX as u64).unwrap()
            as u32
    }
    pub fn parse_i64(v: &str) -> i64 {
        relax_std_check::num::parse::parse_signed(v.as_bytes(), 10, i64::MIN, i64::MAX).unwrap()
    }
    pub fn parse_f64(v: &str) -> f64 {
        relax_std_check::dec2flt::parse_f64(v).unwrap()
    }
    pub fn ints(i: u64) -> String {
        let j = i * 3;
        let args = [Argument::new_display(&i), Argument::new_display(&j)];
        fmt::format(fmt::Arguments::new_v1(&["", " "], &args))
    }
    pub fn float_fixed(f: f64) -> String {
        let args = [Argument::new_display(&f)];
        let ph = [Placeholder::new(
            0,
            ' ',
            ALIGN_UNKNOWN,
            0,
            Count::Is(3),
            Count::Implied,
        )];
        fmt::format(fmt::Arguments::new_v1_formatted(&[""], &args, &ph))
    }
    pub fn float_short(f: f64) -> String {
        let args = [Argument::new_display(&f)];
        fmt::format(fmt::Arguments::new_v1(&[""], &args))
    }
    pub fn write_ints(s: &mut String, i: u64) {
        let j = i * 3;
        let args = [Argument::new_display(&i), Argument::new_display(&j)];
        fmt::write(s, fmt::Arguments::new_v1(&["", " "], &args)).unwrap();
    }
}
