use super::*;
use crate::{
    os::linux::openxr_depth::CxOpenXrDepthMeshPipeline,
    os::linux::vulkan::{
        CxVulkan, CxVulkanOpenXrFoveationImageInfo, CxVulkanOpenXrSessionData,
        OpenXrVulkanRepaintStats,
    },
    xr_tsdf::xr_tsdf_store,
};
pub(super) struct CxOpenXrVulkanSession {
    _color_images: Vec<XrSwapchainImageVulkanKHR>,
    _depth_images: Vec<XrSwapchainImageVulkanKHR>,
    render_targets: CxVulkanOpenXrSessionData,
    depth_mesh_pipeline: CxOpenXrDepthMeshPipeline,
    retired_projection_layers: Vec<RetiredOpenXrVulkanProjectionLayer>,
}

struct RetiredOpenXrVulkanProjectionLayer {
    color_swap_chain: XrSwapchain,
    render_targets: CxVulkanOpenXrSessionData,
}

impl Cx {
    pub(crate) fn openxr_draw_pass_to_vulkan(
        &mut self,
        draw_pass_id: DrawPassId,
        frame: &CxOpenXrFrame,
    ) -> Result<OpenXrVulkanRepaintStats, String> {
        let draw_list_id = self.passes[draw_pass_id]
            .main_draw_list_id
            .ok_or_else(|| "OpenXR Vulkan render failed: missing main draw list".to_string())?;
        let dpi_factor = self.passes[draw_pass_id]
            .dpi_factor
            .ok_or_else(|| "OpenXR Vulkan render failed: missing dpi factor".to_string())?;
        let color_image_index = frame.swap_chain_index as usize;
        let depth_image_index = frame
            .depth_image
            .map(|image| image.swapchain_index as usize);
        let zero_mat = Mat4f { v: [0.0; 16] };
        let render_targets =
            {
                let session =
                    self.os.openxr.session.as_ref().ok_or_else(|| {
                        "OpenXR Vulkan render failed: missing session".to_string()
                    })?;
                let vulkan_session = session.vulkan.as_ref().ok_or_else(|| {
                    "OpenXR Vulkan render failed: missing Vulkan session".to_string()
                })?;
                &vulkan_session.render_targets as *const CxVulkanOpenXrSessionData
            };
        let pass = &mut self.passes[draw_pass_id];
        pass.set_dpi_factor(dpi_factor);
        pass.paint_dirty = true;
        pass.os.shader_variant = SHADER_VARIANT_XR;
        pass.pass_uniforms.camera_projection = frame.eyes[0].proj_mat;
        pass.pass_uniforms.camera_view = frame.eyes[0].view_mat;
        pass.pass_uniforms.camera_projection_r = frame.eyes[1].proj_mat;
        pass.pass_uniforms.camera_view_r = frame.eyes[1].view_mat;
        pass.pass_uniforms.camera_inv = frame.eyes[0].view_mat.invert();
        pass.pass_uniforms.camera_inv_r = frame.eyes[1].view_mat.invert();
        if depth_image_index.is_some() {
            pass.pass_uniforms.depth_projection = frame.eyes[0].depth_proj_mat;
            pass.pass_uniforms.depth_view = frame.eyes[0].depth_view_mat;
            pass.pass_uniforms.depth_projection_r = frame.eyes[1].depth_proj_mat;
            pass.pass_uniforms.depth_view_r = frame.eyes[1].depth_view_mat;
        } else {
            pass.pass_uniforms.depth_projection = zero_mat;
            pass.pass_uniforms.depth_view = zero_mat;
            pass.pass_uniforms.depth_projection_r = zero_mat;
            pass.pass_uniforms.depth_view_r = zero_mat;
        }

        let mut vulkan = self
            .os
            .vulkan
            .take()
            .ok_or_else(|| "OpenXR Vulkan render failed: backend unavailable".to_string())?;
        let result = unsafe {
            vulkan.draw_openxr_view(
                self,
                draw_pass_id,
                draw_list_id,
                &*render_targets,
                color_image_index,
                depth_image_index,
            )
        };
        self.os.vulkan = Some(vulkan);
        let stats = result?;

        #[cfg(target_os = "android")]
        if let Some(request) = self.take_studio_run_view_frame_request(0) {
            let mut vulkan = self.os.vulkan.take().ok_or_else(|| {
                "OpenXR Vulkan frame capture failed: backend unavailable".to_string()
            })?;
            let result = unsafe {
                vulkan.read_openxr_color_image_rgba(&*render_targets, color_image_index, 0)
            };
            self.os.vulkan = Some(vulkan);
            match result {
                Ok(rgba) => {
                    let session = self.os.openxr.session.as_ref().ok_or_else(|| {
                        "OpenXR Vulkan frame capture failed: session unavailable".to_string()
                    })?;
                    self.encode_studio_run_view_frame_async(
                        request,
                        session.width.max(1),
                        session.height.max(1),
                        rgba,
                    );
                }
                Err(err) => {
                    crate::error!("OpenXR Vulkan frame capture failed: {}", err);
                }
            }
        }
        Ok(stats)
    }
}

