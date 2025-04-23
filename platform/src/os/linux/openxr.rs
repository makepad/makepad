use{
    crate::{
        os::{
            linux::{
                openxr_sys::*,
                android::android::CxAndroidDisplay,
                opengl::GlShader,
                gl_sys,
                gl_sys::LibGl,
                android::android_jni::*
            }
        },
        draw_shader::CxDrawShaderMapping,
        event::Event,
        event::xr::*,
        pass::{CxPassParent,PassId},
        cx::{Cx},
        makepad_math::Mat4,
    },
    std::rc::Rc,
    std::sync::{mpsc},
    std::ptr,
};
use makepad_jni_sys as jni_sys;


impl Cx{
    pub(crate) fn openxr_render_loop(&mut self, from_java_rx: &mpsc::Receiver<FromJavaMessage>)->bool{
        if self.os.openxr.session.is_some(){
            loop{
                match from_java_rx.try_recv() {
                    Ok(FromJavaMessage::RenderLoop) => {}, // ignore this one
                    Ok(message) => {
                        self.handle_message(message);
                    },
                    Err(_) => {
                        break;
                    }
                }
            }
            self.openxr_handle_events();
            self.handle_other_events();
            self.openxr_handle_drawing();
            return true
        }
        false
    }
            
    pub(crate) fn openxr_handle_events(&mut self){
        let openxr = &mut self.os.openxr;
        loop{
            let mut event_buffer = XrEventDataBuffer{
                ty:XrStructureType::EVENT_DATA_BUFFER,
                next: 0 as *const _,
                varying: [0;4000]
            };
            if unsafe{(openxr.libxr.as_ref().unwrap().xrPollEvent)(openxr.instance.unwrap(), &mut event_buffer)} != XrResult::SUCCESS{
                break;
            }
            if event_buffer.ty == XrStructureType::EVENT_DATA_SESSION_STATE_CHANGED{
                let edssc = unsafe{*(&event_buffer as *const _ as *const XrEventDataSessionStateChanged)};
                match edssc.state{
                    XrSessionState::IDLE=>{
                    }
                    XrSessionState::FOCUSED=>{
                    }
                    XrSessionState::VISIBLE=>{}
                    XrSessionState::READY=>{
                        openxr.session.as_mut().unwrap().begin_session(
                            &openxr.libxr.as_ref().unwrap(),
                            self.os.activity_thread_id.unwrap(), 
                            self.os.render_thread_id.unwrap()
                        );
                    }
                    XrSessionState::STOPPING=>{
                        openxr.session.as_mut().unwrap().end_session(openxr.libxr.as_ref().unwrap());
                    }
                    XrSessionState::EXITING=>{}
                    _=>()
                }
            }
            /*      
            //crate::log!("{:?}", event_buffer.ty);
            match event_buffer.ty{
                XrStructureType::EVENT_DATA_EVENTS_LOST=>{}
                XrStructureType::EVENT_DATA_INSTANCE_LOSS_PENDING=>{}
                XrStructureType::EVENT_DATA_INTERACTION_PROFILE_CHANGED=>{}
                XrStructureType::EVENT_DATA_PERF_SETTINGS_EXT=>{}
                XrStructureType::EVENT_DATA_REFERENCE_SPACE_CHANGE_PENDING=>{}
                XrStructureType::EVENT_DATA_SESSION_STATE_CHANGED=>{}
                x=>{
                    crate::log!("Unkown xr event {:?}",x);
                }
            }*/
        }
    }
        
    pub(crate) fn openxr_draw_pass_to_multiview(&mut self, pass_id:PassId, frame:&CxOpenXrFrame){
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        let session =  &self.os.openxr.session.as_ref().unwrap();
        let gl = &self.os.display.as_ref().unwrap().libgl;
                
        // alright lets set up the pass matrices
        let dpi_factor = self.passes[pass_id].dpi_factor.unwrap();
        let pass = &mut self.passes[pass_id];
                
        pass.set_dpi_factor(dpi_factor);
        pass.paint_dirty = true;
        pass.os.shader_variant = 1;
                
        // lets set up the right matrices and bind the right framebuffers
        pass.pass_uniforms.camera_projection = frame.eyes[0].proj_mat;
        pass.pass_uniforms.camera_view = frame.eyes[0].view_mat;
        pass.pass_uniforms.camera_projection_r = frame.eyes[1].proj_mat;
        pass.pass_uniforms.camera_view_r = frame.eyes[1].view_mat;
        
        pass.pass_uniforms.depth_projection = frame.eyes[0].depth_proj_mat;
        pass.pass_uniforms.depth_view = frame.eyes[0].depth_view_mat;
        pass.pass_uniforms.depth_projection_r = frame.eyes[1].depth_proj_mat;
        pass.pass_uniforms.depth_view_r = frame.eyes[1].depth_view_mat;
                
                        
        pass.os.pass_uniforms.update_uniform_buffer(self.os.gl(), pass.pass_uniforms.as_slice());
                
        // lets bind the framebuffers
        let gl_frame_buffer = session.gl_frame_buffers[frame.swap_chain_index as usize];
                
        unsafe{
            (gl.glBindFramebuffer)(gl_sys::DRAW_FRAMEBUFFER, gl_frame_buffer);
            (gl.glColorMask)(gl_sys::TRUE, gl_sys::TRUE, gl_sys::TRUE, gl_sys::TRUE);
            (gl.glDepthMask)(gl_sys::TRUE);
            (gl.glEnable)(gl_sys::SCISSOR_TEST);
            (gl.glEnable)(gl_sys::DEPTH_TEST);
            (gl.glDepthFunc)(gl_sys::LEQUAL);
            (gl.glDisable)(gl_sys::CULL_FACE);
            (gl.glBlendEquationSeparate)(gl_sys::FUNC_ADD, gl_sys::FUNC_ADD);
            (gl.glBlendFuncSeparate)(gl_sys::ONE, gl_sys::ONE_MINUS_SRC_ALPHA, gl_sys::ONE, gl_sys::ONE_MINUS_SRC_ALPHA);
            (gl.glEnable)(gl_sys::BLEND);
            (gl.glDisable)(gl_sys::FRAMEBUFFER_SRGB_EXT);
            //crate::log!("{:?} {:?}", session.width, session.height);
            (gl.glViewport)(0, 0, session.width as i32, session.height  as i32);
            (gl.glScissor)(0, 0, session.width as i32, session.height as i32);
                        
            (gl.glClearDepthf)(1.0);
            (gl.glClearColor)(0.0, 0.0, 0.0, 0.0);
            (gl.glClear)(gl_sys::COLOR_BUFFER_BIT | gl_sys::DEPTH_BUFFER_BIT);
            crate::gl_log_error!(gl);
        }
                
        //let panning_offset = if self.os.keyboard_visible {self.os.keyboard_panning_offset} else {0};
                
        let mut zbias = 0.0;
        let zbias_step = -0.1;//self.passes[pass_id].zbias_step;
        self.render_view(
            pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
        );
                
        let gl = &self.os.display.as_ref().unwrap().libgl;
        unsafe{
            (gl.glBindFramebuffer)(gl_sys::DRAW_FRAMEBUFFER, 0);
            crate::gl_log_error!(gl);
            // let da = [gl_sys::DEPTH_ATTACHMENT];
            //  (gl.glInvalidateFramebuffer)(gl_sys::DRAW_FRAMEBUFFER, 1, da.as_ptr() as _);
            //crate::gl_log_error!(gl);
            //glInvalidateFramebuffer(GL_DRAW_FRAMEBUFFER, 1, depthAttachment);
                                    
        }
                 
                
                
        // Discard the depth buffer, so the tiler won't need to write it back out to memory.
    }
        
