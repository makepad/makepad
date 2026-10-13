#![feature(try_trait_v2, try_trait_v2_residual)]
// rustc_ast_lowering/src/expr.rs:1945-2033 @8d1a764.
use std::ops::ControlFlow;
use std::task::Poll;
fn flow(x: ControlFlow<u32, u32>) -> ControlFlow<u32, u32> {
    let v = x?;
    ControlFlow::Continue(v + 1)
}
fn option(x: Option<u32>) -> Option<u32> { Some(x? + 1) }
fn result(x: Result<u32, u8>) -> Result<u32, u32> { Ok(x? + 1) }
#[test]
fn try_carriers() {
    assert!(matches!(flow(ControlFlow::Continue(2)), ControlFlow::Continue(3)));
    assert!(matches!(flow(ControlFlow::Break(4)), ControlFlow::Break(4)));
    assert_eq!(option(Some(2)), Some(3));
    assert_eq!(option(None), None);
    assert_eq!(result(Ok(2)), Ok(3));
    assert_eq!(result(Err(4)), Err(4));
}
fn poll_result(x: Poll<Result<u32, u8>>) -> Poll<Result<u32, u32>> {
    match x? { Poll::Ready(v) => Poll::Ready(Ok(v + 1)), Poll::Pending => Poll::Pending }
}
fn poll_option(x: Poll<Option<Result<u32, u8>>>) -> Poll<Option<Result<u32, u32>>> {
    match x? { Poll::Ready(Some(v)) => Poll::Ready(Some(Ok(v + 1))), Poll::Ready(None) => Poll::Ready(None), Poll::Pending => Poll::Pending }
}
#[test]
fn try_poll_branches() {
    assert_eq!(poll_result(Poll::Ready(Ok(2))), Poll::Ready(Ok(3)));
    assert_eq!(poll_result(Poll::Ready(Err(4))), Poll::Ready(Err(4)));
    assert_eq!(poll_result(Poll::Pending), Poll::Pending);
    assert_eq!(poll_option(Poll::Ready(Some(Ok(2)))), Poll::Ready(Some(Ok(3))));
    assert_eq!(poll_option(Poll::Ready(Some(Err(4)))), Poll::Ready(Some(Err(4))));
    assert_eq!(poll_option(Poll::Ready(None)), Poll::Ready(None));
    assert_eq!(poll_option(Poll::Pending), Poll::Pending);
}

enum Carrier<T> { Value(T), Stop }
impl<T> std::ops::Residual<T> for Carrier<std::convert::Infallible> {
    type TryType = Carrier<T>;
}
impl<T> std::ops::Try for Carrier<T> {
    type Output = T;
    type Residual = Carrier<std::convert::Infallible>;
    fn from_output(value: T) -> Self { Self::Value(value) }
    fn branch(self) -> ControlFlow<Self::Residual, T> {
        match self { Self::Value(value) => ControlFlow::Continue(value), Self::Stop => ControlFlow::Break(Carrier::Stop) }
    }
}
impl<T> std::ops::FromResidual<Carrier<std::convert::Infallible>> for Carrier<T> {
    fn from_residual(residual: Carrier<std::convert::Infallible>) -> Self {
        match residual { Carrier::Stop => Self::Stop, Carrier::Value(never) => match never {} }
    }
}
fn carrier(value: Carrier<u32>) -> Carrier<u32> { Carrier::Value(value? + 1) }
#[test]
fn user_defined_try_carrier() {
    assert!(matches!(carrier(Carrier::Value(3)), Carrier::Value(4)));
    assert!(matches!(carrier(Carrier::Stop), Carrier::Stop));
}
