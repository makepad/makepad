#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub tab_column_count: usize,
    pub indent_column_count: usize,
    pub fold_level: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_column_count: 4,
            indent_column_count: 4,
            fold_level: 2,
        }
    }
}