    pub (crate) fn openxr_handle_repaint(&mut self, frame:&CxOpenXrFrame) {
        //opengl_cx.make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            self.passes[*pass_id].set_time(self.os.timers.time_now() as f32);
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Xr=>{
                    self.openxr_draw_pass_to_multiview(*pass_id, frame);
                }
                CxPassParent::Window(_) => {
                    // this cant exist..
                }
                CxPassParent::Pass(_) => {
                    self.draw_pass_to_texture(*pass_id, None);
                },
                CxPassParent::None => {
                    self.draw_pass_to_texture(*pass_id, None);
                }
            }
        }
                
                
    }
        
    pub(crate) fn openxr_handle_drawing(&mut self){
        let openxr = &mut self.os.openxr;
        if let Ok(frame) = CxOpenXrFrame::begin_frame(
            openxr.libxr.as_ref().unwrap(),
            openxr.session.as_ref().unwrap()
        ){
            // lets send in the state
            let event = openxr.session.as_mut().unwrap().new_xr_update_event(
                openxr.libxr.as_ref().unwrap(),
                &frame
            );
            let last_state = openxr.session.as_ref().unwrap().inputs.last_state.clone();
            if let Some(event) = event{
                self.call_event_handler(&Event::XrUpdate(event));
            }
                            
            if !self.new_next_frames.is_empty() {
                self.call_next_frame_event(self.os.timers.time_now());
            }
            if self.need_redrawing() {
                self.new_draw_event.xr_state = Some(last_state);
                self.call_draw_event();
                self.opengl_compile_shaders();
            }
            
            // copy on the depth image
            self.os.openxr.session.as_mut().unwrap().depth_swap_chain_index = frame.depth_image.map(|v| v.swapchain_index as usize).unwrap_or(0);
            
            self.openxr_handle_repaint(&frame);
            
            let openxr = &mut self.os.openxr;
            frame.end_frame(
                openxr.libxr.as_ref().unwrap(),
                openxr.session.as_mut().unwrap()
            );
        }
    }
}


#[derive(Default)]
pub struct CxOpenXr{
    loader: Option<LibOpenXrLoader>,
    libxr: Option<LibOpenXr>,
    instance: Option<XrInstance>,
    system_id: Option<XrSystemId>,
    pub session: Option<CxOpenXrSession>
}

impl CxOpenXr{
        
    pub(crate) fn depth_texture_hook(&self, gl:&LibGl, shgl:&GlShader, mapping:&CxDrawShaderMapping){
        // lets set the texture
        if let Some(session) = &self.session{
            let i = mapping.textures.len();
            if let Some(loc) = shgl.xr_depth_texture.loc{
                unsafe{
                    let di = &session.depth_images[session.depth_swap_chain_index];
                    (gl.glActiveTexture)(gl_sys::TEXTURE0 + i as u32);
                    (gl.glBindTexture)(gl_sys::TEXTURE_2D_ARRAY, di.image); 
                    (gl.glTexParameteri)(gl_sys::TEXTURE_2D_ARRAY, gl_sys::TEXTURE_WRAP_S, gl_sys::CLAMP_TO_EDGE as _); 
                    (gl.glTexParameteri)(gl_sys::TEXTURE_2D_ARRAY, gl_sys::TEXTURE_WRAP_T, gl_sys::CLAMP_TO_EDGE as _); 
                    (gl.glTexParameteri)(gl_sys::TEXTURE_2D_ARRAY, gl_sys::TEXTURE_MIN_FILTER, gl_sys::NEAREST as _); 
                    (gl.glTexParameteri)(gl_sys::TEXTURE_2D_ARRAY, gl_sys::TEXTURE_MAG_FILTER, gl_sys::NEAREST as _); 
                    (gl.glUniform1i)(loc, i as i32);
                }
            }
        }
    }
    
