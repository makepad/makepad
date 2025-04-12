
use crate::os::linux::openxr_sys::*;
use crate::os::linux::android::android::CxAndroidDisplay;
use crate::os::linux::gl_sys;
use crate::cx::{Cx};
use self::super::android_jni::{ *};
use std::sync::{mpsc};
use std::ptr;
use crate::makepad_math::Mat4;

#[derive(Default, Clone, Copy)]
pub struct CxAndroidOpenXrEye{
    pub local_from_eye: XrPosef,
    pub view: Mat4,
    pub proj: Mat4
}

pub struct CxAndroidOpenXrFrame{
    pub depth_image: Option<XrEnvironmentDepthImageMETA>,
    pub frame_state: XrFrameState,
    pub swap_chain_index: u32,
    pub screen_near_z: f32,
    pub screen_far_z: f32,
    pub projections: [XrView;2],
    pub local_from_head: XrSpaceLocation,
    pub eyes: [CxAndroidOpenXrEye;2],
}

pub struct CxAndroidOpenXrSession{
    pub color_images: Vec<XrSwapchainImageOpenGLESKHR>,
    pub depth_images: Vec<XrSwapchainImageOpenGLESKHR>,
    pub color_swap_chain: XrSwapchain,
    pub depth_swap_chain: XrEnvironmentDepthSwapchainMETA,
    pub depth_provider: XrEnvironmentDepthProviderMETA,
    pub passthrough_layer: XrPassthroughLayerFB,
    pub width: u32,
    pub height: u32,
    pub head_space: XrSpace,
    pub local_space: XrSpace,
    pub handle: XrSession,
    pub frame: Option<CxAndroidOpenXrFrame>,
    pub active: bool
}

#[derive(Default)]
pub struct CxAndroidOpenXr{
    loader: Option<LibOpenXrLoader>,
    openxr: Option<LibOpenXr>,
    instance: Option<XrInstance>,
    system_id: Option<XrSystemId>,
    pub session: Option<CxAndroidOpenXrSession>
}

impl Cx{
    pub(crate) fn quest_openxr_mode(&mut self, from_java_rx: &mpsc::Receiver<FromJavaMessage>)->bool{
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
            self.handle_xr_events();
            self.handle_other_events();
            
            self.handle_openxr_drawing();
            // self.handle_drawing();
            return true
        }
        false
    }
    
        
    pub(crate) fn handle_xr_events(&mut self){
        let openxr = &mut self.os.openxr;
        loop{
            let mut event_buffer = XrEventDataBuffer{
                ty:XrType::EVENT_DATA_BUFFER,
                next: 0 as *const _,
                varying: [0;4000]
            };
            if unsafe{(openxr.openxr.as_ref().unwrap().xrPollEvent)(openxr.instance.unwrap(), &mut event_buffer)} != XrResult::SUCCESS{
                break;
            }
            if event_buffer.ty == XrType::EVENT_DATA_SESSION_STATE_CHANGED{
                let edssc = unsafe{*(&event_buffer as *const _ as *const XrEventDataSessionStateChanged)};
                match edssc.state{
                    XrSessionState::IDLE=>{}
                    XrSessionState::FOCUSED=>{}
                    XrSessionState::VISIBLE=>{}
                    XrSessionState::READY=>{
                        openxr.begin_session(self.os.activity_thread_id.unwrap(), self.os.render_thread_id.unwrap());
                    }
                    XrSessionState::STOPPING=>{}
                    XrSessionState::EXITING=>{}
                    _=>()
                }
            }
            /*      
            //crate::log!("{:?}", event_buffer.ty);
            match event_buffer.ty{
                XrType::EVENT_DATA_EVENTS_LOST=>{}
                XrType::EVENT_DATA_INSTANCE_LOSS_PENDING=>{}
                XrType::EVENT_DATA_INTERACTION_PROFILE_CHANGED=>{}
                XrType::EVENT_DATA_PERF_SETTINGS_EXT=>{}
                XrType::EVENT_DATA_REFERENCE_SPACE_CHANGE_PENDING=>{}
                XrType::EVENT_DATA_SESSION_STATE_CHANGED=>{}
                x=>{
                    crate::log!("Unkown xr event {:?}",x);
                }
            }*/
        }
    }
    
     pub(crate) fn handle_openxr_drawing(&mut self){
         self.os.openxr.begin_frame();
         self.os.openxr.end_frame();
     }
}

