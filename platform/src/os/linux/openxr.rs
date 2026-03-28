#[cfg(use_vulkan)]
use crate::os::linux::vulkan::CxVulkan;
#[cfg(use_vulkan)]
use ash::vk::{self, Handle};
use makepad_jni_sys as jni_sys;
#[path = "openxr_opengl.rs"]
mod openxr_opengl;
#[cfg(use_vulkan)]
#[path = "openxr_vulkan.rs"]
mod openxr_vulkan;
use {
    crate::{
        cx::{Cx, OsType},
        draw_pass::{CxDrawPassParent, DrawPassId},
        draw_shader::CxDrawShaderMapping,
        event::Event,
        makepad_math::Mat4f,
        makepad_micro_serde::*,
        os::linux::{
            android::android::CxAndroidDisplay,
            android::android_jni::*,
            gl_sys,
            gl_sys::LibGl,
            opengl::{GlShader, SHADER_VARIANT_XR},
            openxr_anchor::*,
            openxr_input::*,
            openxr_sys::*,
        },
    },
    std::ptr,
    std::sync::mpsc,
};

#[cfg(use_vulkan)]
const OPENXR_DEPTH_MESH_READBACK_ENABLED: bool = true;

impl Cx {
    pub(crate) fn openxr_render_loop(
        &mut self,
        from_java_rx: &mpsc::Receiver<FromJavaMessage>,
    ) -> bool {
        if self.os.openxr.session.is_some() {
            loop {
                match from_java_rx.try_recv() {
                    Ok(FromJavaMessage::RenderLoop) => {} // ignore this one
                    Ok(message) => {
                        self.handle_message(message);
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
            self.openxr_handle_events();
            self.handle_other_events();
            self.openxr_handle_drawing();
            return true;
        }
        false
    }

    pub(crate) fn openxr_handle_events(&mut self) {
        let mut redraw_requested = false;
        let openxr = &mut self.os.openxr;
        loop {
            let mut event_buffer = XrEventDataBuffer {
                ty: XrStructureType::EVENT_DATA_BUFFER,
                next: 0 as *const _,
                varying: [0; 4000],
            };
            if unsafe {
                (openxr.libxr.as_ref().unwrap().xrPollEvent)(
                    openxr.instance.unwrap(),
                    &mut event_buffer,
                )
            } != XrResult::SUCCESS
            {
                break;
            }

            match event_buffer.ty {
                XrStructureType::EVENT_DATA_SESSION_STATE_CHANGED => {
                    let edssc = &unsafe {
                        *(&event_buffer as *const _ as *const XrEventDataSessionStateChanged)
                    };
                    match edssc.state {
                        XrSessionState::IDLE => {}
                        XrSessionState::FOCUSED => {}
                        XrSessionState::VISIBLE => {}
                        XrSessionState::READY => {
                            openxr.session.as_mut().unwrap().begin_session(
                                openxr.libxr.as_ref().unwrap(),
                                self.os.activity_thread_id.unwrap(),
                                self.os.render_thread_id.unwrap(),
                            );
                            redraw_requested = true;
                        }
                        XrSessionState::STOPPING => {
                            openxr
                                .session
                                .as_mut()
                                .unwrap()
                                .end_session(openxr.libxr.as_ref().unwrap());
                        }
                        XrSessionState::EXITING => {
                            crate::log!("EXITING!");
                        }
                        _ => (),
                    }
                }
                XrStructureType::EVENT_DATA_DISPLAY_REFRESH_RATE_CHANGED_FB => {
                    let event = &unsafe {
                        *(&event_buffer as *const _
                            as *const XrEventDataDisplayRefreshRateChangedFB)
                    };
                    if let Some(session) = &mut openxr.session {
                        if event.to_display_refresh_rate.is_finite()
                            && event.to_display_refresh_rate > 0.0
                        {
                            session.active_display_refresh_rate_hz =
                                Some(event.to_display_refresh_rate);
                            self.os.xr_display_refresh_rate_active_hz =
                                Some(event.to_display_refresh_rate);
                            crate::log!(
                                "OpenXR display refresh changed: {:.1} Hz -> {:.1} Hz",
                                event.from_display_refresh_rate,
                                event.to_display_refresh_rate
                            );
                        }
                    }
                }
                XrStructureType::EVENT_DATA_REFERENCE_SPACE_CHANGE_PENDING => {
                    let reset_generation =
                        crate::xr_tsdf::xr_tsdf_store().request_reset();
                    redraw_requested = true;
                    crate::log!(
                        "OpenXR reference space changed, resetting depth mesh pipeline generation={}",
                        reset_generation
                    );
                }
                _ => {
                    if let Some(session) = &mut openxr.session {
                        session.handle_anchor_events(
                            openxr.libxr.as_ref().unwrap(),
                            &event_buffer,
                            &self.os_type,
                        );
                    }
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
        if redraw_requested {
            self.redraw_all();
        }
    }

    pub(crate) fn openxr_handle_repaint(&mut self, frame: &CxOpenXrFrame) {
        //opengl_cx.make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        #[cfg(use_vulkan)]
        let use_vulkan_xr = self
            .os
            .openxr
            .session
            .as_ref()
            .and_then(|session| session.vulkan.as_ref())
            .is_some();
        for draw_pass_id in &passes_todo {
            self.passes[*draw_pass_id].set_time(self.os.timers.time_now() as f32);
            match self.passes[*draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {
                    #[cfg(use_vulkan)]
                    if use_vulkan_xr {
                        if let Err(err) = self.openxr_draw_pass_to_vulkan(*draw_pass_id, frame) {
                            crate::error!("OpenXR Vulkan draw failed: {err}");
                        }
                        continue;
                    }
                    self.openxr_draw_pass_to_multiview(*draw_pass_id, frame);
                }
                CxDrawPassParent::Window(_) => {
                    // this cant exist..
                }
                CxDrawPassParent::DrawPass(_) => {
                    #[cfg(target_os = "android")]
                    self.draw_pass_to_texture_for_active_backend(*draw_pass_id);
                    #[cfg(not(target_os = "android"))]
                    self.draw_pass_to_texture(*draw_pass_id, None);
                }
                CxDrawPassParent::None => {
                    #[cfg(target_os = "android")]
                    self.draw_pass_to_texture_for_active_backend(*draw_pass_id);
                    #[cfg(not(target_os = "android"))]
                    self.draw_pass_to_texture(*draw_pass_id, None);
                }
            }
        }
    }

    pub(crate) fn openxr_handle_drawing(&mut self) {
        let frame = {
            let openxr = &mut self.os.openxr;
            CxOpenXrFrame::begin_frame(
                openxr.libxr.as_ref().unwrap(),
                openxr.session.as_mut().unwrap(),
            )
        };
        if let Ok(frame) = frame {
            let (event, last_state, active_refresh_rate_hz, effective_frame_time_ms) = {
                let openxr = &mut self.os.openxr;
                let session = openxr.session.as_mut().unwrap();
                session.depth_swap_chain_index = frame
                    .depth_image
                    .map(|v| v.swapchain_index as usize)
                    .unwrap_or(0);
                session.frame_state = frame.frame_state;
                let effective_frame_time_ms =
                    session.last_predicted_display_time.and_then(|last| {
                        let delta_nanos =
                            frame.frame_state.predicted_display_time.as_nanos() - last.as_nanos();
                        (delta_nanos > 0).then_some(delta_nanos as f64 / 1_000_000.0)
                    });
                session.last_predicted_display_time =
                    Some(frame.frame_state.predicted_display_time);
                let active_refresh_rate_hz = session.active_display_refresh_rate_hz.or_else(|| {
                    let predicted_period_nanos =
                        frame.frame_state.predicted_display_period.as_nanos();
                    if predicted_period_nanos > 0 {
                        Some((1_000_000_000.0 / predicted_period_nanos as f64) as f32)
                    } else {
                        None
                    }
                });
                (
                    session.new_xr_update_event(openxr.libxr.as_ref().unwrap(), &frame),
                    session.inputs.last_state.clone(),
                    active_refresh_rate_hz,
                    effective_frame_time_ms,
                )
            };
            self.os.xr_display_refresh_rate_active_hz = active_refresh_rate_hz;
            self.os.xr_effective_frame_time_ms = effective_frame_time_ms;
            self.os.xr_effective_frame_rate_hz = effective_frame_time_ms
                .filter(|ms| *ms > 0.0)
                .map(|ms| 1000.0 / ms);
            if let Some(event) = event {
                self.call_event_handler(&Event::XrUpdate(event));
            }

            let time_now = self.os.timers.time_now();
            if !self.new_next_frames.is_empty() {
                self.call_next_frame_event(time_now);
            }
            if self.need_redrawing() {
                self.new_draw_event.xr_state = Some(last_state);
                self.call_draw_event(time_now);
                self.compile_shaders_for_active_backend();
            }

            self.openxr_handle_repaint(&frame);

            #[cfg(use_vulkan)]
            if OPENXR_DEPTH_MESH_READBACK_ENABLED {
                if let Some(depth_image_index) =
                    frame.depth_image.map(|v| v.swapchain_index as usize)
                {
                    let (openxr, vulkan) = (&mut self.os.openxr, &mut self.os.vulkan);
                    if let (Some(session), Some(vulkan)) =
                        (openxr.session.as_mut(), vulkan.as_mut())
                    {
                        if let Some(vulkan_session) = session.vulkan.as_mut() {
                            if let Err(err) = vulkan_session.submit_depth_mesh_job(
                                vulkan,
                                &frame,
                                depth_image_index,
                            ) {
                                crate::warning!("OpenXR depth mesh update failed: {err}");
                            }
                        }
                    }
                }
            }

            {
                let openxr = &mut self.os.openxr;
                frame.end_frame(
                    openxr.libxr.as_ref().unwrap(),
                    openxr.session.as_mut().unwrap(),
                );
            }

            #[cfg(use_vulkan)]
            {
                let requested_scale = self.os.xr_buffer_scale_requested;
                let active_scale = self.os.xr_buffer_scale_active;
                if (requested_scale - active_scale).abs() >= 0.0001 && self.os.in_xr_mode {
                    let options = self.current_android_xr_options();
                    let resize_result = {
                        let (openxr, vulkan) = (&mut self.os.openxr, &mut self.os.vulkan);
                        if let Some(vulkan) = vulkan.as_mut() {
                            openxr.resize_projection_layer(vulkan, options)
                        } else {
                            Err(
                                "Android XR projection resize failed: Vulkan backend unavailable"
                                    .to_string(),
                            )
                        }
                    };
                    if let Err(err) = resize_result {
                        crate::warning!(
                            "Android XR render scale resize failed at scale {:.2}, keeping {:.2}: {}",
                            requested_scale,
                            active_scale,
                            err
                        );
                        self.os.xr_buffer_scale_requested = active_scale;
                    } else {
                        self.os.xr_buffer_scale_active = requested_scale;
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub struct CxOpenXr {
    loader: Option<LibOpenXrLoader>,
    pub libxr: Option<LibOpenXr>,
    pub instance: Option<XrInstance>,
    system_id: Option<XrSystemId>,
    pub session: Option<CxOpenXrSession>,
    pub(crate) logged_waiting_for_session: bool,
}

impl CxOpenXr {
    pub fn create_instance(&mut self, activity_handle: jni_sys::jobject) -> Result<(), String> {
        if self.instance.is_some() && self.libxr.is_some() && self.system_id.is_some() {
            return Ok(());
        }

        self.loader = Some(LibOpenXrLoader::try_load()?);

        // lets load em up!
        let loader = &self.loader.as_ref().unwrap();
        let loader_info = XrLoaderInitInfoAndroidKHR {
            ty: XrStructureType::LOADER_INIT_INFO_ANDROID_KHR,
            next: ptr::null(),
            application_vm: makepad_android_state::get_java_vm() as *mut _,
            application_context: activity_handle as *mut _,
        };

        unsafe { (loader.xrInitializeLoaderKHR)(&loader_info as *const _ as _) }
            .to_result("xrInitializeLoaderKHR")?;

        let exts = xr_array_fetch(XrExtensionProperties::default(), |cap, len, buf| {
            unsafe { (loader.xrEnumerateInstanceExtensionProperties)(ptr::null(), cap, len, buf) }
                .to_result("xrEnumerateInstanceExtensionProperties")
        })?;

        let has_extension = |name: &'static str| {
            exts.iter()
                .any(|e| xr_string_zero_terminated(&e.extension_name) == name)
        };

        #[cfg(use_vulkan)]
        let mut exts_needed = vec![
            "XR_EXT_performance_settings\0",
            "XR_EXT_hand_tracking\0",
            "XR_EXT_hand_interaction\0",
            "XR_KHR_android_thread_settings\0",
            "XR_FB_passthrough\0",
            "XR_META_environment_depth\0",
            "XR_META_touch_controller_plus\0",
            "XR_META_detached_controllers\0",
            "XR_META_simultaneous_hands_and_controllers\0",
            "XR_FB_hand_tracking_mesh\0",
            "XR_FB_hand_tracking_aim\0",
            "XR_META_colocation_discovery\0",
            "XR_META_spatial_entity_persistence\0",
            "XR_META_spatial_entity_sharing\0",
            "XR_META_spatial_entity_group_sharing\0",
            "XR_FB_spatial_entity\0",
            "XR_FB_spatial_entity_query\0",
        ];

        #[cfg(use_vulkan)]
        {
            let fixed_foveation_exts = [
                "XR_FB_swapchain_update_state\0",
                "XR_FB_foveation\0",
                "XR_FB_foveation_configuration\0",
                "XR_FB_foveation_vulkan\0",
            ];
            let missing_fixed_foveation_exts: Vec<&str> = fixed_foveation_exts
                .iter()
                .copied()
                .filter(|name| !has_extension(name))
                .collect();
            if missing_fixed_foveation_exts.is_empty() {
                exts_needed.extend_from_slice(&fixed_foveation_exts);
            } else {
                crate::warning!(
                    "OpenXR fixed foveation extensions unavailable on this runtime: {:?}",
                    missing_fixed_foveation_exts
                );
            }
        }

        #[cfg(use_vulkan)]
        {
            let display_refresh_rate_ext = "XR_FB_display_refresh_rate\0";
            if has_extension(display_refresh_rate_ext) {
                exts_needed.push(display_refresh_rate_ext);
            } else {
                crate::warning!(
                    "OpenXR display refresh rate extension unavailable on this runtime"
                );
            }

            if !has_extension("XR_KHR_vulkan_enable2\0") {
                return Err(
                    "OpenXR Vulkan: XR_KHR_vulkan_enable2 is required on this Quest path"
                        .to_string(),
                );
            }
            exts_needed.insert(0, "XR_KHR_vulkan_enable2\0");
        }

        #[cfg(not(use_vulkan))]
        let mut exts_needed = vec![
            "XR_KHR_opengl_es_enable\0",
            "XR_EXT_performance_settings\0",
            "XR_EXT_hand_tracking\0",
            "XR_EXT_hand_interaction\0",
            "XR_KHR_android_thread_settings\0",
            "XR_FB_passthrough\0",
            "XR_META_environment_depth\0",
            "XR_META_touch_controller_plus\0",
            "XR_META_detached_controllers\0",
            "XR_META_simultaneous_hands_and_controllers\0",
            "XR_FB_hand_tracking_mesh\0",
            "XR_FB_hand_tracking_aim\0",
            "XR_META_colocation_discovery\0",
            "XR_META_spatial_entity_persistence\0",
            "XR_META_spatial_entity_sharing\0",
            "XR_META_spatial_entity_group_sharing\0",
            "XR_FB_spatial_entity\0",
            "XR_FB_spatial_entity_query\0",
        ];

        #[cfg(not(use_vulkan))]
        {
            let display_refresh_rate_ext = "XR_FB_display_refresh_rate\0";
            if has_extension(display_refresh_rate_ext) {
                exts_needed.push(display_refresh_rate_ext);
            } else {
                crate::warning!(
                    "OpenXR display refresh rate extension unavailable on this runtime"
                );
            }
        }

        let missing_exts: Vec<&str> = exts_needed
            .iter()
            .copied()
            .filter(|name| !has_extension(name))
            .collect();
        if !missing_exts.is_empty() {
            crate::warning!(
                "OpenXR runtime missing requested extensions: {:?}",
                missing_exts
            );
        }

        let ext_name_ptrs: Vec<*const std::os::raw::c_char> = exts_needed
            .iter()
            .map(|ext| ext.as_ptr() as *const std::os::raw::c_char)
            .collect();

        let create_info = XrInstanceCreateInfo {
            ty: XrStructureType::INSTANCE_CREATE_INFO,
            next: 0 as *const _,
            create_flags: XrInstanceCreateFlags(0),
            application_info: XrApplicationInfo {
                application_name: xr_to_string("makepad_example_simple"),
                application_version: 0,
                engine_name: xr_to_string("Makepad"),
                engine_version: 0,
                api_version: XP_API_VERSION_1_0,
            },
            enabled_api_layer_count: 0,
            enabled_api_layer_names: 0 as *const *const _,
            enabled_extension_count: exts_needed.len() as u32,
            enabled_extension_names: ext_name_ptrs.as_ptr(),
        };
        let mut instance = XrInstance(0);
        unsafe { (loader.xrCreateInstance)(&create_info, &mut instance) }
            .to_result("xrCreateInstance")?;

        let xr = match LibOpenXr::try_load(loader, instance) {
            Ok(xr) => xr,
            Err(err) => {
                loader.destroy_instance(instance);
                return Err(err);
            }
        };

        let mut instance_props = XrInstanceProperties::default();
        if let Err(err) = unsafe { (xr.xrGetInstanceProperties)(instance, &mut instance_props) }
            .to_result("xrGetInstanceProperties")
        {
            loader.destroy_instance(instance);
            return Err(err);
        }

        let mut sys_info = XrSystemGetInfo::default();
        sys_info.form_factor = XrFormFactor::HEAD_MOUNTED_DISPLAY;

        let mut sys_id = XrSystemId(0);
        if let Err(err) = unsafe { (xr.xrGetSystem)(instance, &mut sys_info, &mut sys_id) }
            .to_result("xrGetSystem")
        {
            loader.destroy_instance(instance);
            return Err(err);
        }

        let mut sys_props = XrSystemProperties::default();
        if let Err(err) = unsafe { (xr.xrGetSystemProperties)(instance, sys_id, &mut sys_props) }
            .to_result("xrGetSystemProperties")
        {
            loader.destroy_instance(instance);
            return Err(err);
        }

        #[cfg(not(use_vulkan))]
        {
            let mut ogles_req = XrGraphicsRequirementsOpenGLESKHR::default();
            if let Err(err) = unsafe {
                (xr.xrGetOpenGLESGraphicsRequirementsKHR)(instance, sys_id, &mut ogles_req)
            }
            .to_result("xrGetOpenGLESGraphicsRequirementsKHR")
            {
                loader.destroy_instance(instance);
                return Err(err);
            }
        }

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
        self.instance = Some(instance);
        self.system_id = Some(sys_id);
        self.libxr = Some(xr);
        self.logged_waiting_for_session = false;
        Ok(())
    }

    #[cfg(use_vulkan)]
    pub fn create_vulkan_backend(
        &self,
        window: *mut crate::os::linux::android::ndk_sys::ANativeWindow,
        width: u32,
        height: u32,
    ) -> Result<CxVulkan, String> {
        CxVulkan::new_from_openxr(
            self.libxr
                .as_ref()
                .ok_or_else(|| "OpenXR Vulkan backend init failed: libxr not loaded".to_string())?,
            self.instance.ok_or_else(|| {
                "OpenXR Vulkan backend init failed: instance unavailable".to_string()
            })?,
            self.system_id.ok_or_else(|| {
                "OpenXR Vulkan backend init failed: system_id unavailable".to_string()
            })?,
            window,
            width,
            height,
        )
    }

    pub fn create_session(
        &mut self,
        display: &CxAndroidDisplay,
        #[cfg(use_vulkan)] vulkan: Option<&mut CxVulkan>,
        options: CxOpenXrOptions,
        os_type: &OsType,
    ) -> Result<(), String> {
        if self.libxr.is_none() {
            return Err("create session called before libxr load?".into());
        }
        self.session = Some(CxOpenXrSession::create_session(
            self.libxr.as_ref().unwrap(),
            self.system_id.unwrap(),
            self.instance.unwrap(),
            display,
            #[cfg(use_vulkan)]
            vulkan,
            options,
        )?);
        if let Some(session) = &mut self.session {
            let _ = session.update_active_display_refresh_rate(self.libxr.as_ref().unwrap());
            session.get_local_anchor(self.libxr.as_ref().unwrap(), os_type);
        }
        self.logged_waiting_for_session = false;
        // self.get_local_anchor();
        Ok(())
    }

    pub fn destroy_session(
        &mut self,
        libgl: &LibGl,
        #[cfg(use_vulkan)] vulkan: Option<&mut CxVulkan>,
    ) -> Result<(), String> {
        if let Some(session) = self.session.take() {
            session.destroy_session(
                self.libxr.as_ref().ok_or_else(|| {
                    "OpenXR destroy_session failed: libxr unavailable".to_string()
                })?,
                libgl,
                #[cfg(use_vulkan)]
                vulkan,
            )?;
        }
        self.logged_waiting_for_session = false;
        Ok(())
    }

    #[cfg(use_vulkan)]
    pub fn resize_projection_layer(
        &mut self,
        vulkan: &mut CxVulkan,
        options: CxOpenXrOptions,
    ) -> Result<(), String> {
        let xr = self
            .libxr
            .as_ref()
            .ok_or_else(|| "OpenXR projection resize failed: libxr unavailable".to_string())?;
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "OpenXR projection resize failed: session unavailable".to_string())?;
        session.resize_projection_layer_vulkan(xr, vulkan, options)
    }

    pub fn destroy_instance(
        &mut self,
        libgl: &LibGl,
        #[cfg(use_vulkan)] vulkan: Option<&mut CxVulkan>,
    ) -> Result<(), String> {
        crate::log!("OPENXR DESTROY INSTANCE");
        if let Err(e) = self.destroy_session(
            libgl,
            #[cfg(use_vulkan)]
            vulkan,
        ) {
            crate::log!("OpenXR destroy destroy_session error: {e}")
        }

        let xr = self.libxr.as_ref().ok_or("")?;
        let instance = self.instance.take().ok_or("")?;
        let _system_id = self.system_id.take().ok_or("")?;
        unsafe { (xr.xrDestroyInstance)(instance) }.log_error("xrDestroyInstance");
        Ok(())
    }
    /*


    pub fn advertise_anchors(&mut self, anchors:XrAnchors){
        if let Some(session) = &mut self.session{
            let xr =  self.libxr.as_ref().unwrap();
            session.advertise_anchors(xr, anchors);
        }
    }

    pub fn set_local_anchor(&mut self, anchors:XrAnchors){
        if let Some(session) = &mut self.session{
            let _xr =  self.libxr.as_ref().unwrap();
            session.set_local_anchor(xr, anchors);
            //session.local_anchor = Some(pose);
            //session.create_local_anchor_request(pose, xr);
            //session.create_shareable_anchor_request(pose, xr);
        }
    }

    pub fn discover_anchor(&mut self, id:u8){
        if let Some(session) = &mut self.session{
            let xr =  self.libxr.as_ref().unwrap();
            session.discover_anchor(xr, id);
        }
    }*/
}

pub struct CxOpenXrSession {
    color_images: Vec<XrSwapchainImageOpenGLESKHR>,
    depth_images: Vec<XrSwapchainImageOpenGLESKHR>,
    gl_depth_textures: Vec<u32>,
    gl_frame_buffers: Vec<u32>,
    #[cfg(use_vulkan)]
    vulkan: Option<openxr_vulkan::CxOpenXrVulkanSession>,
    pub color_swap_chain: XrSwapchain,
    pub depth_swap_chain: XrEnvironmentDepthSwapchainMETA,
    pub depth_provider: XrEnvironmentDepthProviderMETA,
    pub passthrough: XrPassthroughFB,
    pub passthrough_layer: XrPassthroughLayerFB,
    pub width: u32,
    pub height: u32,
    pub recommended_width: u32,
    pub recommended_height: u32,
    pub head_space: XrSpace,
    pub local_space: XrSpace,
    pub handle: XrSession,
    pub active: bool,
    pub order_counter: u8,

    pub anchor: CxOpenXrAnchor,
    pub inputs: CxOpenXrInputs,
    debug_inactive_begin_frame_logs: u32,

    // leaked from Frame onto state
    pub depth_swap_chain_index: usize,
    pub frame_state: XrFrameState,
    pub active_display_refresh_rate_hz: Option<f32>,
    last_predicted_display_time: Option<XrTime>,
}

#[derive(SerBin, DeBin)]
#[allow(dead_code)]
struct AnchorAdvertisement {
    group_uuid: XrUuid,
    anchor_uuid: XrUuid,
}

impl CxOpenXrSession {
    fn update_active_display_refresh_rate(
        &mut self,
        xr: &LibOpenXr,
    ) -> Result<Option<f32>, String> {
        let Some(get_display_refresh_rate) = xr.xrGetDisplayRefreshRateFB else {
            return Ok(self.active_display_refresh_rate_hz);
        };
        let mut refresh_rate_hz = 0.0f32;
        unsafe { (get_display_refresh_rate)(self.handle, &mut refresh_rate_hz) }
            .to_result("xrGetDisplayRefreshRateFB")?;
        self.active_display_refresh_rate_hz =
            (refresh_rate_hz.is_finite() && refresh_rate_hz > 0.0).then_some(refresh_rate_hz);
        Ok(self.active_display_refresh_rate_hz)
    }

    fn create_reference_space_with_fallback(
        xr: &LibOpenXr,
        session: XrSession,
        candidates: &[XrReferenceSpaceType],
    ) -> Result<(XrSpace, XrReferenceSpaceType), String> {
        let mut errors = Vec::new();

        for &space_type in candidates {
            let create_info = XrReferenceSpaceCreateInfo {
                reference_space_type: space_type,
                ..Default::default()
            };
            let mut space = XrSpace(0);
            let result = unsafe { (xr.xrCreateReferenceSpace)(session, &create_info, &mut space) };
            if result == XrResult::SUCCESS {
                return Ok((space, space_type));
            }
            errors.push(format!("{space_type:?}: {result:?}"));
        }

        Err(format!(
            "xrCreateReferenceSpace failed for all candidates: {}",
            errors.join(", ")
        ))
    }

    fn describe_primary_stereo_session(
        xr: &LibOpenXr,
        instance: XrInstance,
        system_id: XrSystemId,
        session: XrSession,
        options: CxOpenXrOptions,
    ) -> Result<(XrSpace, XrSpace, u32, u32, u32, u32), String> {
        let configs = xr_array_fetch(XrViewConfigurationType::default(), |cap, len, buf| {
            unsafe { (xr.xrEnumerateViewConfigurations)(instance, system_id, cap, len, buf) }
                .to_result("xrEnumerateViewConfigurations")
        })?;

        if !configs
            .iter()
            .any(|v| *v == XrViewConfigurationType::PRIMARY_STEREO)
        {
            return Err("Could not find PRIMARY STEREO viewconfiguration".to_string());
        }

        let mut config_props = XrViewConfigurationProperties::default();
        unsafe {
            (xr.xrGetViewConfigurationProperties)(
                instance,
                system_id,
                XrViewConfigurationType::PRIMARY_STEREO,
                &mut config_props,
            )
        }
        .to_result("xrGetViewConfigurationProperties")?;

        let config_views = xr_array_fetch(XrViewConfigurationView::default(), |cap, len, buf| {
            unsafe {
                (xr.xrEnumerateViewConfigurationViews)(
                    instance,
                    system_id,
                    XrViewConfigurationType::PRIMARY_STEREO,
                    cap,
                    len,
                    buf,
                )
            }
            .to_result("xrEnumerateViewConfigurationViews")
        })?;

        let mut head_space = XrSpace(0);
        let head_space_info = XrReferenceSpaceCreateInfo {
            reference_space_type: XrReferenceSpaceType::VIEW,
            ..Default::default()
        };
        unsafe { (xr.xrCreateReferenceSpace)(session, &head_space_info, &mut head_space) }
            .to_result("xrCreateReferenceSpace")?;

        let (local_space, _local_space_type) = Self::create_reference_space_with_fallback(
            xr,
            session,
            &[
                XrReferenceSpaceType::LOCAL_FLOOR,
                XrReferenceSpaceType::STAGE,
                XrReferenceSpaceType::LOCAL,
            ],
        )?;
        let recommended_width = config_views[0].recommended_image_rect_width;
        let recommended_height = config_views[0].recommended_image_rect_height;
        let width = ((recommended_width as f32) * options.buffer_scale).max(1.0) as u32;
        let height = ((recommended_height as f32) * options.buffer_scale).max(1.0) as u32;

        Ok((
            head_space,
            local_space,
            recommended_width,
            recommended_height,
            width,
            height,
        ))
    }

    fn create_passthrough_and_depth(
        xr: &LibOpenXr,
        session: XrSession,
        options: CxOpenXrOptions,
    ) -> Result<
        (
            XrPassthroughFB,
            XrPassthroughLayerFB,
            XrEnvironmentDepthProviderMETA,
            XrEnvironmentDepthSwapchainMETA,
        ),
        String,
    > {
        let mut passthrough = XrPassthroughFB(0);
        let ptci = XrPassthroughCreateInfoFB {
            flags: XrPassthroughFlagsFB(0),
            ..Default::default()
        };
        unsafe { (xr.xrCreatePassthroughFB)(session, &ptci, &mut passthrough) }
            .to_result("xrCreatePassthroughFB")?;

        let plci = XrPassthroughLayerCreateInfoFB {
            passthrough,
            purpose: XrPassthroughLayerPurposeFB::RECONSTRUCTION,
            ..Default::default()
        };
        let mut passthrough_layer = XrPassthroughLayerFB(0);
        unsafe { (xr.xrCreatePassthroughLayerFB)(session, &plci, &mut passthrough_layer) }
            .to_result("xrCreatePassthroughLayerFB")?;
        unsafe { (xr.xrPassthroughStartFB)(passthrough) }.to_result("xrPassthroughStartFB")?;
        unsafe { (xr.xrPassthroughLayerResumeFB)(passthrough_layer) }
            .to_result("xrPassthroughLayerResumeFB")?;

        let edpci = XrEnvironmentDepthProviderCreateInfoMETA {
            create_flags: XrEnvironmentDepthProviderCreateFlagsMETA(0),
            ..Default::default()
        };
        let mut depth_provider = XrEnvironmentDepthProviderMETA(0);
        unsafe { (xr.xrCreateEnvironmentDepthProviderMETA)(session, &edpci, &mut depth_provider) }
            .to_result("xrCreateEnvironmentDepthProviderMETA")?;

        let edhrsi = XrEnvironmentDepthHandRemovalSetInfoMETA {
            enabled: XrBool32::from_bool(options.remove_hands_from_depth),
            ..Default::default()
        };
        unsafe { (xr.xrSetEnvironmentDepthHandRemovalMETA)(depth_provider, &edhrsi) }
            .to_result("xrSetEnvironmentDepthHandRemovalMETA")?;

        let edsci = XrEnvironmentDepthSwapchainCreateInfoMETA {
            ty: XrStructureType::ENVIRONMENT_DEPTH_SWAPCHAIN_CREATE_INFO_META,
            next: 0 as *const _,
            create_flags: XrEnvironmentDepthSwapchainCreateFlagsMETA(0),
        };
        let mut depth_swap_chain = XrEnvironmentDepthSwapchainMETA(0);
        unsafe {
            (xr.xrCreateEnvironmentDepthSwapchainMETA)(
                depth_provider,
                &edsci,
                &mut depth_swap_chain,
            )
        }
        .to_result("xrCreateEnvironmentDepthSwapchainMETA")?;

        let mut edss = XrEnvironmentDepthSwapchainStateMETA {
            ty: XrStructureType::ENVIRONMENT_DEPTH_SWAPCHAIN_STATE_META,
            next: 0 as *mut _,
            width: 0,
            height: 0,
        };
        unsafe { (xr.xrGetEnvironmentDepthSwapchainStateMETA)(depth_swap_chain, &mut edss) }
            .to_result("xrGetEnvironmentDepthSwapchainStateMETA")?;

        Ok((
            passthrough,
            passthrough_layer,
            depth_provider,
            depth_swap_chain,
        ))
    }

    pub fn create_session(
        xr: &LibOpenXr,
        system_id: XrSystemId,
        instance: XrInstance,
        display: &CxAndroidDisplay,
        #[cfg(use_vulkan)] vulkan: Option<&mut CxVulkan>,
        options: CxOpenXrOptions,
    ) -> Result<CxOpenXrSession, String> {
        #[cfg(use_vulkan)]
        if let Some(vulkan) = vulkan {
            return Self::create_session_vulkan(xr, system_id, instance, vulkan, options);
        }

        Self::create_session_gles(xr, system_id, instance, display, options)
    }

    pub fn destroy_session(
        self,
        xr: &LibOpenXr,
        gl: &LibGl,
        #[cfg(use_vulkan)] vulkan: Option<&mut CxVulkan>,
    ) -> Result<(), String> {
        crate::log!("OPENXR DESTROY SESSION");
        #[cfg(use_vulkan)]
        let mut session = self;
        #[cfg(not(use_vulkan))]
        let mut session = self;

        if session.active {
            unsafe { (xr.xrEndSession)(session.handle) }.log_error("xrEndSession");
            session.active = false;
        }

        #[cfg(use_vulkan)]
        session.destroy_session_vulkan(xr, vulkan);
        // alright lets destroy some things on the session
        unsafe { (xr.xrStopEnvironmentDepthProviderMETA)(session.depth_provider) }
            .log_error("xrStopEnvironmentDepthProviderMETA");
        unsafe { (xr.xrDestroyEnvironmentDepthProviderMETA)(session.depth_provider) }
            .log_error("xrDestroyEnvironmentDepthProviderMETA");
        unsafe { (xr.xrPassthroughPauseFB)(session.passthrough) }.log_error("xrPassthroughPauseFB");
        unsafe { (xr.xrDestroyPassthroughFB)(session.passthrough) }
            .log_error("xrDestroyPassthroughFB");
        unsafe { (xr.xrDestroySwapchain)(session.color_swap_chain) }
            .log_error("xrDestroySwapchain");
        unsafe { (xr.xrDestroyEnvironmentDepthSwapchainMETA)(session.depth_swap_chain) }
            .log_error("xrDestroyEnvironmentDepthSwapchainMETA");
        unsafe { (xr.xrDestroySpace)(session.head_space) }.log_error("xrDestroySpace");
        unsafe { (xr.xrDestroySpace)(session.local_space) }.log_error("xrDestroySpace");
        unsafe { (xr.xrDestroySession)(session.handle) }.log_error("xrDestroySession");
        session.destroy_session_gles(gl);
        session.inputs.destroy_input(xr);

        Ok(())
    }

    fn begin_session(&mut self, xr: &LibOpenXr, activity_thread: u64, render_thread: u64) {
        assert!(self.active == false);
        let session_begin_info = XrSessionBeginInfo {
            ty: XrStructureType::SESSION_BEGIN_INFO,
            next: 0 as *const _,
            primary_view_configuration_type: XrViewConfigurationType::PRIMARY_STEREO,
        };

        let begin_result = unsafe { (xr.xrBeginSession)(self.handle, &session_begin_info) };
        if begin_result != XrResult::SUCCESS {
            crate::error!("OpenXR begin_session failed: {:?}", begin_result);
            return;
        }

        self.active = true;
        unsafe {
            (xr.xrPerfSettingsSetPerformanceLevelEXT)(
                self.handle,
                XrPerfSettingsDomainEXT::CPU,
                XrPerfSettingsLevelEXT::SUSTAINED_HIGH,
            )
        }
        .log_error("xrPerfSettingsSetPerformanceLevelEXT CPU");

        unsafe {
            (xr.xrPerfSettingsSetPerformanceLevelEXT)(
                self.handle,
                XrPerfSettingsDomainEXT::GPU,
                XrPerfSettingsLevelEXT::SUSTAINED_HIGH,
            )
        }
        .log_error("xrPerfSettingsSetPerformanceLevelEXT GPU");

        unsafe {
            (xr.xrSetAndroidApplicationThreadKHR)(
                self.handle,
                XrAndroidThreadTypeKHR::APPLICATION_MAIN,
                activity_thread as u32,
            )
        }
        .log_error("xrSetAndroidApplicationThreadKHR");

        unsafe {
            (xr.xrSetAndroidApplicationThreadKHR)(
                self.handle,
                XrAndroidThreadTypeKHR::RENDERER_MAIN,
                render_thread as u32,
            )
        }
        .log_error("xrSetAndroidApplicationThreadKHR");

        if let Err(err) = self.update_active_display_refresh_rate(xr) {
            crate::warning!("OpenXR failed to query active display refresh rate: {err}");
        }
    }

    fn end_session(&mut self, xr: &LibOpenXr) {
        crate::log!(
            "OpenXR end_session handle={:?} active={}",
            self.handle,
            self.active
        );
        unsafe { (xr.xrEndSession)(self.handle) }.log_error("xrEndSession");
        self.active = false;
    }
}

#[derive(Default, Clone, Copy)]
pub struct CxOpenXrEye {
    pub local_from_eye: XrPosef,
    pub view_mat: Mat4f,
    pub proj_mat: Mat4f,
    pub depth_proj_mat: Mat4f,
    pub depth_view_mat: Mat4f,
}

pub struct CxOpenXrFrame {
    pub depth_image: Option<XrEnvironmentDepthImageMETA>,
    pub frame_state: XrFrameState,
    pub swap_chain_index: u32,
    pub screen_near_z: f32,
    pub screen_far_z: f32,
    pub projections: [XrView; 2],
    pub local_from_head: XrSpaceLocation,
    pub eyes: [CxOpenXrEye; 2],
}

impl CxOpenXrFrame {
    fn begin_frame(xr: &LibOpenXr, session: &mut CxOpenXrSession) -> Result<CxOpenXrFrame, ()> {
        if !session.active {
            if session.debug_inactive_begin_frame_logs < 5 {
                crate::log!("OpenXR begin_frame skipped because session is not active yet");
                session.debug_inactive_begin_frame_logs += 1;
            }
            return Err(());
        }

        let mut fi = XrFrameWaitInfo::default();
        let mut frame_state = XrFrameState::default();
        unsafe { (xr.xrWaitFrame)(session.handle, &mut fi, &mut frame_state) }
            .log_error("xrWaitFrame");

        let mut bf = XrFrameBeginInfo::default();
        unsafe { (xr.xrBeginFrame)(session.handle, &mut bf) }.log_error("xrBeginFrame");

        let mut local_from_head = XrSpaceLocation::default();

        unsafe {
            (xr.xrLocateSpace)(
                session.head_space,
                session.local_space,
                frame_state.predicted_display_time,
                &mut local_from_head,
            )
        }
        .log_error("xrLocateSpace");

        let projection_info = XrViewLocateInfo {
            view_configuration_type: XrViewConfigurationType::PRIMARY_STEREO,
            display_time: frame_state.predicted_display_time,
            space: session.head_space,
            ..Default::default()
        };

        let mut view_state = XrViewState::default();
        let mut projections = [XrView::default(); 2];
        let mut num_views = 0;
        unsafe {
            (xr.xrLocateViews)(
                session.handle,
                &projection_info,
                &mut view_state,
                2,
                &mut num_views,
                &mut projections as *mut _,
            )
        }
        .log_error("xrLocateViews");

        // TODO poll tracked controllers here

        let mut swap_chain_index = 0;
        let acquire_info = XrSwapchainImageAcquireInfo::default();
        unsafe {
            (xr.xrAcquireSwapchainImage)(
                session.color_swap_chain,
                &acquire_info,
                &mut swap_chain_index,
            )
        }
        .log_error("xrAcquireSwapchainImage");

        // TODO COMPUTE XR EYE MATRICES FOR MAKEPAD RENDERER

        let wait_info = XrSwapchainImageWaitInfo {
            timeout: XrDuration(1000000000),
            ..Default::default()
        };
        let mut wait_retries = 0;
        loop {
            if unsafe { (xr.xrWaitSwapchainImage)(session.color_swap_chain, &wait_info) }
                != XrResult::TIMEOUT_EXPIRED
            {
                break;
            }
            wait_retries += 1;
            crate::log!("OpenXR retry xrWaitSwapchainImage retry={wait_retries}");
        }

        let environment_depth_acquire_info = XrEnvironmentDepthImageAcquireInfoMETA {
            space: session.local_space,
            display_time: frame_state.predicted_display_time,
            ..Default::default()
        };

        let mut di = XrEnvironmentDepthImageMETA::default();
        let result = unsafe {
            (xr.xrAcquireEnvironmentDepthImageMETA)(
                session.depth_provider,
                &environment_depth_acquire_info,
                &mut di,
            )
        };
        let depth_image = if result == XrResult::SUCCESS {
            Some(di)
        } else {
            //crate::log!("FAIL {:?}",result);
            None
        };

        // TODO compute depth image matrices to go into makepad world
        let mut eyes = [CxOpenXrEye::default(); 2];

        #[cfg(use_vulkan)]
        let screen_near_z = 0.05;
        #[cfg(not(use_vulkan))]
        let screen_near_z = 0.1;
        let screen_far_z = 15.0;

        for eye in 0..2 {
            let head_from_eye = projections[eye].pose;
            let local_from_head = local_from_head.pose;
            let local_from_eye = XrPosef::multiply(&local_from_head, &head_from_eye);
            eyes[eye].local_from_eye = local_from_eye;

            // lets compute eye matrices and depth matrices
            if let Some(depth_image) = &depth_image {
                let local_from_depth_eye = depth_image.views[eye].pose;
                let depth_eye_from_local = local_from_depth_eye.invert();
                let depth_view_mat = depth_eye_from_local.to_mat4();
                let depth_proj_mat = Mat4f::from_camera_fov(
                    &depth_image.views[eye].fov,
                    depth_image.near_z,
                    if depth_image.far_z.is_finite() {
                        depth_image.far_z
                    } else {
                        0.0
                    },
                );
                eyes[eye].depth_view_mat = depth_view_mat;
                eyes[eye].depth_proj_mat = depth_proj_mat;
            }
            let eye_from_local = local_from_eye.invert();
            eyes[eye].view_mat = eye_from_local.to_mat4();
            eyes[eye].proj_mat =
                Mat4f::from_camera_fov(&projections[eye].fov, screen_near_z, screen_far_z);
        }

        Ok(CxOpenXrFrame {
            projections,
            local_from_head,
            frame_state,
            depth_image,
            eyes,
            swap_chain_index,
            screen_near_z,
            screen_far_z,
        })
        //projection_info
        //crate::log!("{:?}", fs);
    }

    fn end_frame(self, xr: &LibOpenXr, session: &mut CxOpenXrSession) {
        let release_info = XrSwapchainImageReleaseInfo::default();

        unsafe { (xr.xrReleaseSwapchainImage)(session.color_swap_chain, &release_info) }
            .log_error("xrReleaseSwapchainImage");

        // alright lets do the compositor

        let comp_passthrough = XrCompositionLayerPassthroughFB {
            layer_handle: session.passthrough_layer,
            flags: XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA,
            ..Default::default()
        };

        let mut proj_views = [XrCompositionLayerProjectionView::default(); 2];

        for eye in 0..2 {
            proj_views[eye] = XrCompositionLayerProjectionView {
                pose: self.eyes[eye].local_from_eye,
                fov: self.projections[eye].fov,
                sub_image: XrSwapchainSubImage {
                    swapchain: session.color_swap_chain,
                    image_rect: XrRect2Di {
                        offset: XrOffset2Di { x: 0, y: 0 },
                        extent: XrExtent2Di {
                            width: session.width as i32,
                            height: session.height as i32,
                        },
                    },
                    image_array_index: eye as u32,
                },
                ..Default::default()
            };
        }

        let comp_proj = XrCompositionLayerProjection {
            space: session.local_space,
            view_count: 2,
            views: &proj_views as *const _,
            layer_flags: XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA
                | XrCompositionLayerFlags::CORRECT_CHROMATIC_ABERRATION,
            //XrCompositionLayerFlags::UNPREMULTIPLIED_ALPHA,
            ..Default::default()
        };

        let layers = [
            &comp_passthrough as *const _ as *const XrCompositionLayerBaseHeader,
            &comp_proj as *const _ as *const XrCompositionLayerBaseHeader,
        ];

        let fei = XrFrameEndInfo {
            display_time: self.frame_state.predicted_display_time,
            environment_blend_mode: XrEnvironmentBlendMode::OPAQUE,
            layer_count: layers.len() as _,
            layers: &layers as *const *const _,
            ..Default::default()
        };

        unsafe { (xr.xrEndFrame)(session.handle, &fei) }.log_error("xrEndFrame");
    }
}

#[derive(Clone, Copy)]
pub struct CxOpenXrOptions {
    pub buffer_scale: f32,
    pub multisamples: usize,
    pub remove_hands_from_depth: bool,
    pub fixed_foveation_level: u8,
}
