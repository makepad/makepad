use {crate::Selection, std::slice};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SelectionSet {
    selections: Vec<Selection>,
}

impl SelectionSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }

    pub fn len(&self) -> usize {
        self.selections.len()
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.selections.iter(),
        }
    }

    pub fn update<F>(&mut self, mut f: F)
    where
        F: FnMut(Selection) -> Selection,
    {
        for selection in &mut self.selections {
            *selection = f(*selection);
        }
        self.normalize();
    }

    pub fn insert(&mut self, selection: Selection) {
        let mut index = match self
            .selections
            .binary_search_by_key(&selection.start(), |selection| selection.start())
        {
            Ok(index) => index,
            Err(index) => index,
        };
        self.selections.insert(index, selection);
        if index > 0 && self.merge(index - 1) {
            index -= 1;
        }
        while index < self.selections.len() - 1 {
            if !self.merge(index) {
                break;
            }
        }
    }

    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(Selection) -> bool,
    {
        self.selections.retain(|&selection| f(selection));
    }

    pub fn clear(&mut self) {
        self.selections.clear()
    }

    fn normalize(&mut self) {
        if self.selections.is_empty() {
            return;
        }
        self.selections.sort_by_key(|selection| selection.start());
        let mut index = 0;
        while index < self.selections.len() - 1 {
            if !self.merge(index) {
                index += 1;
            }
        }
    }

    fn merge(&mut self, index: usize) -> bool {
        if let Some(merged_selection) = self.selections[index].merge(self.selections[index + 1]) {
            self.selections[index] = merged_selection;
            self.selections.remove(index + 1);
            true
        } else {
            false
        }
    }
}

impl Extend<Selection> for SelectionSet {
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = Selection>,
    {
        self.selections.extend(iter);
        self.normalize();
    }
}

impl FromIterator<Selection> for SelectionSet {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = Selection>,
    {
        let mut selections = SelectionSet::new();
        selections.extend(iter);
        selections
    }
}

impl<'a> IntoIterator for &'a SelectionSet {
    type Item = Selection;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, Selection>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Selection;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().copied()
    }
}
