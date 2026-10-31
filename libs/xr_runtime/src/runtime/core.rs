use super::state::*;
use crate::abi::*;
use std::collections::{HashSet, VecDeque};
use std::ffi::c_char;
use std::sync::{Arc, Mutex};

macro_rules! api {
    ($name:ident($($arg:ident: $ty:ty),* $(,)?) $body:block) => {
        pub(super) unsafe extern "system" fn $name($($arg: $ty),*) -> Result {
            match (|| -> Xr<()> { $body })() {
                Ok(()) => SUCCESS,
                Err(code) => {
                    if code < 0 && trace_errors() { eprintln!("[makepad-openxr] {} returned {code}", stringify!($name)); }
                    code
                }
            }
        }
    };
}

#[path = "actions.rs"]
mod actions;
use actions::*;
#[path = "graphics.rs"]
pub(super) mod graphics;
use graphics::*;
#[path = "swapchains.rs"]
pub(crate) mod swapchains;
use swapchains::*;

pub const EXTENSIONS: &[(&str, u32)] = &[("XR_KHR_vulkan_enable", 8), ("XR_KHR_vulkan_enable2", 2)];

pub unsafe extern "system" fn get_instance_proc_addr(instance: Handle, name: *const c_char, function: *mut Proc) -> Result {
    if function.is_null() { return VALIDATION; }
    *function = None;
    let name = match text(name, 256) { Ok(s) => s, Err(e) => return e };
    let global = matches!(name.as_str(), "xrGetInstanceProcAddr" | "xrEnumerateApiLayerProperties" | "xrEnumerateInstanceExtensionProperties" | "xrCreateInstance");
    if instance == 0 && !global { return HANDLE_INVALID; }
    if instance != 0 && registry().instance(instance).is_err() { return HANDLE_INVALID; }
    macro_rules! dispatch {
        ($($label:literal => $f:ident),* $(,)?) => {
            match name.as_str() {
                $($label => Some(std::mem::transmute::<*const (), unsafe extern "system" fn()>($f as *const ())),)*
                _ => None,
            }
        };
    }
    *function = dispatch! {
        "xrGetInstanceProcAddr" => get_instance_proc_addr,
        "xrEnumerateApiLayerProperties" => enumerate_layers,
        "xrEnumerateInstanceExtensionProperties" => enumerate_extensions,
        "xrCreateInstance" => create_instance,
        "xrDestroyInstance" => destroy_instance,
        "xrGetInstanceProperties" => instance_properties,
        "xrGetSystem" => get_system,
        "xrGetSystemProperties" => system_properties,
        "xrEnumerateEnvironmentBlendModes" => blend_modes,
        "xrEnumerateViewConfigurations" => view_configurations,
        "xrGetViewConfigurationProperties" => view_properties,
        "xrEnumerateViewConfigurationViews" => view_configuration_views,
        "xrPollEvent" => poll_event,
        "xrStringToPath" => string_to_path,
        "xrPathToString" => path_to_string,
        "xrCreateSession" => create_session,
        "xrDestroySession" => destroy_session,
        "xrBeginSession" => begin_session,
        "xrRequestExitSession" => request_exit,
        "xrEndSession" => end_session,
        "xrEnumerateReferenceSpaces" => reference_spaces,
        "xrCreateReferenceSpace" => create_reference_space,
        "xrGetReferenceSpaceBoundsRect" => bounds,
        "xrDestroySpace" => destroy_space,
        "xrLocateSpace" => locate_space,
        "xrLocateViews" => locate_views,
        "xrWaitFrame" => wait_frame,
        "xrBeginFrame" => begin_frame,
        "xrEndFrame" => end_frame,
        "xrResultToString" => result_to_string,
        "xrStructureTypeToString" => structure_to_string,
        "xrCreateActionSet" => create_action_set,
        "xrDestroyActionSet" => destroy_action_set,
        "xrCreateAction" => create_action,
        "xrDestroyAction" => destroy_action,
        "xrSuggestInteractionProfileBindings" => suggest_bindings,
        "xrAttachSessionActionSets" => attach_action_sets,
        "xrGetCurrentInteractionProfile" => interaction_profile,
        "xrSyncActions" => sync_actions,
        "xrGetActionStateBoolean" => action_boolean,
        "xrGetActionStateFloat" => action_float,
        "xrGetActionStateVector2f" => action_vector,
        "xrGetActionStatePose" => action_pose,
        "xrCreateActionSpace" => create_action_space,
        "xrEnumerateBoundSourcesForAction" => bound_sources,
        "xrGetInputSourceLocalizedName" => input_name,
        "xrApplyHapticFeedback" => apply_haptic,
        "xrStopHapticFeedback" => stop_haptic,
        "xrGetVulkanGraphicsRequirementsKHR" => graphics_requirements,
        "xrGetVulkanGraphicsRequirements2KHR" => graphics_requirements2,
        "xrGetVulkanInstanceExtensionsKHR" => vulkan_instance_extensions,
        "xrGetVulkanDeviceExtensionsKHR" => vulkan_device_extensions,
        "xrGetVulkanGraphicsDeviceKHR" => get_vulkan_graphics_device,
        "xrGetVulkanGraphicsDevice2KHR" => get_vulkan_graphics_device2,
        "xrCreateVulkanInstanceKHR" => create_vulkan_instance,
        "xrCreateVulkanDeviceKHR" => create_vulkan_device,
        "xrEnumerateSwapchainFormats" => swapchain_formats,
        "xrCreateSwapchain" => create_swapchain,
        "xrDestroySwapchain" => destroy_swapchain,
        "xrEnumerateSwapchainImages" => swapchain_images,
        "xrAcquireSwapchainImage" => acquire_swapchain,
        "xrWaitSwapchainImage" => wait_swapchain,
        "xrReleaseSwapchainImage" => release_swapchain,
    };
    if (*function).is_some() { SUCCESS } else { FUNCTION_UNSUPPORTED }
}

