use {
    super::num::{One, Zero},
    std::ops::{Add, Div, Mul, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Point<T> {
    pub x: T,
    pub y: T,
}

impl<T> Point<T> {
    pub const fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    pub fn apply_transform(self, t: Transform<T>) -> Self
    where
        T: Add<Output = T> + Copy + Mul<Output = T>,
    {
        Self::new(
            self.x * t.xx + self.y * t.yx + t.tx,
            self.x * t.xy + self.y * t.yy + t.ty,
        )
    }
}

impl<T> Add<Size<T>> for Point<T>
where
    T: Add<Output = T>,
{
    type Output = Self;

    fn add(self, size: Size<T>) -> Self::Output {
        Self::new(self.x + size.width, self.y + size.height)
    }
}

impl<T> Div<T> for Point<T>
where
    T: Div<Output = T> + Copy,
{
    type Output = Self;

    fn div(self, scalar: T) -> Self::Output {
        Self::new(self.x / scalar, self.y / scalar)
    }
}

impl<T> Mul<T> for Point<T>
where
    T: Mul<Output = T> + Copy,
{
    type Output = Self;

    fn mul(self, scalar: T) -> Self::Output {
        Self::new(self.x * scalar, self.y * scalar)
    }
}

impl<T> Sub for Point<T>
where
    T: Sub<Output = T>,
{
    type Output = Size<T>;

    fn sub(self, other: Self) -> Self::Output {
        Size::new(self.x - other.x, self.y - other.y)
    }
}

impl<T> Sub<Size<T>> for Point<T>
where
    T: Sub<Output = T>,
{
    type Output = Self;

    fn sub(self, size: Size<T>) -> Self::Output {
        Self::new(self.x - size.width, self.y - size.height)
    }
}

impl<T> From<Size<T>> for Point<T> {
    fn from(size: Size<T>) -> Self {
        Self::new(size.width, size.height)
    }
}

impl<T> Zero for Point<T>
where
    T: Zero,
{
    const ZERO: Self = Self::new(T::ZERO, T::ZERO);
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Size<T> {
    pub width: T,
    pub height: T,
}

impl<T> Size<T> {
    pub const fn new(width: T, height: T) -> Self {
        Self { width, height }
    }

    pub fn apply_transform(self, t: Transform<T>) -> Self
    where
        T: Add<Output = T> + Copy + Mul<Output = T>,
    {
        Self::new(
            self.width * t.xx + self.height * t.yx,
            self.width * t.xy + self.height * t.yy,
        )
    }
}

impl<T> Add for Size<T>
where
    T: Add<Output = T>,
{
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self::new(self.width + other.width, self.height + other.height)
    }
}

impl<T> From<T> for Size<T>
where
    T: Copy,
{
    fn from(scalar: T) -> Self {
        Self::new(scalar, scalar)
    }
}

impl<T> From<Point<T>> for Size<T> {
    fn from(point: Point<T>) -> Self {
        Self::new(point.x, point.y)
    }
}

impl<T> Div<T> for Size<T>
where
    T: Div<Output = T> + Copy,
{
    type Output = Self;

    fn div(self, scalar: T) -> Self::Output {
        Self::new(self.width / scalar, self.height / scalar)
    }
}

impl<T> Mul<T> for Size<T>
where
    T: Mul<Output = T> + Copy,
{
    type Output = Self;

    fn mul(self, scalar: T) -> Self::Output {
        Self::new(self.width * scalar, self.height * scalar)
    }
}

impl<T> Sub for Size<T>
where
    T: Sub<Output = T>,
{
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        Self::new(self.width - other.width, self.height - other.height)
    }
}

impl<T> Zero for Size<T>
where
    T: Zero,
{
    const ZERO: Self = Self::new(T::ZERO, T::ZERO);
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rect<T> {
    pub origin: Point<T>,
    pub size: Size<T>,
}

impl<T> Rect<T> {
    pub const fn new(origin: Point<T>, size: Size<T>) -> Self {
        Self { origin, size }
    }

    pub fn is_empty(self) -> bool
    where
        T: Eq + PartialEq + Zero,
    {
        self.size == Size::ZERO
    }

    pub fn contains_point(self, point: Point<T>) -> bool
    where
        T: Add<Output = T> + Copy + Ord,
    {
        if !(self.min().x..self.max().x).contains(&point.x) {
            return false;
        }
        if !(self.min().y..self.max().y).contains(&point.y) {
            return false;
        }
        true
    }

    pub fn contains_rect(self, other: Self) -> bool
    where
        T: Add<Output = T> + Copy + Ord,
    {
        if !self.contains_point(other.min()) && self.min() != other.min() {
            return false;
        }
        if !self.contains_point(other.max()) && self.max() != other.max() {
            return false;
        }
        true
    }

    pub fn min(self) -> Point<T>
    where
        T: Copy,
    {
        self.origin
    }

    pub fn max(self) -> Point<T>
    where
        T: Add<Output = T> + Copy,
    {
        self.origin + self.size
    }

    pub fn pad(self, padding: impl Into<Size<T>>) -> Self
    where
        T: Add<Output = T> + Copy + Sub<Output = T>,
    {
        self._pad(padding.into())
    }

    fn _pad(self, padding: Size<T>) -> Self
    where
        T: Add<Output = T> + Copy + Sub<Output = T>,
    {
        Self::new(self.origin - padding, self.size + padding + padding)
    }

    pub fn apply_transform(self, t: Transform<T>) -> Self
    where
        T: Add<Output = T> + Copy + Mul<Output = T>,
    {
        Self::new(self.origin.apply_transform(t), self.size.apply_transform(t))
    }

    pub fn union(self, other: impl Into<Self>) -> Self
    where
        T: Add<Output = T> + Copy + Ord + Sub<Output = T>,
    {
        self._union(other.into())
    }

    fn _union(self, other: Self) -> Self
    where
        T: Add<Output = T> + Copy + Ord + Sub<Output = T>,
    {
        let min = Point::new(
            self.min().x.min(other.min().x),
            self.min().y.min(other.min().y),
        );
        let max = Point::new(
            self.max().x.max(other.max().x),
            self.max().y.max(other.max().y),
        );
        Self::new(min, max - min)
    }
}

impl<T> From<Size<T>> for Rect<T>
where
    T: Default + Zero,
{
    fn from(size: Size<T>) -> Self {
        Self::new(Point::ZERO, size)
    }
}

impl<T> Zero for Rect<T>
where
    T: Zero,
{
    const ZERO: Self = Self::new(Point::ZERO, Size::ZERO);
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Transform<T> {
    pub xx: T,
    pub xy: T,
    pub yx: T,
    pub yy: T,
    pub tx: T,
    pub ty: T,
}

impl<T> Transform<T> {
    pub fn identity() -> Self
    where
        T: One + Zero,
    {
        Self {
            xx: T::ONE,
            xy: T::ZERO,
            yx: T::ZERO,
            yy: T::ONE,
            tx: T::ZERO,
            ty: T::ZERO,
        }
    }

    pub fn from_scale(sx: T, sy: T) -> Self
    where
        T: Zero,
    {
        Self {
            xx: sx,
            xy: T::ZERO,
            yx: T::ZERO,
            yy: sy,
            tx: T::ZERO,
            ty: T::ZERO,
        }
    }

    pub fn from_scale_uniform(s: T) -> Self
    where
        T: Copy + Zero,
    {
        Self::from_scale(s, s)
    }

    pub fn from_translate(tx: T, ty: T) -> Self
    where
        T: One + Zero,
    {
        Self {
            xx: T::ONE,
            xy: T::ZERO,
            yx: T::ZERO,
            yy: T::ONE,
            tx,
            ty,
        }
    }

    pub fn translate(self, tx: T, ty: T) -> Self
    where
        T: Add<Output = T> + Copy,
    {
        Self {
            tx: self.tx + tx,
            ty: self.ty + ty,
            ..self
        }
    }

    pub fn scale(self, sx: T, sy: T) -> Self
    where
        T: Add<Output = T> + Copy + Mul<Output = T> + Zero,
    {
        Self {
            xx: self.xx * sx,
            xy: self.xy * sy,
            yx: self.yx * sx,
            yy: self.yy * sy,
            tx: self.tx * sx,
            ty: self.ty * sy,
        }
    }

    pub fn scale_uniform(self, s: T) -> Self
    where
        T: Add<Output = T> + Copy + Mul<Output = T> + Zero,
    {
        self.scale(s, s)
    }

    pub fn concat(self, other: Self) -> Self
    where
        T: Add<Output = T> + Copy + Mul<Output = T>,
    {
        Self {
            xx: self.xx * other.xx + self.xy * other.yx,
            xy: self.xx * other.xy + self.xy * other.yy,
            yx: self.yx * other.xx + self.yy * other.yx,
            yy: self.yx * other.xy + self.yy * other.yy,
            tx: self.tx * other.xx + self.ty * other.yx + other.tx,
            ty: self.tx * other.xy + self.ty * other.yy + other.ty,
        }
    }
}

impl<T> Default for Transform<T>
where
    T: One + Zero,
{
    fn default() -> Self {
        Self::identity()
    }
}
