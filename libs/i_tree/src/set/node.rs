#[derive(PartialEq, Clone, Copy)]
pub(super) enum Color {
    Red,
    Black,
}

#[derive(Clone)]
pub(super) struct Node<V> {
    pub(super) parent: u32,
    pub(super) left: u32,
    pub(super) right: u32,
    pub(super) color: Color,
    pub(super) value: V,
}

impl<V: Clone + Default> Default for Node<V> {
    #[inline]
    fn default() -> Self {
        Self {
            parent: 0,
            left: 0,
            right: 0,
            color: Color::Red,
            value: V::default(),
        }
    }
}
