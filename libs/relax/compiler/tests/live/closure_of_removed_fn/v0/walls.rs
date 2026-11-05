use crate::grid::Grid;

pub fn count(n: usize) -> usize {
    Grid::of(n, |k| k as u8).segs.len()
}

/// a struct the edit changes too (a field goes): the layouts are laid out again
pub struct Walls {
    pub gaps: Vec<u32>,
    pub thin: Vec<u32>,
}

pub fn walls(n: usize) -> usize {
    let w = Walls { gaps: vec![1; n], thin: Vec::new() };
    w.gaps.len() + w.thin.len()
}

pub fn held(n: usize) -> u32 {
    Held::new(n).grid.segs.iter().map(|s| s.1).sum()
}

pub struct Held {
    grid: Grid<u32>,
}

fn margin(n: usize) -> usize {
    n / 2
}

impl Held {
    pub fn new(n: usize) -> Self {
        Self { grid: Grid::of(n + margin(n), |k| k as u32 * 10) }
    }
}
