use super::*;
use crate::protocol::Tracking;

fn name(p: *const c_char, length: usize, localized: bool) -> Xr<String> {
    let s = unsafe { text(p, length)? };
    if s.is_empty() || (!localized && !s.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_ .".contains(&c) && c != b' ')) {
        return Err(if localized { LOCALIZED_NAME_INVALID } else { NAME_INVALID });
    } Ok(s)
}
api!(create_action_set(instance: Handle, info: *const ActionSetCreateInfo, out: *mut Handle) {
    let info = input(info, 28)?; if out.is_null() { return Err(VALIDATION); }
    let name = name(info.name.as_ptr(), 64, false)?;
    let localized_name = name_text(info.localized_name.as_ptr(), 128)?;
    let mut r = registry(); r.instance(instance)?;
    if r.sets.values().any(|s| s.instance == instance && s.name == name) { return Err(NAME_DUPLICATED); }
    if r.sets.values().any(|s| s.instance == instance && s.localized_name == localized_name) { return Err(LOCALIZED_NAME_DUPLICATED); }
    let id = r.id(); r.sets.insert(id, ActionSet { instance, name, localized_name, priority: info.priority }); *out = id; Ok(())
});
fn name_text(p: *const c_char, n: usize) -> Xr<String> { name(p, n, true) }
api!(destroy_action_set(id: Handle) {
    let mut r = registry(); r.sets.remove(&id).ok_or(HANDLE_INVALID)?;
    r.actions.retain(|_, a| a.set != id); Ok(())
});
api!(create_action(set: Handle, info: *const ActionCreateInfo, out: *mut Handle) {
    let info = input(info, 29)?; if out.is_null() { return Err(VALIDATION); }
    if !matches!(info.action_type, 1..=4 | 100) { return Err(VALIDATION); }
    let name = name(info.name.as_ptr(), 64, false)?;
    let localized_name = name_text(info.localized_name.as_ptr(), 128)?;
    let paths = array(info.paths, info.path_count)?.to_vec();
    let mut r = registry(); let instance = r.sets.get(&set).ok_or(HANDLE_INVALID)?.instance;
    for p in &paths { let path = r.path(instance, *p)?; if !matches!(path, "/user/hand/left" | "/user/hand/right") { return Err(PATH_UNSUPPORTED); } }
    if r.sessions.values().any(|s| lock(&s.state).attached.as_ref().is_some_and(|sets| sets.contains(&set))) { return Err(ACTIONSETS_ALREADY_ATTACHED); }
    if r.actions.values().any(|a| a.set == set && a.name == name) { return Err(NAME_DUPLICATED); }
    if r.actions.values().any(|a| a.set == set && a.localized_name == localized_name) { return Err(LOCALIZED_NAME_DUPLICATED); }
    let id = r.id(); r.actions.insert(id, Action { set, name, localized_name, kind: info.action_type, paths, bindings: Vec::new() }); *out = id; Ok(())
});
api!(destroy_action(id: Handle) { registry().actions.remove(&id).ok_or(HANDLE_INVALID)?; Ok(()) });
fn profile_binding(profile: &str, path: &str) -> bool {
    let Some((hand, component)) = path.strip_prefix("/user/hand/").and_then(|p| p.split_once('/')) else { return false; };
    if !matches!(hand, "left" | "right") { return false; }
    let known = matches!(profile, "khr/simple_controller" | "oculus/touch_controller" | "htc/vive_controller" | "valve/index_controller" | "microsoft/motion_controller" | "google/daydream_controller");
    if !known { return false; }
    if matches!(component, "input/grip/pose" | "input/aim/pose") { return true; }
    if component == "output/haptic" { return profile != "google/daydream_controller"; }
    match profile {
        "khr/simple_controller" => matches!(component, "input/select/click" | "input/menu/click"),
        "oculus/touch_controller" => matches!(component, "input/squeeze/value" | "input/trigger/value" | "input/trigger/touch" | "input/thumbstick" | "input/thumbstick/x" | "input/thumbstick/y" | "input/thumbstick/click" | "input/thumbstick/touch" | "input/thumbrest/touch")
            || (hand == "left" && matches!(component, "input/x/click" | "input/x/touch" | "input/y/click" | "input/y/touch" | "input/menu/click"))
            || (hand == "right" && matches!(component, "input/a/click" | "input/a/touch" | "input/b/click" | "input/b/touch" | "input/system/click")),
        "htc/vive_controller" => matches!(component, "input/system/click" | "input/menu/click" | "input/squeeze/click" | "input/trigger/click" | "input/trigger/value" | "input/trackpad" | "input/trackpad/x" | "input/trackpad/y" | "input/trackpad/click" | "input/trackpad/touch"),
        "microsoft/motion_controller" => matches!(component, "input/menu/click" | "input/squeeze/click" | "input/trigger/value" | "input/thumbstick" | "input/thumbstick/x" | "input/thumbstick/y" | "input/thumbstick/click" | "input/trackpad" | "input/trackpad/x" | "input/trackpad/y" | "input/trackpad/click" | "input/trackpad/touch"),
        "valve/index_controller" => matches!(component, "input/system/click" | "input/system/touch" | "input/a/click" | "input/a/touch" | "input/b/click" | "input/b/touch" | "input/squeeze/value" | "input/squeeze/force" | "input/trigger/click" | "input/trigger/value" | "input/trigger/touch" | "input/thumbstick" | "input/thumbstick/x" | "input/thumbstick/y" | "input/thumbstick/click" | "input/thumbstick/touch" | "input/trackpad" | "input/trackpad/x" | "input/trackpad/y" | "input/trackpad/force" | "input/trackpad/touch"),
        "google/daydream_controller" => matches!(component, "input/select/click" | "input/trackpad" | "input/trackpad/x" | "input/trackpad/y" | "input/trackpad/touch" | "input/trackpad/click"),
        _ => false,
    }
}
fn binding_component(profile: &str, path: &str, kind: i32) -> Option<String> {
    // OpenXR accepts parent paths. Resolve their component according to the
    // action type (boolean prefers click, float prefers value, pose uses pose).
    let components: &[&str] = match kind { 1 => &["click", "value"], 2 => &["value", "click"], 4 => &["pose"], _ => &[] };
    for component in components {
        let candidate = format!("{path}/{component}");
        if profile_binding(profile, &candidate) { return Some(candidate); }
    }
    if profile_binding(profile, path) { return Some(path.to_owned()); }
    // A valid input may be unsuitable for this action type: accept it and
    // leave it unbound, rather than rejecting the entire controller profile.
    if ["click", "touch", "value", "force", "pose", "x", "y"].iter()
        .any(|component| profile_binding(profile, &format!("{path}/{component}"))) { return Some(path.to_owned()); }
    None
}
api!(suggest_bindings(instance: Handle, info: *const SuggestedBindings) {
    let info = input(info, 51)?; let mut r = registry();
    let profile = r.path(instance, info.profile)?.strip_prefix("/interaction_profiles/").ok_or(PATH_UNSUPPORTED)?.to_string();
    let mut bindings = Vec::new();
    for b in array(info.bindings, info.count)? {
        let action = r.actions.get(&b.action).ok_or(HANDLE_INVALID)?;
        if r.sets.get(&action.set).ok_or(HANDLE_INVALID)?.instance != instance { return Err(HANDLE_INVALID); }
        if r.sessions.values().any(|s| lock(&s.state).attached.as_ref().is_some_and(|sets| sets.contains(&action.set))) { return Err(ACTIONSETS_ALREADY_ATTACHED); }
        let path = r.path(instance, b.binding)?.to_string();
        let Some(path) = binding_component(&profile, &path, action.kind) else {
            if trace_errors() { eprintln!("[makepad-openxr] unsupported binding {profile}: {path}"); }
            return Err(PATH_UNSUPPORTED);
        };
        bindings.push((b.action, path));
    }
    let i = r.instance_mut(instance)?;
    i.suggested.insert(profile, bindings);
    // Valid suggestions for other controllers are accepted, but never mixed
    // into the Touch bindings. A repeated suggestion replaces that profile.
    let chosen = i.suggested.get("oculus/touch_controller").or_else(|| i.suggested.get("khr/simple_controller")).cloned().unwrap_or_default();
    let sets: HashSet<_> = r.sets.iter().filter(|(_, s)| s.instance == instance).map(|(id, _)| *id).collect();
    for action in r.actions.values_mut().filter(|a| sets.contains(&a.set)) { action.bindings.clear(); }
    for (a, path) in chosen { if let Some(a) = r.actions.get_mut(&a) { if !a.bindings.contains(&path) { a.bindings.push(path); } } }
    Ok(())
});
api!(attach_action_sets(id: Handle, info: *const SessionActionSetsAttachInfo) {
    let info = input(info, 60)?; let s = session(id)?; let sets = array(info.sets, info.count)?.to_vec();
    let mut r = registry();
    for set in &sets { if r.sets.get(set).ok_or(HANDLE_INVALID)?.instance != s.instance { return Err(HANDLE_INVALID); } }
    let mut st = lock(&s.state); if st.attached.is_some() { return Err(ACTIONSETS_ALREADY_ATTACHED); }
    st.attached = Some(sets); r.instance_mut(s.instance)?.events.push_back(Event::Profile(id)); Ok(())
});
api!(interaction_profile(id: Handle, path: Handle, state: *mut InteractionProfileState) {
    let s = session(id)?; let out = output(state, 53)?; let mut r = registry();
    if !matches!(r.path(s.instance, path)?, "/user/hand/left" | "/user/hand/right") { return Err(PATH_UNSUPPORTED); }
    let i = r.instance_mut(s.instance)?;
    let profile = if i.suggested.contains_key("oculus/touch_controller") { "/interaction_profiles/oculus/touch_controller" }
        else if i.suggested.contains_key("khr/simple_controller") { "/interaction_profiles/khr/simple_controller" }
        else { out.profile = 0; return Ok(()); };
    let n = match i.paths.iter().position(|p| p == profile) { Some(n) => n, None => { i.paths.push(profile.into()); i.paths.len() - 1 } };
    out.profile = n as u64 + 1; Ok(())
});
api!(sync_actions(id: Handle, info: *const ActionsSyncInfo) {
    let info = input(info, 61)?; let s = session(id)?; let mut active = Vec::new(); let r = registry();
    let mut st = lock(&s.state); let attached = st.attached.as_ref().ok_or(ACTIONSET_NOT_ATTACHED)?;
    for a in array(info.sets, info.count)? {
        if !attached.contains(&a.set) { return Err(ACTIONSET_NOT_ATTACHED); }
        if a.path != 0 { r.path(s.instance, a.path)?; }
        active.push((a.set, a.path));
    }
    active.sort_by_key(|(set, _)| std::cmp::Reverse(r.sets.get(set).map_or(0, |s| s.priority)));
    st.active_sets = active; st.previous_input = st.input.take(); st.input = st.tracking.clone();
    if !st.running || st.stopping { return Err(SESSION_NOT_FOCUSED); } Ok(())
});

