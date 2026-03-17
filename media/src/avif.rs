use crate::{dav1d_ffi::Dav1dDecoder, yuv};
use std::io::Cursor;

#[derive(Debug, Clone)]
pub struct AvifImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub icc_profile: Option<Vec<u8>>,
}

pub fn decode_primary_rgba(bytes: &[u8]) -> Result<AvifImage, String> {
    if !crate::has_dav1d() {
        return Err("AVIF decode unavailable: makepad-media built without dav1d".to_string());
    }

    let mut cursor = Cursor::new(bytes);
    let ctx = mp4parse::read_avif(&mut cursor, mp4parse::ParseStrictness::Normal)
        .map_err(|e| format!("avif parse: {e:?}"))?;

    let coded = ctx
        .primary_item_coded_data()
        .ok_or_else(|| "avif parse: missing primary item coded data".to_string())?;

    let mut decoder = Dav1dDecoder::new().map_err(|e| format!("dav1d init: {e}"))?;
    let sent = decoder
        .send_data(coded, 0)
        .map_err(|e| format!("dav1d send primary: {e}"))?;
    if !sent {
        return Err("dav1d send primary: EAGAIN".to_string());
    }

    let primary = decoder
        .get_picture()
        .map_err(|e| format!("dav1d get primary: {e}"))?
        .ok_or_else(|| "dav1d get primary: no decoded picture".to_string())?;

    let mut rgba = Vec::new();
    let (width, height) = yuv::picture_to_rgba(&primary, &mut rgba);

    if let Some(alpha_coded) = ctx.alpha_item_coded_data() {
        if !alpha_coded.is_empty() {
            let mut alpha_decoder = Dav1dDecoder::new().map_err(|e| format!("dav1d init alpha: {e}"))?;
            let alpha_sent = alpha_decoder
                .send_data(alpha_coded, 0)
                .map_err(|e| format!("dav1d send alpha: {e}"))?;
            if alpha_sent {
                if let Some(alpha_pic) = alpha_decoder
                    .get_picture()
                    .map_err(|e| format!("dav1d get alpha: {e}"))?
                {
                    apply_alpha_from_luma(&alpha_pic, width, height, &mut rgba);
                }
            }
        }
    }

    let icc_profile = ctx.icc_colour_information().and_then(|res| res.ok().map(|p| p.to_vec()));

    Ok(AvifImage {
        width,
        height,
        rgba,
        icc_profile,
    })
}

fn apply_alpha_from_luma(
    pic: &crate::dav1d_ffi::DecodedPicture,
    expected_width: u32,
    expected_height: u32,
    rgba: &mut [u8],
) {
    if pic.width() != expected_width || pic.height() != expected_height {
        return;
    }

    let w = pic.width() as usize;
    let h = pic.height() as usize;
    let bpc = pic.bpc();
    let (y_ptr, y_stride) = pic.plane_y();

    if bpc <= 8 {
        for row in 0..h {
            let src = unsafe { std::slice::from_raw_parts(y_ptr.offset(row as isize * y_stride), w) };
            let dst_off = row * w * 4;
            for col in 0..w {
                rgba[dst_off + col * 4 + 3] = src[col];
            }
        }
    } else {
        let stride_px = y_stride / 2;
        for row in 0..h {
            let src = unsafe {
                std::slice::from_raw_parts((y_ptr as *const u16).offset(row as isize * stride_px), w)
            };
            let dst_off = row * w * 4;
            for col in 0..w {
                rgba[dst_off + col * 4 + 3] = (src[col] >> 2) as u8;
            }
        }
    }
}