impl CxAndroidOpenXr{
    pub fn init(&mut self, activity_handle:u64)->Result<(),String>{
        self.loader = Some(LibOpenXrLoader::try_load()?);
        
        // lets load em up!
        let loader = &self.loader.as_ref().unwrap();
        let loader_info = XrLoaderInitInfoAndroidKHR {
            ty: XrType::LOADER_INIT_INFO_ANDROID_KHR,
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
        ];
        
        for ext_needed in exts_needed{
            if !exts.iter().any(|e|{
                xr_string_zero_terminated(&e.extension_name) == ext_needed
            }){
                crate::log!("Needed extension {} not found", ext_needed);
            }
        }
        
        let create_info = XrInstanceCreateInfo{
            ty: XrType::INSTANCE_CREATE_INFO,
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
            enabled_extension_count: 5 as u32,
            enabled_extension_names: &xr_static_str_array(&exts_needed) as *const *const _
        };
        let mut instance = XrInstance(0);
        unsafe{(loader.xrCreateInstance)(&create_info, &mut instance)}.to_result("xrCreateInstance")?;
        self.instance = Some(instance);
        
        self.openxr = Some(LibOpenXr::try_load(loader, instance)?);
        let openxr = &self.openxr.as_ref().unwrap();
        
        let mut instance_props = XrInstanceProperties::default();
        unsafe{(openxr.xrGetInstanceProperties)(instance, &mut instance_props)}.to_result("xrGetInstanceProperties")?;
        
        crate::log!(
            "OpenXR Runtime loaded: {}, Version: {}.{}.{}", 
            xr_string(&instance_props.runtime_name),
            instance_props.runtime_version.major(),        
            instance_props.runtime_version.minor(),        
            instance_props.runtime_version.patch(),        
        );
        
        let mut sys_info = XrSystemGetInfo::default();
        sys_info.form_factor = XrFormFactor::HEAD_MOUNTED_DISPLAY;
        
        let mut sys_id = XrSystemId(0);
        unsafe{(openxr.xrGetSystem)(instance, &mut sys_info, &mut sys_id)}.to_result("xrGetSystem")?;
        self.system_id = Some(sys_id);
    
        let mut sys_props = XrSystemProperties::default();
        unsafe{(openxr.xrGetSystemProperties)(instance, sys_id, &mut sys_props)}.to_result("xrGetSystemProperties")?;
        
        crate::log!("OpenXR System props name:{} vendor_id:{}",xr_string(&sys_props.system_name), sys_props.vendor_id);
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
        );
        
        let mut ogles_req = XrGraphicsRequirementsOpenGLESKHR::default();
        unsafe{(openxr.xrGetOpenGLESGraphicsRequirementsKHR)(instance, sys_id, &mut ogles_req)}.to_result("xrGetOpenGLESGraphicsRequirementsKHR")?;
        
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
    
