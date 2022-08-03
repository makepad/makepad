#![no_main]

use {
    libfuzzer_sys::{arbitrary, arbitrary::Arbitrary, fuzz_target},
    makepad_collections::BTreeString,
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
    let mut string = BTreeString::from(string);
    for operation in operations {
        match operation {
            Operation::Append(other_string) => {
                string.append(BTreeString::from(other_string));
            }
            Operation::SplitOff(at) => {
                if !string.is_char_boundary(at) {
                    continue;
                }
                string.split_off(at);
            }
            Operation::TruncateFront(start) => {
                if !string.is_char_boundary(start) {
                    continue;
                }
                string.truncate_front(start);
            }
            Operation::TruncateBack(end) => {
                if !string.is_char_boundary(end) {
                    continue;
                }
                string.truncate_back(end);
            }
        }
    }
});
