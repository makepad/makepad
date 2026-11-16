// rejects: E0733
// Three async fns in a ring, each awaiting the next by value: every future holds itself.
pub async fn first(n: u32) -> u32 {
    if n == 0 { 0 } else { second(n - 1).await + 1 }
}
pub async fn second(n: u32) -> u32 {
    third(n).await ^ 1
}
pub async fn third(n: u32) -> u32 {
    first(n).await * 3
}

#[test]
fn from_the_last() {
    let _ = third(2);
}
