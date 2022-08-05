pub trait StrUtils {
    fn count_chars(&self) -> usize;
    fn count_line_breaks(&self) -> usize;
    fn char_to_byte(&self, char_index: usize) -> usize;
    fn line_to_byte(&self, line_index: usize) -> usize;
}

impl StrUtils for str {
    fn count_chars(&self) -> usize {
        let mut count = 0;
        for byte in self.bytes() {
            count += (byte as i8 >= -0x40) as usize;
        }
        count
    }

    fn count_line_breaks(&self) -> usize {
        let mut count = 0;
        for byte in self.bytes() {
            count += (byte == 0x0A) as usize;
        }
        count
    }

    fn char_to_byte(&self, char_index: usize) -> usize {
        let mut byte_index = 0;
        let mut char_count = 0;
        for byte in self.bytes() {
            char_count += (byte as i8 >= -0x40) as usize;
            if char_count > char_index {
                break;
            }
            byte_index += 1;
        }
        byte_index
    }

    fn line_to_byte(&self, line_index: usize) -> usize {
        let mut byte_index = 0;
        let mut line_break_count = 0;
        for byte in self.bytes() {
            line_break_count += (byte == 0x0A) as usize;
            if line_break_count >= line_index {
                break;
            }
            byte_index += 1;
        }
        byte_index
    }
}
