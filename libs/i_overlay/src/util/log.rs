pub(crate) trait Int {
    fn log2_sqrt(&self) -> usize;
}

impl Int for usize {
    #[inline]
    fn log2_sqrt(&self) -> usize {
        let z = self.leading_zeros();
        let i = (usize::BITS - z) as usize;
        let n = (i + 1) >> 1;
        1 << n
    }
}

#[cfg(test)]
mod tests {
    use crate::util::log::Int;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn test_0() {
        let tests: Vec<[usize; 2]> = vec![[0, 1], [1, 2], [3, 2], [15, 4], [16, 8], [255, 16], [256, 32]];

        for test in tests {
            let a = test[0].log2_sqrt();
            let b = test[1];
            assert_eq!(a, b);
        }
    }
}
