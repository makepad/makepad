//! Byte layouts a byte-buffer hardware stream encoder works with (Android's MediaCodec): tightly
//! packed NV12 into the codec's input layout (rows `stride` bytes apart, the chroma plane from
//! row `slice_height` on), and key access units with the stream's parameter sets in front.
//! Plain byte work with no platform dependency, so it is tested on every OS.

/// The bytes a codec's input buffer needs for a `width` x `height` NV12 picture whose rows are
/// `stride` bytes apart and whose chroma plane starts at row `slice_height`.
pub fn strided_nv12_len(height: u32, stride: u32, slice_height: u32) -> usize {
    stride as usize * slice_height as usize + stride as usize * (height as usize / 2)
}

/// Copies tightly packed NV12 (`width` x `height` luma bytes, then `height/2` rows of
/// interleaved CbCr) into `out` in the codec's layout ([`strided_nv12_len`] bytes). The padding
/// between rows is left as it is. The length written, or why it does not fit.
pub fn nv12_to_strided(nv12: &[u8], width: u32, height: u32, stride: u32, slice_height: u32, out: &mut [u8]) -> Result<usize, String> {
    let (w, h, stride, slice) = (width as usize, height as usize, stride as usize, slice_height as usize);
    if stride < w || slice < h {
        return Err(format!("input layout stride {stride} / slice height {slice} is smaller than the {w}x{h} picture"));
    }
    if nv12.len() < w * h + w * (h / 2) {
        return Err(format!("{} bytes are no {w}x{h} NV12 picture", nv12.len()));
    }
    let needed = strided_nv12_len(height, stride as u32, slice as u32);
    if out.len() < needed {
        return Err(format!("the input buffer holds {} bytes, the picture needs {needed}", out.len()));
    }
    for row in 0..h {
        out[row * stride..row * stride + w].copy_from_slice(&nv12[row * w..row * w + w]);
    }
    let chroma = &nv12[w * h..];
    for row in 0..h / 2 {
        let at = stride * slice + row * stride;
        out[at..at + w].copy_from_slice(&chroma[row * w..row * w + w]);
    }
    Ok(needed)
}

/// The type of the first NAL unit of an Annex-B access unit (H.264: its low 5 bits; H.265:
/// bits 1..6 of its first header byte); `None` without a start code.
pub fn first_nal_type(access_unit: &[u8], hevc: bool) -> Option<u8> {
    let header = if access_unit.starts_with(&[0, 0, 0, 1]) {
        access_unit.get(4)
    } else if access_unit.starts_with(&[0, 0, 1]) {
        access_unit.get(3)
    } else {
        None
    }?;
    Some(if hevc { (header >> 1) & 0x3f } else { header & 0x1f })
}

/// Whether the access unit begins with a parameter set (H.264 SPS; H.265 VPS or SPS): a key
/// access unit a decoder can start from.
pub fn starts_with_parameter_set(access_unit: &[u8], hevc: bool) -> bool {
    match first_nal_type(access_unit, hevc) {
        Some(t) if hevc => t == 32 || t == 33,
        Some(t) => t == 7,
        None => false,
    }
}

/// A key access unit as the stream sends it: the codec's config bytes (the parameter sets,
/// Annex-B) in front of `frame`, unless the encoder already put them there.
pub fn key_unit(config: &[u8], frame: &[u8], hevc: bool) -> Vec<u8> {
    if config.is_empty() || starts_with_parameter_set(frame, hevc) {
        return frame.to_vec();
    }
    let mut out = Vec::with_capacity(config.len() + frame.len());
    out.extend_from_slice(config);
    out.extend_from_slice(frame);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_goes_into_the_codec_layout() {
        // 4x2: luma 0..8, chroma 100..104
        let nv12: Vec<u8> = (0..8).chain(100..104).collect();
        let (stride, slice) = (6, 4);
        let mut out = vec![0xee; strided_nv12_len(2, stride, slice)];
        assert_eq!(out.len(), 6 * 4 + 6);
        assert_eq!(nv12_to_strided(&nv12, 4, 2, stride, slice, &mut out), Ok(30));
        assert_eq!(&out[0..4], &[0, 1, 2, 3]);
        assert_eq!(&out[4..6], &[0xee, 0xee], "row padding untouched");
        assert_eq!(&out[6..10], &[4, 5, 6, 7]);
        assert_eq!(&out[12..24], &[0xee; 12], "rows between the luma and the slice height untouched");
        assert_eq!(&out[24..28], &[100, 101, 102, 103], "chroma from row `slice` on");
    }

    #[test]
    fn a_tight_layout_is_a_copy() {
        let nv12: Vec<u8> = (0..24).collect();
        let mut out = vec![0; 24];
        assert_eq!(nv12_to_strided(&nv12, 4, 4, 4, 4, &mut out), Ok(24));
        assert_eq!(out, nv12);
    }

    #[test]
    fn a_short_buffer_or_layout_is_refused() {
        let nv12 = vec![0u8; 24];
        assert!(nv12_to_strided(&nv12, 4, 4, 4, 4, &mut [0; 23]).is_err());
        assert!(nv12_to_strided(&nv12, 4, 4, 2, 4, &mut [0; 64]).is_err());
        assert!(nv12_to_strided(&nv12, 4, 4, 4, 2, &mut [0; 64]).is_err());
        assert!(nv12_to_strided(&nv12[..10], 4, 4, 4, 4, &mut [0; 64]).is_err());
    }

    #[test]
    fn the_config_goes_in_front_of_a_key_frame_once() {
        let config = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3];
        let idr = [0, 0, 0, 1, 0x65, 9, 9];
        let unit = key_unit(&config, &idr, false);
        assert_eq!(&unit[..config.len()], &config);
        assert_eq!(&unit[config.len()..], &idr);
        assert!(starts_with_parameter_set(&unit, false));
        // an encoder that prepends its parameter sets itself: unchanged
        assert_eq!(key_unit(&config, &unit, false), unit);
        // no config seen yet: the frame as it is
        assert_eq!(key_unit(&[], &idr, false), idr.to_vec());
    }

    #[test]
    fn hevc_parameter_sets_are_recognised() {
        // VPS (32 << 1 = 0x40), an IDR_W_RADL slice (19 << 1 = 0x26), 3-byte start codes
        assert!(starts_with_parameter_set(&[0, 0, 1, 0x40, 1], true));
        assert!(!starts_with_parameter_set(&[0, 0, 1, 0x26, 1], true));
        assert_eq!(first_nal_type(&[0, 0, 1, 0x26, 1], true), Some(19));
        assert_eq!(first_nal_type(&[1, 2, 3], false), None);
        assert!(!starts_with_parameter_set(&[0, 0, 0, 1, 0x65], false));
    }
}
