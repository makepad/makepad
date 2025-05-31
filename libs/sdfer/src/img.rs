// NOTE(eddyb) this is a separate module so that privacy affects sibling modules.
// FIXME(eddyb) deduplicate with `image` crate?

use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

// HACK(eddyb) only exists to allow toggling precision for testing purposes.
#[cfg(sdfer_use_f64_instead_of_f32)]
type f32 = f64;

/// `[0, 1]` represented by uniformly spaced `u8` values (`0..=255`),
/// i.e. `Unorm8(byte)` corresponds to the `f32` value `byte as f32 / 255.0`.
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Unorm8(u8);

impl Unorm8 {
    pub const MIN: Self = Self::from_bits(0);
    pub const MAX: Self = Self::from_bits(u8::MAX);

    #[inline(always)]
    pub fn encode(x: f32) -> Self {
        // NOTE(eddyb) manual `clamp` not needed, `(_: f32) as u8` will saturate:
        // https://doc.rust-lang.org/reference/expressions/operator-expr.html#numeric-cast
        Self((x * 255.0).round() as u8)
    }

    #[inline(always)]
    pub fn decode(self) -> f32 {
        self.0 as f32 / 255.0
    }

    #[inline(always)]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    #[inline(always)]
    pub const fn to_bits(self) -> u8 {
        self.0
    }
}

#[derive(Default, Copy, Clone)]
pub struct Image2d<T, Storage: AsRef<[T]> = Vec<T>> {
    width: usize,
    height: usize,
    data: Storage,
    _marker: PhantomData<T>,
}

impl<T, Storage: AsRef<[T]>> Image2d<T, Storage> {
    pub fn new(width: usize, height: usize) -> Self
    where
        T: Default,
        Storage: FromIterator<T>,
    {
        Self::from_fn(width, height, |_, _| T::default())
    }

