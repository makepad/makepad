use {super::*, proptest::prelude::*};

#[test]
fn test_builder() {
    let mut builder = Builder::new();
    builder.push_str("abcdefg\r");
    builder.push_str("\nabcdefg");
    let rope = builder.build();
    println!("{:?}", rope);
    assert_eq!(rope.line_len(), 2);
}

fn arbitrary_string() -> impl Strategy<Value = String> {
    "(.|[\u{000A}-\u{000D}\u{0085}\u{2028}-\u{2029}])*"
}

fn arbitrary_string_and_unaligned_byte_index() -> impl Strategy<Value = (String, usize)> {
    arbitrary_string().prop_flat_map(|string| {
        let byte_len = string.len();
        (Just(string), 0..=byte_len)
    })
}

fn arbitrary_string_and_two_unaligned_byte_indices() -> impl Strategy<Value = (String, usize, usize)>
{
    arbitrary_string().prop_flat_map(|string| {
        let byte_len = string.len();
        (Just(string), 0..=byte_len, 0..=byte_len)
    })
}

fn arbitrary_string_and_byte_index() -> impl Strategy<Value = (String, usize)> {
    arbitrary_string_and_unaligned_byte_index().prop_map(|(string, mut byte_index)| {
        while !string.is_char_boundary(byte_index) {
            byte_index -= 1;
        }
        (string, byte_index)
    })
}

fn arbitrary_string_and_two_byte_indices() -> impl Strategy<Value = (String, usize, usize)> {
    arbitrary_string_and_two_unaligned_byte_indices().prop_map(
        |(string, mut byte_index_0, mut byte_index_1)| {
            while !string.is_char_boundary(byte_index_0) {
                byte_index_0 -= 1;
            }
            while !string.is_char_boundary(byte_index_1) {
                byte_index_1 -= 1;
            }
            (string, byte_index_0, byte_index_1)
        },
    )
}

fn arbitrary_string_and_char_index() -> impl Strategy<Value = (String, usize)> {
    arbitrary_string().prop_flat_map(|string| {
        let char_len = string.count_chars();
        (Just(string), 0..=char_len)
    })
}

fn arbitrary_string_and_line_index() -> impl Strategy<Value = (String, usize)> {
    arbitrary_string().prop_flat_map(|string| {
        let line_len = string.count_line_breaks() + 1;
        (Just(string), 0..line_len)
    })
}

fn arbitrary_string_and_byte_range() -> impl Strategy<Value = (String, Range<usize>)> {
    arbitrary_string_and_byte_index()
        .prop_flat_map(|(string, end)| (Just(string), 0..=end, Just(end)))
        .prop_map(|(string, mut start, end)| {
            while !string.is_char_boundary(start) {
                start -= 1;
            }
            (string, start..end)
        })
}

fn arbitrary_string_and_byte_range_and_unaligned_byte_index(
) -> impl Strategy<Value = (String, Range<usize>, usize)> {
    arbitrary_string_and_byte_range().prop_flat_map(|(string, range)| {
        let byte_len = range.len();
        (Just(string), Just(range), 0..=byte_len)
    })
}

fn arbitrary_string_and_byte_range_and_two_unaligned_byte_indices(
) -> impl Strategy<Value = (String, Range<usize>, usize, usize)> {
    arbitrary_string_and_byte_range().prop_flat_map(|(string, range)| {
        let byte_len = range.len();
        (Just(string), Just(range), 0..=byte_len, 0..=byte_len)
    })
}

fn arbitrary_string_and_byte_range_and_byte_index(
) -> impl Strategy<Value = (String, Range<usize>, usize)> {
    arbitrary_string_and_byte_range_and_unaligned_byte_index().prop_map(
        |(string, range, mut byte_index)| {
            let slice = &string[range.clone()];
            while !slice.is_char_boundary(byte_index) {
                byte_index -= 1;
            }
            (string, range, byte_index)
        },
    )
}

fn arbitrary_string_and_byte_range_and_two_byte_indices(
) -> impl Strategy<Value = (String, Range<usize>, usize, usize)> {
    arbitrary_string_and_byte_range_and_two_unaligned_byte_indices().prop_map(
        |(string, range, mut byte_index_0, mut byte_index_1)| {
            let slice = &string[range.clone()];
            while !slice.is_char_boundary(byte_index_0) {
                byte_index_0 -= 1;
            }
            while !slice.is_char_boundary(byte_index_1) {
                byte_index_1 -= 1;
            }
            (string, range, byte_index_0, byte_index_1)
        },
    )
}

