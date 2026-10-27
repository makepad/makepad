//! The projection layer's swapchain (from the retired Android backend's
//! `platform/src/os/linux/openxr_vulkan.rs`): the colour format pick, the two-layer (one per
//! eye) swapchain and FB fixed foveation. The old file also drew Cx passes into the swapchain
//! with multiview and made the depth readback images; on the HAL, Cx draws the stereo window
//! itself and the XR thread copies each eye into its layer (`super::Chain`).
use super::sys::*;
use makepad_vulkan::vk;

/// The colour format of every XR swapchain. `preferred_format` first (the HAL's frames hold
/// sRGB-encoded colour, so an sRGB format shows them unchanged); then the old fallback list,
/// sRGB before UNORM for the same reason.
pub fn pick_vulkan_color_format(supported_formats: &[i64], preferred_format: vk::Format) -> Option<vk::Format> {
    let is_supported = |format: vk::Format| supported_formats.contains(&i64::from(format.as_raw()));

    if preferred_format != vk::Format::UNDEFINED && is_supported(preferred_format) {
        return Some(preferred_format);
    }

    [
        vk::Format::B8G8R8A8_SRGB,
        vk::Format::R8G8B8A8_SRGB,
        vk::Format::B8G8R8A8_UNORM,
        vk::Format::R8G8B8A8_UNORM,
    ]
    .into_iter()
    .find(|format| is_supported(*format))
}

#[allow(dead_code)]
pub fn desired_fixed_foveation_level(level: u8) -> Option<XrFoveationLevelFB> {
    match level {
        0 => None,
        1 => Some(XrFoveationLevelFB::LOW),
        2 => Some(XrFoveationLevelFB::MEDIUM),
        _ => Some(XrFoveationLevelFB::HIGH),
    }
}

/// Fixed foveation on `swapchain` (XR_FB_foveation + configuration + swapchain_update_state).
/// It takes effect where the app renders into the swapchain with the runtime's fragment density
/// map; the HAL copies finished frames into it, so the session does not ask for it yet (it comes
/// with rendering straight into the swapchain images).
#[allow(dead_code)]
pub fn try_enable_fixed_foveation(xr: &LibOpenXr, session: XrSession, swapchain: XrSwapchain, level: XrFoveationLevelFB) -> Result<(), String> {
    let create_profile = xr.xrCreateFoveationProfileFB.ok_or_else(|| "xrCreateFoveationProfileFB unavailable even though fixed foveation was requested".to_string())?;
    let update_swapchain = xr.xrUpdateSwapchainFB.ok_or_else(|| "xrUpdateSwapchainFB unavailable even though fixed foveation was requested".to_string())?;
    let destroy_profile = xr.xrDestroyFoveationProfileFB.ok_or_else(|| "xrDestroyFoveationProfileFB unavailable even though fixed foveation was requested".to_string())?;

    let level_info = XrFoveationLevelProfileCreateInfoFB { level, verticalOffset: 0.0, dynamic: XrFoveationDynamicFB::DISABLED, ..Default::default() };
    let profile_info = XrFoveationProfileCreateInfoFB { next: &level_info as *const _ as *const _, ..Default::default() };
    let mut profile = XrFoveationProfileFB(0);
    unsafe { (create_profile)(session, &profile_info, &mut profile) }.to_result("xrCreateFoveationProfileFB")?;

    let state = XrSwapchainStateFoveationFB { profile, ..Default::default() };
    let update_result = unsafe { (update_swapchain)(swapchain, &state as *const _ as *const XrSwapchainStateBaseHeaderFB) }.to_result("xrUpdateSwapchainFB");
    unsafe { (destroy_profile)(profile) }.log_error("xrDestroyFoveationProfileFB");
    update_result
}

/// A swapchain of `array_size` layers (2: the projection layer, one per eye, as the old
/// backend made it; 1: a quad layer) and its Vulkan images. The app renders into its images
/// (viewed in the UNORM format of the frames: mutable format) or the HAL copies frames into it
/// (a transfer destination).
pub fn create_swapchain(xr: &LibOpenXr, session: XrSession, format: vk::Format, width: u32, height: u32, array_size: u32, create_flags: XrSwapchainCreateFlags) -> Result<(XrSwapchain, Vec<vk::Image>), String> {
    let swap_chain_create_info = XrSwapchainCreateInfo {
        create_flags,
        usage_flags: XrSwapchainUsageFlags::SAMPLED | XrSwapchainUsageFlags::COLOR_ATTACHMENT | XrSwapchainUsageFlags::TRANSFER_DST | XrSwapchainUsageFlags::MUTABLE_FORMAT,
        format: i64::from(format.as_raw()),
        width,
        height,
        sample_count: 1,
        face_count: 1,
        array_size,
        mip_count: 1,
        ..Default::default()
    };
    let mut swap_chain = XrSwapchain(0);
    unsafe { (xr.xrCreateSwapchain)(session, &swap_chain_create_info, &mut swap_chain) }.to_result("xrCreateSwapchain")?;
    let images = xr_array_fetch(XrSwapchainImageVulkanKHR::default(), |cap, len, buf| {
        unsafe { (xr.xrEnumerateSwapchainImages)(swap_chain, cap, len, buf as *mut std::ffi::c_void) }.to_result("xrEnumerateSwapchainImages")
    });
    match images {
        Ok(images) => Ok((swap_chain, images.iter().map(|image| vk::Image::from_raw(image.image)).collect())),
        Err(e) => {
            unsafe { (xr.xrDestroySwapchain)(swap_chain) }.log_error("xrDestroySwapchain");
            Err(e)
        }
    }
}
