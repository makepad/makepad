//! Raw `IMFTransform` vtable calls shared by the Windows stream encoder and
//! decoder.
//!
//! WHY THIS EXISTS: the vendored `windows-rs` subset checked into this repo
//! (`libs/windows/windows-rs`, pre-generated — this crate has no codegen
//! step of its own to re-run) only generated the *implement-this-interface*
//! side of `IMFTransform` (the `IMFTransform_Impl` trait and the raw
//! `IMFTransform_Vtbl` struct — needed just so `GetTransformForStream` in
//! `windows_encoder.rs` has a `Vtable` type to satisfy `windows::core::
//! Interface`), not client-callable wrapper methods for `ProcessInput` /
//! `ProcessOutput` / `SetInputType` / `SetOutputType` / etc. Those calls are
//! the entire point of driving an MFT directly for streaming, so this module
//! calls through the (fully present) `IMFTransform_Vtbl` function pointers
//! by hand — the same thing the generated wrapper methods on every OTHER
//! interface in this crate do internally.
//!
//! UNVERIFIED: written and only ever `cargo check`ed (cross-compiled, never
//! linked or run — there is no Windows machine available). See the module
//! docs on `windows_stream_encoder`/`windows_stream_decoder` for the full
//! list of hand-derived constants this implies.

use windows::core::{Interface, GUID};
use windows::Win32::Media::MediaFoundation::{
    ICodecAPI, IMFAttributes, IMFMediaType, IMFSample, IMFTransform, MFT_MESSAGE_TYPE, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_INFO,
};
use windows::Win32::System::Variant::{VARENUM, VARIANT};

// mftransform.h — stable since Vista, not part of the vendored binding
// subset (see module doc). FLUSH/NOTIFY_END_OF_STREAM are unused today
// (neither backend currently issues an explicit flush/end-of-stream — a
// live session just drops the transform) but are kept alongside the ones
// that ARE used since they're one small, complete, documented set.
#[allow(dead_code)]
pub(crate) const MFT_MESSAGE_COMMAND_FLUSH: i32 = 0x0000_0000;
#[allow(dead_code)]
pub(crate) const MFT_MESSAGE_NOTIFY_END_OF_STREAM: i32 = 0x1000_0002;
pub(crate) const MFT_MESSAGE_NOTIFY_START_OF_STREAM: i32 = 0x1000_0003;
pub(crate) const MFT_MESSAGE_NOTIFY_BEGIN_STREAMING: i32 = 0x1000_0000;

/// `MF_E_TRANSFORM_NEED_MORE_INPUT` (mferror.h) — the pull loop's normal
/// "nothing more to give you right now" terminal condition.
pub(crate) const MF_E_TRANSFORM_NEED_MORE_INPUT: i32 = 0xC00D_6D72u32 as i32;
/// `MF_E_TRANSFORM_TYPE_NOT_SET` (mferror.h, 0xC00D6D60) — `ProcessOutput`
/// before any output type is committed. Negotiate one and retry.
pub(crate) const MF_E_TRANSFORM_TYPE_NOT_SET: i32 = 0xC00D_6D60u32 as i32;
/// `MF_E_TRANSFORM_STREAM_CHANGE` (mferror.h, 0xC00D6D61) — `ProcessOutput`
/// signals this once the decoder has parsed enough of the bitstream (SPS)
/// to know the real output format/dimensions; the caller must re-negotiate
/// the output type (see `get_output_available_type`) and retry.
pub(crate) const MF_E_TRANSFORM_STREAM_CHANGE: i32 = 0xC00D_6D61u32 as i32;
/// `MF_E_NOTACCEPTING` (mferror.h) — `ProcessInput` refused because output
/// is pending; drain, then offer the sample again.
pub(crate) const MF_E_NOTACCEPTING: i32 = 0xC00D_36B5u32 as i32;
/// `MF_E_BUFFERTOOSMALL` (mferror.h) — the caller-allocated output sample
/// is smaller than `MFT_OUTPUT_STREAM_INFO::cbSize` after a stream change.
pub(crate) const MF_E_BUFFERTOOSMALL: i32 = 0xC00D_36B1u32 as i32;

