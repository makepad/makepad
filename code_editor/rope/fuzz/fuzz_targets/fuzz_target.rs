#![no_main]

use {
    libfuzzer_sys::{arbitrary, arbitrary::Arbitrary, fuzz_target},
    makepad_rope::Rope,
};

#[derive(Arbitrary, Debug)]
enum Operation<'a> {
    Append(&'a str),
    SplitOff(usize),
    TruncateFront(usize),
    TruncateBack(usize),
}

fuzz_target!(|data: (&str, Vec<Operation<'_>>)| {
    let (string, operations) = data;
    let mut rope = Rope::from(string);
    for operation in operations {
        match operation {
            Operation::Append(other_string) => {
                rope.append(Rope::from(other_string));
                rope.assert_valid();
            }
            Operation::SplitOff(at) => {
                if !rope.is_char_boundary(at) {
                    continue;
                }
                rope.split_off(at);
                rope.assert_valid();
            }
            Operation::TruncateFront(start) => {
                if !rope.is_char_boundary(start) {
                    continue;
                }
                rope.truncate_front(start);
                rope.assert_valid();
            }
            Operation::TruncateBack(end) => {
                if !rope.is_char_boundary(end) {
                    continue;
                }
                rope.truncate_back(end);
                rope.assert_valid();
            }
        }
    }
});
