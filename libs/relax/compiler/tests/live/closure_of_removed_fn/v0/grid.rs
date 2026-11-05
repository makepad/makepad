pub struct Grid<T> {
    pub segs: Vec<(usize, T)>,
}

impl<T> Grid<T> {
    fn new(segs: Vec<(usize, T)>) -> Self {
        Self { segs }
    }

    pub fn of(n: usize, tag: impl Fn(usize) -> T) -> Self {
        let mut segs = Vec::new();
        for i in 0..n {
            segs.push((i, tag(i)));
        }
        Self::new(segs)
    }
}
