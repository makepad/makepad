// The NAOMS pilot's repro: without `--features x` the test module's import does not resolve, so
// rustc rejects the cfg(test) lib (E0432) and `cargo test` runs nothing. `makepad test` must fail the
// same way, not pass `a` and fail only `b` at run time.
#[cfg(feature = "x")]
pub struct Thing;

#[cfg(test)]
mod t {
    use crate::Thing;
    #[test]
    fn a() {
        assert!(true)
    }
    #[test]
    fn b() {
        let _ = Thing;
    }
}
