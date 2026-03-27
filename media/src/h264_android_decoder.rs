//! Android NDK MediaCodec-based H.264 frame decoder.
//!
//! Uses the `AMediaCodec` C API (NDK, API 21+) to hardware-decode H.264
//! Annex B NAL units into I420 YUV frames.

#![cfg(target_os = "android")]

use makepad_platform::{
    MseDecodedFrame, VideoFrameDecoder,
    video_decode::yuv::{YuvColorMatrix, YuvLayout, YuvPlaneData},
};
use std::ffi::c_void;
use std::os::raw::c_char;
use std::ptr;

// ---------------------------------------------------------------------------
// NDK AMediaCodec / AMediaFormat FFI (from <media/NdkMediaCodec.h>)
// ---------------------------------------------------------------------------

type AMediaCodec = c_void;
type AMediaFormat = c_void;

#[repr(C)]
struct AMediaCodecBufferInfo {
    offset: i32,
    size: i32,
    presentation_time_us: i64,
    flags: u32,
}

const AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED: isize = -2;
const AMEDIACODEC_INFO_TRY_AGAIN_LATER: isize = -1;
const AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM: u32 = 4;

const AMEDIA_OK: i32 = 0;

/// NV12 (YUV420SemiPlanar) color format constant.
const COLOR_FORMAT_YUV420_SEMIPLANAR: i32 = 21;
/// I420 (YUV420Planar) color format constant.
const COLOR_FORMAT_YUV420_PLANAR: i32 = 19;

#[link(name = "mediandk")]
extern "C" {
    fn AMediaCodec_createDecoderByType(mime_type: *const c_char) -> *mut AMediaCodec;
    fn AMediaCodec_configure(
        codec: *mut AMediaCodec,
        format: *mut AMediaFormat,
        surface: *mut c_void,
        crypto: *mut c_void,
        flags: u32,
    ) -> i32;
    fn AMediaCodec_start(codec: *mut AMediaCodec) -> i32;
    fn AMediaCodec_stop(codec: *mut AMediaCodec) -> i32;
    fn AMediaCodec_delete(codec: *mut AMediaCodec) -> i32;
    fn AMediaCodec_flush(codec: *mut AMediaCodec) -> i32;
    fn AMediaCodec_dequeueInputBuffer(codec: *mut AMediaCodec, timeout_us: i64) -> isize;
    fn AMediaCodec_getInputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        out_size: *mut usize,
    ) -> *mut u8;
    fn AMediaCodec_queueInputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        offset: usize,
        size: usize,
        time: u64,
        flags: u32,
    ) -> i32;
    fn AMediaCodec_dequeueOutputBuffer(
        codec: *mut AMediaCodec,
        info: *mut AMediaCodecBufferInfo,
        timeout_us: i64,
    ) -> isize;
    fn AMediaCodec_getOutputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        out_size: *mut usize,
    ) -> *mut u8;
    fn AMediaCodec_releaseOutputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        render: bool,
    ) -> i32;
    fn AMediaCodec_getOutputFormat(codec: *mut AMediaCodec) -> *mut AMediaFormat;

    fn AMediaFormat_new() -> *mut AMediaFormat;
    fn AMediaFormat_delete(format: *mut AMediaFormat) -> i32;
    fn AMediaFormat_setString(format: *mut AMediaFormat, name: *const c_char, value: *const c_char);
    fn AMediaFormat_setInt32(format: *mut AMediaFormat, name: *const c_char, value: i32);
    fn AMediaFormat_getInt32(
        format: *mut AMediaFormat,
        name: *const c_char,
        out: *mut i32,
    ) -> bool;
    fn AMediaFormat_setBuffer(
        format: *mut AMediaFormat,
        name: *const c_char,
        data: *const u8,
        size: usize,
    );
}

// ---------------------------------------------------------------------------
// Annex B SPS/PPS extraction
// ---------------------------------------------------------------------------

