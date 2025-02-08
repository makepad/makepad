use super::{
    image::{Subimage, SubimageMut},
    pixels::R,
};

pub const PADDING: usize = 4;

pub fn coverage_to_sdf(coverage: &Subimage<'_, R<u8>>, output: &mut SubimageMut<'_, R<u8>>) {
    use {
        super::geom::{Point, Size},
        sdfer::{esdt, esdt::Params, Image2d, Unorm8},
    };

    assert_eq!(
        output.size() - coverage.size(),
        Size::new(2 * PADDING, 2 * PADDING)
    );
    let mut pixels = Vec::with_capacity(coverage.size().width * coverage.size().height);
    for y in 0..coverage.size().height {
        for x in 0..coverage.size().width {
            pixels.push(Unorm8::from_bits(coverage[Point::new(x, y)].r));
        }
    }
    let mut coverage = Image2d::from_storage(coverage.size().width, coverage.size().height, pixels);
    let (sdf, _) = esdt::glyph_to_sdf(
        &mut coverage,
        Params {
            pad: PADDING,
            ..Params::default()
        },
        None,
    );
    for y in 0..sdf.height() {
        for x in 0..sdf.width() {
            output[Point::new(x, y)] = R::new(sdf[(x, y)].to_bits())
        }
    }
}
