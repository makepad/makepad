#[cfg(test)]
mod t {
    #[test]
    fn uses_mid() {
        assert!(tdk_mid::mid_value() > 1);
    }
}