fn arbitrary_string_and_byte_range_and_char_index(
) -> impl Strategy<Value = (String, Range<usize>, usize)> {
    arbitrary_string_and_byte_range().prop_flat_map(|(string, range)| {
        let char_len = string[range.clone()].count_chars();
        (Just(string), Just(range), 0..=char_len)
    })
}

fn arbitrary_string_and_byte_range_and_line_index(
) -> impl Strategy<Value = (String, Range<usize>, usize)> {
    arbitrary_string_and_byte_range().prop_flat_map(|(string, range)| {
        let line_len = string[range.clone()].count_line_breaks() + 1;
        (Just(string), Just(range), 0..line_len)
    })
}

proptest! {
    #[test]
    fn is_empty(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.is_empty(), string.is_empty());
    }

    #[test]
    fn byte_len(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.byte_len(), string.len());
    }

    #[test]
    fn char_len(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.char_len(), string.count_chars());
    }

    #[test]
    fn line_len(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.line_len(), string.count_line_breaks() + 1);
    }

    #[test]
    fn is_char_boundary((string, byte_index) in arbitrary_string_and_unaligned_byte_index()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.is_char_boundary(byte_index), string.is_char_boundary(byte_index));
    }

    #[test]
    fn byte_to_char((string, byte_index) in arbitrary_string_and_byte_index()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.byte_to_char(byte_index), string[..byte_index].count_chars());
    }

    #[test]
    fn byte_to_line((string, byte_index) in arbitrary_string_and_byte_index()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.byte_to_line(byte_index), string[..byte_index].count_line_breaks() + 1);
    }

    #[test]
    fn char_to_byte((string, char_index) in arbitrary_string_and_char_index()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.char_to_byte(char_index), string.char_to_byte(char_index));
    }

    #[test]
    fn line_to_byte((string, line_index) in arbitrary_string_and_line_index()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.line_to_byte(line_index), string.line_to_byte(line_index));
    }

    #[test]
    fn chunk_cursor_front(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        let chunk_cursor = rope.chunk_cursor_front();
        assert_eq!(chunk_cursor.byte_position(), 0);
        assert_eq!(
            chunk_cursor.current(),
            &string[..chunk_cursor.current().len()]
        );
    }

    #[test]
    fn chunk_cursor_back(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        let chunk_cursor = rope.chunk_cursor_back();
        assert_eq!(chunk_cursor.byte_position() + chunk_cursor.current().len(), string.len());
        assert_eq!(
            chunk_cursor.current(),
            &string[string.len() - chunk_cursor.current().len()..]
        );
    }

    #[test]
    fn chunk_cursor_at((string, byte_index) in arbitrary_string_and_unaligned_byte_index()) {
        let rope = Rope::from(&string);
        let chunk_cursor = rope.chunk_cursor_at(byte_index);
        assert!(
            chunk_cursor.byte_position() <= byte_index
                && byte_index <= chunk_cursor.byte_position() + chunk_cursor.current().len()
        );
        assert_eq!(
            chunk_cursor.current(),
            &string[chunk_cursor.byte_position()..][..chunk_cursor.current().len()]
        );
    }

    #[test]
    fn chunk_cursor_move_to((string, byte_index_0, byte_index_1) in arbitrary_string_and_two_unaligned_byte_indices()) {
        let rope = Rope::from(&string);
        let mut chunk_cursor = rope.chunk_cursor_at(byte_index_0);
        chunk_cursor.move_to(byte_index_1);
        assert!(
            chunk_cursor.byte_position() <= byte_index_1
                && chunk_cursor.byte_position() + chunk_cursor.current().len() >= byte_index_1
        );
        assert_eq!(
            chunk_cursor.current(),
            &string[chunk_cursor.byte_position()..][..chunk_cursor.current().len()]
        );
    }

    #[test]
    fn cursor_front(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        let cursor = rope.cursor_front();
        assert_eq!(cursor.current_char(), string.chars().next())
    }

    #[test]
    fn cursor_back(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        let cursor = rope.cursor_back();
        assert_eq!(cursor.current_char(), None);
    }

    #[test]
    fn cursor_at((string, byte_index) in arbitrary_string_and_byte_index()) {
        let rope = Rope::from(&string);
        let cursor = rope.cursor_at(byte_index);
        assert_eq!(cursor.current_char(), string[byte_index..].chars().next());
    }

    #[test]
    fn cursor_is_at_char_boundary((string, byte_index) in arbitrary_string_and_unaligned_byte_index()) {
        let rope = Rope::from(&string);
        let cursor = rope.cursor_at(byte_index);
        assert_eq!(cursor.is_at_char_boundary(), string.is_char_boundary(byte_index));
    }

    #[test]
    fn cursor_move_next_byte(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        let mut cursor = rope.cursor_front();
        let mut bytes = string.bytes();
        while !cursor.is_at_back() {
            assert_eq!(cursor.current_byte(), bytes.next());
            cursor.move_next_byte();
        }
        assert!(bytes.next().is_none());
    }

    #[test]
    fn cursor_move_prev_byte(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        let mut cursor = rope.cursor_back();
        let mut bytes_rev = string.bytes().rev();
        while !cursor.is_at_front() {
            cursor.move_prev_byte();
            assert_eq!(cursor.current_byte(), bytes_rev.next());
        }
        assert!(bytes_rev.next().is_none());
    }

    #[test]
    fn cursor_move_next_char(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        let mut cursor = rope.cursor_front();
        let mut chars = string.chars();
        while !cursor.is_at_back() {
            assert_eq!(cursor.current_char(), chars.next());
            cursor.move_next_char();
        }
        assert!(chars.next().is_none());
    }

    #[test]
    fn cursor_move_prev_char(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        let mut cursor = rope.cursor_back();
        let mut chars_rev = string.chars().rev();
        while !cursor.is_at_front() {
            cursor.move_prev_char();
            assert_eq!(cursor.current_char(), chars_rev.next());
        }
        assert!(chars_rev.next().is_none());
    }

    #[test]
    fn cursor_move_to((string, byte_index_0, byte_index_1) in arbitrary_string_and_two_byte_indices()) {
        let rope = Rope::from(&string);
        let mut cursor = rope.cursor_at(byte_index_0);
        cursor.move_to(byte_index_1);
        assert_eq!(cursor.current_char(), string[cursor.byte_position()..].chars().next());
    }

    #[test]
    fn chunks(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.chunks().collect::<String>(), string);
    }

    #[test]
    fn chunks_rev(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        assert_eq!(
            rope.chunks_rev().flat_map(|chunk| chunk.chars().rev()).collect::<String>(),
            string.chars().rev().collect::<String>()
        );
    }

    #[test]
    fn bytes(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.bytes().collect::<Vec<_>>(), string.bytes().collect::<Vec<_>>());
    }

    #[test]
    fn bytes_rev(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        println!("{:?} {:#?}", string, rope);
        assert_eq!(rope.bytes_rev().collect::<Vec<_>>(), string.bytes().rev().collect::<Vec<_>>());
    }

    #[test]
    fn chars(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.chars().collect::<Vec<_>>(), string.chars().collect::<Vec<_>>());
    }

    #[test]
    fn chars_rev(string in arbitrary_string()) {
        let rope = Rope::from(&string);
        assert_eq!(rope.chars_rev().collect::<Vec<_>>(), string.chars().rev().collect::<Vec<_>>());
    }

    #[test]
    fn append(mut string_0 in arbitrary_string(), string_1 in arbitrary_string()) {
        let mut rope_0 = Rope::from(&string_0);
        let rope_1 = Rope::from(&string_1);
        rope_0.append(rope_1);
        string_0.push_str(&string_1);
        assert_eq!(rope_0.chunks().collect::<String>(), string_0);
    }

    #[test]
    fn split_off((mut string, byte_index) in arbitrary_string_and_byte_index()) {
        let mut rope = Rope::from(&string);
        let other_rope = rope.split_off(byte_index);
        let other_string = string.split_off(byte_index);
        assert_eq!(rope.chunks().collect::<String>(), string);
        assert_eq!(other_rope.chunks().collect::<String>(), other_string);
    }

    #[test]
    fn truncate_front((mut string, byte_start) in arbitrary_string_and_byte_index()) {
        let mut rope = Rope::from(&string);
        rope.truncate_front(byte_start);
        string.replace_range(..byte_start, "");
        assert_eq!(rope.chunks().collect::<String>(), string);
    }

    #[test]
    fn truncate_back((mut string, byte_end) in arbitrary_string_and_byte_index()) {
        let mut rope = Rope::from(&string);
        rope.truncate_back(byte_end);
        string.truncate(byte_end);
        assert_eq!(rope.chunks().collect::<String>(), string);
    }

    #[test]
    fn slice_to_rope((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(rope_slice.to_rope().chunks().collect::<String>(), string_slice);
    }

    #[test]
    fn test_cmp((string_0, string_1) in (arbitrary_string(), arbitrary_string())) {
        let rope_0 = Rope::from(&string_0);
        let rope_1 = Rope::from(&string_1);
        assert_eq!(rope_0.cmp(&rope_1), string_0.cmp(&string_1));
    }

    #[test]
    fn slice_is_empty((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(rope_slice.is_empty(), string_slice.is_empty());
    }

    #[test]
    fn slice_byte_len((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        println!("{:?} {:#?}", string_slice, rope_slice);
        assert_eq!(rope_slice.byte_len(), string_slice.len());
    }

    #[test]
    fn slice_char_len((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(rope_slice.char_len(), string_slice.count_chars());
    }

    #[test]
    fn slice_line_len((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(rope_slice.line_len(), string_slice.count_line_breaks() + 1);
    }

    #[test]
    fn slice_is_char_boundary((string, byte_range, byte_index) in arbitrary_string_and_byte_range_and_unaligned_byte_index()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(
            rope_slice.is_char_boundary(byte_index),
            string_slice.is_char_boundary(byte_index),
        );
    }

    #[test]
    fn slice_byte_to_char((string, byte_range, byte_index) in arbitrary_string_and_byte_range_and_byte_index()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(rope_slice.byte_to_char(byte_index), string_slice[..byte_index].count_chars());
    }

    #[test]
    fn slice_byte_to_line((string, byte_range, byte_index) in arbitrary_string_and_byte_range_and_byte_index()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(
            rope_slice.byte_to_line(byte_index),
            string_slice[..byte_index].count_line_breaks() + 1,
        );
    }

    #[test]
    fn slice_char_to_byte((string, byte_range, char_index) in arbitrary_string_and_byte_range_and_char_index()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(rope_slice.char_to_byte(char_index), string_slice.char_to_byte(char_index));
    }

    #[test]
    fn slice_line_to_byte((string, byte_range, line_index) in arbitrary_string_and_byte_range_and_line_index()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(rope_slice.line_to_byte(line_index), string_slice.line_to_byte(line_index));
    }

    #[test]
    fn slice_chunk_cursor_front((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let chunk_cursor = rope_slice.chunk_cursor_front();
        assert_eq!(chunk_cursor.byte_position(), 0);
        assert_eq!(
            chunk_cursor.current(),
            &string_slice[..chunk_cursor.current().len()]
        );
    }

    #[test]
    fn slice_chunk_cursor_back((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let chunk_cursor = rope_slice.chunk_cursor_back();
        assert_eq!(chunk_cursor.byte_position() + chunk_cursor.current().len(), string_slice.len());
        assert_eq!(
            chunk_cursor.current(),
            &string_slice[string_slice.len() - chunk_cursor.current().len()..]
        );
    }

    #[test]
    fn slice_chunk_cursor_at((string, byte_range, byte_index) in arbitrary_string_and_byte_range_and_unaligned_byte_index()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let chunk_cursor = rope_slice.chunk_cursor_at(byte_index);
        assert!(
            chunk_cursor.byte_position() <= byte_index
                && byte_index <= chunk_cursor.byte_position() + chunk_cursor.current().len()
        );
        assert_eq!(
            chunk_cursor.current(),
            &string_slice[chunk_cursor.byte_position()..][..chunk_cursor.current().len()]
        );
    }

    #[test]
    fn slice_chunk_cursor_move_to((string, byte_range, byte_index_0, byte_index_1) in arbitrary_string_and_byte_range_and_two_unaligned_byte_indices()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let mut chunk_cursor = rope_slice.chunk_cursor_at(byte_index_0);
        chunk_cursor.move_to(byte_index_1);
        assert!(
            chunk_cursor.byte_position() <= byte_index_1
                && chunk_cursor.byte_position() + chunk_cursor.current().len() >= byte_index_1
        );
        assert_eq!(
            chunk_cursor.current(),
            &string_slice[chunk_cursor.byte_position()..][..chunk_cursor.current().len()]
        );
    }

    #[test]
    fn slice_cursor_front((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let cursor = rope_slice.cursor_front();
        assert_eq!(cursor.current_char(), string_slice.chars().next())
    }

    #[test]
    fn slice_cursor_back((string, byte_range) in arbitrary_string_and_byte_range()) {
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let cursor = rope_slice.cursor_back();
        assert_eq!(cursor.current_char(), None);
    }

    #[test]
    fn slice_cursor_at((string, byte_range, byte_index) in arbitrary_string_and_byte_range_and_byte_index()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let cursor = rope_slice.cursor_at(byte_index);
        assert_eq!(cursor.current_char(), string_slice[byte_index..].chars().next());
    }

    #[test]
    fn slice_cursor_is_at_char_boundary((string, byte_range, byte_index) in arbitrary_string_and_byte_range_and_unaligned_byte_index()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let cursor = rope_slice.cursor_at(byte_index);
        assert_eq!(cursor.is_at_char_boundary(), string_slice.is_char_boundary(byte_index));
    }

    #[test]
    fn slice_cursor_move_next_byte((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let mut cursor = rope_slice.cursor_front();
        let mut bytes = string_slice.bytes();
        while !cursor.is_at_back() {
            assert_eq!(cursor.current_byte(), bytes.next());
            cursor.move_next_byte();
        }
        assert!(bytes.next().is_none());
    }

    #[test]
    fn slice_cursor_move_prev_byte((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let mut cursor = rope_slice.cursor_back();
        let mut bytes_rev = string_slice.bytes().rev();
        while !cursor.is_at_front() {
            cursor.move_prev_byte();
            assert_eq!(cursor.current_byte(), bytes_rev.next());
        }
        assert!(bytes_rev.next().is_none());
    }

    #[test]
    fn slice_cursor_move_next_char((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let mut cursor = rope_slice.cursor_front();
        let mut chars = string_slice.chars();
        while !cursor.is_at_back() {
            assert_eq!(cursor.current_char(), chars.next());
            cursor.move_next_char();
        }
        assert!(chars.next().is_none());
    }

    #[test]
    fn slice_cursor_move_prev_char((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let mut cursor = rope_slice.cursor_back();
        let mut chars_rev = string_slice.chars().rev();
        while !cursor.is_at_front() {
            cursor.move_prev_char();
            assert_eq!(cursor.current_char(), chars_rev.next());
        }
        assert!(chars_rev.next().is_none());
    }

    #[test]
    fn slice_cursor_move_to((string, byte_range, byte_index_0, byte_index_1) in arbitrary_string_and_byte_range_and_two_byte_indices()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        let mut cursor = rope_slice.cursor_at(byte_index_0);
        cursor.move_to(byte_index_1);
        assert_eq!(cursor.current_char(), string_slice[cursor.byte_position()..].chars().next());
    }

    #[test]
    fn slice_chunks((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(rope_slice.chunks().collect::<String>(), string_slice);
    }

    #[test]
    fn slice_chunks_rev((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(
            rope_slice.chunks_rev().flat_map(|chunk| chunk.chars().rev()).collect::<String>(),
            string_slice.chars().rev().collect::<String>(),
        );
    }

    #[test]
    fn slice_bytes((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(
            rope_slice.bytes().collect::<Vec<_>>(),
            string_slice.bytes().collect::<Vec<_>>()
        );
    }

    #[test]
    fn slice_bytes_rev((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(
            rope_slice.bytes_rev().collect::<Vec<_>>(),
            string_slice.bytes().rev().collect::<Vec<_>>()
        );
    }

    #[test]
    fn slice_chars((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(
            rope_slice.chars().collect::<Vec<_>>(),
            string_slice.chars().collect::<Vec<_>>()
        );
    }

    #[test]
    fn slice_chars_rev((string, byte_range) in arbitrary_string_and_byte_range()) {
        let string_slice = &string[byte_range.clone()];
        let rope = Rope::from(&string);
        let rope_slice = rope.slice(byte_range);
        assert_eq!(
            rope_slice.chars_rev().collect::<Vec<_>>(),
            string_slice.chars().rev().collect::<Vec<_>>()
        );
    }

    #[test]
    fn slice_cmp(((string_0, byte_range_0), (string_1, byte_range_1)) in (arbitrary_string_and_byte_range(), arbitrary_string_and_byte_range())) {
        let string_slice_0 = &string_0[byte_range_0.clone()];
        let rope_0 = Rope::from(&string_0);
        let rope_slice_0 = rope_0.slice(byte_range_0);
        let string_slice_1 = &string_1[byte_range_1.clone()];
        let rope_1 = Rope::from(&string_1);
        let rope_slice_1 = rope_1.slice(byte_range_1);
        assert_eq!(rope_slice_0.cmp(&rope_slice_1), string_slice_0.cmp(string_slice_1));
    }
}
