use super::*;

impl Cx {
    pub(crate) fn openxr_draw_pass_to_multiview(
        &mut self,
        draw_pass_id: DrawPassId,
        frame: &CxOpenXrFrame,
    ) {
        let draw_list_id = self.passes[draw_pass_id].main_draw_list_id.unwrap();
        {
            let session = self.os.openxr.session.as_ref().unwrap();
            let gl = &self.os.display.as_ref().unwrap().libgl;
            let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
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

            pass.pass_uniforms.depth_projection = frame.eyes[0].depth_proj_mat;
            pass.pass_uniforms.depth_view = frame.eyes[0].depth_view_mat;
            pass.pass_uniforms.depth_projection_r = frame.eyes[1].depth_proj_mat;
            pass.pass_uniforms.depth_view_r = frame.eyes[1].depth_view_mat;

            pass.os
                .pass_uniforms
                .update_uniform_buffer(self.os.gl(), pass.pass_uniforms.as_slice());

            let gl_frame_buffer = session.gl_frame_buffers[frame.swap_chain_index as usize];

            unsafe {
                (gl.glBindFramebuffer)(gl_sys::DRAW_FRAMEBUFFER, gl_frame_buffer);
                (gl.glColorMask)(gl_sys::TRUE, gl_sys::TRUE, gl_sys::TRUE, gl_sys::TRUE);
                (gl.glDepthMask)(gl_sys::TRUE);
                (gl.glEnable)(gl_sys::SCISSOR_TEST);
                (gl.glEnable)(gl_sys::DEPTH_TEST);
                (gl.glDepthFunc)(gl_sys::LEQUAL);
                (gl.glDisable)(gl_sys::CULL_FACE);
                (gl.glBlendEquationSeparate)(gl_sys::FUNC_ADD, gl_sys::FUNC_ADD);
                (gl.glBlendFuncSeparate)(
                    gl_sys::ONE,
                    gl_sys::ONE_MINUS_SRC_ALPHA,
                    gl_sys::ONE,
                    gl_sys::ONE_MINUS_SRC_ALPHA,
                );
                (gl.glEnable)(gl_sys::BLEND);
                (gl.glDisable)(gl_sys::FRAMEBUFFER_SRGB_EXT);
                (gl.glViewport)(0, 0, session.width as i32, session.height as i32);
                (gl.glScissor)(0, 0, session.width as i32, session.height as i32);

                (gl.glClearDepthf)(1.0);
                (gl.glClearColor)(0.0, 0.0, 0.0, 0.0);
                (gl.glClear)(gl_sys::COLOR_BUFFER_BIT | gl_sys::DEPTH_BUFFER_BIT);
                crate::gl_log_error!(gl);
            }
        }

        let mut zbias = 0.0;
        let zbias_step = self.passes[draw_pass_id].zbias_step;
        self.render_view(draw_pass_id, draw_list_id, &mut zbias, zbias_step);

        #[cfg(target_os = "android")]
        if let Some(request) = self.take_studio_run_view_frame_request(0) {
            let session = self.os.openxr.session.as_ref().unwrap();
            let gl = &self.os.display.as_ref().unwrap().libgl;
            let w = session.width.max(1);
            let h = session.height.max(1);
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            unsafe {
                (gl.glReadPixels)(
                    0,
                    0,
                    w as i32,
                    h as i32,
                    gl_sys::RGBA,
                    gl_sys::UNSIGNED_BYTE,
                    pixels.as_mut_ptr() as *mut _,
                );
            }
            let stride = (w * 4) as usize;
            for y in 0..(h as usize / 2) {
                let top = y * stride;
                let bot = ((h as usize) - 1 - y) * stride;
                for x in 0..stride {
                    pixels.swap(top + x, bot + x);
                }
            }
            self.encode_studio_run_view_frame_async(request, w, h, pixels);
        }

        let gl = &self.os.display.as_ref().unwrap().libgl;
        unsafe {
            (gl.glBindFramebuffer)(gl_sys::DRAW_FRAMEBUFFER, 0);
            crate::gl_log_error!(gl);
        }
    }
}

impl CxOpenXr {
    pub(crate) fn depth_texture_hook(
        &self,
        gl: &LibGl,
        shgl: &GlShader,
        mapping: &CxDrawShaderMapping,
    ) {
        if let Some(session) = &self.session {
            if session.depth_images.is_empty() {
                return;
            }
            let i = mapping.textures.len();
            if let Some(loc) = shgl.xr_depth_texture.loc {
                unsafe {
                    let di = &session.depth_images[session.depth_swap_chain_index];
                    (gl.glActiveTexture)(gl_sys::TEXTURE0 + i as u32);
                    (gl.glBindTexture)(gl_sys::TEXTURE_2D_ARRAY, di.image);
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D_ARRAY,
                        gl_sys::TEXTURE_WRAP_S,
                        gl_sys::CLAMP_TO_EDGE as _,
                    );
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D_ARRAY,
                        gl_sys::TEXTURE_WRAP_T,
                        gl_sys::CLAMP_TO_EDGE as _,
                    );
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D_ARRAY,
                        gl_sys::TEXTURE_MIN_FILTER,
                        gl_sys::NEAREST as _,
                    );
                    (gl.glTexParameteri)(
                        gl_sys::TEXTURE_2D_ARRAY,
                        gl_sys::TEXTURE_MAG_FILTER,
                        gl_sys::NEAREST as _,
                    );
                    (gl.glUniform1i)(loc, i as i32);
                }
            }
        }
    }
}

