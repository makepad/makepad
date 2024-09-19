use std::ops::{Add, Sub};

#[derive(Clone, Copy, Debug)]
pub struct RectUsize {
    pub origin: PointUsize,
    pub size: SizeUsize,
}

impl RectUsize {
    pub fn new(origin: PointUsize, size: SizeUsize) -> Self {
        Self { origin, size }
    }

    pub fn min(self) -> PointUsize {
        self.origin
    }

    pub fn max(self) -> PointUsize {
        self.origin + self.size
    }

    pub fn union(self, other: Self) -> Self {
        let min = self.min().min(other.min());
        let max = self.max().max(other.max());
        Self::new(min, max - min)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PointUsize {
    pub x: usize,
    pub y: usize,
}

impl PointUsize {
    pub fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }

    pub fn min(self, other: Self) -> Self {
        Self {
            x: self.x.min(other.x),
            y: self.y.min(other.y),
        }
    }

    pub fn max(self, other: Self) -> Self {
        Self {
            x: self.x.max(other.x),
            y: self.y.max(other.y),
        }
    }
}

impl Add<SizeUsize> for PointUsize {
    type Output = Self;

    fn add(self, other: SizeUsize) -> Self::Output {
        PointUsize::new(self.x + other.width, self.y + other.height)
    }
}

impl Sub for PointUsize {
    type Output = SizeUsize;

    fn sub(self, other: Self) -> Self::Output {
        SizeUsize::new(self.x - other.x, self.y - other.y)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SizeUsize {
    pub width: usize,
    pub height: usize,
}

impl SizeUsize {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }
}