api!(enumerate_layers(capacity: u32, count: *mut u32, layers: *mut ApiLayerProperties) {
    enumerate(&[], capacity, count, layers)
});
api!(enumerate_extensions(layer: *const c_char, capacity: u32, count: *mut u32, extensions: *mut ExtensionProperties) {
    if !layer.is_null() && !text(layer, 256)?.is_empty() { return Err(API_LAYER_NOT_PRESENT); }
    let mut values = Vec::new();
    for (name, version) in EXTENSIONS {
        let mut e = ExtensionProperties { ty: 2, next: std::ptr::null_mut(), name: [0; 128], version: *version };
        set_text(&mut e.name, name); values.push(e);
    }
    if capacity > 0 {
        for i in 0..(capacity as usize).min(values.len()) { output(extensions.add(i), 2)?; }
    }
    enumerate(&values, capacity, count, extensions)
});
api!(create_instance(info: *const InstanceCreateInfo, instance: *mut Handle) {
    let info = input(info, 3)?;
    if instance.is_null() || info.flags != 0 { return Err(VALIDATION); }
    *instance = 0;
    if info.app.api_version >> 48 != 1 || (info.app.api_version >> 32) & 0xffff != 0 { return Err(API_UNSUPPORTED); }
    if info.layer_count != 0 { return Err(API_LAYER_NOT_PRESENT); }
    let mut extensions = HashSet::new();
    for name in array(info.extensions, info.extension_count)? {
        let name = text(*name, 128)?;
        if !EXTENSIONS.iter().any(|e| e.0 == name) { return Err(EXTENSION_NOT_PRESENT); }
        extensions.insert(name);
    }
    let name = text(info.app.name.as_ptr(), 128)?;
    let mut r = registry(); let id = r.id();
    r.instances.insert(id, Instance { name: name.clone(), extensions, paths: Vec::new(), events: VecDeque::new(), headset: None, connection: None, graphics_checked: false, graphics_limits: None, vulkan: None, suggested: Default::default() });
    *instance = id;
    eprintln!("[makepad-openxr] {name}: runtime instance {id}");
    Ok(())
});
api!(destroy_instance(instance: Handle) {
    let sessions = { let r = registry(); r.instance(instance)?; r.sessions.iter().filter(|(_, s)| s.instance == instance).map(|(id, _)| *id).collect::<Vec<_>>() };
    for id in sessions { destroy_session(id); }
    let mut r = registry();
    let sets: HashSet<_> = r.sets.iter().filter(|(_, s)| s.instance == instance).map(|(id, _)| *id).collect();
    r.actions.retain(|_, a| !sets.contains(&a.set));
    r.sets.retain(|id, _| !sets.contains(id));
    r.instances.remove(&instance);
    Ok(())
});
api!(instance_properties(instance: Handle, properties: *mut InstanceProperties) {
    registry().instance(instance)?;
    let out = output(properties, 32)?;
    out.version = API_VERSION;
    set_text(&mut out.name, "Makepad XR"); Ok(())
});
api!(get_system(instance: Handle, info: *const SystemGetInfo, out: *mut Handle) {
    let info = input(info, 4)?;
    if out.is_null() { return Err(VALIDATION); }
    *out = 0;
    registry().instance(instance)?;
    if info.form_factor != 1 { return Err(FORM_FACTOR_UNSUPPORTED); }
    connect(instance)?; *out = 1; Ok(())
});
api!(system_properties(instance: Handle, id: Handle, properties: *mut SystemProperties) {
    let headset = system(instance, id)?;
    let out = output(properties, 5)?;
    out.system = id; out.vendor = 0;
    set_text(&mut out.name, "Makepad remote headset");
    let limits = graphics::system_limits(instance, &headset)?;
    out.graphics = GraphicsProperties { max_height: limits[1], max_width: limits[0], max_layers: 1 };
    out.tracking = TrackingProperties { orientation: 1, position: 1 }; Ok(())
});
api!(blend_modes(instance: Handle, id: Handle, view_type: i32, capacity: u32, count: *mut u32, modes: *mut i32) {
    system(instance, id)?; stereo(view_type)?; enumerate(&[1], capacity, count, modes)
});
fn stereo(view: i32) -> Xr<()> { if view == 2 { Ok(()) } else { Err(VIEW_CONFIGURATION_UNSUPPORTED) } }
api!(view_configurations(instance: Handle, id: Handle, capacity: u32, count: *mut u32, views: *mut i32) {
    system(instance, id)?; enumerate(&[2], capacity, count, views)
});
api!(view_properties(instance: Handle, id: Handle, view_type: i32, properties: *mut ViewConfigurationProperties) {
    system(instance, id)?; stereo(view_type)?;
    let out = output(properties, 45)?; out.view_type = 2; out.mutable_fov = 0; Ok(())
});
api!(view_configuration_views(instance: Handle, id: Handle, view_type: i32, capacity: u32, count: *mut u32, views: *mut ViewConfigurationView) {
    let h = system(instance, id)?; stereo(view_type)?;
    if count.is_null() { return Err(VALIDATION); } *count = 2;
    if capacity == 0 { return Ok(()); } if capacity < 2 { return Err(SIZE_INSUFFICIENT); }
    let limits = graphics::system_limits(instance, &h)?;
    for n in 0..2 {
        let out = output(views.add(n), 41)?;
        out.recommended_width = h.eye[0]; out.max_width = limits[0];
        out.recommended_height = h.eye[1]; out.max_height = limits[1];
        out.recommended_samples = 1; out.max_samples = 1;
    } Ok(())
});
api!(poll_event(instance: Handle, buffer: *mut EventBuffer) {
    output(buffer, 16)?;
    let event = registry().instance_mut(instance)?.events.pop_front().ok_or(EVENT_UNAVAILABLE)?;
    match event {
        Event::State(session, state, time) => buffer.cast::<EventSessionStateChanged>().write(EventSessionStateChanged { ty: 18, next: std::ptr::null(), session, state, time }),
        Event::Profile(session) => buffer.cast::<EventInteractionProfileChanged>().write(EventInteractionProfileChanged { ty: 52, next: std::ptr::null(), session }),
    } Ok(())
});
api!(string_to_path(instance: Handle, string: *const c_char, path: *mut Handle) {
    if path.is_null() { return Err(VALIDATION); }
    let value = text(string, 256).map_err(|_| PATH_FORMAT_INVALID)?;
    if !value.starts_with('/') || value.ends_with('/') || value.contains("//") || !value.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"/-_.".contains(&c)) { return Err(PATH_FORMAT_INVALID); }
    let mut r = registry(); let i = r.instance_mut(instance)?;
    let index = match i.paths.iter().position(|s| s == &value) { Some(n) => n, None => { i.paths.push(value); i.paths.len() - 1 } };
    *path = index as u64 + 1; Ok(())
});
api!(path_to_string(instance: Handle, path: Handle, capacity: u32, count: *mut u32, buffer: *mut c_char) {
    let r = registry(); let path = r.path(instance, path)?;
    let mut bytes: Vec<c_char> = path.bytes().map(|c| c as c_char).collect(); bytes.push(0);
    enumerate(&bytes, capacity, count, buffer)
});
api!(create_session(instance: Handle, info: *const SessionCreateInfo, out: *mut Handle) {
    let info = input(info, 8)?;
    if out.is_null() || info.flags != 0 { return Err(VALIDATION); } *out = 0;
    let h = system(instance, info.system)?;
    let binding = input(info.next.cast::<GraphicsBindingVulkan>(), 1000025000)?;
    if !registry().instance(instance)?.graphics_checked { return Err(GRAPHICS_REQUIREMENTS_CALL_MISSING); }
    let gpu = super::gpu::Gpu::new(binding, h.render_device)?;
    let mut r = registry(); let i = r.instance(instance)?;
    if !i.graphics_checked { return Err(GRAPHICS_REQUIREMENTS_CALL_MISSING); }
    if info.next.is_null() { return Err(GRAPHICS_DEVICE_INVALID); }
    input(info.next.cast::<GraphicsBindingVulkan>(), 1000025000)?;
    if r.sessions.values().any(|s| s.instance == instance) { return Err(VALIDATION); }
    let connection = i.connection.clone().ok_or(FORM_FACTOR_UNAVAILABLE)?;
    let cancel = lock(&connection).cancel_handle().map_err(io_error)?;
    let id = r.id();
    r.sessions.insert(id, Arc::new(Session { instance, connection, cancel, headset: h, gpu, exports: Mutex::new([None, None]), retired: Mutex::new(Vec::new()), frame_done: std::sync::Condvar::new(), state: Mutex::new(SessionState::default()) }));
    let i = r.instance_mut(instance)?;
    i.events.push_back(Event::State(id, 1, clock())); i.events.push_back(Event::State(id, 2, clock()));
    *out = id; Ok(())
});
api!(destroy_session(id: Handle) {
    let s = session(id)?;
    { let mut st = lock(&s.state); st.lost = true; s.frame_done.notify_all(); }
    let _ = s.cancel.shutdown(std::net::Shutdown::Both);
    let mut r = registry(); r.sessions.remove(&id); r.spaces.retain(|_, s| s.session != id); r.swapchains.retain(|_, s| s.session != id);
    if let Ok(i) = r.instance_mut(s.instance) { i.connection = None; i.events.retain(|e| match e { Event::State(s, ..) | Event::Profile(s) => *s != id }); }
    Ok(())
});
api!(begin_session(id: Handle, info: *const SessionBeginInfo) {
    stereo(input(info, 10)?.view_type)?;
    let s = session(id)?; let mut st = lock(&s.state);
    if st.running { return Err(SESSION_RUNNING); } if st.lost { return Err(SESSION_LOST); }
    st.running = true; st.stopping = false;
    drop(st); event(id, 3); event(id, 4); event(id, 5); Ok(())
});
api!(request_exit(id: Handle) {
    let s = session(id)?; let mut st = lock(&s.state);
    if !st.running { return Err(SESSION_NOT_RUNNING); } st.stopping = true;
    drop(st); event(id, 6); Ok(())
});
api!(end_session(id: Handle) {
    let s = session(id)?; let mut st = lock(&s.state);
    if !st.running { return Err(SESSION_NOT_RUNNING); } if !st.stopping { return Err(SESSION_NOT_STOPPING); }
    st.running = false; st.frame = 0;
    s.frame_done.notify_all();
    drop(st); event(id, 1); event(id, 8); Ok(())
});
api!(reference_spaces(id: Handle, capacity: u32, count: *mut u32, spaces: *mut i32) {
    session(id)?; enumerate(&[1, 2, 3], capacity, count, spaces)
});
api!(create_reference_space(id: Handle, info: *const ReferenceSpaceCreateInfo, out: *mut Handle) {
    session(id)?; let info = input(info, 37)?;
    if out.is_null() { return Err(VALIDATION); }
    if !matches!(info.space_type, 1..=3) { return Err(REFERENCE_SPACE_UNSUPPORTED); }
    if !valid_pose(info.pose) { return Err(POSE_INVALID); }
    let mut r = registry(); let handle = r.id();
    r.spaces.insert(handle, Space { session: id, kind: SpaceKind::Reference(info.space_type), offset: info.pose });
    *out = handle; Ok(())
});
api!(bounds(id: Handle, kind: i32, bounds: *mut Extent2f) {
    session(id)?; if bounds.is_null() { return Err(VALIDATION); }
    if kind != 3 { return Err(REFERENCE_SPACE_UNSUPPORTED); }
    *bounds = Extent2f { width: 0., height: 0. }; Err(SPACE_BOUNDS_UNAVAILABLE)
});
api!(destroy_space(id: Handle) { registry().spaces.remove(&id).ok_or(HANDLE_INVALID)?; Ok(()) });