/// `MF_LOW_LATENCY` (mfapi.h) on the transform's attribute store: the
/// Microsoft H.264 decoder then emits every picture as soon as it is
/// decoded instead of holding a reorder window — without it a 2–3 frame
/// live pipeline never sees a single output frame.
pub(crate) const MF_LOW_LATENCY: GUID = GUID::from_u128(0x9c27891a_ed7a_40e1_88e8_b22727a024ee);
/// `CODECAPI_AVLowLatencyMode` (codecapi.h) — the same switch through the
/// decoder's `ICodecAPI`, which is where the H.264 decoder documents it.
pub(crate) const CODECAPI_AV_LOW_LATENCY_MODE: GUID = GUID::from_u128(0x9c27891a_ed7a_40e1_88e8_b22727a024ee);

/// Bit in `MFT_OUTPUT_STREAM_INFO::dwFlags` meaning the MFT allocates its
/// own output samples (caller must pass `None` to `process_output`,
/// otherwise the caller must pre-allocate one of `cbSize` bytes).
/// mftransform.h: WHOLE_SAMPLES 0x1, SINGLE_SAMPLE_PER_BUFFER 0x2,
/// FIXED_SAMPLE_SIZE 0x4, DISCARDABLE 0x8, OPTIONAL 0x10, PROVIDES_SAMPLES
/// 0x100, CAN_PROVIDE_SAMPLES 0x200 — the Microsoft H.264 decoder reports
/// 0x107, so reading bit 0 as "provides samples" hands it no buffer and
/// every `ProcessOutput` is `E_INVALIDARG`.
pub(crate) const MFT_OUTPUT_STREAM_PROVIDES_SAMPLES: u32 = 0x0000_0100;

pub(crate) unsafe fn process_message(transform: &IMFTransform, message: i32, param: usize) -> windows::core::Result<()> {
    let vtbl = Interface::vtable(transform);
    (vtbl.ProcessMessage)(Interface::as_raw(transform), MFT_MESSAGE_TYPE(message), param).ok()
}

/// `IMFTransform::GetAttributes` — the transform-level store where
/// [`MF_LOW_LATENCY`] lives. Optional for an MFT (`E_NOTIMPL` is normal).
pub(crate) unsafe fn get_attributes(transform: &IMFTransform) -> windows::core::Result<IMFAttributes> {
    let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
    let vtbl = Interface::vtable(transform);
    (vtbl.GetAttributes)(Interface::as_raw(transform), &mut raw).ok()?;
    Ok(IMFAttributes::from_raw(raw))
}

/// `IMFTransform::GetOutputCurrentType(0)` — the committed output type,
/// carrying the real frame size once the decoder has seen the SPS.
pub(crate) unsafe fn get_output_current_type(transform: &IMFTransform) -> windows::core::Result<IMFMediaType> {
    let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
    let vtbl = Interface::vtable(transform);
    (vtbl.GetOutputCurrentType)(Interface::as_raw(transform), 0, &mut raw).ok()?;
    Ok(IMFMediaType::from_raw(raw))
}

pub(crate) unsafe fn set_input_type(transform: &IMFTransform, media_type: &IMFMediaType) -> windows::core::Result<()> {
    let vtbl = Interface::vtable(transform);
    (vtbl.SetInputType)(Interface::as_raw(transform), 0, Interface::as_raw(media_type), 0).ok()
}

pub(crate) unsafe fn set_output_type(transform: &IMFTransform, media_type: &IMFMediaType) -> windows::core::Result<()> {
    let vtbl = Interface::vtable(transform);
    (vtbl.SetOutputType)(Interface::as_raw(transform), 0, Interface::as_raw(media_type), 0).ok()
}

