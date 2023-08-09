#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InlineWidget {
    pub column_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockWidget {
    pub height: f64,
}
