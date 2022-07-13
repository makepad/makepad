use {
    crate::{Leaf, Node},
    std::sync::Arc,
};

#[derive(Clone)]
pub struct Branch<L: Leaf> {
    info: L::Info,
    children: Arc<Vec<Node<L>>>,
}

impl<L: Leaf> Branch<L> {
    fn move_left(&mut self, other: &mut Self, end: usize) {
        use crate::Info;

        let info = L::Info::from_nodes(&other.children[..end]);
        other.info -= info;
        self.info += info;
        let children = Arc::make_mut(&mut other.children).drain(..end);
        Arc::make_mut(&mut self.children).extend(children);
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        use crate::Info;

        let info = L::Info::from_nodes(&self.children[start..]);
        self.info -= info;
        other.info += info;
        let children = Arc::make_mut(&mut self.children).drain(start..);
        Arc::make_mut(&mut other.children).splice(..0, children);
    }
}