    pub fn create_instance(&mut self, activity_handle: jni_sys::jobject)->Result<(),String>{
        self.loader = Some(LibOpenXrLoader::try_load()?);
                
        // lets load em up!
        let loader = &self.loader.as_ref().unwrap();
        let loader_info = XrLoaderInitInfoAndroidKHR {
            ty: XrStructureType::LOADER_INIT_INFO_ANDROID_KHR,
            next: ptr::null(),
            application_vm: makepad_android_state::get_java_vm() as *mut _,
            application_context: activity_handle as *mut _,
        };
                
        unsafe{(loader.xrInitializeLoaderKHR)(&loader_info as *const _ as _,)}.to_result("xrInitializeLoaderKHR")?;
                
        let exts = xr_array_fetch(XrExtensionProperties::default(), |cap, len, buf|{
            unsafe{(loader.xrEnumerateInstanceExtensionProperties)(
                ptr::null(),
                cap,
                len, 
                buf
            )}.to_result("xrEnumerateInstanceExtensionProperties")
        })?;
                
        let exts_needed = [
            "XR_KHR_opengl_es_enable\0",
            "XR_EXT_performance_settings\0",
            "XR_KHR_android_thread_settings\0",
            "XR_FB_passthrough\0",
            "XR_META_environment_depth\0",
            "XR_META_touch_controller_plus\0",
            "XR_EXT_hand_interaction\0",
            "XR_META_detached_controllers\0",
            "XR_META_simultaneous_hands_and_controllers\0",
            "XR_EXT_hand_tracking\0",
            "XR_FB_hand_tracking_mesh\0",
            "XR_FB_hand_tracking_aim\0",
        ];
                
        for ext_needed in exts_needed{
            if !exts.iter().any(|e|{
                xr_string_zero_terminated(&e.extension_name) == ext_needed
            }){
                crate::log!("Needed extension {} not found", ext_needed);
            }
        }
                
        let create_info = XrInstanceCreateInfo{
            ty: XrStructureType::INSTANCE_CREATE_INFO,
            next: 0 as *const _,
            create_flags: XrInstanceCreateFlags(0),
            application_info: XrApplicationInfo{
                application_name: xr_to_string("makepad_example_simple"),
                application_version: 0,
                engine_name: xr_to_string("Makepad"),
                engine_version: 0,
                api_version: XP_API_VERSION_1_0,
            },
            enabled_api_layer_count: 0,
            enabled_api_layer_names: 0 as *const *const _,
            enabled_extension_count: exts_needed.len() as u32,
            enabled_extension_names: &xr_static_str_array(&exts_needed) as *const *const _
        };
        let mut instance = XrInstance(0);
        unsafe{(loader.xrCreateInstance)(&create_info, &mut instance)}.to_result("xrCreateInstance")?;
        self.instance = Some(instance);
                
        self.libxr = Some(LibOpenXr::try_load(loader, instance)?);
        let xr = &self.libxr.as_ref().unwrap();
                
        let mut instance_props = XrInstanceProperties::default();
        unsafe{(xr.xrGetInstanceProperties)(instance, &mut instance_props)}.to_result("xrGetInstanceProperties")?;
        /*
        crate::log!(
            "OpenXR Runtime loaded: {}, Version: {}.{}.{}", 
            xr_string(&instance_props.runtime_name),
            instance_props.runtime_version.major(),        
            instance_props.runtime_version.minor(),        
            instance_props.runtime_version.patch(),        
        );*/
                
        let mut sys_info = XrSystemGetInfo::default();
        sys_info.form_factor = XrFormFactor::HEAD_MOUNTED_DISPLAY;
                
        let mut sys_id = XrSystemId(0);
        unsafe{(xr.xrGetSystem)(instance, &mut sys_info, &mut sys_id)}.to_result("xrGetSystem")?;
        self.system_id = Some(sys_id);
            
        let mut sys_props = XrSystemProperties::default();
        unsafe{(xr.xrGetSystemProperties)(instance, sys_id, &mut sys_props)}.to_result("xrGetSystemProperties")?;
                
        /*crate::log!("OpenXR System props name:{} vendor_id:{}",xr_string(&sys_props.system_name), sys_props.vendor_id);*/
        /*
        crate::log!(
            "OpenXR System graphics max_width:{}, max_height:{}, max_layer:{}",
            sys_props.graphics_properties.max_swapchain_image_height,
            sys_props.graphics_properties.max_swapchain_image_width,
            sys_props.graphics_properties.max_layer_count
        );
        crate::log!(
            "OpenXR System tracking Orientation:{}, Position:{}",
            sys_props.tracking_properties.orientation_tracking.to_bool(),
            sys_props.tracking_properties.position_tracking.to_bool(),
        );*/
                
        let mut ogles_req = XrGraphicsRequirementsOpenGLESKHR::default();
        unsafe{(xr.xrGetOpenGLESGraphicsRequirementsKHR)(instance, sys_id, &mut ogles_req)}.to_result("xrGetOpenGLESGraphicsRequirementsKHR")?;
                
        // alright its apparently time to create the EGL context
                
        /*
        // lets enumerate api layers
        let (_result, layers) = xr_array_fetch(XrApiLayerProperties::default(), |cap, len, buf|{
            unsafe{(loader.xrEnumerateApiLayerProperties)(
                cap,
                len, 
                buf
            )};
        }); 
        for layer in layers{
            crate::log!("layer: {}", std::str::from_utf8(&layer.layer_name).unwrap());
        }*/
                
        // lets try load the lib
                
        Ok(())
    }
        
    pub fn create_session(&mut self, display:&CxAndroidDisplay, options:CxOpenXrOptions)->Result<(), String>{
        self.session = Some(CxOpenXrSession::create_session(
            self.libxr.as_ref().unwrap(),
            self.system_id.unwrap(),
            self.instance.unwrap(),
            display,
            options
        )?);
        Ok(())
    }
        
    pub fn destroy_instance(&mut self, libgl:&LibGl)->Result<(), String>{
        if let Some(session) = self.session.take(){
            if let Err(e) = session.destroy_session(
                self.libxr.as_ref().unwrap(),
                libgl
            ){
                crate::log!("OpenXR destroy destroy_session error: {e}")
            }
        }
                
        let xr =  self.libxr.as_ref().ok_or("")?;
        let instance = self.instance.take().ok_or("")?;
        let _system_id = self.system_id.take().ok_or("")?;
        unsafe{(xr.xrDestroyInstance)(instance)}
        .log_error("xrDestroyInstance");
        Ok(())
    }
    
}


pub struct CxOpenXrSession{
    pub depth_swap_chain_index: usize,
    pub color_images: Vec<XrSwapchainImageOpenGLESKHR>,
    pub depth_images: Vec<XrSwapchainImageOpenGLESKHR>,
    pub gl_depth_textures: Vec<u32>,
    pub gl_frame_buffers: Vec<u32>,
    pub color_swap_chain: XrSwapchain,
    pub depth_swap_chain: XrEnvironmentDepthSwapchainMETA,
    pub depth_provider: XrEnvironmentDepthProviderMETA,
    pub passthrough: XrPassthroughFB,
    pub passthrough_layer: XrPassthroughLayerFB,
    pub width: u32,
    pub height: u32,
    pub head_space: XrSpace,
    pub local_space: XrSpace,
    pub handle: XrSession,
    pub active: bool,
    pub inputs: CxOpenXrInputs,
}

impl CxOpenXrSession{
            
