pub(super) trait BitOp {
    fn fill(start: u32, end: u32) -> u64;
}

impl BitOp for u64 {
    #[inline]
    fn fill(start: u32, end: u32) -> u64 {
        ((1u64 << (end - start + 1)) - 1) << start
    }
}
#[cfg(test)]
mod tests {
    use crate::seg::bit::BitOp;

    #[test]
    fn test_00() {
        assert_eq!(u64::fill(0, 2), 0b111);
        assert_eq!(u64::fill(1, 2), 0b110);
        assert_eq!(u64::fill(2, 2), 0b100);
    }
}