impl CxOpenXrVulkanSession {
    pub(crate) fn submit_depth_mesh_job(
        &mut self,
        vulkan: &mut CxVulkan,
        frame: &CxOpenXrFrame,
        depth_image_index: usize,
    ) -> Result<(), String> {
        self.depth_mesh_pipeline
            .submit(vulkan, &self.render_targets, frame, depth_image_index)
    }
}

impl CxOpenXrSession {
    fn pick_vulkan_color_format(
        supported_formats: &[i64],
        preferred_format: vk::Format,
    ) -> Option<vk::Format> {
        let is_supported =
            |format: vk::Format| supported_formats.contains(&i64::from(format.as_raw()));

        if preferred_format != vk::Format::UNDEFINED && is_supported(preferred_format) {
            return Some(preferred_format);
        }

        [
            vk::Format::B8G8R8A8_UNORM,
            vk::Format::R8G8B8A8_UNORM,
            vk::Format::B8G8R8A8_SRGB,
            vk::Format::R8G8B8A8_SRGB,
        ]
        .into_iter()
        .find(|format| is_supported(*format))
    }

    fn desired_fixed_foveation_level(level: u8) -> Option<XrFoveationLevelFB> {
        match level {
            0 => None,
            1 => Some(XrFoveationLevelFB::LOW),
            2 => Some(XrFoveationLevelFB::MEDIUM),
            _ => Some(XrFoveationLevelFB::HIGH),
        }
    }

    fn try_enable_fixed_foveation(
        xr: &LibOpenXr,
        session: XrSession,
        swapchain: XrSwapchain,
        level: XrFoveationLevelFB,
    ) -> Result<(), String> {
        let create_profile = xr.xrCreateFoveationProfileFB.ok_or_else(|| {
            "xrCreateFoveationProfileFB unavailable even though fixed foveation was requested"
                .to_string()
        })?;
        let update_swapchain = xr.xrUpdateSwapchainFB.ok_or_else(|| {
            "xrUpdateSwapchainFB unavailable even though fixed foveation was requested".to_string()
        })?;
        let destroy_profile = xr.xrDestroyFoveationProfileFB.ok_or_else(|| {
            "xrDestroyFoveationProfileFB unavailable even though fixed foveation was requested"
                .to_string()
        })?;

        let level_info = XrFoveationLevelProfileCreateInfoFB {
            level,
            verticalOffset: 0.0,
            dynamic: XrFoveationDynamicFB::DISABLED,
            ..Default::default()
        };
        let profile_info = XrFoveationProfileCreateInfoFB {
            next: &level_info as *const _ as *const _,
            ..Default::default()
        };
        let mut profile = XrFoveationProfileFB(0);
        unsafe { (create_profile)(session, &profile_info, &mut profile) }
            .to_result("xrCreateFoveationProfileFB")?;

        let state = XrSwapchainStateFoveationFB {
            profile,
            ..Default::default()
        };
        let update_result = unsafe {
            (update_swapchain)(
                swapchain,
                &state as *const _ as *const XrSwapchainStateBaseHeaderFB,
            )
        }
        .to_result("xrUpdateSwapchainFB");
        unsafe { (destroy_profile)(profile) }.log_error("xrDestroyFoveationProfileFB");
        update_result
    }