    pub fn create_session(
        xr: &LibOpenXr, 
        system_id: XrSystemId,
        instance: XrInstance,
        display:&CxAndroidDisplay, options:CxOpenXrOptions
    )->Result<CxOpenXrSession,String>{
                        
        let gfx_binding = XrGraphicsBindingOpenGLESAndroidKHR{
            ty: XrStructureType::GRAPHICS_BINDING_OPENGL_ES_ANDROID_KHR,
            next: 0 as *const _,
            display: display.egl_display,
            config: display.egl_config,
            context: display.egl_context,
        };
        let session_create = XrSessionCreateInfo{
            ty: XrStructureType::SESSION_CREATE_INFO,
            next: &gfx_binding as *const _ as *const _,
            create_flags: XrSessionCreateFlags(0),
            system_id
        };
                        
        let mut session = XrSession(0);
                        
        unsafe{(xr.xrCreateSession)(instance, &session_create, &mut session)}.to_result("xrCreateSession")?;
                        
        let configs = xr_array_fetch(XrViewConfigurationType::default(), |cap, len, buf|{
            unsafe{(xr.xrEnumerateViewConfigurations)(
                instance,
                system_id,
                cap,
                len, 
                buf
            )}.to_result("xrEnumerateViewConfigurations")
        })?;
                        
        if !configs.iter().any(|v| *v == XrViewConfigurationType::PRIMARY_STEREO){
            return Err(format!("Could not find PRIMARY STEREO viewconfiguration"));
        }
                        
        let mut config_props = XrViewConfigurationProperties::default();
                        
        unsafe{(xr.xrGetViewConfigurationProperties)(instance, system_id, XrViewConfigurationType::PRIMARY_STEREO, &mut  config_props)}.to_result("xrGetViewConfigurationProperties")?;
        /*
        crate::log!(
            "OpenXR System Config type: {:?}, FovMutable:{}",
            config_props.view_configuration_type,
            config_props.fov_mutable.to_bool(),
        );*/
                        
        let config_views = xr_array_fetch(XrViewConfigurationView::default(), |cap, len, buf|{
            unsafe{(xr.xrEnumerateViewConfigurationViews)(
                instance,
                system_id,
                XrViewConfigurationType::PRIMARY_STEREO,
                cap,
                len, 
                buf
            )}.to_result("xrEnumerateViewConfigurationViews")
        })?;
                        
        let mut head_space = XrSpace(0);
        let head_space_info = XrReferenceSpaceCreateInfo{
            reference_space_type: XrReferenceSpaceType::VIEW,
            ..Default::default()
        };
        unsafe{(xr.xrCreateReferenceSpace)(session, &head_space_info, &mut head_space)}.to_result("xrCreateReferenceSpace")?;
                        
        let mut local_space = XrSpace(0);
        let local_space_info = XrReferenceSpaceCreateInfo{
            reference_space_type: XrReferenceSpaceType::LOCAL,
            ..Default::default()
        };
        unsafe{(xr.xrCreateReferenceSpace)(session, &local_space_info, &mut local_space)}.to_result("xrCreateReferenceSpace")?;
                                
        let format = gl_sys::SRGB8_ALPHA8L;
                        
        let width = ((config_views[0].recommended_image_rect_width as f32) * options.buffer_scale) as u32;
                        
        let height = ((config_views[0].recommended_image_rect_height as f32) * options.buffer_scale) as u32;
                        
                        
        let swap_chain_create_info = XrSwapchainCreateInfo{
            usage_flags: 
            XrSwapchainUsageFlags::SAMPLED | XrSwapchainUsageFlags::COLOR_ATTACHMENT,
            format: format as _,
            width,
            height,
            sample_count: 1,
            face_count:1,
            array_size:2,
            mip_count:1,
            ..Default::default()
        };
                        
        let mut color_swap_chain = XrSwapchain(0);
        unsafe{(xr.xrCreateSwapchain)(session, &swap_chain_create_info, &mut color_swap_chain)}.to_result("xrCreateSwapchain")?;
                        
        let color_images = xr_array_fetch(XrSwapchainImageOpenGLESKHR::default(), |cap, len, buf|{
            unsafe{(xr.xrEnumerateSwapchainImages)(
                color_swap_chain,
                cap,
                len, 
                buf
            )}.to_result("xrEnumerateSwapchainImages")
        })?;
                        
        let mut passthrough = XrPassthroughFB(0);
        let ptci = XrPassthroughCreateInfoFB{
            flags: XrPassthroughFlagsFB(0),
            ..Default::default()
        };
                        
        unsafe{(xr.xrCreatePassthroughFB)(session, &ptci, &mut passthrough)}.to_result("xrCreatePassthroughFB")?;
                        
        let plci = XrPassthroughLayerCreateInfoFB{
            passthrough: passthrough,
            purpose: XrPassthroughLayerPurposeFB::RECONSTRUCTION,
            ..Default::default()
        };
                        
        let mut passthrough_layer = XrPassthroughLayerFB(0);
        unsafe{(xr.xrCreatePassthroughLayerFB)(session, &plci, &mut passthrough_layer)}.to_result("xrCreatePassthroughLayerFB")?;
        unsafe{(xr.xrPassthroughStartFB)(passthrough)}.to_result("xrPassthroughStartFB")?;
        unsafe{(xr.xrPassthroughLayerResumeFB)(passthrough_layer)}.to_result("xrPassthroughLayerResumeFB")?;
                        
        let edpci = XrEnvironmentDepthProviderCreateInfoMETA{
            create_flags: XrEnvironmentDepthProviderCreateFlagsMETA(0),
            ..Default::default()
        };
                        
        let mut depth_provider = XrEnvironmentDepthProviderMETA(0);
        unsafe{(xr.xrCreateEnvironmentDepthProviderMETA)(session, &edpci, &mut depth_provider)}.to_result("xrCreateEnvironmentDepthProviderMETA")?;
        
        
        let edhrsi = XrEnvironmentDepthHandRemovalSetInfoMETA{
            enabled: XrBool32::from_bool(options.remove_hands_from_depth),
            ..Default::default()
        };
        unsafe{(xr.xrSetEnvironmentDepthHandRemovalMETA)(depth_provider, & edhrsi)}.to_result("xrSetEnvironmentDepthHandRemovalMETA")?;
        
        let edsci = XrEnvironmentDepthSwapchainCreateInfoMETA{
            ty: XrStructureType::ENVIRONMENT_DEPTH_SWAPCHAIN_CREATE_INFO_META,
            next: 0 as * const _,
            create_flags: XrEnvironmentDepthSwapchainCreateFlagsMETA(0)
        };
                        
        let mut depth_swap_chain = XrEnvironmentDepthSwapchainMETA(0);
        unsafe{(xr.xrCreateEnvironmentDepthSwapchainMETA)(depth_provider, &edsci, &mut depth_swap_chain)}.to_result("xrCreateEnvironmentDepthSwapchainMETA")?;
                        
        let mut edss = XrEnvironmentDepthSwapchainStateMETA{
            ty: XrStructureType::ENVIRONMENT_DEPTH_SWAPCHAIN_STATE_META,
            next: 0 as * mut _,
            width: 0,
            height: 0
        };
        unsafe{(xr.xrGetEnvironmentDepthSwapchainStateMETA)(depth_swap_chain, &mut edss)}.to_result("xrGetEnvironmentDepthSwapchainStateMETA")?;
                        
        let depth_images = xr_array_fetch(XrSwapchainImageOpenGLESKHR::default(), |cap, len, buf|{
            unsafe{(xr.xrEnumerateEnvironmentDepthSwapchainImagesMETA)(
                depth_swap_chain,
                cap,
                len, 
                buf
            )}.to_result("xrEnumerateEnvironmentDepthSwapchainImagesMETA")
        })?;
                        
        let swap_chain_len = color_images.len();
        let mut gl_depth_textures = vec![0;swap_chain_len];
        let mut gl_frame_buffers = vec![0;swap_chain_len];
        let gl = &display.libgl;
                        
        // create the openGL textures
        for i in 0..swap_chain_len{
            let color_texture = color_images[i].image;
            unsafe{
                (gl.glBindTexture)(gl_sys::TEXTURE_2D_ARRAY, color_texture);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D_ARRAY, gl_sys::TEXTURE_WRAP_S, gl_sys::CLAMP_TO_BORDER as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D_ARRAY, gl_sys::TEXTURE_WRAP_S, gl_sys::CLAMP_TO_BORDER as i32);
                let border_color = [0f32;4];
                (gl.glTexParameterfv)(gl_sys::TEXTURE_2D_ARRAY, gl_sys::TEXTURE_BORDER_COLOR, border_color.as_ptr() as * const _);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D_ARRAY, gl_sys::TEXTURE_MIN_FILTER, gl_sys::LINEAR as i32);
                (gl.glTexParameteri)(gl_sys::TEXTURE_2D_ARRAY, gl_sys::TEXTURE_MAG_FILTER, gl_sys::LINEAR as i32);
                (gl.glBindTexture)(gl_sys::TEXTURE_2D_ARRAY, 0);
                                                                    
                // generate depth buffer
                (gl.glGenTextures)(1, &mut gl_depth_textures[i]);
                (gl.glBindTexture)(gl_sys::TEXTURE_2D_ARRAY, gl_depth_textures[i]);
                (gl.glTexStorage3D)(gl_sys::TEXTURE_2D_ARRAY, 1, gl_sys::DEPTH_COMPONENT24, width as i32, height as i32, 2);
                                                
                (gl.glGenFramebuffers)(1, &mut gl_frame_buffers[i]);
                (gl.glBindFramebuffer)(gl_sys::DRAW_FRAMEBUFFER, gl_frame_buffers[i]);
                                
                if options.multisamples > 1{
                                                                            
                    (gl.glFramebufferTextureMultisampleMultiviewOVR.unwrap())(
                        gl_sys::DRAW_FRAMEBUFFER,
                        gl_sys::DEPTH_ATTACHMENT,
                        gl_depth_textures[i],
                        0,
                        options.multisamples as _,
                        0,
                        2
                    );
                    (gl.glFramebufferTextureMultisampleMultiviewOVR.unwrap())(
                        gl_sys::DRAW_FRAMEBUFFER,
                        gl_sys::COLOR_ATTACHMENT0,
                        color_texture,
                        0,
                        options.multisamples as _,
                        0,
                        2
                    );
                }
                else{
                                                            
                    (gl.glFramebufferTextureMultiviewOVR.unwrap())(
                        gl_sys::DRAW_FRAMEBUFFER,
                        gl_sys::DEPTH_ATTACHMENT,
                        gl_depth_textures[i],
                        0,
                        0,
                        2
                    );
                    (gl.glFramebufferTextureMultiviewOVR.unwrap())(
                        gl_sys::DRAW_FRAMEBUFFER,
                        gl_sys::COLOR_ATTACHMENT0,
                        color_texture,
                        0,
                        0,
                        2
                    );
                }
                (gl.glBindFramebuffer)(gl_sys::DRAW_FRAMEBUFFER, 0);
            }
        }
        
        unsafe{(xr.xrStartEnvironmentDepthProviderMETA)(depth_provider)}
            .to_result("xrStartEnvironmentDepthProviderMETA")?;
        
        let inputs = CxOpenXrInputs::new_inputs(xr, session, instance)?;
                
        Ok(CxOpenXrSession{
            color_images,
            depth_images,
            color_swap_chain,
            depth_swap_chain,
            gl_depth_textures,
            gl_frame_buffers,
            depth_provider,
            passthrough,
            passthrough_layer,
            width,
            height,
            handle:session,
            head_space,
            local_space,
            active:false,
            depth_swap_chain_index: 0,
            inputs
        })
    }
            
    pub fn destroy_session(self, xr: &LibOpenXr, gl:&LibGl)->Result<(),String>{
        // alright lets destroy some things on the session
        unsafe{(xr.xrStopEnvironmentDepthProviderMETA)(self.depth_provider)}
        .log_error("xrStopEnvironmentDepthProviderMETA");
        unsafe{(xr.xrDestroyEnvironmentDepthProviderMETA)(self.depth_provider)}
        .log_error("xrDestroyEnvironmentDepthProviderMETA");
        unsafe{(xr.xrPassthroughPauseFB)(self.passthrough)}
        .log_error("xrPassthroughPauseFB");
        unsafe{(xr.xrDestroyPassthroughFB)(self.passthrough)}
        .log_error("xrDestroyPassthroughFB");
        unsafe{(xr.xrDestroySwapchain)(self.color_swap_chain)}
        .log_error("xrDestroySwapchain");
        unsafe{(xr.xrDestroyEnvironmentDepthSwapchainMETA)(self.depth_swap_chain)}
        .log_error("xrDestroyEnvironmentDepthSwapchainMETA");
        unsafe{(xr.xrDestroySpace)(self.head_space)}
        .log_error("xrDestroySpace");
        unsafe{(xr.xrDestroySpace)(self.local_space)}
        .log_error("xrDestroySpace");
        unsafe{(xr.xrDestroySession)(self.handle)}
        .log_error("xrDestroySession");
                            
        let swap_chain_len = self.color_images.len();
        // destroy the GL resources
        for i in 0..swap_chain_len{
            unsafe{
                (gl.glDeleteTextures)(1, &self.gl_depth_textures[i]);
                (gl.glDeleteFramebuffers)(1, &self.gl_frame_buffers[i]);
            }
        }
                            
        Ok(())
    }
        
            
    fn begin_session(&mut self, xr: &LibOpenXr, activity_thread: u64, render_thread: u64){
        assert!(self.active == false);
                         
        let session_begin_info = XrSessionBeginInfo{
            ty: XrStructureType::SESSION_BEGIN_INFO,
            next: 0 as * const _,
            primary_view_configuration_type: XrViewConfigurationType::PRIMARY_STEREO,
        };
                        
        if unsafe{(xr.xrBeginSession)(self.handle, &session_begin_info)} != XrResult::SUCCESS{
            return
        }
                        
        self.active = true;
                        
        unsafe{(xr.xrPerfSettingsSetPerformanceLevelEXT)(
            self.handle, 
            XrPerfSettingsDomainEXT::CPU,
            XrPerfSettingsLevelEXT::SUSTAINED_HIGH
        )}.log_error("xrPerfSettingsSetPerformanceLevelEXT CPU");
                        
        unsafe{(xr.xrPerfSettingsSetPerformanceLevelEXT)(
            self.handle, 
            XrPerfSettingsDomainEXT::GPU,
            XrPerfSettingsLevelEXT::BOOST
        )}.log_error("xrPerfSettingsSetPerformanceLevelEXT GPU");
                        
                        
        unsafe{(xr.xrSetAndroidApplicationThreadKHR)(
            self.handle, 
            XrAndroidThreadTypeKHR::APPLICATION_MAIN,
            activity_thread as u32
        )}.log_error("xrSetAndroidApplicationThreadKHR");
                        
        unsafe{(xr.xrSetAndroidApplicationThreadKHR)(
            self.handle, 
            XrAndroidThreadTypeKHR::RENDERER_MAIN,
            render_thread as u32
        )}.log_error("xrSetAndroidApplicationThreadKHR");     
    }
        
    fn end_session(&mut self, xr: &LibOpenXr){
        unsafe{(xr.xrEndSession)(self.handle)}.log_error("xrEndSession");
        self.active = false;
    }
        
    fn new_xr_update_event(&mut self, xr: &LibOpenXr, frame:&CxOpenXrFrame)->Option<XrUpdateEvent>{
        let predicted_display_time = frame.frame_state.predicted_display_time;
        let local_space = self.local_space;
                
        let active_action_set = XrActiveActionSet{
            action_set: self.inputs.action_set,
            subaction_path: XrPath(0)
        };
                
        let sync_info = XrActionsSyncInfo{
            count_active_action_sets: 1,
            active_action_sets: &active_action_set as * const _,
            ..Default::default()
        };
                
        if unsafe{(xr.xrSyncActions)(self.handle, &sync_info)} != XrResult::SUCCESS{
            return None
        };
                
        let left_controller = self.inputs.left_controller.poll(xr, self.handle, local_space, predicted_display_time);
        let right_controller = self.inputs.right_controller.poll(xr, self.handle, local_space, predicted_display_time);
                
        let left_hand = self.inputs.left_hand.poll(xr, self.handle, local_space, predicted_display_time);
        let right_hand = self.inputs.right_hand.poll(xr, self.handle, local_space, predicted_display_time);
                        
                
        let new_state = Rc::new(XrState{
            time: (frame.frame_state.predicted_display_time.as_nanos() as f64) / 1e9f64,
            head_pose: frame.local_from_head.pose,
            left_controller,
            right_controller,
            left_hand,
            right_hand,
        });
        let last_state = self.inputs.last_state.clone();
        self.inputs.last_state = new_state.clone();
        Some(XrUpdateEvent{
            state: new_state,
            last: last_state
        })
    }
}

