//! What VideoToolbox can do with H.264 here: a capability query of the machine, not of a
//! session, answered once. Moved from os/apple/apple_media.rs (`probe_apple_h264`).

use crate::media::H264Probe;
use makepad_apple_sys::*;
use std::sync::OnceLock;

fn vt_is_hardware_encode_supported(codec_type: u32) -> bool {
    use std::ffi::c_void;

    unsafe extern "C" {
        fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
    }

    type ProbeFn = unsafe extern "C" fn(codec_type: u32) -> BOOL;

    unsafe {
        let framework_path =
            b"/System/Library/Frameworks/VideoToolbox.framework/Versions/A/VideoToolbox\0";
        let handle = dlopen(framework_path.as_ptr() as *const i8, 1); // RTLD_LAZY
        if handle.is_null() {
            return false;
        }
        let symbol = dlsym(
            handle,
            b"VTIsHardwareEncodeSupported\0".as_ptr() as *const i8,
        );
        if symbol.is_null() {
            return false;
        }
        let probe: ProbeFn = std::mem::transmute(symbol);
        probe(codec_type) == YES
    }
}

fn probe_apple_h264() -> H264Probe {
    unsafe {
        let mut probe = H264Probe::default();

        probe.encode_hardware = vt_is_hardware_encode_supported(kCMVideoCodecType_H264);

        let mut enc: VTCompressionSessionRef = std::ptr::null_mut();
        let enc_status = VTCompressionSessionCreate(
            std::ptr::null(),
            64,
            64,
            kCMVideoCodecType_H264,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            None,
            std::ptr::null_mut(),
            &mut enc,
        );
        let encode_available = enc_status == 0 && !enc.is_null();
        if encode_available {
            VTCompressionSessionInvalidate(enc);
            CFRelease(enc as *const std::ffi::c_void);
        }
        probe.encode_software = encode_available && !probe.encode_hardware;

        probe.decode_hardware = VTIsHardwareDecodeSupported(kCMVideoCodecType_H264) == YES;

        // Baseline 64x64 SPS/PPS probe stream.
        let sps: [u8; 23] = [
            0x67, 0x42, 0xC0, 0x1E, 0xDA, 0x02, 0x80, 0x2D, 0xD0, 0x80, 0x88, 0x45, 0xA1, 0x00,
            0x00, 0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xF1,
        ];
        let pps: [u8; 4] = [0x68, 0xCE, 0x3C, 0x80];
        let mut format_desc: CMFormatDescriptionRef = std::ptr::null_mut();
        let parameter_set_ptrs: [*const u8; 2] = [sps.as_ptr(), pps.as_ptr()];
        let parameter_set_sizes: [usize; 2] = [sps.len(), pps.len()];

        let fmt_status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
            std::ptr::null(),
            2,
            parameter_set_ptrs.as_ptr(),
            parameter_set_sizes.as_ptr(),
            4,
            &mut format_desc,
        );

        let mut decode_available = false;
        if fmt_status == 0 && !format_desc.is_null() {
            let mut dec: VTDecompressionSessionRef = std::ptr::null_mut();
            let callback_record = VTDecompressionOutputCallbackRecord {
                decompressionOutputCallback: None,
                decompressionOutputRefCon: std::ptr::null_mut(),
            };
            let dec_status = VTDecompressionSessionCreate(
                std::ptr::null(),
                format_desc,
                std::ptr::null(),
                std::ptr::null(),
                &callback_record,
                &mut dec,
            );
            decode_available = dec_status == 0 && !dec.is_null();
            if !dec.is_null() {
                VTDecompressionSessionInvalidate(dec);
                CFRelease(dec as *const std::ffi::c_void);
            }
            CFRelease(format_desc as *const std::ffi::c_void);
        }

        probe.decode_software = decode_available && !probe.decode_hardware;
        probe
    }
}


/// The H.264 encode / decode support of this machine (probed on first use). An op: the probe's
/// one result lives in this crate.
#[makepad_relax_api::relax_api]
pub fn video_h264_probe() -> H264Probe {
    static PROBE: OnceLock<H264Probe> = OnceLock::new();
    *PROBE.get_or_init(probe_apple_h264)
}
