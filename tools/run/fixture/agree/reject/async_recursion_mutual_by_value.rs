// rejects: E0733
// Two async fns awaiting each other by value: each future holds the other, so both hold themselves.
// The tests make each end the first one used.
pub async fn ping(n: u32) -> u32 {
    if n == 0 { 0 } else { pong(n - 1).await + 1 }
}
pub async fn pong(n: u32) -> u32 {
    ping(n).await * 2
}

#[test]
fn ping_first() {
    let _ = ping(3);
}

#[test]
fn pong_first() {
    let _ = pong(3);
}
