use crate::core::edge_data::OverlayEdgeData;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;
use i_shape::int::path::IntPath;

pub type SideFill = u8;
pub type DataVectorPath<I, D = ()> = Vec<DataVectorEdge<I, D>>;
pub type DataVectorShape<I, D = ()> = Vec<DataVectorPath<I, D>>;
pub type VectorPath<I> = DataVectorPath<I>;
pub type VectorShape<I> = DataVectorShape<I>;

pub const SUBJ_LEFT: u8 = 0b0001;
pub const SUBJ_RIGHT: u8 = 0b0010;
pub const CLIP_LEFT: u8 = 0b0100;
pub const CLIP_RIGHT: u8 = 0b1000;

pub trait Reverse {
    fn reverse(self) -> Self;
}

impl Reverse for SideFill {
    fn reverse(self) -> Self {
        let subj_left = self & SUBJ_LEFT;
        let subj_right = self & SUBJ_RIGHT;
        let clip_left = self & CLIP_LEFT;
        let clip_right = self & CLIP_RIGHT;

        (subj_left << 1) | (subj_right >> 1) | (clip_left << 1) | (clip_right >> 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataVectorEdge<I: IntNumber, D = ()> {
    pub a: IntPoint<I>,
    pub b: IntPoint<I>,
    pub fill: SideFill,
    pub data: D,
}

impl<I: IntNumber, D: OverlayEdgeData> DataVectorEdge<I, D> {
    pub(crate) fn new(fill: SideFill, a: IntPoint<I>, b: IntPoint<I>, data: D) -> Self {
        let mut store = D::Store::default();
        Self::new_with_store(fill, a, b, data, &mut store)
    }

    pub(crate) fn new_with_store(
        fill: SideFill,
        a: IntPoint<I>,
        b: IntPoint<I>,
        data: D,
        store: &mut D::Store,
    ) -> Self {
        let (fill, data) = if a < b {
            (fill, data)
        } else {
            (fill.reverse(), data.reversed(store))
        };

        Self { a, b, fill, data }
    }
}

pub trait ToPath<I: IntNumber> {
    fn to_path(&self) -> IntPath<I>;
}

impl<I: IntNumber> ToPath<I> for VectorPath<I> {
    fn to_path(&self) -> IntPath<I> {
        self.iter().map(|e| e.a).collect()
    }
}
