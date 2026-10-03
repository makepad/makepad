//! Sample ownership and codec configuration shared by the stream pumps.
use windows::core::{GUID, HRESULT};
use windows::Win32::Media::MediaFoundation::{
    ICodecAPI, IMFSample, IMFTransform, MFT_OUTPUT_DATA_BUFFER,
};
use windows::Win32::System::Variant::{VARIANT, VT_UI4};

pub(crate) unsafe fn process_output(
    transform: &IMFTransform,
    provided_sample: Option<IMFSample>,
) -> (HRESULT, u32, Option<IMFSample>) {
    let mut buffer = MFT_OUTPUT_DATA_BUFFER::default();
    buffer.pSample = provided_sample.map_or(std::ptr::null_mut(), IMFSample::into_raw);
    let mut status = 0;
    let hr = transform
        .ProcessOutput(0, std::slice::from_mut(&mut buffer), &mut status)
        .err()
        .unwrap_or(HRESULT(0));
    // Both fields are owned out references, including any event collection
    // returned with a stream-change status.
    if !buffer.pEvents.is_null() {
        windows::core::release(buffer.pEvents);
    }
    let sample = IMFSample::from_raw(buffer.pSample).ok();
    (hr, buffer.dwStatus, sample)
}

pub(crate) unsafe fn set_codec_api_u32(
    transform: &IMFTransform,
    property: &GUID,
    value: u32,
) -> Result<(), HRESULT> {
    let codec_api = ICodecAPI::query(transform.as_raw())?;
    let mut variant = VARIANT::default();
    variant.Anonymous.Anonymous.vt = VT_UI4;
    variant.Anonymous.Anonymous.Anonymous.ulVal = value;
    codec_api.SetValue(property, &variant)
}
