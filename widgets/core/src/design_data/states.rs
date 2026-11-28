use crate::{makepad_draw::*, animator::{AnimatorState, Ease as AnimEase, Play}, widget::*, widget_tree::live_id_token};

/// One animator track (hover, down, focus, disabled…) of a widget, read
/// from its script cascade, ready to POSE the layer's instance slice: the
/// values the off and on states apply to this layer, per shader input.
#[derive(Clone)]
pub struct StateTrack {
    pub group: String,
    pub fields: Vec<StateField>,
    /// Into `on` from `off` (the track's own `from` map, `all` fallback).
    pub on_play: Play,
    pub on_ease: AnimEase,
    /// Back into `off`.
    pub off_play: Play,
    pub off_ease: AnimEase,
}

#[derive(Clone)]
pub struct StateField {
    pub id: LiveId,
    /// Slot values of the pose; None = whatever the live widget has.
    pub off: Option<Vec<f32>>,
    pub on: Option<Vec<f32>>,
}

/// The widget's animator tracks that touch `layer`, from its script
/// cascade (the instance's own animator, else its type's): per group the
/// default state is "off" and the first other state is "on".
pub fn animator_tracks(cx: &mut Cx, widget: &WidgetRef, layer: &str) -> Vec<StateTrack> {
    let mut out = Vec::new();
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return out;
    }
    let layer_id = LiveId::from_str(layer);
    cx.with_vm(|vm| {
        let anim = vm.value(source, id!(animator));
        let Some(anim_obj) = anim.as_object() else { return };
        let mut groups: Vec<LiveId> = Vec::new();
        for lvl in vm.construction_chain(anim) {
            for key in lvl.own_keys() {
                if !groups.contains(key) {
                    groups.push(*key);
                }
            }
        }
        for group in groups {
            let group_value = vm.value(anim_obj, group);
            let Some(group_obj) = group_value.as_object() else { continue };
            let mut default = LiveId(0);
            let mut states: Vec<LiveId> = Vec::new();
            for lvl in vm.construction_chain(group_value) {
                for key in lvl.own_keys() {
                    if *key == id!(default) {
                        continue;
                    }
                    if !states.contains(key) {
                        states.push(*key);
                    }
                }
            }
            if let Some(id) = vm.value(group_obj, id!(default)).as_id() {
                default = id;
            }
            if states.is_empty() {
                continue;
            }
            let off = if states.contains(&default) { default } else { states[0] };
            let Some(on) = states.iter().copied().find(|s| *s != off) else { continue };
            let off_state = AnimatorState::script_from_value(vm, vm.value(group_obj, off));
            let on_state = AnimatorState::script_from_value(vm, vm.value(group_obj, on));
            let off_values = pose_values(vm, &off_state, layer_id);
            let on_values = pose_values(vm, &on_state, layer_id);
            if off_values.is_empty() && on_values.is_empty() {
                // This track never touches the layer (a text-only state).
                continue;
            }
            let mut fields: Vec<StateField> = Vec::new();
            for (id, v) in &off_values {
                fields.push(StateField { id: *id, off: Some(v.clone()), on: None });
            }
            for (id, v) in &on_values {
                match fields.iter_mut().find(|f| f.id == *id) {
                    Some(f) => f.on = Some(v.clone()),
                    None => fields.push(StateField { id: *id, off: None, on: Some(v.clone()) }),
                }
            }
            let on_play = on_state
                .from
                .get(&off)
                .or_else(|| on_state.from.get(&id!(all)))
                .copied()
                .unwrap_or(Play::Forward { duration: 0.3 });
            let off_play = off_state
                .from
                .get(&on)
                .or_else(|| off_state.from.get(&id!(all)))
                .copied()
                .unwrap_or(Play::Forward { duration: 0.3 });
            out.push(StateTrack {
                group: format!("{}", live_id_token(group)),
                fields,
                on_play,
                on_ease: on_state.ease.unwrap_or(AnimEase::Linear),
                off_play,
                off_ease: off_state.ease.unwrap_or(AnimEase::Linear),
            });
        }
    });
    out
}

