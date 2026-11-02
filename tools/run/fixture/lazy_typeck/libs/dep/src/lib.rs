// lazy_typeck.sh: a dependency outside the app scope (under libs/) whose `broken` rustc rejects
// (E0308). `makepad check` and `makepad test` report it; `makepad run` reports it from its
// background check, and a call of it parks until a live edit fixes it.
pub fn fine(x: u32) -> u32 {
    x + 1
}

pub fn broken(x: u32) -> u32 {
    let n: u32 = "ten";
    x * n
}

#[test]
fn fine_adds_one() {
    assert_eq!(fine(2), 3);
}