    pub fn create_xr_session(&mut self, display:&CxAndroidDisplay)->Result<(),String>{
        if self.openxr.is_none(){
            return Err("OpenXR library not loaded".into());
        }
        let openxr = &self.openxr.as_ref().unwrap();
        let system_id = self.system_id.ok_or("XR Not initalised")?;
        let instance = self.instance.ok_or("XR Not initalised")?;
        
        let gfx_binding = XrGraphicsBindingOpenGLESAndroidKHR{
            ty: XrType::GRAPHICS_BINDING_OPENGL_ES_ANDROID_KHR,
            next: 0 as *const _,
            display: display.egl_display,
            config: display.egl_config,
            context: display.egl_context,
        };
        let session_create = XrSessionCreateInfo{
            ty: XrType::SESSION_CREATE_INFO,
            next: &gfx_binding as *const _ as *const _,
            create_flags: XrSessionCreateFlags(0),
            system_id
        };
        
        let mut session = XrSession(0);
        
        unsafe{(openxr.xrCreateSession)(instance, &session_create, &mut session)}.to_result("xrCreateSession")?;
        
        let configs = xr_array_fetch(XrViewConfigurationType::default(), |cap, len, buf|{
            unsafe{(openxr.xrEnumerateViewConfigurations)(
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
        
        unsafe{(openxr.xrGetViewConfigurationProperties)(instance, system_id, XrViewConfigurationType::PRIMARY_STEREO, &mut  config_props)}.to_result("xrGetViewConfigurationProperties")?;
        
        crate::log!(
            "OpenXR System Config type: {:?}, FovMutable:{}",
            config_props.view_configuration_type,
            config_props.fov_mutable.to_bool(),
        );
        
        let config_views = xr_array_fetch(XrViewConfigurationView::default(), |cap, len, buf|{
            unsafe{(openxr.xrEnumerateViewConfigurationViews)(
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
        unsafe{(openxr.xrCreateReferenceSpace)(session, &head_space_info, &mut head_space)}.to_result("xrCreateReferenceSpace")?;
        
        let mut local_space = XrSpace(0);
        let local_space_info = XrReferenceSpaceCreateInfo{
            reference_space_type: XrReferenceSpaceType::LOCAL,
            ..Default::default()
        };
        unsafe{(openxr.xrCreateReferenceSpace)(session, &local_space_info, &mut local_space)}.to_result("xrCreateReferenceSpace")?;
                
        let format = gl_sys::SRGB8_ALPHA8L;
        let width = config_views[0].recommended_image_rect_width;
        let height = config_views[0].recommended_image_rect_height;
        
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
        unsafe{(openxr.xrCreateSwapchain)(session, &swap_chain_create_info, &mut color_swap_chain)}.to_result("xrCreateSwapchain")?;
        
        let color_images = xr_array_fetch(XrSwapchainImageOpenGLESKHR::default(), |cap, len, buf|{
            unsafe{(openxr.xrEnumerateSwapchainImages)(
                color_swap_chain,
                cap,
                len, 
                buf
            )}.to_result("xrEnumerateSwapchainImages")
        })?;
        
        let mut _projections = [
            XrView::default(),
            XrView::default(),
        ];
        
        let mut pass_through_fb = XrPassthroughFB(0);
        let ptci = XrPassthroughCreateInfoFB{
            ty: XrType::PASSTHROUGH_CREATE_INFO_FB,
            next: 0 as *const _,
            flags: XrPassthroughFlagsFB(0)
        };
        
        unsafe{(openxr.xrCreatePassthroughFB)(session, &ptci, &mut pass_through_fb)}.to_result("xrCreatePassthroughFB")?;
        
        let plci = XrPassthroughLayerCreateInfoFB{
            passthrough: pass_through_fb,
            purpose: XrPassthroughLayerPurposeFB::RECONSTRUCTION,
            ..Default::default()
        };
        
        let mut passthrough_layer = XrPassthroughLayerFB(0);
        unsafe{(openxr.xrCreatePassthroughLayerFB)(session, &plci, &mut passthrough_layer)}.to_result("xrCreatePassthroughLayerFB")?;
        unsafe{(openxr.xrPassthroughStartFB)(pass_through_fb)}.to_result("xrPassthroughStartFB")?;
        unsafe{(openxr.xrPassthroughLayerResumeFB)(passthrough_layer)}.to_result("xrPassthroughLayerResumeFB")?;
        
        let edpci = XrEnvironmentDepthProviderCreateInfoMETA{
            ty: XrType::ENVIRONMENT_DEPTH_PROVIDER_CREATE_INFO_META,
            next: 0 as * const _,
            create_flags: XrEnvironmentDepthProviderCreateFlagsMETA(0)
        };
        
        let mut depth_provider = XrEnvironmentDepthProviderMETA(0);
        unsafe{(openxr.xrCreateEnvironmentDepthProviderMETA)(session, &edpci, &mut depth_provider)}.to_result("xrCreateEnvironmentDepthProviderMETA")?;
        
        let edsci = XrEnvironmentDepthSwapchainCreateInfoMETA{
            ty: XrType::ENVIRONMENT_DEPTH_SWAPCHAIN_CREATE_INFO_META,
            next: 0 as * const _,
            create_flags: XrEnvironmentDepthSwapchainCreateFlagsMETA(0)
        };
        
        let mut depth_swap_chain = XrEnvironmentDepthSwapchainMETA(0);
        unsafe{(openxr.xrCreateEnvironmentDepthSwapchainMETA)(depth_provider, &edsci, &mut depth_swap_chain)}.to_result("xrCreateEnvironmentDepthSwapchainMETA")?;
        
        let mut edss = XrEnvironmentDepthSwapchainStateMETA{
            ty: XrType::ENVIRONMENT_DEPTH_SWAPCHAIN_STATE_META,
            next: 0 as * mut _,
            width: 0,
            height: 0
        };
        unsafe{(openxr.xrGetEnvironmentDepthSwapchainStateMETA)(depth_swap_chain, &mut edss)}.to_result("xrGetEnvironmentDepthSwapchainStateMETA")?;
        
        let depth_images = xr_array_fetch(XrSwapchainImageOpenGLESKHR::default(), |cap, len, buf|{
            unsafe{(openxr.xrEnumerateEnvironmentDepthSwapchainImagesMETA)(
                depth_swap_chain,
                cap,
                len, 
                buf
            )}.to_result("xrEnumerateEnvironmentDepthSwapchainImagesMETA")
        })?;
        
        self.session = Some(CxAndroidOpenXrSession{
            color_images,
            depth_images,
            color_swap_chain,
            depth_swap_chain,
            depth_provider,
            passthrough_layer,
            width,
            height,
            handle:session,
            head_space,
            local_space,
            active:false,
            frame: None
        });
        
        /*
        for view in config_views{
            crate::log!(
                "OpenXR view rec_width:{}  rec_height:{}, rec_sample_count: {}",
                view.recommended_image_rect_width, 
                view.recommended_image_rect_height,
                view.recommended_swapchain_sample_count,
            );
            crate::log!(
                "OpenXR view max_width:{} max_height:{}, max_sample_count: {}",
                view.max_image_rect_width, 
                view.max_image_rect_height,
                view.max_swapchain_sample_count,
            );
        }*/
        
        Ok(())
    }
    
    /*  
    static const int CPU_LEVEL = 2;
    static const int GPU_LEVEL = 3;
    static const int NUM_MULTI_SAMPLES = 4;
    static const int MAX_NUM_EYES = 2;
    */
    fn begin_session(&mut self, activity_thread: u64, render_thread: u64){
        let openxr = &self.openxr.as_ref().unwrap();
        let session = self.session.as_mut().unwrap();
        
        assert!(session.active == false);
         
        let session_begin_info = XrSessionBeginInfo{
            ty: XrType::SESSION_BEGIN_INFO,
            next: 0 as * const _,
            primary_view_configuration_type: XrViewConfigurationType::PRIMARY_STEREO,
        };
        
        if unsafe{(openxr.xrBeginSession)(session.handle, &session_begin_info)} != XrResult::SUCCESS{
            return
        }
        crate::log!("OpenXR Session started");
        session.active = true;
        
        unsafe{(openxr.xrPerfSettingsSetPerformanceLevelEXT)(
            session.handle, 
            XrPerfSettingsDomainEXT::CPU,
            XrPerfSettingsLevelEXT::SUSTAINED_HIGH
        )}.log_error("xrPerfSettingsSetPerformanceLevelEXT CPU");
        
        unsafe{(openxr.xrPerfSettingsSetPerformanceLevelEXT)(
            session.handle, 
            XrPerfSettingsDomainEXT::GPU,
            XrPerfSettingsLevelEXT::BOOST
        )}.log_error("xrPerfSettingsSetPerformanceLevelEXT GPU");
        
        
        unsafe{(openxr.xrSetAndroidApplicationThreadKHR)(
            session.handle, 
            XrAndroidThreadTypeKHR::APPLICATION_MAIN,
            activity_thread as u32
        )}.log_error("xrSetAndroidApplicationThreadKHR");
        
        unsafe{(openxr.xrSetAndroidApplicationThreadKHR)(
            session.handle, 
            XrAndroidThreadTypeKHR::RENDERER_MAIN,
            render_thread as u32
        )}.log_error("xrSetAndroidApplicationThreadKHR");     
    }
    
    fn begin_frame(&mut self){
        let openxr = &self.openxr.as_ref().unwrap();
        let session = self.session.as_mut().unwrap();
                        
        let mut fi = XrFrameWaitInfo::default();
        let mut frame_state = XrFrameState::default();
        unsafe{(openxr.xrWaitFrame)(session.handle, &mut fi, &mut frame_state)}.log_error("xrWaitFrame");
        
        let mut bf = XrFrameBeginInfo::default();
        unsafe{(openxr.xrBeginFrame)(session.handle, &mut bf)}.log_error("xrBeginFrame");
        
        let mut local_from_head = XrSpaceLocation::default();
        
        unsafe{(openxr.xrLocateSpace)(
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
        unsafe{(openxr.xrLocateViews)(
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
        unsafe{(openxr.xrAcquireSwapchainImage)(
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
            if unsafe{(openxr.xrWaitSwapchainImage)(
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
        let depth_image = if unsafe{(openxr.xrAcquireEnvironmentDepthImageMETA)(
            session.depth_provider,
            &environment_depth_acquire_info,
            &mut di
        )} == XrResult::SUCCESS{
            Some(di)
        }else{
            None
        };
        
        // TODO compute depth image matrices to go into makepad world
        let mut eyes = [CxAndroidOpenXrEye::default();2];
        
        for eye in 0..2{
            let head_from_eye = projections[eye].pose;
            let local_from_head = local_from_head.pose;
            eyes[eye].local_from_eye = XrPosef::multiply(&local_from_head, &head_from_eye);
        }
        
        session.frame = Some(CxAndroidOpenXrFrame{
            projections,
            local_from_head,
            frame_state,
            depth_image,
            eyes,
            swap_chain_index,
            screen_near_z: 0.1, 
            screen_far_z: 10.0,
        });
        
        
        
        //projection_info
        //crate::log!("{:?}", fs);
    }
    
    fn end_frame(&mut self){
        let openxr = &self.openxr.as_ref().unwrap();
        let session = self.session.as_mut().unwrap();
        let frame = session.frame.take().unwrap();
        
        let release_info = XrSwapchainImageReleaseInfo::default();
        
        unsafe{(openxr.xrReleaseSwapchainImage)(
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
                pose: frame.eyes[eye].local_from_eye,
                fov: frame.projections[eye].fov,
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
        
        let _comp_proj = XrCompositionLayerProjection{
            space: session.local_space,
            view_count: 2,
            views: &proj_views as *const _,
            layer_flags: 
            XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA |
            XrCompositionLayerFlags::CORRECT_CHROMATIC_ABERRATION |
            XrCompositionLayerFlags::UNPREMULTIPLIED_ALPHA,
            ..Default::default()
        };
        
        let layers = [
            &comp_passthrough as *const _ as *const XrCompositionLayerBaseHeader,
            //&comp_proj as *const _ as *const XrCompositionLayerBaseHeader
        ];
        
        let fei = XrFrameEndInfo{
            display_time: frame.frame_state.predicted_display_time,
            environment_blend_mode: XrEnvironmentBlendMode::OPAQUE,
            layer_count: layers.len() as _,
            layers: &layers as *const *const _,
            ..Default::default()
        };
        
        unsafe{(openxr.xrEndFrame)(session.handle, &fei)}.log_error("xrEndFrame");
    }
}