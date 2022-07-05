mod test_data;

#[test]
fn test_graphemes() {
    for test in test_data::grapheme::TEST_DATA {
        use makepad_segment::str::StrExt;

        assert_eq!(
            test.join("")
                .as_str()
                .graphemes()
                .collect::<Vec<_>>()
                .as_slice(),
            test,
        );
    }
}

#[test]
fn test_graphemes_rev() {
    for test in test_data::grapheme::TEST_DATA {
        use makepad_segment::str::StrExt;

        assert_eq!(
            test.join("").as_str().graphemes().rev().collect::<Vec<_>>(),
            test.iter().cloned().rev().collect::<Vec<_>>(),
        );
    }
}

#[test]
fn test_grapheme_indices() {
    for test in test_data::grapheme::TEST_DATA {
        use makepad_segment::str::StrExt;

        let mut start = 0;
        assert_eq!(
            test.join("")
                .as_str()
                .grapheme_indices()
                .collect::<Vec<_>>(),
            test
                .iter()
                .cloned()
                .map(|word| {
                    let position = start;
                    start += word.len();
                    (position, word)
                })
                .collect::<Vec<_>>(),
        );
    }
}

#[test]
fn test_grapheme_indices_rev() {
    for test in test_data::grapheme::TEST_DATA {
        use makepad_segment::str::StrExt;

        let string = test.join("");
        let mut end = string.len(); 
        assert_eq!(
            string
                .as_str()
                .grapheme_indices()
                .rev()
                .collect::<Vec<_>>(),
            test
                .iter()
                .cloned()
                .rev()
                .map(|word| {
                    end -= word.len();
                    (end, word)
                })
                .collect::<Vec<_>>(),
        );
    }
}

#[test]
fn test_words() {
    for test in test_data::word::TEST_DATA {
        use makepad_segment::str::StrExt;

        assert_eq!(
            test.join("")
                .as_str()
                .words()
                .collect::<Vec<_>>()
                .as_slice(),
            test,
        );
    }
}

#[test]
fn test_words_rev() {
    for test in test_data::word::TEST_DATA {
        use makepad_segment::str::StrExt;

        assert_eq!(
            test.join("").as_str().words().rev().collect::<Vec<_>>(),
            test.iter().cloned().rev().collect::<Vec<_>>(),
        );
    }
}

#[test]
fn test_word_indices() {
    for test in test_data::word::TEST_DATA {
        use makepad_segment::str::StrExt;

        let mut start = 0;
        assert_eq!(
            test.join("")
                .as_str()
                .word_indices()
                .collect::<Vec<_>>(),
            test
                .iter()
                .cloned()
                .map(|word| {
                    let position = start;
                    start += word.len();
                    (position, word)
                })
                .collect::<Vec<_>>(),
        );
    }
}

#[test]
fn test_word_indices_rev() {
    for test in test_data::word::TEST_DATA {
        use makepad_segment::str::StrExt;

        let string = test.join("");
        let mut end = string.len(); 
        assert_eq!(
            string
                .as_str()
                .word_indices()
                .rev()
                .collect::<Vec<_>>(),
            test
                .iter()
                .cloned()
                .rev()
                .map(|word| {
                    end -= word.len();
                    (end, word)
                })
                .collect::<Vec<_>>(),
        );
    }
}