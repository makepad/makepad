#[derive(Debug, Clone, Copy)]
pub struct SegRange<R> {
    pub min: R,
    pub max: R,
}

pub trait SegExpCollection<R, E, V> {
    type Iter<'a>: Iterator<Item = V>
    where
        Self: 'a;

    fn insert_by_range(&mut self, range: SegRange<R>, val: V);
    fn iter_by_range(&mut self, range: SegRange<R>, time: E) -> Self::Iter<'_>;

    fn clear(&mut self);
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_00() {}
}
