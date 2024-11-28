use {
    crate::{Cursor, Info, Leaf, Node},
    std::fmt,
};

pub struct Slice<'a, L>
where
    L: Leaf,
{
    root: Option<&'a Node<L>>,
    start_info: L::Info,
    end_info: L::Info,
}

impl<'a, L> Slice<'a, L>
where
    L: Leaf,
{
    pub(super) fn new(root: Option<&'a Node<L>>, start: usize, end: usize) -> Self {
        match root {
            Some(mut root) => {
                let mut start = start;
                let mut end = end;
                loop {
                    match root {
                        Node::Leaf(_) => break,
                        Node::Branch(branch) => {
                            let mut index = 0;
                            let mut acc_info = L::Info::empty();
                            for info in branch.infos().iter().copied() {
                                let next_acc_info = acc_info.combine(info);
                                if start >= acc_info.len() && end <= next_acc_info.len() {
                                    break;
                                }
                                index += 1;
                                acc_info = next_acc_info;
                            }
                            if index == branch.len() {
                                break;
                            }
                            root = &branch.nodes()[index];
                            start -= acc_info.len();
                            end -= acc_info.len();
                        }
                    }
                }
                Self {
                    root: Some(root),
                    start_info: root.info_to(start),
                    end_info: root.info_to(end),
                }
            }
            None => Self {
                root: None,
                start_info: Info::empty(),
                end_info: Info::empty(),
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.start_info.len() == self.end_info.len()
    }

    pub fn len(&self) -> usize {
        self.end_info.len() - self.start_info.len()
    }

    pub fn start_info(&self) -> L::Info {
        self.start_info
    }

    pub fn end_info(&self) -> L::Info {
        self.end_info
    }

    pub fn find_by(&self, f: impl FnMut(L::Info) -> bool) -> (&L, L::Info) {
        self.root.as_ref().unwrap().find_leaf_by(f)
    }

    pub fn cursor_start(&self) -> Cursor<'a, L> {
        Cursor::start(self.root, self.start_info.len(), self.end_info.len())
    }

    pub fn cursor_end(&self) -> Cursor<'a, L> {
        Cursor::end(self.root, self.start_info.len(), self.end_info.len())
    }

    pub fn cursor(&self, index: usize) -> Cursor<'a, L> {
        Cursor::new(self.root, self.start_info.len(), self.end_info.len(), index)
    }

    pub fn slice(&self, start: usize, end: usize) -> Slice<'a, L> {
        Slice::new(
            self.root,
            self.start_info.len() + start,
            self.start_info.len() + end,
        )
    }
}

impl<'a, L> Clone for Slice<'a, L>
where
    L: Leaf,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, L> Copy for Slice<'a, L> where L: Leaf {}

impl<'a, L> fmt::Debug for Slice<'a, L>
where
    L: Leaf + fmt::Debug,
    L::Info: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Slice")
            .field("root", &self.root)
            .field("start_info", &self.start_info)
            .field("end_info", &self.end_info)
            .finish()
    }
}