#[derive(Default, Clone, Copy)]
pub struct CxOpenXrEye{
    pub local_from_eye: XrPosef,
    pub view_mat: Mat4,
    pub proj_mat: Mat4,
    pub depth_proj_mat: Mat4,
    pub depth_view_mat: Mat4,
}

pub struct CxOpenXrFrame{
    pub depth_image: Option<XrEnvironmentDepthImageMETA>,
    pub frame_state: XrFrameState,
    pub swap_chain_index: u32,
    pub screen_near_z: f32,
    pub screen_far_z: f32,
    pub projections: [XrView;2],
    pub local_from_head: XrSpaceLocation,
    pub eyes: [CxOpenXrEye;2],
}

impl CxOpenXrFrame{
        
    fn begin_frame(xr:&LibOpenXr, session:&CxOpenXrSession)->Result<CxOpenXrFrame,()>{
                
        if !session.active{
            return Err(())
        }
                                
        let mut fi = XrFrameWaitInfo::default();
        let mut frame_state = XrFrameState::default();
        unsafe{(xr.xrWaitFrame)(session.handle, &mut fi, &mut frame_state)}.log_error("xrWaitFrame");
                
        let mut bf = XrFrameBeginInfo::default();
        unsafe{(xr.xrBeginFrame)(session.handle, &mut bf)}.log_error("xrBeginFrame");
                
        let mut local_from_head = XrSpaceLocation::default();
                
        unsafe{(xr.xrLocateSpace)(
            session.head_space,
            session.local_space,
            frame_state.predicted_display_time,
            &mut local_from_head
        )}.log_error("xrLocateSpace");
                        
        let projection_info = XrViewLocateInfo{
            view_configuration_type: XrViewConfigurationType::PRIMARY_STEREO,
            display_time: frame_state.predicted_display_time,
            space: session.head_space,
            ..Default::default()
        };
                
        let mut view_state = XrViewState::default();
        let mut projections = [XrView::default();2];
        let mut num_views = 0;
        unsafe{(xr.xrLocateViews)(
            session.handle,
            &projection_info,
            &mut view_state,
            2,
            &mut num_views, 
            &mut projections as *mut _
        )}.log_error("xrLocateViews");
                
        // TODO poll tracked controllers here
                
        let mut swap_chain_index = 0;
        let acquire_info = XrSwapchainImageAcquireInfo::default();
        unsafe{(xr.xrAcquireSwapchainImage)(
            session.color_swap_chain,
            &acquire_info,
            &mut swap_chain_index
        )}.log_error("xrAcquireSwapchainImage");
                         
        // TODO COMPUTE XR EYE MATRICES FOR MAKEPAD RENDERER
                
                
        let wait_info = XrSwapchainImageWaitInfo{
            timeout: XrDuration(1000000000),
            ..Default::default()
        };
        loop{
            if unsafe{(xr.xrWaitSwapchainImage)(
                session.color_swap_chain,
                &wait_info
            )} != XrResult::TIMEOUT_EXPIRED{
                break
            }
            crate::log!("retry xrWaitSwapchainImage");
        }
                
        let environment_depth_acquire_info = XrEnvironmentDepthImageAcquireInfoMETA{
            space: session.local_space,
            display_time: frame_state.predicted_display_time,
            ..Default::default()
        };
                
        let mut di = XrEnvironmentDepthImageMETA::default();
        let result = unsafe{(xr.xrAcquireEnvironmentDepthImageMETA)(
            session.depth_provider,
            &environment_depth_acquire_info,
            &mut di
        )};
        let depth_image = if result == XrResult::SUCCESS{
            Some(di)
        }else{
            //crate::log!("FAIL {:?}",result);
            None
        };
        
        
                
        // TODO compute depth image matrices to go into makepad world
        let mut eyes = [CxOpenXrEye::default();2];
                
        for eye in 0..2{
            let head_from_eye = projections[eye].pose;
            let local_from_head = local_from_head.pose;
            let local_from_eye =XrPosef::multiply(&local_from_head, &head_from_eye);
            eyes[eye].local_from_eye  = local_from_eye;
                        
            // lets compute eye matrices and depth matrices
            if let Some(depth_image) = &depth_image{
                let local_from_depth_eye = depth_image.views[eye].pose;
                let depth_eye_from_local = local_from_depth_eye.invert();
                let depth_view_mat = depth_eye_from_local.to_mat4();
                let depth_proj_mat = Mat4::from_camera_fov(
                    &depth_image.views[eye].fov, 
                    depth_image.near_z,
                    if depth_image.far_z.is_finite(){depth_image.far_z}else{0.0}, 
                );
                eyes[eye].depth_view_mat = depth_view_mat;
                eyes[eye].depth_proj_mat = depth_proj_mat;
            }
            let eye_from_local = local_from_eye.invert();
            eyes[eye].view_mat = eye_from_local.to_mat4();
            eyes[eye].proj_mat = Mat4::from_camera_fov(&projections[eye].fov, 0.1, 100.0);
                        
        }
        
        Ok(CxOpenXrFrame{
            projections,
            local_from_head,
            frame_state,
            depth_image,
            eyes,
            swap_chain_index,
            screen_near_z: 0.1, 
            screen_far_z: 10.0,
        })
        //projection_info
        //crate::log!("{:?}", fs);
    }
        
