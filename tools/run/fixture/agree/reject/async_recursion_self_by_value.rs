// rejects: E0733
// An async fn awaiting itself by value holds its own future across the await: rustc's layout cycle.
pub async fn countdown(n: u32) -> u32 {
    if n == 0 { 0 } else { countdown(n - 1).await + 1 }
}

#[test]
fn uses_it() {
    let _ = countdown(3);
}