fn space_pose(r: &Registry, id: Handle, s: &Session, frame: &crate::protocol::Tracking) -> Xr<Pose> {
    let space = r.spaces.get(&id).ok_or(HANDLE_INVALID)?;
    if r.sessions.get(&space.session).is_none_or(|found| !std::ptr::eq(found.as_ref(), s)) { return Err(HANDLE_INVALID); }
    let origin = match space.kind {
        SpaceKind::Reference(1) => pose(frame.head),
        SpaceKind::Reference(_) => Pose::default(),
        SpaceKind::Action { action, path } => {
            let action = r.actions.get(&action).ok_or(HANDLE_INVALID)?;
            let path = if path == 0 { "" } else { r.path(s.instance, path)? };
            let binding = action.bindings.iter().find(|b| path.is_empty() || b.starts_with(path)).ok_or(SESSION_NOT_FOCUSED)?;
            let hand = usize::from(binding.starts_with("/user/hand/right/"));
            let controller = frame.controllers[hand];
            if controller.buttons & (1 << 5) == 0 { return Err(SESSION_NOT_FOCUSED); }
            pose(if binding.ends_with("/aim/pose") { controller.aim_pose } else { controller.grip_pose })
        }
    };
    Ok(compose(origin, space.offset))
}
api!(locate_space(id: Handle, base: Handle, time: Time, location: *mut SpaceLocation) {
    let out = output(location, 42)?; out.flags = 0;
    if time <= 0 { return Err(TIME_INVALID); }
    let r = registry(); let space = r.spaces.get(&id).ok_or(HANDLE_INVALID)?;
    let s = r.session(space.session)?; let st = lock(&s.state);
    let Some(frame) = &st.tracking else { return Ok(()); };
    let (a, b) = match (space_pose(&r, id, &s, frame), space_pose(&r, base, &s, frame)) {
        (Ok(a), Ok(b)) => (a, b), (Err(SESSION_NOT_FOCUSED), _) | (_, Err(SESSION_NOT_FOCUSED)) => return Ok(()),
        (Err(e), _) | (_, Err(e)) => return Err(e),
    };
    out.pose = compose(inverse(b), a); out.flags = 15;
    let mut next = out.next.cast::<Base>();
    while !next.is_null() {
        if (*next).ty == 43 { let velocity = &mut *next.cast::<SpaceVelocity>(); velocity.flags = 0; velocity.linear = [0.; 3]; velocity.angular = [0.; 3]; }
        next = (*next).next as *mut Base;
    } Ok(())
});
api!(locate_views(id: Handle, info: *const ViewLocateInfo, state: *mut ViewState, capacity: u32, count: *mut u32, views: *mut View) {
    let info = input(info, 6)?; stereo(info.view_type)?;
    let out = output(state, 11)?; out.flags = 0;
    if count.is_null() { return Err(VALIDATION); } *count = 2;
    if info.time <= 0 { return Err(TIME_INVALID); }
    let s = session(id)?; let r = registry(); let st = lock(&s.state);
    let Some(frame) = &st.tracking else { return Ok(()); };
    let base = inverse(space_pose(&r, info.space, &s, frame)?);
    if capacity == 0 { return Ok(()); } if capacity < 2 { return Err(SIZE_INSUFFICIENT); }
    for n in 0..2 {
        let out = output(views.add(n), 7)?; out.pose = compose(base, pose(frame.eyes[n].pose)); out.fov = frame.eyes[n].fov;
    } out.flags = 15; Ok(())
});
api!(wait_frame(id: Handle, info: *const FrameWaitInfo, state: *mut FrameState) {
    if !info.is_null() { input(info, 33)?; } let out = output(state, 44)?;
    let s = session(id)?;
    {
        let mut st = lock(&s.state);
        if st.waiting { return Err(CALL_ORDER_INVALID); }
        st.waiting = true;
        // Games pipeline WaitFrame on their simulation thread with EndFrame on
        // their render thread. Our one-frame transport serializes at this wait;
        // it must not reject a valid pipelined frame or hold the IPC lock here.
        while st.frame != 0 && st.running && !st.lost {
            st = s.frame_done.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        if st.lost { st.waiting = false; return Err(SESSION_LOST); }
        if !st.running { st.waiting = false; return Err(SESSION_NOT_RUNNING); }
        st.frame = 3;
    }
    let reply = haptics(&s).and_then(|()| {
        let mut c = lock(&s.connection);
        c.send(&crate::protocol::Message::WaitFrame, None).and_then(|_| c.recv()).map_err(io_error)
    });
    let mut st = lock(&s.state);
    st.waiting = false;
    match reply {
        Ok((crate::protocol::Message::Tracking(frame), _)) => {
            out.display_time = frame.display_time; out.period = frame.period; out.should_render = u32::from(!st.stopping);
            st.previous = st.tracking.take(); st.tracking = Some(frame); st.frame = 1; Ok(())
        }
        _ => { st.lost = true; st.frame = 0; drop(st); event(id, 7); Err(SESSION_LOST) }
    }
});
api!(begin_frame(id: Handle, info: *const FrameBeginInfo) {
    if !info.is_null() { input(info, 46)?; } let s = session(id)?; let mut st = lock(&s.state);
    if !st.running { return Err(SESSION_NOT_RUNNING); } if st.frame != 1 { return Err(CALL_ORDER_INVALID); }
    st.frame = 2; Ok(())
});
api!(end_frame(id: Handle, info: *const FrameEndInfo) {
    let info = input(info, 12)?; let s = session(id)?;
    if info.blend_mode != 1 { return Err(ENVIRONMENT_BLEND_MODE_UNSUPPORTED); }
    let tracking = { let st = lock(&s.state); if !st.running { return Err(SESSION_NOT_RUNNING); }
        if st.frame != 2 { return Err(CALL_ORDER_INVALID); }
        let f = st.tracking.as_ref().ok_or(CALL_ORDER_INVALID)?;
        if info.display_time != f.display_time { return Err(TIME_INVALID); } f.clone() };
    haptics(&s)?;
    if info.layer_count == 0 {
        lock(&s.connection).send(&crate::protocol::Message::Empty { token: tracking.token }, None).map_err(io_error)?;
    } else { projection(id, &s, info, &tracking)?; }
    lock(&s.state).frame = 0; s.frame_done.notify_all(); Ok(())
});
api!(result_to_string(instance: Handle, result: Result, buffer: *mut c_char) {
    registry().instance(instance)?; if buffer.is_null() { return Err(VALIDATION); }
    let value = match result { SUCCESS => "XR_SUCCESS".into(), SESSION_LOST => "XR_ERROR_SESSION_LOST".into(), FORM_FACTOR_UNAVAILABLE => "XR_ERROR_FORM_FACTOR_UNAVAILABLE".into(), _ => format!("XR_UNKNOWN_{}_{result}", if result < 0 { "FAILURE" } else { "SUCCESS" }) };
    set_text(&mut *(buffer.cast::<[c_char; 64]>()), &value); Ok(())
});
api!(structure_to_string(instance: Handle, kind: i32, buffer: *mut c_char) {
    registry().instance(instance)?; if buffer.is_null() { return Err(VALIDATION); }
    set_text(&mut *(buffer.cast::<[c_char; 64]>()), &format!("XR_UNKNOWN_STRUCTURE_TYPE_{kind}")); Ok(())
});