/// Find Annex B start code positions (both 3-byte 00 00 01 and 4-byte 00 00 00 01).
fn find_annexb_nals(data: &[u8]) -> Vec<(usize, usize)> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i < data.len() {
        // 4-byte start code
        if i + 3 < data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1 {
            let start = i;
            let nal_start = i + 4;
            // find next start code
            let mut end = data.len();
            let mut j = nal_start;
            while j + 2 < data.len() {
                if data[j] == 0 && data[j + 1] == 0 && (data[j + 2] == 1 || (j + 3 < data.len() && data[j + 2] == 0 && data[j + 3] == 1)) {
                    end = j;
                    break;
                }
                j += 1;
            }
            nals.push((start, end));
            i = end;
            continue;
        }
        // 3-byte start code
        if i + 2 < data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let start = i;
            let nal_start = i + 3;
            let mut end = data.len();
            let mut j = nal_start;
            while j + 2 < data.len() {
                if data[j] == 0 && data[j + 1] == 0 && (data[j + 2] == 1 || (j + 3 < data.len() && data[j + 2] == 0 && data[j + 3] == 1)) {
                    end = j;
                    break;
                }
                j += 1;
            }
            nals.push((start, end));
            i = end;
            continue;
        }
        i += 1;
    }
    nals
}

/// Extract SPS and PPS NAL units (with start codes) from Annex B data.
/// SPS: nal_type 7, PPS: nal_type 8.
fn extract_sps_pps(data: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let nals = find_annexb_nals(data);
    let mut sps = None;
    let mut pps = None;
    for (start, end) in nals {
        // Find the NAL header byte (first byte after start code)
        let nal_start = if start + 3 < end && data[start + 2] == 0 && data[start + 3] == 1 {
            start + 4
        } else {
            start + 3
        };
        if nal_start >= end {
            continue;
        }
        let nal_type = data[nal_start] & 0x1F;
        if nal_type == 7 && sps.is_none() {
            sps = Some(data[start..end].to_vec());
        } else if nal_type == 8 && pps.is_none() {
            pps = Some(data[start..end].to_vec());
        }
    }
    (sps, pps)
}

// ---------------------------------------------------------------------------
// AndroidH264Decoder
// ---------------------------------------------------------------------------

pub struct AndroidH264Decoder {
    codec: *mut AMediaCodec,
    width: u32,
    height: u32,
    stride: u32,
    slice_height: u32,
    color_format: i32,
}

unsafe impl Send for AndroidH264Decoder {}

impl AndroidH264Decoder {
    /// Create decoder from Annex B SPS/PPS data and initial dimensions.
    pub fn new(sps_pps_annexb: &[u8], width: u32, height: u32) -> Result<Self, String> {
        unsafe {
            let mime = b"video/avc\0".as_ptr() as *const c_char;
            let codec = AMediaCodec_createDecoderByType(mime);
            if codec.is_null() {
                return Err("AMediaCodec_createDecoderByType failed".into());
            }

            let format = AMediaFormat_new();
            if format.is_null() {
                AMediaCodec_delete(codec);
                return Err("AMediaFormat_new failed".into());
            }

            // Set required format keys
            AMediaFormat_setString(
                format,
                b"mime\0".as_ptr() as *const c_char,
                mime,
            );
            AMediaFormat_setInt32(
                format,
                b"width\0".as_ptr() as *const c_char,
                width as i32,
            );
            AMediaFormat_setInt32(
                format,
                b"height\0".as_ptr() as *const c_char,
                height as i32,
            );

            // Extract and set CSD (codec-specific data) buffers
            if !sps_pps_annexb.is_empty() {
                let (sps, pps) = extract_sps_pps(sps_pps_annexb);
                if let Some(sps) = &sps {
                    AMediaFormat_setBuffer(
                        format,
                        b"csd-0\0".as_ptr() as *const c_char,
                        sps.as_ptr(),
                        sps.len(),
                    );
                }
                if let Some(pps) = &pps {
                    AMediaFormat_setBuffer(
                        format,
                        b"csd-1\0".as_ptr() as *const c_char,
                        pps.as_ptr(),
                        pps.len(),
                    );
                }
            }

            let status = AMediaCodec_configure(
                codec,
                format,
                ptr::null_mut(),
                ptr::null_mut(),
                0,
            );
            AMediaFormat_delete(format);

            if status != AMEDIA_OK {
                AMediaCodec_delete(codec);
                return Err(format!("AMediaCodec_configure failed: {}", status));
            }

            let status = AMediaCodec_start(codec);
            if status != AMEDIA_OK {
                AMediaCodec_delete(codec);
                return Err(format!("AMediaCodec_start failed: {}", status));
            }

            Ok(AndroidH264Decoder {
                codec,
                width,
                height,
                stride: width,
                slice_height: height,
                color_format: COLOR_FORMAT_YUV420_SEMIPLANAR,
            })
        }
    }

