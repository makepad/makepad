#![feature(try_trait_v2)]
// rustc library/core/src/ops/try_trait.rs:160 @8d1a764:
// Try's residual must recover a TryType with the same output and residual.
struct Carrier;
impl std::ops::Try for Carrier {
    type Output = ();
    type Residual = ();
    fn from_output(_: ()) -> Self { Carrier }
    fn branch(self) -> std::ops::ControlFlow<(), ()> {
        std::ops::ControlFlow::Continue(())
    }
}
impl std::ops::FromResidual<()> for Carrier {
    fn from_residual(_: ()) -> Self { Carrier }
}
fn main() {}