/// A state's `apply` values for one layer, as shader slots: numbers are
/// one slot, colours four (rgba 0..1), bools one.
pub fn pose_values(vm: &mut ScriptVm, state: &AnimatorState, layer_id: LiveId) -> Vec<(LiveId, Vec<f32>)> {
    let mut out = Vec::new();
    let Some(apply) = state.apply.as_ref().map(|apply| apply.as_object()) else { return out };
    let layer_value = vm.value(apply, layer_id);
    let Some(layer_obj) = layer_value.as_object() else { return out };
    let mut keys: Vec<LiveId> = Vec::new();
    for lvl in vm.construction_chain(layer_value) {
        for key in lvl.own_keys() {
            if !keys.contains(key) {
                keys.push(*key);
            }
        }
    }
    for key in keys {
        let mut value = vm.value(layer_obj, key);
        // `snap(1.0)` / `instance(x)` wrap the value in an object keyed
        // `value`; the pose is the wrapped scalar.
        if value.as_color().is_none() && value.as_bool().is_none() && value.as_number().is_none() {
            if let Some(obj) = value.as_object() {
                let inner = vm.value(obj, id!(value));
                if !inner.is_nil() {
                    value = inner;
                }
            }
        }
        let slots = if let Some(c) = value.as_color() {
            vec![
                ((c >> 24) & 0xff) as f32 / 255.0,
                ((c >> 16) & 0xff) as f32 / 255.0,
                ((c >> 8) & 0xff) as f32 / 255.0,
                (c & 0xff) as f32 / 255.0,
            ]
        } else if let Some(b) = value.as_bool() {
            vec![if b { 1.0 } else { 0.0 }]
        } else if let Some(n) = value.as_number() {
            vec![n as f32]
        } else {
            continue;
        };
        out.push((key, slots));
    }
    out
}

/// Each state that supplies a pose for this layer, with its animator group's name.
fn layer_states(vm: &mut ScriptVm, source: ScriptObject, layer: LiveId) -> Vec<(String, ScriptValue)> {
    let mut states = Vec::new();
    if source == ScriptObject::ZERO { return states; }
    let animator = vm.value(source, id!(animator));
    let Some(object) = animator.as_object() else { return states; };
    let mut groups = Vec::new();
    super::props::collect_prop_keys(vm.heap(), object, &mut groups);
    for group in groups {
        let Some(object) = vm.value(object, group).as_object() else { continue; };
        let mut keys = Vec::new();
        super::props::collect_prop_keys(vm.heap(), object, &mut keys);
        for key in keys {
            if key == id!(default) { continue; }
            let value = vm.value(object, key);
            let Some(object) = value.as_object() else { continue; };
            let Some(apply) = vm.value(object, id!(apply)).as_object() else { continue; };
            let layer_value = vm.value(apply, layer);
            if layer_value.as_object().is_some() {
                states.push((format!("{}.{}", live_id_token(group), live_id_token(key)), layer_value));
            }
        }
    }
    states
}

pub(super) fn remote(cx: &mut Cx, widget: &WidgetRef, layer: Option<&str>) -> Result<String, String> {
    use super::json_str;
    let layers: Vec<&str> = match layer {
        Some(name) => vec![name],
        None => widget.layer_areas().into_iter().map(|(name, _)| name).collect(),
    };
    let source = widget.script_source();
    let mut out = Vec::new();
    for layer in layers {
        let names = cx.with_vm(|vm| layer_states(vm, source, LiveId::from_str(layer)))
            .into_iter().map(|(name, _)| json_str(&name)).collect::<Vec<_>>();
        let tracks = animator_tracks(cx, widget, layer).into_iter().map(|track| {
            let slots = |values: &Option<Vec<f32>>| match values {
                Some(v) => format!("[{}]", v.iter().map(|n| if n.is_finite() { n.to_string() } else { "null".into() }).collect::<Vec<_>>().join(",")),
                None => "null".into(),
            };
            let fields = track.fields.iter().map(|f| {
                format!(r#"{{"name":{},"off":{},"on":{}}}"#, json_str(&live_id_token(f.id)), slots(&f.off), slots(&f.on))
            }).collect::<Vec<_>>().join(",");
            format!(r#"{{"group":{},"fields":[{fields}],"on_play":{},"on_ease":{},"off_play":{},"off_ease":{}}}"#,
                json_str(&track.group), json_str(&format!("{:?}", track.on_play)), json_str(&format!("{:?}", track.on_ease)),
                json_str(&format!("{:?}", track.off_play)), json_str(&format!("{:?}", track.off_ease)))
        }).collect::<Vec<_>>();
        out.push(format!(r#"{{"layer":{},"states":[{}],"tracks":[{}]}}"#, json_str(layer), names.join(","), tracks.join(",")));
    }
    Ok(format!(r#"{{"layers":[{}]}}"#, out.join(",")))
}

pub(super) fn pose(cx: &mut Cx, widget: &WidgetRef, layer: &str, state: &str) -> Result<String, String> {
    let source = widget.script_source();
    let layer = LiveId::from_str(layer);
    cx.with_vm(|vm| {
        let states = layer_states(vm, source, layer);
        let mut matching = states.iter().filter(|(name, _)| name == state || name.rsplit_once('.').is_some_and(|(_, s)| s == state));
        let (_, value) = matching.next().ok_or_else(|| format!("no state {state:?} on this layer"))?;
        if matching.next().is_some() { return Err("ambiguous state; use group.state".to_string()); }
        let apply = vm.new_object();
        vm.set_value(apply, layer, *value);
        widget.clone().script_apply(vm, &Apply::Animate, &mut Scope::empty(), apply.into());
        Ok(())
    })?;
    widget.redraw(cx);
    cx.redraw_all();
    Ok(r#"{"ok":true}"#.into())
}
