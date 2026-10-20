// Recursive `async fn`s that box the recursive future are well-formed (the check for E0733 only
// follows futures held by value); the ui suite has the by-value compile-fail cases. (Not run:
// lowering a poll function whose frame holds its own future type as a temporary is a JIT limit.)
use std::future::Future;
use std::pin::Pin;

#[allow(dead_code)]
async fn sum(n: u32) -> u32 {
    if n == 0 { 0 } else { n + Box::pin(sum(n - 1)).await }
}
#[allow(dead_code)]
fn sum_dyn(n: u32) -> Pin<Box<dyn Future<Output = u32>>> {
    Box::pin(async move { if n == 0 { 0 } else { n + sum_dyn(n - 1).await } })
}

#[test]
fn boxed_recursion_is_well_formed() {}