pub(crate) unsafe fn get_output_stream_info(transform: &IMFTransform) -> windows::core::Result<MFT_OUTPUT_STREAM_INFO> {
    let mut info = MFT_OUTPUT_STREAM_INFO::default();
    let vtbl = Interface::vtable(transform);
    (vtbl.GetOutputStreamInfo)(Interface::as_raw(transform), 0, &mut info).ok()?;
    Ok(info)
}

/// `IMFTransform::GetOutputAvailableType(0, type_index)` — enumerates the
/// output media types the transform currently offers (only meaningful for a
/// decoder after enough input has been fed for it to know its real output
/// format; see [`MF_E_TRANSFORM_STREAM_CHANGE`]).
pub(crate) unsafe fn get_output_available_type(
    transform: &IMFTransform,
    type_index: u32,
) -> windows::core::Result<IMFMediaType> {
    let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
    let vtbl = Interface::vtable(transform);
    (vtbl.GetOutputAvailableType)(Interface::as_raw(transform), 0, type_index, &mut raw).ok()?;
    Ok(IMFMediaType::from_raw(raw))
}

/// `Ok(())` accepted; the caller is expected to inspect the *raw* HRESULT
/// via `.map_err` rather than pattern-match a friendly error, since the
/// caller (the ProcessInput/ProcessOutput pump loop) treats "rejected,
/// drain first" generically rather than matching the exact (unverified)
/// `MF_E_NOTACCEPTING` code.
pub(crate) unsafe fn process_input(transform: &IMFTransform, sample: &IMFSample) -> windows::core::HRESULT {
    let vtbl = Interface::vtable(transform);
    (vtbl.ProcessInput)(Interface::as_raw(transform), 0, Interface::as_raw(sample), 0)
}

/// Pulls one output sample. Before the output type is negotiated the caller
/// passes `None` so the MFT can report `MF_E_TRANSFORM_STREAM_CHANGE`.
/// Afterwards, `provided_sample` must be `Some` (pre-allocated by the caller,
/// sized from `get_output_stream_info().cbSize`) unless
/// `MFT_OUTPUT_STREAM_PROVIDES_SAMPLES` is set, in which case it must be
/// `None`. Returns the raw HRESULT (compare against
/// [`MF_E_TRANSFORM_NEED_MORE_INPUT`] for the loop's exit condition) and the
/// output sample when one was produced.
pub(crate) unsafe fn process_output(
    transform: &IMFTransform,
    provided_sample: Option<IMFSample>,
) -> (windows::core::HRESULT, u32, Option<IMFSample>) {
    let mut buffer = MFT_OUTPUT_DATA_BUFFER {
        dwStreamID: 0,
        pSample: core::mem::ManuallyDrop::new(provided_sample),
        dwStatus: 0,
        pEvents: core::mem::ManuallyDrop::new(None),
    };
    let mut status = 0u32;
    let vtbl = Interface::vtable(transform);
    let hr = (vtbl.ProcessOutput)(Interface::as_raw(transform), 0, 1, &mut buffer, &mut status);
    let sample = core::mem::ManuallyDrop::into_inner(buffer.pSample);
    (hr, buffer.dwStatus, sample)
}

/// `ICodecAPI::SetValue(guid, VT_UI4)` on the transform — how the Microsoft
/// codecs take their codec-level switches. codecapi.h documents
/// `CODECAPI_AVLowLatencyMode` as VT_BOOL, but the H.264 decoder rejects
/// that with "VT_UI4 != pValue->vt"; it wants the number.
/// Err when the MFT has no `ICodecAPI` or refuses the property.
pub(crate) unsafe fn set_codec_api_u32(transform: &IMFTransform, property: &GUID, value: u32) -> windows::core::Result<()> {
    let codec_api: ICodecAPI = transform.cast()?;
    let mut variant = VARIANT::default();
    (*variant.Anonymous.Anonymous).vt = VARENUM(19); // VT_UI4
    (*variant.Anonymous.Anonymous).Anonymous.ulVal = value;
    codec_api.SetValue(property, &variant)
}