    /// Read output format keys to update width, height, stride, slice_height, color_format.
    unsafe fn update_output_format(&mut self) {
        let format = AMediaCodec_getOutputFormat(self.codec);
        if format.is_null() {
            return;
        }

        let mut val: i32 = 0;
        if AMediaFormat_getInt32(format, b"width\0".as_ptr() as *const c_char, &mut val) {
            self.width = val as u32;
        }
        if AMediaFormat_getInt32(format, b"height\0".as_ptr() as *const c_char, &mut val) {
            self.height = val as u32;
        }
        if AMediaFormat_getInt32(format, b"stride\0".as_ptr() as *const c_char, &mut val) {
            self.stride = val as u32;
        }
        if AMediaFormat_getInt32(format, b"slice-height\0".as_ptr() as *const c_char, &mut val) {
            self.slice_height = val as u32;
        }
        if AMediaFormat_getInt32(format, b"color-format\0".as_ptr() as *const c_char, &mut val) {
            self.color_format = val;
        }

        // stride/slice_height may be 0 if not reported
        if self.stride == 0 {
            self.stride = self.width;
        }
        if self.slice_height == 0 {
            self.slice_height = self.height;
        }

        AMediaFormat_delete(format);
    }

    /// Extract I420 YuvPlaneData from a raw output buffer.
    ///
    /// MediaCodec output is typically NV12 (semi-planar) or I420 (planar).
    /// The buffer layout uses stride and slice_height for padding.
    fn extract_yuv(&self, buf: &[u8]) -> Option<YuvPlaneData> {
        let w = self.width as usize;
        let h = self.height as usize;
        let stride = self.stride as usize;
        let slice_h = self.slice_height as usize;

        let y_plane_size = stride * slice_h;
        if buf.len() < y_plane_size {
            return None;
        }

        // Extract Y plane (may have stride padding)
        let mut y = Vec::with_capacity(w * h);
        for row in 0..h {
            let start = row * stride;
            if start + w > buf.len() {
                return None;
            }
            y.extend_from_slice(&buf[start..start + w]);
        }

        let uv_w = w / 2;
        let uv_h = h / 2;
        let uv_stride = stride / 2;

        if self.color_format == COLOR_FORMAT_YUV420_PLANAR {
            // I420: Y plane, then U plane, then V plane
            let u_offset = y_plane_size;
            let u_plane_size = uv_stride * (slice_h / 2);
            let v_offset = u_offset + u_plane_size;

            let mut u = Vec::with_capacity(uv_w * uv_h);
            let mut v = Vec::with_capacity(uv_w * uv_h);
            for row in 0..uv_h {
                let us = u_offset + row * uv_stride;
                let vs = v_offset + row * uv_stride;
                if us + uv_w > buf.len() || vs + uv_w > buf.len() {
                    return None;
                }
                u.extend_from_slice(&buf[us..us + uv_w]);
                v.extend_from_slice(&buf[vs..vs + uv_w]);
            }

            Some(YuvPlaneData {
                y,
                u,
                v,
                width: w as u32,
                height: h as u32,
                layout: YuvLayout::I420,
                matrix: YuvColorMatrix::BT709,
            })
        } else {
            // NV12 (default): Y plane, then interleaved UV plane
            let uv_offset = y_plane_size;
            let interleaved_stride = stride; // NV12 UV row stride equals Y stride

            let mut u = Vec::with_capacity(uv_w * uv_h);
            let mut v = Vec::with_capacity(uv_w * uv_h);
            for row in 0..uv_h {
                let row_start = uv_offset + row * interleaved_stride;
                for col in 0..uv_w {
                    let idx = row_start + col * 2;
                    if idx + 1 >= buf.len() {
                        return None;
                    }
                    u.push(buf[idx]);
                    v.push(buf[idx + 1]);
                }
            }

            Some(YuvPlaneData {
                y,
                u,
                v,
                width: w as u32,
                height: h as u32,
                layout: YuvLayout::I420,
                matrix: YuvColorMatrix::BT709,
            })
        }
    }
}

