use crate::grid::Grid;

pub fn count(n: usize) -> usize {
    Grid::of(n, |k| k as u8).segs.len()
}

/// a struct the edit changes too (a field goes): the layouts are laid out again
pub struct Walls {
    pub gaps: Vec<u32>,
}

pub fn walls(n: usize) -> usize {
    let w = Walls { gaps: vec![1; n] };
    w.gaps.len()
}

pub fn held(n: usize) -> u32 {
    Grid::of(n, |k| k as u32).segs.iter().map(|s| s.1).sum()
}
