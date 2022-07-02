mod test_data;

#[test]
fn test_graphemes() {
    for test in test_data::grapheme::TEST_DATA {
        use makepad_text_segmentation::str::StrExt;

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
        use makepad_text_segmentation::str::StrExt;

        assert_eq!(
            test.join("").as_str().graphemes().rev().collect::<Vec<_>>(),
            test.iter().cloned().rev().collect::<Vec<_>>(),
        );
    }
}

#[test]
fn test_words() {
    for test in test_data::word::TEST_DATA {
        use makepad_text_segmentation::str::StrExt;

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
        use makepad_text_segmentation::str::StrExt;

        assert_eq!(
            test.join("").as_str().words().rev().collect::<Vec<_>>(),
            test.iter().cloned().rev().collect::<Vec<_>>(),
        );
    }
}