    pub fn from_fn(width: usize, height: usize, mut f: impl FnMut(usize, usize) -> T) -> Self
    where
        Storage: FromIterator<T>,
    {
        Self::from_storage(
            width,
            height,
            (0..height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .map(|(x, y)| f(x, y))
                .collect(),
        )
    }

    pub fn from_storage(width: usize, height: usize, storage: Storage) -> Self {
        assert_eq!(storage.as_ref().len(), width * height);
        Self {
            width,
            height,
            data: storage,
            _marker: PhantomData,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn reborrow(&self) -> Image2d<T, &[T]> {
        Image2d {
            width: self.width,
            height: self.height,
            data: self.data.as_ref(),
            _marker: PhantomData,
        }
    }

    pub fn reborrow_mut(&mut self) -> Image2d<T, &mut [T]>
    where
        Storage: AsMut<[T]>,
    {
        Image2d {
            width: self.width,
            height: self.height,
            data: self.data.as_mut(),
            _marker: PhantomData,
        }
    }

    pub fn cursor_at(&mut self, x: usize, y: usize) -> Image2dCursor<'_, T>
    where
        Storage: AsMut<[T]>,
    {
        let mut cursor = Image2dCursor {
            image: self.reborrow_mut(),
            xy_offset: 0,
        };
        cursor.reset((x, y));
        cursor
    }
}

impl<T, Storage: AsRef<[T]>> Index<(usize, usize)> for Image2d<T, Storage> {
    type Output = T;

    fn index(&self, (x, y): (usize, usize)) -> &T {
        &self.data.as_ref()[y * self.width..][..self.width][x]
    }
}

impl<T, Storage: AsMut<[T]> + AsRef<[T]>> IndexMut<(usize, usize)> for Image2d<T, Storage> {
    fn index_mut(&mut self, (x, y): (usize, usize)) -> &mut T {
        &mut self.data.as_mut()[y * self.width..][..self.width][x]
    }
}

#[cfg(feature = "image")]
impl From<image::GrayImage> for Image2d<Unorm8> {
    fn from(img: image::GrayImage) -> Self {
        Self {
            width: img.width().try_into().unwrap(),
            height: img.height().try_into().unwrap(),
            // HACK(eddyb) this should be a noop if the right specializations
            // all kick in, and LLVM optimizes out the in-place transformation.
            data: img.into_vec().into_iter().map(Unorm8::from_bits).collect(),
            _marker: PhantomData,
        }
    }
}

#[cfg(feature = "image")]
impl From<Image2d<Unorm8>> for image::GrayImage {
    fn from(img: Image2d<Unorm8>) -> Self {
        image::GrayImage::from_vec(
            img.width().try_into().unwrap(),
            img.height().try_into().unwrap(),
            // HACK(eddyb) this should be a noop if the right specializations
            // all kick in, and LLVM optimizes out the in-place transformation.
            img.data.into_iter().map(Unorm8::to_bits).collect(),
        )
        .unwrap()
    }
}

impl<T: Copy> Image2d<T> {
    fn resize_and_fill_with(&mut self, width: usize, height: usize, initial: T) {
        self.width = width;
        self.height = height;
        self.data.clear();
        self.data.resize(width * height, initial);
    }
}

#[derive(Default)]
pub struct Bitmap {
    width: usize,
    height: usize,
    bit_8x8_blocks: Image2d<u64>,
}

pub struct BitmapEntry<'a> {
    bit_8x8_block: &'a mut u64,
    mask: u64,
}

impl Bitmap {
    #[inline(always)]
    pub fn new(width: usize, height: usize) -> Self {
        let mut r = Self::default();
        r.resize_and_fill_with(width, height, false);
        r
    }

    #[inline(always)]
    pub(crate) fn resize_and_fill_with(&mut self, width: usize, height: usize, initial: bool) {
        self.width = width;
        self.height = height;
        self.bit_8x8_blocks.resize_and_fill_with(
            width.div_ceil(8),
            height.div_ceil(8),
            if initial { !0 } else { 0 },
        );
    }

    #[inline(always)]
    pub fn width(&self) -> usize {
        self.width
    }

    #[inline(always)]
    pub fn height(&self) -> usize {
        self.height
    }

    const BW: usize = 8;
    const BH: usize = 8;

    #[inline(always)]
    const fn bit_8x8_block_xy_and_mask(x: usize, y: usize) -> ((usize, usize), u64) {
        (
            (x / Self::BW, y / Self::BH),
            1 << ((y % Self::BH) * Self::BW + x % Self::BW),
        )
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize) -> bool {
        let (block_xy, mask) = Self::bit_8x8_block_xy_and_mask(x, y);
        (self.bit_8x8_blocks[block_xy] & mask) != 0
    }

    #[inline(always)]
    pub fn at(&mut self, x: usize, y: usize) -> BitmapEntry<'_> {
        let (block_xy, mask) = Self::bit_8x8_block_xy_and_mask(x, y);
        BitmapEntry {
            bit_8x8_block: &mut self.bit_8x8_blocks[block_xy],
            mask,
        }
    }

    #[inline(always)]
    pub fn cursor_at(&mut self, x: usize, y: usize) -> BitmapCursor<'_> {
        let mut cursor = BitmapCursor {
            bit_8x8_blocks: self.bit_8x8_blocks.cursor_at(0, 0),
            intra_block_xy: (0, 0),
        };
        cursor.reset((x, y));
        cursor
    }
}

impl BitmapEntry<'_> {
    #[inline(always)]
    pub fn get(&self) -> bool {
        (*self.bit_8x8_block & self.mask) != 0
    }

    #[inline(always)]
    pub fn set(&mut self, value: bool) {
        if value {
            *self.bit_8x8_block |= self.mask;
        } else {
            *self.bit_8x8_block &= !self.mask;
        }
    }
}

