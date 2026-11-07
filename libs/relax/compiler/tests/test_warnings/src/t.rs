use std::collections::HashMap;

#[test]
fn a() {
    let unused = 3;
    super::parse("4");
    assert_eq!(super::half(4), 2);
}
