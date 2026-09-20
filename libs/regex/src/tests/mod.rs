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

mod word_boundary {
    use crate::Regex;

    fn span(pattern: &str, text: &str) -> Option<(usize, usize)> {
        let regex = Regex::new(pattern).unwrap();
        let mut slots = [None; 2];
        regex.run(text, &mut slots).then(|| (slots[0].unwrap(), slots[1].unwrap()))
    }

    #[test]
    fn word_boundary_escapes() {
        assert_eq!(span(r"\bfoo\b", "a foo b"), Some((2, 5)));
        assert_eq!(span(r"\bfoo\b", "afoo foob"), None);
        assert_eq!(span(r"\Bfoo", "afoo"), Some((1, 4)));
        assert_eq!(span(r"\Bfoo", "foo"), None);
        assert_eq!(span(r"\bfn\b", "fn main"), Some((0, 2)));
        assert!(Regex::new(r"\q").is_err(), "unknown escapes stay errors");
    }
}

mod earliest_match {
    use crate::{ParseOptions, Regex};

    fn regex(pattern: &str) -> Regex {
        Regex::new_with_options(
            pattern,
            ParseOptions {
                multiline: true,
                ..ParseOptions::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn the_earliest_match_is_the_first_one_and_run_reports_the_last() {
        let text = "x\nx\n\nx\n";
        let anchored = regex("^x");
        assert_eq!(anchored.earliest_match(text), Some((0, 1)));
        assert_eq!(anchored.earliest_match(&text[2..]), Some((0, 1)));
        assert_eq!(anchored.earliest_match(&text[4..]), Some((1, 2)));
        let mut slots = [None; 2];
        assert!(anchored.run(text, &mut slots));
        assert_eq!(slots, [Some(5), Some(6)], "run: the match that ends last");
        assert_eq!(regex("abc|b").earliest_match("xabc"), Some((2, 3)), "the first end, not the longest");
        assert_eq!(regex("ab|abc").earliest_match("xabcabc"), Some((1, 3)));
        assert_eq!(regex("b+").earliest_match("abbb"), Some((1, 2)), "the shortest end");
        assert_eq!(regex("z").earliest_match("abc"), None);
        assert_eq!(regex("").earliest_match("abc"), Some((0, 0)));
        assert_eq!(regex("^.*$").earliest_match("ab\ncd"), Some((0, 2)));
        assert_eq!(regex("\\bcd\\b").earliest_match("abcd cd"), Some((5, 7)));
        // non-ASCII input with a word boundary takes the NFA path
        assert_eq!(regex("\\bcd\\b").earliest_match("é abcd cd"), Some((8, 10)));
    }
}

mod ascii_word_boundary {
    use crate::{ParseOptions, Regex};

    fn find(pattern: &str, ascii: bool, text: &str) -> Option<(usize, usize)> {
        Regex::new_with_options(
            pattern,
            ParseOptions {
                multiline: true,
                ascii_word_boundary: ascii,
                ..ParseOptions::default()
            },
        )
        .unwrap()
        .earliest_match(text)
    }

    #[test]
    fn ascii_boundaries_stay_in_the_dfa_and_agree_with_the_nfa() {
        // ASCII text: both semantics agree
        assert_eq!(find(r"\bcd\b", true, "abcd cd"), Some((5, 7)));
        assert_eq!(find(r"\bcd\b", false, "abcd cd"), Some((5, 7)));
        // a non-ASCII letter: Unicode makes it a word character, ASCII does not
        assert_eq!(find(r"\bcd\b", false, "écd cd"), Some((5, 7)));
        assert_eq!(find(r"\bcd\b", true, "écd cd"), Some((2, 4)));
        // the NFA (captures) follows the same rule as the DFA
        let regex = Regex::new_with_options(
            r"(\bcd\b)",
            ParseOptions { ascii_word_boundary: true, ..ParseOptions::default() },
        )
        .unwrap();
        let mut slots = [None; 4];
        assert!(regex.run("écd cd", &mut slots));
        assert_eq!(slots[..2], slots[2..], "the capture is the match");
        assert!(matches!(slots[0], Some(2) | Some(5)), "{slots:?}");
        // no NFA hand-off: a long non-ASCII text is still linear and fast
        let text = "é ".repeat(200_000) + "cd";
        let started = std::time::Instant::now();
        assert_eq!(find(r"\bcd\b", true, &text), Some((text.len() - 2, text.len())));
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "{:?}", started.elapsed());
    }
}