    fn end_frame(self, xr: &LibOpenXr, session:&CxOpenXrSession){
        let release_info = XrSwapchainImageReleaseInfo::default();
                
        unsafe{(xr.xrReleaseSwapchainImage)(
            session.color_swap_chain,
            &release_info
        )}.log_error("xrReleaseSwapchainImage");
                
        // alright lets do the compositor
                
        let comp_passthrough = XrCompositionLayerPassthroughFB{
            layer_handle: session.passthrough_layer,
            flags: XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA,
            ..Default::default()
        };
                        
        let mut proj_views = [XrCompositionLayerProjectionView::default();2];
                                         
        for eye in 0..2{
            proj_views[eye] = XrCompositionLayerProjectionView{
                pose: self.eyes[eye].local_from_eye,
                fov: self.projections[eye].fov,
                sub_image:XrSwapchainSubImage{
                    swapchain: session.color_swap_chain,
                    image_rect: XrRect2Di{
                        offset:XrOffset2Di{x: 0,y: 0},
                        extent:XrExtent2Di{
                            width: session.width as i32,
                            height: session.height as i32
                        }
                    },
                    image_array_index: eye as u32
                },
                ..Default::default()
            };
        }
                
        let comp_proj = XrCompositionLayerProjection{
            space: session.local_space,
            view_count: 2,
            views: &proj_views as *const _,
            layer_flags: 
            XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA |
            XrCompositionLayerFlags::CORRECT_CHROMATIC_ABERRATION, 
            //XrCompositionLayerFlags::UNPREMULTIPLIED_ALPHA,
            ..Default::default()
        };
                
        let layers = [
            &comp_passthrough as *const _ as *const XrCompositionLayerBaseHeader,
            &comp_proj as *const _ as *const XrCompositionLayerBaseHeader
        ];
                
        let fei = XrFrameEndInfo{
            display_time: self.frame_state.predicted_display_time,
            environment_blend_mode: XrEnvironmentBlendMode::OPAQUE,
            layer_count: layers.len() as _,
            layers: &layers as *const *const _,
            ..Default::default()
        };
                
        unsafe{(xr.xrEndFrame)(session.handle, &fei)}.log_error("xrEndFrame");
    }
    
}


