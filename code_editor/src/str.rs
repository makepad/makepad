pub trait StrExt {
    fn indent(&self) -> Option<&str>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn indent(&self) -> Option<&str> {
        self.char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| &self[..index])
    }

    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_> {
        SplitWhitespaceBoundaries { string: self }
    }
}

#[derive(Clone, Debug)]
pub struct SplitWhitespaceBoundaries<'a> {
    string: &'a str,
}

impl<'a> Iterator for SplitWhitespaceBoundaries<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut prev_char_is_whitespace = None;
        let index = self
            .string
            .char_indices()
            .find_map(|(index, next_char)| {
                let next_char_is_whitespace = next_char.is_whitespace();
                let is_whitespace_boundary = prev_char_is_whitespace
                    .map_or(false, |prev_char_is_whitespace| {
                        prev_char_is_whitespace != next_char_is_whitespace
                    });
                prev_char_is_whitespace = Some(next_char_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string_0, string_1) = self.string.split_at(index);
        self.string = string_1;
        Some(string_0)
    }
}