#[derive(Clone, Copy, Default, PartialEq)]
struct Value { active: bool, boolean: bool, scalar: f32, vector: [f32; 2] }
fn value(action: &Action, path: &str, frame: Option<&Tracking>) -> Value {
    let Some(frame) = frame else { return Value::default() };
    let mut out = Value::default();
    for binding in action.bindings.iter().filter(|b| path.is_empty() || b.starts_with(path)) {
        let hand = usize::from(binding.starts_with("/user/hand/right/")); let c = frame.controllers[hand];
        if c.buttons & (1 << 5) == 0 { continue; }
        let component = binding.strip_prefix(if hand == 0 { "/user/hand/left/" } else { "/user/hand/right/" }).unwrap_or("");
        let (boolean, scalar, vector) = match component {
            "input/trigger/value" | "input/select/value" => (c.trigger > 0.5, c.trigger, [0.; 2]),
            "input/trigger/click" | "input/select/click" => (c.trigger > 0.5, c.trigger, [0.; 2]),
            "input/squeeze/value" => (c.grip > 0.5, c.grip, [0.; 2]),
            "input/squeeze/click" => (c.grip > 0.5, c.grip, [0.; 2]),
            "input/thumbstick" => (false, 0., c.stick),
            "input/thumbstick/x" => (false, c.stick[0], [0.; 2]),
            "input/thumbstick/y" => (false, c.stick[1], [0.; 2]),
            "input/grip/pose" | "input/aim/pose" | "output/haptic" => (false, 0., [0.; 2]),
            other => {
                let bit = match other { "input/x/click" => 0, "input/y/click" => 1, "input/a/click" => 2, "input/b/click" => 3,
                    "input/menu/click" => 4, "input/thumbstick/click" => 6, "input/x/touch" => 7, "input/y/touch" => 8,
                    "input/a/touch" => 9, "input/b/touch" => 10, "input/thumbstick/touch" => 11, "input/trigger/touch" => 12, "input/thumbrest/touch" => 13, _ => continue };
                let b = c.buttons & (1 << bit) != 0; (b, if b { 1. } else { 0. }, [0.; 2])
            }
        };
        out.active = true; out.boolean |= boolean;
        if scalar.abs() > out.scalar.abs() { out.scalar = scalar; }
        if vector[0]*vector[0]+vector[1]*vector[1] > out.vector[0]*out.vector[0]+out.vector[1]*out.vector[1] { out.vector = vector; }
    } out
}
fn action_state(id: Handle, info: &ActionStateGetInfo, kind: i32) -> Xr<(Value, bool, Time)> {
    let s = session(id)?; let r = registry(); let a = r.actions.get(&info.action).ok_or(HANDLE_INVALID)?;
    if a.kind != kind { return Err(ACTION_TYPE_MISMATCH); }
    if r.sets.get(&a.set).ok_or(HANDLE_INVALID)?.instance != s.instance { return Err(HANDLE_INVALID); }
    let path = if info.path == 0 { "" } else {
        if !a.paths.contains(&info.path) { return Err(PATH_UNSUPPORTED); } r.path(s.instance, info.path)?
    };
    let mut st = lock(&s.state);
    if !st.attached.as_ref().is_some_and(|sets| sets.contains(&a.set)) { return Err(ACTIONSET_NOT_ATTACHED); }
    if !st.active_sets.iter().any(|(set, p)| *set == a.set && (*p == 0 || info.path == 0 || *p == info.path)) { return Ok((Value::default(), false, 0)); }
    let now = value(a, path, st.input.as_ref()); let previous = value(a, path, st.previous_input.as_ref());
    let changed = now.active && (match kind { 1 => now.boolean != previous.boolean, 2 => now.scalar != previous.scalar, 3 => now.vector != previous.vector, _ => false } || !previous.active);
    if changed { let time = st.input.as_ref().map_or(0, |t| t.display_time); st.changes.insert((info.action, info.path), time); }
    Ok((now, changed, st.changes.get(&(info.action, info.path)).copied().unwrap_or(0)))
}
api!(action_boolean(id: Handle, info: *const ActionStateGetInfo, out: *mut ActionStateBoolean) {
    let (v, changed, time) = action_state(id, input(info, 58)?, 1)?; let out = output(out, 23)?;
    out.current = u32::from(v.boolean); out.changed = u32::from(changed); out.last_change = time; out.active = u32::from(v.active); Ok(())
});
api!(action_float(id: Handle, info: *const ActionStateGetInfo, out: *mut ActionStateFloat) {
    let (v, changed, time) = action_state(id, input(info, 58)?, 2)?; let out = output(out, 24)?;
    out.current = v.scalar; out.changed = u32::from(changed); out.last_change = time; out.active = u32::from(v.active); Ok(())
});
api!(action_vector(id: Handle, info: *const ActionStateGetInfo, out: *mut ActionStateVector2f) {
    let (v, changed, time) = action_state(id, input(info, 58)?, 3)?; let out = output(out, 25)?;
    out.current = v.vector; out.changed = u32::from(changed); out.last_change = time; out.active = u32::from(v.active); Ok(())
});
api!(action_pose(id: Handle, info: *const ActionStateGetInfo, out: *mut ActionStatePose) {
    let (v, _, _) = action_state(id, input(info, 58)?, 4)?; output(out, 27)?.active = u32::from(v.active); Ok(())
});
api!(create_action_space(id: Handle, info: *const ActionSpaceCreateInfo, out: *mut Handle) {
    let info = input(info, 38)?; let s = session(id)?; if out.is_null() { return Err(VALIDATION); }
    if !valid_pose(info.pose) { return Err(POSE_INVALID); }
    let mut r = registry(); let a = r.actions.get(&info.action).ok_or(HANDLE_INVALID)?;
    if a.kind != 4 { return Err(ACTION_TYPE_MISMATCH); }
    if r.sets.get(&a.set).ok_or(HANDLE_INVALID)?.instance != s.instance { return Err(HANDLE_INVALID); }
    if info.path != 0 && !a.paths.contains(&info.path) { return Err(PATH_UNSUPPORTED); }
    let handle = r.id(); r.spaces.insert(handle, Space { session: id, kind: SpaceKind::Action { action: info.action, path: info.path }, offset: info.pose }); *out = handle; Ok(())
});
api!(bound_sources(id: Handle, info: *const BoundSourcesInfo, capacity: u32, count: *mut u32, sources: *mut Handle) {
    let info = input(info, 62)?; let s = session(id)?; let mut r = registry();
    let a = r.actions.get(&info.action).ok_or(HANDLE_INVALID)?;
    if r.sets.get(&a.set).ok_or(HANDLE_INVALID)?.instance != s.instance { return Err(HANDLE_INVALID); }
    let paths = a.bindings.clone(); let i = r.instance_mut(s.instance)?;
    let values: Vec<_> = paths.iter().map(|p| {
        let index = match i.paths.iter().position(|s| s == p) { Some(n) => n, None => { i.paths.push(p.clone()); i.paths.len() - 1 } }; index as u64 + 1
    }).collect(); enumerate(&values, capacity, count, sources)
});
api!(input_name(id: Handle, info: *const InputSourceLocalizedNameInfo, capacity: u32, count: *mut u32, buffer: *mut c_char) {
    let info = input(info, 63)?; let s = session(id)?; let r = registry(); let path = r.path(s.instance, info.source)?;
    let mut bytes: Vec<c_char> = path.bytes().map(|v| v as c_char).collect(); bytes.push(0); enumerate(&bytes, capacity, count, buffer)
});
fn haptic(id: Handle, info: &HapticActionInfo, amplitude: f32, duration: Time) -> Xr<()> {
    let s = session(id)?; let r = registry(); let a = r.actions.get(&info.action).ok_or(HANDLE_INVALID)?;
    if a.kind != 100 { return Err(ACTION_TYPE_MISMATCH); }
    let path = if info.path == 0 { "" } else { if !a.paths.contains(&info.path) { return Err(PATH_UNSUPPORTED); } r.path(s.instance, info.path)? };
    let mut st = lock(&s.state);
    if !st.attached.as_ref().is_some_and(|sets| sets.contains(&a.set)) { return Err(ACTIONSET_NOT_ATTACHED); }
    for b in a.bindings.iter().filter(|b| b.ends_with("/output/haptic") && (path.is_empty() || b.starts_with(path))) {
        st.haptics[usize::from(b.starts_with("/user/hand/right/"))] = Some((amplitude, duration));
    } Ok(())
}
api!(apply_haptic(id: Handle, info: *const HapticActionInfo, pulse: *const Base) {
    let info = input(info, 59)?; let pulse = input(pulse.cast::<HapticVibration>(), 13)?;
    if !pulse.amplitude.is_finite() || !(0.0..=1.0).contains(&pulse.amplitude) || pulse.duration < -1 { return Err(VALIDATION); }
    haptic(id, info, pulse.amplitude, pulse.duration)
});
api!(stop_haptic(id: Handle, info: *const HapticActionInfo) { haptic(id, input(info, 59)?, 0., 0) });
