use {
    crate::Selection,
    std::{
        ops::{Deref, Index},
        slice::Iter,
        vec::IntoIter,
    },
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SelectionSet {
    selections: Vec<Selection>,
}

impl SelectionSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn replace(&mut self, index: usize, f: impl FnOnce(Selection) -> Selection) -> usize {
        let selection = self.remove(index);
        self.insert(f(selection))
    }

    pub fn remove(&mut self, index: usize) -> Selection {
        self.selections.remove(index)
    }

    pub fn insert(&mut self, selection: Selection) -> usize {
        let index = match self
            .selections
            .binary_search_by_key(&selection.start(), |selection| selection.start())
        {
            Ok(index) => index,
            Err(index) => index,
        };
        self.selections.insert(index, selection);
        index
    }

    pub fn clear(&mut self) {
        self.selections.clear()
    }

    pub fn into_vec(self) -> Vec<Selection> {
        self.selections
    }
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self {
            selections: Vec::new(),
        }
    }
}

impl Deref for SelectionSet {
    type Target = [Selection];

    fn deref(&self) -> &Self::Target {
        self.as_selections()
    }
}

impl From<Vec<Selection>> for SelectionSet {
    fn from(mut vec: Vec<Selection>) -> Self {
        vec.sort_unstable_by_key(|selection| selection.start());
        Self { selections: vec }
    }
}

impl FromIterator<Selection> for SelectionSet {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = Selection>,
    {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}

impl Index<usize> for SelectionSet {
    type Output = Selection;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).unwrap()
    }
}

impl<'a> IntoIterator for &'a SelectionSet {
    type Item = &'a Selection;
    type IntoIter = Iter<'a, Selection>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for SelectionSet {
    type Item = Selection;
    type IntoIter = IntoIter<Selection>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}
