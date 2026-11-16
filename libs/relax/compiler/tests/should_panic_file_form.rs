// `#[should_panic]` under `makepad test <file>` as libtest takes it: such a test passes by panicking,
// with the expected text when it names one (the `expected = ..` and the `= ..` forms), and a panic
// the test catches itself passes too. Relax issue 231: the file form ran a `#[should_panic]` test as
// a plain one, which failed with a report.

#[test]
#[should_panic]
fn a01_plain() {
    let v: Vec<u8> = Vec::new();
    let i = v.len() + 3;
    let _ = v[i];
}

#[test]
#[should_panic(expected = "is negative")]
fn a02_expected_in_a_formatted_message() {
    let n = std::hint::black_box(-4i32);
    if n < 0 {
        panic!("{n} is negative");
    }
}

#[test]
#[should_panic = "out of range"]
fn a03_name_value_form() {
    panic!("index out of range here");
}

#[test]
fn a04_caught_by_the_test() {
    let r = std::panic::catch_unwind(|| panic!("caught"));
    assert!(r.is_err());
}
