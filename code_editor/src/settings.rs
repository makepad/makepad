#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub use_soft_tabs: bool,
    pub indent_column_count: usize,
    pub fold_level: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            use_soft_tabs: true,
            indent_column_count: 4,
            fold_level: 2,
        }
    }
}
