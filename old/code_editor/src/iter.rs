pub trait IteratorExt: Iterator {
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>;
}

impl<T> IteratorExt for T
where
    T: Iterator,
{
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>,
    {
        Merge {
            prev_item: None,
            iter: self,
            f,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Merge<I, F>
where
    I: Iterator,
{
    prev_item: Option<I::Item>,
    iter: I,
    f: F,
}

impl<I, F> Iterator for Merge<I, F>
where
    I: Iterator,
    F: FnMut(I::Item, I::Item) -> Result<I::Item, (I::Item, I::Item)>,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.prev_item.take(), self.iter.next()) {
                (Some(prev_item), Some(item)) => match (self.f)(prev_item, item) {
                    Ok(merged_item) => {
                        self.prev_item = Some(merged_item);
                        continue;
                    }
                    Err((prev_item, item)) => {
                        self.prev_item = Some(item);
                        break Some(prev_item);
                    }
                },
                (None, Some(item)) => {
                    self.prev_item = Some(item);
                    continue;
                }
                (Some(prev_item), None) => break Some(prev_item),
                (None, None) => break None,
            }
        }
    }
}
