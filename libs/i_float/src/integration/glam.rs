use crate::{
    float::{compatible::FloatPointCompatible, point::FloatPoint},
    int::point::IntPoint,
};

// glam::Vec2 / f32
impl FloatPointCompatible for glam::Vec2 {
    type Scalar = f32;

    #[inline(always)]
    fn from_xy(x: f32, y: f32) -> Self {
        glam::Vec2::new(x, y)
    }

    #[inline(always)]
    fn x(&self) -> f32 {
        self.x
    }

    #[inline(always)]
    fn y(&self) -> f32 {
        self.y
    }
}

impl From<FloatPoint<f32>> for glam::Vec2 {
    #[inline(always)]
    fn from(point: FloatPoint<f32>) -> Self {
        glam::Vec2::new(point.x, point.y)
    }
}

impl From<glam::Vec2> for FloatPoint<f32> {
    #[inline(always)]
    fn from(point: glam::Vec2) -> Self {
        FloatPoint::new(point.x, point.y)
    }
}

// glam::DVec2 / f64
impl FloatPointCompatible for glam::DVec2 {
    type Scalar = f64;

    #[inline(always)]
    fn from_xy(x: f64, y: f64) -> Self {
        glam::DVec2::new(x, y)
    }

    #[inline(always)]
    fn x(&self) -> f64 {
        self.x
    }

    #[inline(always)]
    fn y(&self) -> f64 {
        self.y
    }
}

impl From<FloatPoint<f64>> for glam::DVec2 {
    #[inline(always)]
    fn from(point: FloatPoint<f64>) -> Self {
        glam::DVec2::new(point.x, point.y)
    }
}

impl From<glam::DVec2> for FloatPoint<f64> {
    #[inline(always)]
    fn from(point: glam::DVec2) -> Self {
        FloatPoint::new(point.x, point.y)
    }
}

// glam IVec2 / IntPoint / i32
impl From<IntPoint> for glam::IVec2 {
    #[inline(always)]
    fn from(point: IntPoint) -> Self {
        glam::IVec2::new(point.x, point.y)
    }
}

impl From<glam::IVec2> for IntPoint {
    #[inline(always)]
    fn from(point: glam::IVec2) -> Self {
        IntPoint::new(point.x, point.y)
    }
}