impl VideoFrameDecoder for AndroidH264Decoder {
    fn push_data(&mut self, data: &[u8], pts_ms: u64) -> Result<(), String> {
        unsafe {
            let idx = AMediaCodec_dequeueInputBuffer(self.codec, 10_000); // 10ms timeout
            if idx < 0 {
                return Err("no input buffer available".into());
            }
            let idx = idx as usize;

            let mut buf_size: usize = 0;
            let buf_ptr = AMediaCodec_getInputBuffer(self.codec, idx, &mut buf_size);
            if buf_ptr.is_null() {
                return Err("AMediaCodec_getInputBuffer returned null".into());
            }

            if data.len() > buf_size {
                // Queue empty buffer to avoid stalling the codec
                AMediaCodec_queueInputBuffer(self.codec, idx, 0, 0, 0, 0);
                return Err(format!(
                    "input data ({}) exceeds buffer size ({})",
                    data.len(),
                    buf_size
                ));
            }

            ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr, data.len());

            let pts_us = pts_ms * 1000;
            let status = AMediaCodec_queueInputBuffer(
                self.codec,
                idx,
                0,
                data.len(),
                pts_us,
                0,
            );
            if status != AMEDIA_OK {
                return Err(format!("AMediaCodec_queueInputBuffer failed: {}", status));
            }

            Ok(())
        }
    }

    fn pull_frame(&mut self) -> Result<Option<MseDecodedFrame>, String> {
        unsafe {
            let mut info = AMediaCodecBufferInfo {
                offset: 0,
                size: 0,
                presentation_time_us: 0,
                flags: 0,
            };

            let idx = AMediaCodec_dequeueOutputBuffer(self.codec, &mut info, 0); // non-blocking

            if idx == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED {
                self.update_output_format();
                // Retry once after format change
                let idx2 = AMediaCodec_dequeueOutputBuffer(self.codec, &mut info, 0);
                if idx2 < 0 {
                    return Ok(None);
                }
                return self.read_and_release(idx2 as usize, &info);
            }

            if idx == AMEDIACODEC_INFO_TRY_AGAIN_LATER || idx < 0 {
                return Ok(None);
            }

            self.read_and_release(idx as usize, &info)
        }
    }

    fn flush(&mut self) {
        unsafe {
            AMediaCodec_flush(self.codec);
        }
    }
}

impl AndroidH264Decoder {
    unsafe fn read_and_release(
        &self,
        idx: usize,
        info: &AMediaCodecBufferInfo,
    ) -> Result<Option<MseDecodedFrame>, String> {
        let mut buf_size: usize = 0;
        let buf_ptr = AMediaCodec_getOutputBuffer(self.codec, idx, &mut buf_size);

        let frame = if !buf_ptr.is_null() && info.size > 0 {
            let offset = info.offset as usize;
            let size = info.size as usize;
            if offset + size <= buf_size {
                let data = std::slice::from_raw_parts(buf_ptr.add(offset), size);
                self.extract_yuv(data).map(|yuv| MseDecodedFrame {
                    track_id: 0,
                    pts_ms: (info.presentation_time_us / 1000) as u64,
                    yuv,
                })
            } else {
                None
            }
        } else {
            None
        };

        AMediaCodec_releaseOutputBuffer(self.codec, idx, false);
        Ok(frame)
    }
}

impl Drop for AndroidH264Decoder {
    fn drop(&mut self) {
        unsafe {
            AMediaCodec_stop(self.codec);
            AMediaCodec_delete(self.codec);
        }
    }
}
