#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub tab_column_count: usize,
    pub fold_level: usize,
    pub word_separators: Vec<char>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_column_count: 4,
            fold_level: 2,
            word_separators: vec![
                ' ', '`', '~', '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '-', '=', '+',
                '[', '{', ']', '}', '\\', '|', ';', ':', '\'', '"', '.', '<', '>', '/', '?', ',',
            ],
        }
    }
}
