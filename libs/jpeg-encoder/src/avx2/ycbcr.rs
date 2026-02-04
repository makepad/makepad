#[cfg(target_arch = "x86")]
use core::arch::x86::{
    __m256i, _mm256_add_epi32, _mm256_mullo_epi32, _mm256_set1_epi32, _mm256_set_epi32,
    _mm256_srli_epi32, _mm256_sub_epi32,
};

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{
    __m256i, _mm256_add_epi32, _mm256_mullo_epi32, _mm256_set1_epi32, _mm256_set_epi32,
    _mm256_srli_epi32, _mm256_sub_epi32,
};

use alloc::vec::Vec;

use crate::{rgb_to_ycbcr, ImageBuffer, JpegColorType};

macro_rules! ycbcr_image_avx2 {
    ($name:ident, $num_colors:expr, $o1:expr, $o2:expr, $o3:expr) => {
        pub(crate) struct $name<'a>(pub &'a [u8], pub u16, pub u16);

        impl<'a> $name<'a> {
            #[target_feature(enable = "avx2")]
            unsafe fn fill_buffers_avx2(&self, y: u16, buffers: &mut [Vec<u8>; 4]) {
                unsafe fn load3(data: *const u8) -> __m256i {
                    _mm256_set_epi32(
                        *data as i32,
                        *data.offset(1 * $num_colors) as i32,
                        *data.offset(2 * $num_colors) as i32,
                        *data.offset(3 * $num_colors) as i32,
                        *data.offset(4 * $num_colors) as i32,
                        *data.offset(5 * $num_colors) as i32,
                        *data.offset(6 * $num_colors) as i32,
                        *data.offset(7 * $num_colors) as i32,
                    )
                }

                let mut y_buffer = buffers[0].as_mut_ptr().add(buffers[0].len());
                buffers[0].set_len(buffers[0].len() + self.width() as usize);
                let mut cb_buffer = buffers[1].as_mut_ptr().add(buffers[1].len());
                buffers[1].set_len(buffers[1].len() + self.width() as usize);
                let mut cr_buffer = buffers[2].as_mut_ptr().add(buffers[2].len());
                buffers[2].set_len(buffers[2].len() + self.width() as usize);

                let ymulr = _mm256_set1_epi32(19595);
                let ymulg = _mm256_set1_epi32(38470);
                let ymulb = _mm256_set1_epi32(7471);

                let cbmulr = _mm256_set1_epi32(-11059);
                let cbmulg = _mm256_set1_epi32(21709);
                let cbmulb = _mm256_set1_epi32(32768);

                let crmulr = _mm256_set1_epi32(32768);
                let crmulg = _mm256_set1_epi32(27439);
                let crmulb = _mm256_set1_epi32(5329);

                let mut data = self
                    .0
                    .as_ptr()
                    .offset((y as isize * self.1 as isize * $num_colors));

                for _ in 0..self.width() / 8 {
                    let r = load3(data.offset($o1));
                    let g = load3(data.offset($o2));
                    let b = load3(data.offset($o3));

                    data = data.add($num_colors * 8);

                    let yr = _mm256_mullo_epi32(ymulr, r);
                    let yg = _mm256_mullo_epi32(ymulg, g);
                    let yb = _mm256_mullo_epi32(ymulb, b);

                    let y = _mm256_add_epi32(_mm256_add_epi32(yr, yg), yb);
                    let y = _mm256_add_epi32(y, _mm256_set1_epi32(0x7FFF));
                    let y = _mm256_srli_epi32(y, 16);
                    let y: [i32; 8] = core::mem::transmute(y);

                    let cbr = _mm256_mullo_epi32(cbmulr, r);
                    let cbg = _mm256_mullo_epi32(cbmulg, g);
                    let cbb = _mm256_mullo_epi32(cbmulb, b);

                    let cb = _mm256_add_epi32(_mm256_sub_epi32(cbr, cbg), cbb);
                    let cb = _mm256_add_epi32(cb, _mm256_set1_epi32(128 << 16));
                    let cb = _mm256_add_epi32(cb, _mm256_set1_epi32(0x7FFF));
                    let cb = _mm256_srli_epi32(cb, 16);
                    let cb: [i32; 8] = core::mem::transmute(cb);

                    let crr = _mm256_mullo_epi32(crmulr, r);
                    let crg = _mm256_mullo_epi32(crmulg, g);
                    let crb = _mm256_mullo_epi32(crmulb, b);

                    let cr = _mm256_sub_epi32(_mm256_sub_epi32(crr, crg), crb);
                    let cr = _mm256_add_epi32(cr, _mm256_set1_epi32(128 << 16));
                    let cr = _mm256_add_epi32(cr, _mm256_set1_epi32(0x7FFF));
                    let cr = _mm256_srli_epi32(cr, 16);
                    let cr: [i32; 8] = core::mem::transmute(cr);

                    for y in y.iter().rev() {
                        *y_buffer = *y as u8;
                        y_buffer = y_buffer.offset(1);
                    }

                    for cb in cb.iter().rev() {
                        *cb_buffer = *cb as u8;
                        cb_buffer = cb_buffer.offset(1);
                    }

                    for cr in cr.iter().rev() {
                        *cr_buffer = *cr as u8;
                        cr_buffer = cr_buffer.offset(1);
                    }
                }

                for _ in 0..self.width() % 8 {
                    let (y, cb, cr) =
                        rgb_to_ycbcr(*data.offset($o1), *data.offset($o2), *data.offset($o3));

                    data = data.add($num_colors);

                    *y_buffer = y;
                    y_buffer = y_buffer.offset(1);

                    *cb_buffer = cb;
                    cb_buffer = cb_buffer.offset(1);

                    *cr_buffer = cr;
                    cr_buffer = cr_buffer.offset(1);
                }
            }
        }

        impl<'a> ImageBuffer for $name<'a> {
            fn get_jpeg_color_type(&self) -> JpegColorType {
                JpegColorType::Ycbcr
            }

            fn width(&self) -> u16 {
                self.1
            }

            fn height(&self) -> u16 {
                self.2
            }

            #[inline(always)]
            fn fill_buffers(&self, y: u16, buffers: &mut [Vec<u8>; 4]) {
                unsafe {
                    self.fill_buffers_avx2(y, buffers);
                }
            }
        }
    };
}

ycbcr_image_avx2!(RgbImageAVX2, 3, 0, 1, 2);
ycbcr_image_avx2!(RgbaImageAVX2, 4, 0, 1, 2);
ycbcr_image_avx2!(BgrImageAVX2, 3, 2, 1, 0);
ycbcr_image_avx2!(BgraImageAVX2, 4, 2, 1, 0);