impl CxOpenXrSession {
    pub(super) fn create_session_gles(
        xr: &LibOpenXr,
        system_id: XrSystemId,
        instance: XrInstance,
        display: &CxAndroidDisplay,
        options: CxOpenXrOptions,
    ) -> Result<CxOpenXrSession, String> {
        let gfx_binding = XrGraphicsBindingOpenGLESAndroidKHR {
            ty: XrStructureType::GRAPHICS_BINDING_OPENGL_ES_ANDROID_KHR,
            next: 0 as *const _,
            display: display.egl_display,
            config: display.egl_config,
            context: display.egl_context,
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

        let swap_chain_create_info = XrSwapchainCreateInfo {
            usage_flags: XrSwapchainUsageFlags::SAMPLED | XrSwapchainUsageFlags::COLOR_ATTACHMENT,
            format: gl_sys::SRGB8_ALPHA8L as i64,
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
            xr_array_fetch(XrSwapchainImageOpenGLESKHR::default(), |cap, len, buf| {
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

        let depth_images =
            xr_array_fetch(XrSwapchainImageOpenGLESKHR::default(), |cap, len, buf| {
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

        let swap_chain_len = color_images.len();
        let mut gl_depth_textures = vec![0; swap_chain_len];
        let mut gl_frame_buffers = vec![0; swap_chain_len];
        let gl = &display.libgl;
        for i in 0..swap_chain_len {
            let color_texture = color_images[i].image;
            unsafe {
                (gl.glBindTexture)(gl_sys::TEXTURE_2D_ARRAY, color_texture);
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D_ARRAY,
                    gl_sys::TEXTURE_WRAP_S,
                    gl_sys::CLAMP_TO_BORDER as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D_ARRAY,
                    gl_sys::TEXTURE_WRAP_S,
                    gl_sys::CLAMP_TO_BORDER as i32,
                );
                let border_color = [0f32; 4];
                (gl.glTexParameterfv)(
                    gl_sys::TEXTURE_2D_ARRAY,
                    gl_sys::TEXTURE_BORDER_COLOR,
                    border_color.as_ptr() as *const _,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D_ARRAY,
                    gl_sys::TEXTURE_MIN_FILTER,
                    gl_sys::LINEAR as i32,
                );
                (gl.glTexParameteri)(
                    gl_sys::TEXTURE_2D_ARRAY,
                    gl_sys::TEXTURE_MAG_FILTER,
                    gl_sys::LINEAR as i32,
                );
                (gl.glBindTexture)(gl_sys::TEXTURE_2D_ARRAY, 0);

                (gl.glGenTextures)(1, &mut gl_depth_textures[i]);
                (gl.glBindTexture)(gl_sys::TEXTURE_2D_ARRAY, gl_depth_textures[i]);
                (gl.glTexStorage3D)(
                    gl_sys::TEXTURE_2D_ARRAY,
                    1,
                    gl_sys::DEPTH_COMPONENT24,
                    width as i32,
                    height as i32,
                    2,
                );

                (gl.glGenFramebuffers)(1, &mut gl_frame_buffers[i]);
                (gl.glBindFramebuffer)(gl_sys::DRAW_FRAMEBUFFER, gl_frame_buffers[i]);

                if options.multisamples > 1 {
                    (gl.glFramebufferTextureMultisampleMultiviewOVR.unwrap())(
                        gl_sys::DRAW_FRAMEBUFFER,
                        gl_sys::DEPTH_ATTACHMENT,
                        gl_depth_textures[i],
                        0,
                        options.multisamples as _,
                        0,
                        2,
                    );
                    (gl.glFramebufferTextureMultisampleMultiviewOVR.unwrap())(
                        gl_sys::DRAW_FRAMEBUFFER,
                        gl_sys::COLOR_ATTACHMENT0,
                        color_texture,
                        0,
                        options.multisamples as _,
                        0,
                        2,
                    );
                } else {
                    (gl.glFramebufferTextureMultiviewOVR.unwrap())(
                        gl_sys::DRAW_FRAMEBUFFER,
                        gl_sys::DEPTH_ATTACHMENT,
                        gl_depth_textures[i],
                        0,
                        0,
                        2,
                    );
                    (gl.glFramebufferTextureMultiviewOVR.unwrap())(
                        gl_sys::DRAW_FRAMEBUFFER,
                        gl_sys::COLOR_ATTACHMENT0,
                        color_texture,
                        0,
                        0,
                        2,
                    );
                }
                (gl.glBindFramebuffer)(gl_sys::DRAW_FRAMEBUFFER, 0);
            }
        }

        unsafe { (xr.xrStartEnvironmentDepthProviderMETA)(depth_provider) }
            .to_result("xrStartEnvironmentDepthProviderMETA")?;
        let inputs = CxOpenXrInputs::new_inputs(xr, session, instance)?;

        Ok(CxOpenXrSession {
            order_counter: 0,
            color_images,
            depth_images,
            gl_depth_textures,
            gl_frame_buffers,
            #[cfg(use_vulkan)]
            vulkan: None,
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

    pub(super) fn destroy_session_gles(&self, gl: &LibGl) {
        if !self.color_images.is_empty() {
            for i in 0..self.color_images.len() {
                unsafe {
                    (gl.glDeleteTextures)(1, &self.gl_depth_textures[i]);
                    (gl.glDeleteFramebuffers)(1, &self.gl_frame_buffers[i]);
                }
            }
        }
    }
}