// FIXME(eddyb) this doesn't really belong here, and should use GATs.
pub trait NDCursor<'a, P> {
    type RefMut;
    fn reset(&'a mut self, position: P);
    fn get_mut(&'a mut self) -> Self::RefMut;
    fn advance(&'a mut self, delta: P);
}

pub trait NDCursorExt<P>: for<'a> NDCursor<'a, P> {
    fn zip<C2: NDCursorExt<P>>(self, other: C2) -> NDCursorZip<Self, C2>
    where
        Self: Sized,
    {
        NDCursorZip(self, other)
    }

    // FIXME(eddyb) this is a really bad API but a whole coordinate system would be overkill.
    fn map_abs_and_rel<P2, FA: Fn(P2) -> P, FR: Fn(P2) -> P>(
        self,
        fa: FA,
        fr: FR,
    ) -> NDCursorMapPos<Self, FA, FR>
    where
        Self: Sized,
    {
        NDCursorMapPos(self, fa, fr)
    }
}
impl<P, C: for<'a> NDCursor<'a, P>> NDCursorExt<P> for C {}

pub struct NDCursorZip<C1, C2>(C1, C2);
impl<'a, P: Copy, C1: NDCursor<'a, P>, C2: NDCursor<'a, P>> NDCursor<'a, P>
    for NDCursorZip<C1, C2>
{
    type RefMut = (C1::RefMut, C2::RefMut);
    #[inline(always)]
    fn reset(&'a mut self, position: P) {
        self.0.reset(position);
        self.1.reset(position);
    }
    #[inline(always)]
    fn get_mut(&'a mut self) -> Self::RefMut {
        (self.0.get_mut(), self.1.get_mut())
    }
    #[inline(always)]
    fn advance(&'a mut self, delta: P) {
        self.0.advance(delta);
        self.1.advance(delta);
    }
}

pub struct NDCursorMapPos<C, FA, FR>(C, FA, FR);
impl<'a, C: NDCursor<'a, P>, P, P2, FA: Fn(P2) -> P, FR: Fn(P2) -> P> NDCursor<'a, P2>
    for NDCursorMapPos<C, FA, FR>
{
    type RefMut = C::RefMut;
    #[inline(always)]
    fn reset(&'a mut self, position: P2) {
        self.0.reset((self.1)(position));
    }
    #[inline(always)]
    fn get_mut(&'a mut self) -> Self::RefMut {
        self.0.get_mut()
    }
    #[inline(always)]
    fn advance(&'a mut self, delta: P2) {
        self.0.advance((self.2)(delta));
    }
}

pub struct Image2dCursor<'a, T> {
    // FIXME(eddyb) find a way to use something closer to `slice::IterMut` here.
    image: Image2d<T, &'a mut [T]>,
    xy_offset: usize,
}

impl<'a, T: 'a> NDCursor<'a, (usize, usize)> for Image2dCursor<'_, T> {
    type RefMut = &'a mut T;
    #[inline(always)]
    fn reset(&'a mut self, (x, y): (usize, usize)) {
        self.xy_offset = y * self.image.width + x;
    }
    #[inline(always)]
    fn get_mut(&'a mut self) -> Self::RefMut {
        &mut self.image.data[self.xy_offset]
    }
    #[inline(always)]
    fn advance(&'a mut self, (dx, dy): (usize, usize)) {
        // FIXME(eddyb) check for edge conditions? (should be more like an iterator)
        self.xy_offset += dy * self.image.width + dx;
    }
}

pub struct BitmapCursor<'a> {
    bit_8x8_blocks: Image2dCursor<'a, u64>,
    // FIXME(eddyb) because of this we can't just use `bit_8x8_block_xy_and_mask`.
    intra_block_xy: (u8, u8),
}

impl<'a> NDCursor<'a, (usize, usize)> for BitmapCursor<'_> {
    type RefMut = BitmapEntry<'a>;
    #[inline(always)]
    fn reset(&'a mut self, (x, y): (usize, usize)) {
        self.bit_8x8_blocks.reset((x / Bitmap::BW, y / Bitmap::BH));
        self.intra_block_xy = ((x % Bitmap::BW) as u8, (y % Bitmap::BH) as u8);
    }
    #[inline(always)]
    fn get_mut(&'a mut self) -> Self::RefMut {
        let bxy = self.intra_block_xy;
        let (_, mask) = Bitmap::bit_8x8_block_xy_and_mask(bxy.0 as usize, bxy.1 as usize);
        BitmapEntry {
            bit_8x8_block: self.bit_8x8_blocks.get_mut(),
            mask,
        }
    }
    #[inline(always)]
    fn advance(&'a mut self, (dx, dy): (usize, usize)) {
        // FIXME(eddyb) check for edge conditions? (should be more like an iterator)
        let bxy = self.intra_block_xy;
        let new_bxy = (bxy.0 as usize + dx, bxy.1 as usize + dy);

        let whole_block_dxy = (new_bxy.0 / Bitmap::BW, new_bxy.1 / Bitmap::BH);
        if whole_block_dxy != (0, 0) {
            self.bit_8x8_blocks.advance(whole_block_dxy);
        }

        self.intra_block_xy = (
            (new_bxy.0 % Bitmap::BW) as u8,
            (new_bxy.1 % Bitmap::BH) as u8,
        );
    }
}
