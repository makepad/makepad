use crate::data::arena::Index;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default)]
/// A container for data associated to item existing into another Arena.
pub struct Coarena<T> {
    data: Vec<(u32, T)>,
}

impl<T> Coarena<T> {
    /// A coarena with no element.
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    /// Pre-allocates capacity for `additional` extra elements in this arena.
    pub fn reserve(&mut self, additional: usize) {
        self.data.reserve(additional);
    }

    /// Iterates through all the elements of this coarena.
    pub fn iter(&self) -> impl Iterator<Item = (Index, &T)> {
        self.data
            .iter()
            .enumerate()
            .filter(|(_, elt)| elt.0 != u32::MAX)
            .map(|(i, elt)| (Index::from_raw_parts(i as u32, elt.0), &elt.1))
    }

    /// Gets a specific element from the coarena without specifying its generation number.
    ///
    /// It is strongly encouraged to use `Coarena::get` instead of this method because this method
    /// can suffer from the ABA problem.
    pub fn get_unknown_gen(&self, index: u32) -> Option<&T> {
        self.data.get(index as usize).map(|(_, t)| t)
    }

    /// Gets a specific mutable element from the coarena without specifying its generation number.
    ///
    /// It is strongly encouraged to use `Coarena::get_mut` instead of this method because this method
    /// can suffer from the ABA problem.
    pub fn get_mut_unknown_gen(&mut self, index: u32) -> Option<&mut T> {
        self.data.get_mut(index as usize).map(|(_, t)| t)
    }

    pub(crate) fn get_gen(&self, index: u32) -> Option<u32> {
        self.data
            .get(index as usize)
            .map(|(generation, _)| *generation)
    }

    /// Deletes an element for the coarena and returns its value.
    ///
    /// This method will reset the value to the given `removed_value`.
    pub fn remove(&mut self, index: Index, removed_value: T) -> Option<T> {
        let (i, g) = index.into_raw_parts();
        let data = self.data.get_mut(i as usize)?;
        if g == data.0 {
            data.0 = u32::MAX; // invalidate the generation number.
            Some(std::mem::replace(&mut data.1, removed_value))
        } else {
            None
        }
    }

    /// Gets a specific element from the coarena, if it exists.
    pub fn get(&self, index: Index) -> Option<&T> {
        let (i, g) = index.into_raw_parts();
        self.data
            .get(i as usize)
            .and_then(|(gg, t)| if g == *gg { Some(t) } else { None })
    }

    /// Gets a mutable reference to a specific element from the coarena, if it exists.
    pub fn get_mut(&mut self, index: Index) -> Option<&mut T> {
        let (i, g) = index.into_raw_parts();
        self.data
            .get_mut(i as usize)
            .and_then(|(gg, t)| if g == *gg { Some(t) } else { None })
    }

    /// Inserts an element into this coarena.
    pub fn insert(&mut self, a: Index, value: T)
    where
        T: Clone + Default,
    {
        let (i1, g1) = a.into_raw_parts();

        if self.data.len() <= i1 as usize {
            self.data.resize(i1 as usize + 1, (u32::MAX, T::default()));
        }

        self.data[i1 as usize] = (g1, value);
    }

    /// Ensure that the given element exists in this coarena, and return its mutable reference.
    pub fn ensure_element_exist(&mut self, a: Index, default: T) -> &mut T
    where
        T: Clone,
    {
        let (i1, g1) = a.into_raw_parts();

        if self.data.len() <= i1 as usize {
            self.data
                .resize(i1 as usize + 1, (u32::MAX, default.clone()));
        }

        let data = &mut self.data[i1 as usize];

        if data.0 != g1 {
            *data = (g1, default);
        }

        &mut data.1
    }

    /// Ensure that elements at the two given indices exist in this coarena, and return their references.
    ///
    /// Missing elements are created automatically and initialized with the `default` value.
    pub fn ensure_pair_exists(&mut self, a: Index, b: Index, default: T) -> (&mut T, &mut T)
    where
        T: Clone,
    {
        let (i1, g1) = a.into_raw_parts();
        let (i2, g2) = b.into_raw_parts();

        assert_ne!(i1, i2, "Cannot index the same object twice.");

        let (elt1, elt2) = if i1 > i2 {
            if self.data.len() <= i1 as usize {
                self.data
                    .resize(i1 as usize + 1, (u32::MAX, default.clone()));
            }

            let (left, right) = self.data.split_at_mut(i1 as usize);
            (&mut right[0], &mut left[i2 as usize])
        } else {
            // i2 > i1
            if self.data.len() <= i2 as usize {
                self.data
                    .resize(i2 as usize + 1, (u32::MAX, default.clone()));
            }

            let (left, right) = self.data.split_at_mut(i2 as usize);
            (&mut left[i1 as usize], &mut right[0])
        };

        if elt1.0 != g1 {
            *elt1 = (g1, default.clone());
        }

        if elt2.0 != g2 {
            *elt2 = (g2, default);
        }

        (&mut elt1.1, &mut elt2.1)
    }
}