    fn create_vulkan_projection_layer_resources(
        xr: &LibOpenXr,
        session: XrSession,
        vulkan: &mut CxVulkan,
        options: CxOpenXrOptions,
        width: u32,
        height: u32,
        depth_images: &[XrSwapchainImageVulkanKHR],
        depth_width: u32,
        depth_height: u32,
    ) -> Result<
        (
            XrSwapchain,
            Vec<XrSwapchainImageVulkanKHR>,
            CxVulkanOpenXrSessionData,
        ),
        String,
    > {
        let swapchain_formats = xr_array_fetch(0i64, |cap, len, buf| {
            unsafe { (xr.xrEnumerateSwapchainFormats)(session, cap, len, buf) }
                .to_result("xrEnumerateSwapchainFormats")
        })?;
        let preferred_color_format = vulkan.swapchain_format();
        let color_format =
            Self::pick_vulkan_color_format(&swapchain_formats, preferred_color_format)
                .ok_or_else(|| {
                    format!(
                        "OpenXR Vulkan found no supported 32-bit RGBA swapchain format; preferred={preferred_color_format:?} runtime={swapchain_formats:?}"
                    )
                })?;
        let color_format_raw = i64::from(color_format.as_raw());
        if !swapchain_formats.contains(&color_format_raw) {
            return Err(format!(
                "OpenXR Vulkan swapchain format {:?} not supported by runtime: {:?}",
                color_format, swapchain_formats
            ));
        }

        let desired_fixed_foveation_level =
            Self::desired_fixed_foveation_level(options.fixed_foveation_level);
        let can_use_fixed_foveation = desired_fixed_foveation_level.is_some()
            && vulkan.supports_openxr_fixed_foveation()
            && xr.xrCreateFoveationProfileFB.is_some()
            && xr.xrUpdateSwapchainFB.is_some()
            && xr.xrDestroyFoveationProfileFB.is_some();

        let mut swapchain_foveation_info = XrSwapchainCreateInfoFoveationFB {
            flags: XrSwapchainCreateFoveationFlagsFB::FRAGMENT_DENSITY_MAP_BIT_FB,
            ..Default::default()
        };
        let mut swap_chain_create_info = XrSwapchainCreateInfo {
            usage_flags: XrSwapchainUsageFlags::SAMPLED | XrSwapchainUsageFlags::COLOR_ATTACHMENT,
            format: color_format_raw,
            width,
            height,
            sample_count: 1,
            face_count: 1,
            array_size: 2,
            mip_count: 1,
            ..Default::default()
        };
        if can_use_fixed_foveation {
            swap_chain_create_info.next = &mut swapchain_foveation_info as *mut _ as *const _;
        }
        let mut color_swap_chain = XrSwapchain(0);
        unsafe { (xr.xrCreateSwapchain)(session, &swap_chain_create_info, &mut color_swap_chain) }
            .to_result("xrCreateSwapchain")?;

        let mut fixed_foveation_enabled = false;
        if let Some(level) = desired_fixed_foveation_level {
            if can_use_fixed_foveation {
                if let Err(err) =
                    Self::try_enable_fixed_foveation(xr, session, color_swap_chain, level)
                {
                    crate::warning!(
                        "OpenXR Vulkan: failed to enable fixed foveation, continuing without it: {}",
                        err
                    );
                } else {
                    fixed_foveation_enabled = true;
                }
            } else {
                crate::warning!(
                    "OpenXR Vulkan: fixed foveation requested but unavailable on this runtime/device"
                );
            }
        }

        let mut foveation_images = Vec::new();
        let color_images = if fixed_foveation_enabled {
            let image_count = unsafe {
                let mut count = 0;
                (xr.xrEnumerateSwapchainImages)(color_swap_chain, 0, &mut count, ptr::null_mut())
                    .to_result("xrEnumerateSwapchainImages")?;
                count
            };
            let mut color_images = vec![XrSwapchainImageVulkanKHR::default(); image_count as usize];
            let mut foveation_chain =
                vec![XrSwapchainImageFoveationVulkanFB::default(); image_count as usize];
            for (color, foveation) in color_images.iter_mut().zip(foveation_chain.iter_mut()) {
                color.next = foveation as *mut _ as *mut _;
            }
            let mut enumerated = 0;
            unsafe {
                (xr.xrEnumerateSwapchainImages)(
                    color_swap_chain,
                    image_count,
                    &mut enumerated,
                    color_images.as_mut_ptr() as *mut std::ffi::c_void,
                )
            }
            .to_result("xrEnumerateSwapchainImages")?;
            foveation_images.extend(foveation_chain.iter().map(|image| {
                CxVulkanOpenXrFoveationImageInfo {
                    image: vk::Image::from_raw(image.image),
                }
            }));
            color_images
        } else {
            xr_array_fetch(XrSwapchainImageVulkanKHR::default(), |cap, len, buf| {
                unsafe {
                    (xr.xrEnumerateSwapchainImages)(
                        color_swap_chain,
                        cap,
                        len,
                        buf as *mut std::ffi::c_void,
                    )
                }
                .to_result("xrEnumerateSwapchainImages")
            })?
        };

        let color_vk_images: Vec<vk::Image> = color_images
            .iter()
            .map(|image| vk::Image::from_raw(image.image))
            .collect();
        let depth_vk_images: Vec<vk::Image> = depth_images
            .iter()
            .map(|image| vk::Image::from_raw(image.image))
            .collect();
        let render_targets = match vulkan.create_openxr_session_data(
            &color_vk_images,
            &depth_vk_images,
            color_format,
            width,
            height,
            depth_width,
            depth_height,
            if fixed_foveation_enabled {
                Some(&foveation_images)
            } else {
                None
            },
        ) {
            Ok(render_targets) => render_targets,
            Err(err) => {
                unsafe { (xr.xrDestroySwapchain)(color_swap_chain) }
                    .log_error("xrDestroySwapchain");
                return Err(err);
            }
        };

        Ok((color_swap_chain, color_images, render_targets))
    }

