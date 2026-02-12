pub trait SliceExt<T> {
    fn group_by<P>(&self, predicate: P) -> GroupBy<'_, T, P>
    where
        P: Fn(&T, &T) -> bool;
}

impl<T> SliceExt<T> for [T] {
    fn group_by<P>(&self, predicate: P) -> GroupBy<'_, T, P>
    where
        P: Fn(&T, &T) -> bool,
    {
        GroupBy {
            slice: self,
            predicate,
        }
    }
}

#[derive(Debug)]
pub struct GroupBy<'a, T, P> {
    slice: &'a [T],
    predicate: P,
}

impl<'a, T, P> Iterator for GroupBy<'a, T, P>
where
    P: Fn(&T, &T) -> bool,
{
    type Item = &'a [T];

    fn next(&mut self) -> Option<Self::Item> {
        if self.slice.is_empty() {
            None
        } else {
            let mut len = 1;
            let mut iter = self.slice.windows(2);
            while let Some([l, r]) = iter.next() {
                if (self.predicate)(l, r) {
                    len += 1
                } else {
                    break;
                }
            }
            let (head, tail) = self.slice.split_at(len);
            self.slice = tail;
            Some(head)
        }
    }
}
