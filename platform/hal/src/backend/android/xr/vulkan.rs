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

/// Fixed foveation on `swapchain` (XR_FB_foveation + configuration + swapchain_update_state):
/// the runtime writes the profile into the chain's fragment density maps, which the passes
/// rendering into its images attach (a chain made `foveated`).
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

/// A swapchain of `array_size` layers with the given usage (a depth swapchain: the layer's depth).
#[allow(clippy::too_many_arguments)]
pub fn create_swapchain_for(xr: &LibOpenXr, session: XrSession, format: vk::Format, width: u32, height: u32, array_size: u32, create_flags: XrSwapchainCreateFlags,
    usage_flags: XrSwapchainUsageFlags) -> Result<(XrSwapchain, Vec<vk::Image>), String> {
    create_swapchain_foveated(xr, session, format, width, height, array_size, create_flags, usage_flags, false).map(|(h, i, _)| (h, i))
}

/// `create_swapchain_for`, `foveated`: with a fragment density map per image
/// (XR_FB_foveation_vulkan: the runtime's foveation profile writes them, `try_enable_fixed_foveation`):
/// each image's density map image and its size.
#[allow(clippy::too_many_arguments)]
pub fn create_swapchain_foveated(xr: &LibOpenXr, session: XrSession, format: vk::Format, width: u32, height: u32, array_size: u32, create_flags: XrSwapchainCreateFlags,
    usage_flags: XrSwapchainUsageFlags, foveated: bool) -> Result<(XrSwapchain, Vec<vk::Image>, Vec<(vk::Image, u32, u32)>), String> {
    let foveation = XrSwapchainCreateInfoFoveationFB { flags: XrSwapchainCreateFoveationFlagsFB::FRAGMENT_DENSITY_MAP_BIT_FB, ..Default::default() };
    let swap_chain_create_info = XrSwapchainCreateInfo {
        next: if foveated { &foveation as *const _ as *const std::ffi::c_void } else { std::ptr::null() },
        create_flags,
        usage_flags,
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
    let images = (|| {
        let mut len = 0u32;
        unsafe { (xr.xrEnumerateSwapchainImages)(swap_chain, 0, &mut len, std::ptr::null_mut()) }.to_result("xrEnumerateSwapchainImages")?;
        let mut maps = vec![XrSwapchainImageFoveationVulkanFB::default(); len as usize];
        let mut images = vec![XrSwapchainImageVulkanKHR::default(); len as usize];
        if foveated {
            for (i, m) in images.iter_mut().zip(maps.iter_mut()) { i.next = m as *mut _ as *mut std::ffi::c_void; }
        }
        unsafe { (xr.xrEnumerateSwapchainImages)(swap_chain, len, &mut len, images.as_mut_ptr() as *mut std::ffi::c_void) }.to_result("xrEnumerateSwapchainImages")?;
        let maps: Vec<(vk::Image, u32, u32)> = if foveated { maps.iter().map(|m| (vk::Image::from_raw(m.image), m.width, m.height)).collect() } else { Vec::new() };
        Ok::<_, String>((images.iter().map(|image| vk::Image::from_raw(image.image)).collect::<Vec<_>>(), maps))
    })();
    match images {
        Ok((images, maps)) => Ok((swap_chain, images, maps)),
        Err(e) => {
            unsafe { (xr.xrDestroySwapchain)(swap_chain) }.log_error("xrDestroySwapchain");
            Err(e)
        }
    }
}