    pub(super) fn create_session_vulkan(
        xr: &LibOpenXr,
        system_id: XrSystemId,
        instance: XrInstance,
        vulkan: &mut CxVulkan,
        options: CxOpenXrOptions,
    ) -> Result<CxOpenXrSession, String> {
        let mut graphics_requirements = XrGraphicsRequirementsVulkanKHR::default();
        unsafe {
            (xr.xrGetVulkanGraphicsRequirements2KHR)(
                instance,
                system_id,
                &mut graphics_requirements,
            )
        }
        .to_result("xrGetVulkanGraphicsRequirements2KHR")?;

        let vk_instance = (vulkan.instance_handle().as_raw() as usize) as *const std::ffi::c_void;
        let vk_physical_device =
            (vulkan.physical_device_handle().as_raw() as usize) as *const std::ffi::c_void;
        let vk_device = (vulkan.device_handle().as_raw() as usize) as *const std::ffi::c_void;

        let mut runtime_physical_device = ptr::null();
        let get_info = XrVulkanGraphicsDeviceGetInfoKHR {
            system_id,
            vulkan_instance: vk_instance,
            ..Default::default()
        };
        unsafe {
            (xr.xrGetVulkanGraphicsDevice2KHR)(instance, &get_info, &mut runtime_physical_device)
        }
        .to_result("xrGetVulkanGraphicsDevice2KHR")?;
        if runtime_physical_device != vk_physical_device {
            return Err(format!(
                "OpenXR Vulkan device mismatch: runtime returned {:?}, renderer uses {:?}",
                runtime_physical_device, vk_physical_device
            ));
        }

        let gfx_binding = XrGraphicsBindingVulkanKHR {
            ty: XrStructureType::GRAPHICS_BINDING_VULKAN_KHR,
            next: ptr::null(),
            instance: vk_instance,
            physical_device: vk_physical_device,
            device: vk_device,
            queue_family_index: vulkan.queue_family_index(),
            queue_index: 0,
        };
        let session_create = XrSessionCreateInfo {
            ty: XrStructureType::SESSION_CREATE_INFO,
            next: &gfx_binding as *const _ as *const _,
            create_flags: XrSessionCreateFlags(0),
            system_id,
        };
        let mut session = XrSession(0);
        unsafe { (xr.xrCreateSession)(instance, &session_create, &mut session) }
            .to_result("xrCreateSession")?;

        let (head_space, local_space, recommended_width, recommended_height, width, height) =
            Self::describe_primary_stereo_session(xr, instance, system_id, session, options)?;

        let (passthrough, passthrough_layer, depth_provider, depth_swap_chain) =
            Self::create_passthrough_and_depth(xr, session, options)?;

        let mut depth_swapchain_state = XrEnvironmentDepthSwapchainStateMETA {
            ty: XrStructureType::ENVIRONMENT_DEPTH_SWAPCHAIN_STATE_META,
            next: 0 as *mut _,
            width: 0,
            height: 0,
        };
        unsafe {
            (xr.xrGetEnvironmentDepthSwapchainStateMETA)(
                depth_swap_chain,
                &mut depth_swapchain_state,
            )
        }
        .to_result("xrGetEnvironmentDepthSwapchainStateMETA")?;

        let depth_images =
            xr_array_fetch(XrSwapchainImageVulkanKHR::default(), |cap, len, buf| {
                unsafe {
                    (xr.xrEnumerateEnvironmentDepthSwapchainImagesMETA)(
                        depth_swap_chain,
                        cap,
                        len,
                        buf as *mut std::ffi::c_void,
                    )
                }
                .to_result("xrEnumerateEnvironmentDepthSwapchainImagesMETA")
            })?;
        let (color_swap_chain, color_images, render_targets) =
            Self::create_vulkan_projection_layer_resources(
                xr,
                session,
                vulkan,
                options,
                width,
                height,
                &depth_images,
                depth_swapchain_state.width,
                depth_swapchain_state.height,
            )?;
        unsafe { (xr.xrStartEnvironmentDepthProviderMETA)(depth_provider) }
            .to_result("xrStartEnvironmentDepthProviderMETA")?;
        let inputs = CxOpenXrInputs::new_inputs(xr, session, instance)?;

        Ok(CxOpenXrSession {
            order_counter: 0,
            color_images: Vec::new(),
            depth_images: Vec::new(),
            gl_depth_textures: Vec::new(),
            gl_frame_buffers: Vec::new(),
            vulkan: Some(CxOpenXrVulkanSession {
                _color_images: color_images,
                _depth_images: depth_images,
                render_targets,
                depth_mesh_pipeline: CxOpenXrDepthMeshPipeline::new(),
                retired_projection_layers: Vec::new(),
            }),
            color_swap_chain,
            depth_swap_chain,
            depth_provider,
            passthrough,
            passthrough_layer,
            width,
            height,
            recommended_width,
            recommended_height,
            handle: session,
            head_space,
            local_space,
            active: false,
            anchor: CxOpenXrAnchor::default(),
            debug_inactive_begin_frame_logs: 0,
            depth_swap_chain_index: 0,
            frame_state: XrFrameState::default(),
            active_display_refresh_rate_hz: None,
            last_predicted_display_time: None,
            inputs,
        })
    }

