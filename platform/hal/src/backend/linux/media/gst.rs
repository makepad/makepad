//! GStreamer decoder preference policy, shared with the native Linux path.
use super::gstreamer_sys::*;
use std::ffi::{c_uint, c_void, CString};
pub fn bump_va_decoder_ranks(gst: &LibGStreamer) {
    let (Some(get_registry), Some(lookup), Some(set_rank)) = (
        gst.gst_registry_get,
        gst.gst_registry_lookup_feature,
        gst.gst_plugin_feature_set_rank,
    ) else {
        return;
    };
    // Above primary (256) and vaapidecodebin (258); below NVIDIA primary+1 paths.
    const MODERN_RANK: c_uint = 320;
    const LEGACY_RANK: c_uint = 0;
    unsafe {
        // Ensure the `va` / `vaapi` plugin features are registered before lookup.
        for name in ["vah264dec", "vapostproc", "vaapih264dec", "vaapipostproc"] {
            let ty = CString::new(name).unwrap();
            let el = (gst.gst_element_factory_make)(ty.as_ptr(), std::ptr::null());
            if !el.is_null() {
                (gst.gst_object_unref)(el as *mut c_void);
            }
        }

        let registry = get_registry();
        if registry.is_null() {
            return;
        }
        for name in [
            "vah264dec",
            "vah265dec",
            "vavp9dec",
            "vavp8dec",
            "vaav1dec",
            "vajpegdec",
            "vampeg2dec",
            "vapostproc",
        ] {
            let cname = CString::new(name).unwrap();
            let feature = lookup(registry, cname.as_ptr());
            if !feature.is_null() {
                set_rank(feature, MODERN_RANK);
                (gst.gst_object_unref)(feature as *mut c_void);
            }
        }
        for name in [
            "vaapidecodebin",
            "vaapih264dec",
            "vaapih265dec",
            "vaapivp9dec",
            "vaapivp8dec",
            "vaapiav1dec",
            "vaapijpegdec",
            "vaapimpeg2dec",
            "vaapivc1dec",
            "avdec_h264",
            "avdec_h265",
            "avdec_hev1",
        ] {
            let cname = CString::new(name).unwrap();
            let feature = lookup(registry, cname.as_ptr());
            if !feature.is_null() {
                set_rank(feature, LEGACY_RANK);
                (gst.gst_object_unref)(feature as *mut c_void);
            }
        }
        // Prefer modern vapostproc when present; otherwise keep vaapipostproc ranked
        // for system-memory download paths (it cannot export DMA-Buf on NVIDIA).
        let vapost = {
            let cname = CString::new("vapostproc").unwrap();
            let feature = lookup(registry, cname.as_ptr());
            let present = !feature.is_null();
            if present {
                (gst.gst_object_unref)(feature as *mut c_void);
            }
            present
        };
        let cname = CString::new("vaapipostproc").unwrap();
        let feature = lookup(registry, cname.as_ptr());
        if !feature.is_null() {
            set_rank(feature, if vapost { LEGACY_RANK } else { MODERN_RANK });
            (gst.gst_object_unref)(feature as *mut c_void);
        }
    }
}