pub struct CxOpenXrOptions{
    pub buffer_scale: f32,
    pub multisamples: usize,
    pub remove_hands_from_depth: bool,
}

pub struct CxOpenXrInputs{
    action_set: XrActionSet,
    left_controller: CxOpenXrController,
    right_controller: CxOpenXrController,
    left_hand: CxOpenXrHand,
    right_hand: CxOpenXrHand,
    last_state: Rc<XrState>,
}


pub struct CxOpenXrHand{
    tracker: XrHandTrackerEXT,
    joint_locations: [XrHandJointLocationEXT; HAND_JOINT_COUNT_EXT]
}

impl CxOpenXrHand{
    fn destroy(self, xr: &LibOpenXr){
        unsafe{(xr.xrDestroyHandTrackerEXT)(self.tracker)}
        .log_error("xrDestroyHandTrackerEXT");
    }
        
    fn poll(&mut self, xr: &LibOpenXr, _session:XrSession, local_space:XrSpace, time:XrTime)->XrHand{
        let mut scale = XrHandTrackingScaleFB{
            sensor_output: 1.0,
            current_output: 1.0,
            override_value_input: 1.0,
            override_hand_scale: XrBool32::from_bool(false),
            ..Default::default()
        };
        let mut aim_state = XrHandTrackingAimStateFB{
            next: &mut scale as *mut _  as *mut _,
            ..Default::default()
        };
        let mut locations = XrHandJointLocationsEXT{
            next:  &mut aim_state as *mut _  as *mut _,
            joint_count: HAND_JOINT_COUNT_EXT as _,
            joint_locations: self.joint_locations.as_mut_ptr(),
            ..Default::default()
        };
        let locate_info = XrHandJointsLocateInfoEXT{
            base_space: local_space,
            time,
            ..Default::default()
        }; 
        unsafe{(xr.xrLocateHandJointsEXT)(self.tracker, &locate_info, &mut locations)}
        .log_error("xrLocateHandJointsEXT");
        // alrighty lets convert the joints to our XrHand
        let mut hand = XrHand::default();
        let mut hand_in_view = false;
        let mut s = 0;
        for i in 0..self.joint_locations.len(){
            let tracked = self.joint_locations[i].location_flags.contains(XrSpaceLocationFlags::ORIENTATION_TRACKED) && self.joint_locations[i].location_flags.contains(XrSpaceLocationFlags::POSITION_TRACKED);
            
            // we're going to skip the tips and only store the distance
            if i == 5 || i == 10 || i == 15 || i ==20 || i == 25{
                // only store the distance to the tip so we can fit the entire
                // tracked quest state in a UDP packet :)
                let d = self.joint_locations[i].pose.position - self.joint_locations[i-1].pose.position;
                hand.tips[i/5-1] = d.length();
                continue;
            }
            
            hand.joints[s] = XrHandJoint{
                pose: self.joint_locations[i].pose,
                /*
                valid: self.joint_locations[i].location_flags.contains(XrSpaceLocationFlags::ORIENTATION_VALID) && self.joint_locations[i].location_flags.contains(XrSpaceLocationFlags::POSITION_VALID)
                ,
                tracked: 
                self.joint_locations[i].location_flags.contains(XrSpaceLocationFlags::ORIENTATION_TRACKED) && self.joint_locations[i].location_flags.contains(XrSpaceLocationFlags::POSITION_TRACKED)
                 */     
            };
            s += 1;
            if tracked{
                hand_in_view = true
            }
        }
        hand.in_view = hand_in_view;
        hand
    }
}

pub struct CxOpenXrController{
    path: XrPath,
    trigger_action: XrAction,
    aim_pose_action: XrAction,
    grip_pose_action: XrAction,
    aim_space: XrSpace,
    grip_space: XrSpace,
    detached_aim_space: XrSpace,
    detached_grip_space: XrSpace,
}

impl CxOpenXrController{
    fn destroy(self, xr: &LibOpenXr){
        self.aim_space.destroy(xr);
        self.grip_space.destroy(xr);
        self.detached_aim_space.destroy(xr);
        self.detached_grip_space.destroy(xr);
    }
    
    fn poll(&self, xr: &LibOpenXr, session:XrSession, local_space:XrSpace, time:XrTime)->XrController{
        // lets query the trigger bool
        let trigger = XrActionStateFloat::get(xr, session, self.trigger_action, self.path);
        let grip_state = XrActionStatePose::get(xr, session, self.grip_pose_action, self.path);
        let aim_state = XrActionStatePose::get(xr, session, self.aim_pose_action, self.path);
        XrController{
            grip_pose:  if grip_state.is_active.as_bool(){
                XrSpaceLocation::locate(xr, local_space, time, self.grip_space).pose
            }
            else{
                XrSpaceLocation::locate(xr, local_space, time, self.detached_grip_space).pose
            },
            aim_pose: if aim_state.is_active.as_bool(){
                XrSpaceLocation::locate(xr, local_space, time, self.aim_space).pose
            }
            else{
                XrSpaceLocation::locate(xr, local_space, time, self.detached_aim_space).pose
            },
            trigger: XrFloatButton{
                value: trigger.current_state,
                /*
                last_change_time: trigger.last_change_time.as_secs_f64(),
                analog: trigger.current_state,
                pressed: trigger.current_state > 0.5,
                touched: trigger.is_active.as_bool(),
                last_pressed: false*/
            },
            grip: XrFloatButton::default(),
            buttons: 
                if aim_state.is_active.as_bool(){XrController::ACTIVE}else{0},
            stick: XrStick::default(),            
        }
    }
}

