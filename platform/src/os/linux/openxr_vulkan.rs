use super::*;
use crate::{
    os::linux::openxr_depth::CxOpenXrDepthMeshPipeline,
    os::linux::vulkan::{CxVulkan, CxVulkanOpenXrSessionData},
    xr_depth_mesh::xr_depth_mesh_store,
};

pub(super) struct CxOpenXrVulkanSession {
    _color_images: Vec<XrSwapchainImageVulkanKHR>,
    _depth_images: Vec<XrSwapchainImageVulkanKHR>,
    render_targets: CxVulkanOpenXrSessionData,
    depth_mesh_pipeline: CxOpenXrDepthMeshPipeline,
}

impl Cx {
    pub(crate) fn openxr_draw_pass_to_vulkan(
        &mut self,
        draw_pass_id: DrawPassId,
        frame: &CxOpenXrFrame,
    ) -> Result<(), String> {
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

        for eye in 0..2 {
            let pass = &mut self.passes[draw_pass_id];
            let camera_inv = frame.eyes[eye].view_mat.invert();
            pass.set_dpi_factor(dpi_factor);
            pass.paint_dirty = true;
            pass.os.shader_variant = SHADER_VARIANT_XR;
            pass.pass_uniforms.camera_projection = frame.eyes[eye].proj_mat;
            pass.pass_uniforms.camera_view = frame.eyes[eye].view_mat;
            pass.pass_uniforms.camera_projection_r = frame.eyes[eye].proj_mat;
            pass.pass_uniforms.camera_view_r = frame.eyes[eye].view_mat;
            pass.pass_uniforms.camera_inv = camera_inv;
            pass.pass_uniforms.camera_inv_r = camera_inv;
            if depth_image_index.is_some() {
                pass.pass_uniforms.depth_projection = frame.eyes[eye].depth_proj_mat;
                pass.pass_uniforms.depth_view = frame.eyes[eye].depth_view_mat;
            } else {
                pass.pass_uniforms.depth_projection = zero_mat;
                pass.pass_uniforms.depth_view = zero_mat;
            }
            pass.pass_uniforms.depth_projection_r = pass.pass_uniforms.depth_projection;
            pass.pass_uniforms.depth_view_r = pass.pass_uniforms.depth_view;

            let mut vulkan =
                self.os.vulkan.take().ok_or_else(|| {
                    "OpenXR Vulkan render failed: backend unavailable".to_string()
                })?;
            let result = unsafe {
                vulkan.draw_openxr_view(
                    self,
                    draw_pass_id,
                    draw_list_id,
                    &*render_targets,
                    color_image_index,
                    eye,
                    depth_image_index,
                )
            };
            self.os.vulkan = Some(vulkan);
            result?;
        }

        #[cfg(target_os = "android")]
        if let Some(request) = self.take_studio_run_view_frame_request(0) {
            let mut vulkan = self
                .os
                .vulkan
                .take()
                .ok_or_else(|| "OpenXR Vulkan frame capture failed: backend unavailable".to_string())?;
            let result = unsafe { vulkan.read_openxr_color_image_rgba(&*render_targets, color_image_index, 0) };
            self.os.vulkan = Some(vulkan);
            match result {
                Ok(rgba) => {
                    let session = self
                        .os
                        .openxr
                        .session
                        .as_ref()
                        .ok_or_else(|| "OpenXR Vulkan frame capture failed: session unavailable".to_string())?;
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
        Ok(())
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

        let (head_space, local_space, width, height) =
            Self::describe_primary_stereo_session(xr, instance, system_id, session, options)?;

        let swapchain_formats = xr_array_fetch(0i64, |cap, len, buf| {
            unsafe { (xr.xrEnumerateSwapchainFormats)(session, cap, len, buf) }
                .to_result("xrEnumerateSwapchainFormats")
        })?;
        let color_format = vulkan.swapchain_format();
        let color_format_raw = i64::from(color_format.as_raw());
        if !swapchain_formats.contains(&color_format_raw) {
            return Err(format!(
                "OpenXR Vulkan swapchain format {:?} not supported by runtime: {:?}",
                color_format, swapchain_formats
            ));
        }

        let swap_chain_create_info = XrSwapchainCreateInfo {
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
        let mut color_swap_chain = XrSwapchain(0);
        unsafe { (xr.xrCreateSwapchain)(session, &swap_chain_create_info, &mut color_swap_chain) }
            .to_result("xrCreateSwapchain")?;

        let color_images =
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
            })?;

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

        let color_vk_images: Vec<vk::Image> = color_images
            .iter()
            .map(|image| vk::Image::from_raw(image.image))
            .collect();
        let depth_vk_images: Vec<vk::Image> = depth_images
            .iter()
            .map(|image| vk::Image::from_raw(image.image))
            .collect();
        let render_targets = vulkan.create_openxr_session_data(
            &color_vk_images,
            &depth_vk_images,
            color_format,
            width,
            height,
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
            }),
            color_swap_chain,
            depth_swap_chain,
            depth_provider,
            passthrough,
            passthrough_layer,
            width,
            height,
            handle: session,
            head_space,
            local_space,
            active: false,
            anchor: CxOpenXrAnchor::default(),
            depth_swap_chain_index: 0,
            frame_state: XrFrameState::default(),
            inputs,
        })
    }

    pub(super) fn destroy_session_vulkan(&mut self, vulkan: Option<&mut CxVulkan>) {
        if let Some(vulkan_session) = self.vulkan.take() {
            if let Some(vulkan) = vulkan {
                vulkan.destroy_openxr_session_data(vulkan_session.render_targets);
            } else {
                crate::error!(
                    "OpenXR destroy lost Vulkan backend before XR resources were released"
                );
            }
        }
        xr_depth_mesh_store().clear();
    }
}
