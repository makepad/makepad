#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub tab_column_count: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_column_count: 4,
        }
    }
}
