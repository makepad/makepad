#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub tab_width: usize,
    pub indent_width: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            indent_width: 4,
        }
    }
}
