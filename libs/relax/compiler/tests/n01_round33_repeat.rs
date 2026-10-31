// typeck-audit 1.14: repeat counts are structurally resolved (fn_ctxt/checks.rs:85-186).
fn param<const N: usize>() -> [u8; N] { [7u8; N] }

#[test]
fn repeat_counts() {
    let one: [String; 1] = [String::from("x"); 1];
    let none: [String; 0] = [String::new(); 0];
    let inferred: [u8; 3] = [1; _];
    assert_eq!(one[0], "x");
    assert_eq!(none.len(), 0);
    assert_eq!(inferred, [1, 1, 1]);
    assert_eq!(param::<4>(), [7; 4]);
}
