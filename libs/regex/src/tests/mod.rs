macro_rules! test_regex {
    ($name:ident, $pattern:expr, $str:expr, $($pos:tt)+) => (
        #[test]
        fn $name() {
            use crate::regex::Regex;

            let regex = Regex::new($pattern).unwrap();
            let str = $str;
            let expected: Vec<Option<usize>> = vec![$($pos)+];
            let mut actual = vec![None; expected.len()];
            regex.run(str, &mut actual);
            assert_eq!(expected, actual);
        }
    );
}

mod basic;
mod nullsubexpr;
mod repetitions;
