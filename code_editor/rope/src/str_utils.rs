pub trait StrUtils {
    fn count_chars(&self) -> usize;
    fn count_line_breaks(&self) -> usize;
    fn last_is_cr(&self) -> bool;
    fn first_is_lf(&self) -> bool;
    fn can_split_at(&self, byte_index: usize) -> bool;
    fn char_to_byte(&self, char_index: usize) -> usize;
    fn line_to_byte(&self, line_index: usize) -> usize;
}

impl StrUtils for str {
    fn count_chars(&self) -> usize {
        let mut count = 0;
        for byte in self.bytes() {
            count += (byte & 0xC0 != 0x80) as usize;
        }
        count
    }

    fn count_line_breaks(&self) -> usize {
        count_line_breaks_up_to(self, self.len()).0
    }

    fn last_is_cr(&self) -> bool {
        self.as_bytes().last() == Some(&0x0D)
    }

    fn first_is_lf(&self) -> bool {
        self.as_bytes().first() == Some(&0x0A)
    }

    fn can_split_at(&self, byte_index: usize) -> bool {
        self.is_char_boundary(byte_index)
            && !(self[..byte_index].last_is_cr() && self[byte_index..].first_is_lf())
    }

    fn char_to_byte(&self, char_index: usize) -> usize {
        let mut byte_index = 0;
        let mut char_count = 0;
        for byte in self.bytes() {
            char_count += (byte & 0xC0 != 0x80) as usize;
            if char_count > char_index {
                break;
            }
            byte_index += 1;
        }
        byte_index
    }

    fn line_to_byte(&self, line_index: usize) -> usize {
        count_line_breaks_up_to(self, line_index).1
    }
}

fn count_line_breaks_up_to(string: &str, max_line_break_count: usize) -> (usize, usize) {
    let mut line_break_count = 0;
    let mut byte_index = 0;
    let bytes = string.as_bytes();
    while line_break_count < max_line_break_count && byte_index < bytes.len() {
        let byte = bytes[byte_index];
        if byte >= 0x0A && byte <= 0x0D {
            line_break_count += 1;
            if byte == 0x0D && byte_index + 1 < bytes.len() && bytes[byte_index + 1] == 0x0A {
                byte_index += 2;
            } else {
                byte_index += 1;
            }
        } else if byte == 0xC2 && byte_index + 1 < bytes.len() && bytes[byte_index + 1] == 0x85 {
            line_break_count += 1;
            byte_index += 2;
        } else if byte == 0xE2
            && byte_index + 2 < bytes.len()
            && bytes[byte_index + 1] == 0x80
            && bytes[byte_index + 2] >> 1 == 0x54
        {
            line_break_count += 1;
            byte_index += 3;
        } else {
            byte_index += 1;
        }
    }
    (line_break_count, byte_index)
}
