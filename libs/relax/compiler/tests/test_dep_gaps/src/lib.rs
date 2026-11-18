pub fn good() -> u32 {
    rdg_good::three()
}

#[cfg(test)]
mod t {
    // t0 reaches no body of rdg-bad, and runs first (run.sh: one test thread) for as long as the
    // background type check takes; t1..t3 each call the body that does not compile
    #[test]
    fn t0() {
        assert_eq!(super::good(), 3);
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
    #[test]
    fn t1() {
        assert_eq!(rdg_bad::uses_missing(2), 3);
    }
    #[test]
    fn t2() {
        assert_eq!(rdg_bad::uses_missing(3), 4);
    }
    #[test]
    fn t3() {
        assert_eq!(rdg_bad::uses_missing(4), 5);
    }
}