impl CxOpenXrInputs{
        
    pub fn new_inputs(xr: &LibOpenXr, session:XrSession, instance: XrInstance)->Result<CxOpenXrInputs,String>{
        // create handtrackers
        let left_hand_info = XrHandTrackerCreateInfoEXT{hand: XrHandEXT::LEFT,..Default::default()};
        let mut left_hand_track = XrHandTrackerEXT(0);
        unsafe{(xr.xrCreateHandTrackerEXT)(session, &left_hand_info, &mut left_hand_track)}
        .log_error("xrCreateHandTrackerEXT");
        
        let right_hand_info = XrHandTrackerCreateInfoEXT{hand: XrHandEXT::RIGHT,..Default::default()};
        let mut right_hand_track = XrHandTrackerEXT(0);
        unsafe{(xr.xrCreateHandTrackerEXT)(session, &right_hand_info, &mut right_hand_track)}
        .log_error("xrCreateHandTrackerEXT");
        
        let action_set = XrActionSet::new(
            xr, instance, 1, "makepad_action_set", "Main action set"
        )?;
                
        let left_hand_path = XrPath::new(xr, instance, "/user/hand/left")?;
        let right_hand_path = XrPath::new(xr, instance, "/user/hand/right")?;
        let left_detached_path = XrPath::new(xr, instance, "/user/detached_controller_meta/left")?;
        let right_detached_path = XrPath::new(xr, instance, "/user/detached_controller_meta/right")?;
        let hand_paths = [left_hand_path, right_hand_path];
        let detached_paths = [left_detached_path, right_detached_path];
        
        let trigger_action = XrAction::new(xr, action_set, XrActionType::FLOAT_INPUT, "trigger", "", &hand_paths)?;
        let aim_pose_action = XrAction::new(xr, action_set, XrActionType::POSE_INPUT, "aim_pose", "", &hand_paths)?;
        let grip_pose_action = XrAction::new(xr, action_set, XrActionType::POSE_INPUT, "grip_pose", "", &hand_paths)?;
        let detached_aim_pose_action = XrAction::new(xr, action_set, XrActionType::POSE_INPUT, "detached_aim_pose", "", &detached_paths)?;
        let detached_grip_pose_action = XrAction::new(xr, action_set, XrActionType::POSE_INPUT, "detached_grip_pose", "", &detached_paths)?;
                
        let interaction_profile = XrPath::new(xr, instance,  "/interaction_profiles/meta/touch_controller_plus")?;
                
        let bindings = [
            XrActionSuggestedBinding::new(xr, instance, trigger_action, "/user/hand/left/input/trigger")?,
            XrActionSuggestedBinding::new(xr, instance, trigger_action, "/user/hand/right/input/trigger")?,
            XrActionSuggestedBinding::new(xr, instance, aim_pose_action, "/user/hand/left/input/aim/pose")?,
            XrActionSuggestedBinding::new(xr, instance, aim_pose_action, "/user/hand/right/input/aim/pose")?,
            XrActionSuggestedBinding::new(xr, instance, grip_pose_action, "/user/hand/left/input/grip/pose")?,
            XrActionSuggestedBinding::new(xr, instance, grip_pose_action, "/user/hand/right/input/grip/pose")?,
            XrActionSuggestedBinding::new(xr, instance, detached_aim_pose_action, "/user/detached_controller_meta/left/input/aim/pose")?,
            XrActionSuggestedBinding::new(xr, instance, detached_aim_pose_action, "/user/detached_controller_meta/right/input/aim/pose")?,
            XrActionSuggestedBinding::new(xr, instance, detached_grip_pose_action, "/user/detached_controller_meta/left/input/grip/pose")?,
            XrActionSuggestedBinding::new(xr, instance, detached_grip_pose_action, "/user/detached_controller_meta/right/input/grip/pose")?,
        ];
                
        let suggested_bindings = XrInteractionProfileSuggestedBinding{
            interaction_profile,
            count_suggested_bindings: bindings.len() as _,
            suggested_bindings: bindings.as_ptr(),
            ..Default::default()
        };
                
        unsafe{(xr.xrSuggestInteractionProfileBindings)(
            instance,
            &suggested_bindings
        )}.to_result("xrSuggestInteractionProfileBindings")?;
                
        let attach_info = XrSessionActionSetsAttachInfo{
            count_action_sets: 1,
            action_sets: &action_set as * const _,
            ..Default::default()
        };
                    
        unsafe{(xr.xrAttachSessionActionSets)(
            session,
            &attach_info
        )}.to_result("xrAttachSessionActionSets")?;
        let mut pose = XrPosef::default();
        pose.orientation.w = 1.0;
        
        let resume_info = XrSimultaneousHandsAndControllersTrackingResumeInfoMETA::default();
        unsafe{(xr.xrResumeSimultaneousHandsAndControllersTrackingMETA)(session, &resume_info)}
        .log_error("xrResumeSimultaneousHandsAndControllersTrackingMETA");
        
        Ok(CxOpenXrInputs{
            action_set,
            left_hand: CxOpenXrHand{
                tracker:left_hand_track,
                joint_locations: Default::default()
            },
            right_hand: CxOpenXrHand{
                tracker:right_hand_track,
                joint_locations: Default::default()
            },
            left_controller: CxOpenXrController{
                path: left_hand_path,
                trigger_action,
                aim_pose_action,
                grip_pose_action,
                aim_space: XrSpace::new_action_space(xr, session, aim_pose_action, left_hand_path, pose)?,
                grip_space: XrSpace::new_action_space(xr, session, grip_pose_action, left_hand_path, pose)?,
                detached_aim_space: XrSpace::new_action_space(xr, session, detached_aim_pose_action, left_detached_path, pose)?,
                detached_grip_space: XrSpace::new_action_space(xr, session, detached_grip_pose_action, left_detached_path, pose)?
            },
            right_controller: CxOpenXrController{
                path: right_hand_path,
                trigger_action,
                aim_pose_action,
                grip_pose_action,
                aim_space: XrSpace::new_action_space(xr, session, aim_pose_action, right_hand_path, pose)?,
                grip_space: XrSpace::new_action_space(xr, session, grip_pose_action, right_hand_path, pose)?,
                detached_aim_space: XrSpace::new_action_space(xr, session, detached_aim_pose_action, right_detached_path, pose)?,
                detached_grip_space: XrSpace::new_action_space(xr, session, detached_grip_pose_action, right_detached_path, pose)?
            },
            last_state: Default::default()
        })
    }
    
    pub fn destroy_input(self, xr: &LibOpenXr){
        self.left_controller.destroy(xr);
        self.right_controller.destroy(xr);
        self.left_hand.destroy(xr);
        self.right_hand.destroy(xr);
    }
    
}