    pub(super) fn resize_projection_layer_vulkan(
        &mut self,
        xr: &LibOpenXr,
        vulkan: &mut CxVulkan,
        options: CxOpenXrOptions,
    ) -> Result<(), String> {
        let new_width = ((self.recommended_width as f32) * options.buffer_scale).max(1.0) as u32;
        let new_height = ((self.recommended_height as f32) * options.buffer_scale).max(1.0) as u32;
        if new_width == self.width && new_height == self.height {
            return Ok(());
        }

        let Some(mut vulkan_session) = self.vulkan.take() else {
            return Err(
                "OpenXR Vulkan projection resize failed: session data unavailable".to_string(),
            );
        };
        let create_result = Self::create_vulkan_projection_layer_resources(
            xr,
            self.handle,
            vulkan,
            options,
            new_width,
            new_height,
            &vulkan_session._depth_images,
            vulkan_session.render_targets.depth_width,
            vulkan_session.render_targets.depth_height,
        );

        match create_result {
            Ok((new_color_swap_chain, new_color_images, new_render_targets)) => {
                let old_color_swap_chain =
                    std::mem::replace(&mut self.color_swap_chain, new_color_swap_chain);
                let old_render_targets =
                    std::mem::replace(&mut vulkan_session.render_targets, new_render_targets);
                vulkan_session._color_images = new_color_images;
                vulkan_session
                    .retired_projection_layers
                    .push(RetiredOpenXrVulkanProjectionLayer {
                        color_swap_chain: old_color_swap_chain,
                        render_targets: old_render_targets,
                    });
                self.width = new_width;
                self.height = new_height;
                self.vulkan = Some(vulkan_session);
                Ok(())
            }
            Err(err) => {
                self.vulkan = Some(vulkan_session);
                Err(err)
            }
        }
    }

    pub(super) fn destroy_session_vulkan(&mut self, xr: &LibOpenXr, vulkan: Option<&mut CxVulkan>) {
        if let Some(mut vulkan_session) = self.vulkan.take() {
            if let Some(vulkan) = vulkan {
                for retired in vulkan_session.retired_projection_layers.drain(..) {
                    vulkan.destroy_openxr_session_data(retired.render_targets);
                    unsafe { (xr.xrDestroySwapchain)(retired.color_swap_chain) }
                        .log_error("xrDestroySwapchain");
                }
                vulkan.destroy_openxr_session_data(vulkan_session.render_targets);
            } else {
                crate::error!(
                    "OpenXR destroy lost Vulkan backend before XR resources were released"
                );
            }
        }
        xr_tsdf_store().clear();
    }
}
