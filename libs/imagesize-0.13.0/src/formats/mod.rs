pub mod aesprite;
pub mod bmp;
pub mod dds;
pub mod exr;
pub mod farbfeld;
pub mod gif;
pub mod hdr;
pub mod ico;
pub mod ilbm;
pub mod jpeg;
pub mod jxl;
pub mod ktx2;
pub mod png;
pub mod pnm;
pub mod psd;
pub mod qoi;
pub mod tga;
pub mod tiff;
pub mod vtf;
pub mod webp;

use crate::{container, ImageError, ImageResult, ImageType};
use std::io::{BufRead, Seek};

pub fn image_type<R: BufRead + Seek>(reader: &mut R) -> ImageResult<ImageType> {
    let mut header = [0; 12];
    reader.read_exact(&mut header)?;

    // Currently there are no formats where 1 byte is enough to determine format
    if header.len() < 2 {
        return Err(
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Not enough data").into(),
        );
    }

    // This is vaguely organized in what I assume are the most commonly used formats.
    // I don't know how much this matters for actual execution time.
    if jpeg::matches(&header) {
        return Ok(ImageType::Jpeg);
    }

    if png::matches(&header) {
        return Ok(ImageType::Png);
    }

    if gif::matches(&header) {
        return Ok(ImageType::Gif);
    }

    if tiff::matches(&header) {
        return Ok(ImageType::Tiff);
    }

    if webp::matches(&header) {
        return Ok(ImageType::Webp);
    }

    if let Some(c) = container::heif::matches(&header, reader) {
        return Ok(ImageType::Heif(c));
    }

    if jxl::matches(&header) {
        return Ok(ImageType::Jxl);
    }

    if bmp::matches(&header) {
        return Ok(ImageType::Bmp);
    }

    if psd::matches(&header) {
        return Ok(ImageType::Psd);
    }

    if ico::matches(&header) {
        return Ok(ImageType::Ico);
    }

    if aesprite::matches(&header) {
        return Ok(ImageType::Aseprite);
    }

    if exr::matches(&header) {
        return Ok(ImageType::Exr);
    }

    if hdr::matches(&header) {
        return Ok(ImageType::Hdr);
    }

    if dds::matches(&header) {
        return Ok(ImageType::Dds);
    }

    if ktx2::matches(&header) {
        return Ok(ImageType::Ktx2);
    }

    if qoi::matches(&header) {
        return Ok(ImageType::Qoi);
    }

    if farbfeld::matches(&header) {
        return Ok(ImageType::Farbfeld);
    }

    if pnm::matches(&header) {
        return Ok(ImageType::Pnm);
    }

    if vtf::matches(&header) {
        return Ok(ImageType::Vtf);
    }

    if ilbm::matches(&header) {
        return Ok(ImageType::Ilbm);
    }

    // Keep TGA last because it has the highest probability of false positives
    if tga::matches(&header, reader) {
        return Ok(ImageType::Tga);
    }

    Err(ImageError::NotSupported)
}
