// A panic leaving an `extern "C"` function (here a comparator libc's qsort calls back) cannot
// unwind: the process aborts ("panic in a function that cannot unwind"), as rustc does. Run by
// bench_rt/interp/unwind_cmp.py.
extern "C" {
    fn qsort(base: *mut u8, n: usize, size: usize, cmp: extern "C" fn(*const u8, *const u8) -> i32);
}
extern "C" fn xi_cmp_panics(_a: *const u8, _b: *const u8) -> i32 {
    panic!("comparator");
}
#[test]
fn x02_panic_out_of_extern_c_aborts() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut v = [3u32, 1, 2];
    let _ = std::panic::catch_unwind(move || unsafe { qsort(v.as_mut_ptr() as *mut u8, 3, 4, xi_cmp_panics) });
    println!("| not reached");
}
