use crate::makepad_platform::*;
use makepad_math::Vec4f;
use std::f64::consts::PI;

pub fn script_mod(vm: &mut ScriptVm) {
    let animator = vm.new_module(id!(animator));

    // Register the native apply_transform function that computes the snap value
    // This function receives the snap object and returns the stored value
    let snap_transform_id = vm.add_apply_transform_fn(|vm, object| {
        // The snap object stores the value to return
        script_value!(vm, object.value)
    });

    // Register the 'snap' function on animator module
    // It returns an object with apply_transform set, so when applied to a field
    // it will return the stored value
    vm.add_method(
        animator,
        id!(snap),
        script_args!(value = NIL),
        move |vm, args| {
            let value = script_value!(vm, args.value);
            let snap_obj = vm.bx.heap.new_object();
            vm.bx.heap.set_value_def(snap_obj, id!(value).into(), value);
            vm.bx
                .heap
                .set_object_apply_transform(snap_obj, snap_transform_id);
            snap_obj.into()
        },
    );

    // Register the native timeline apply_transform function
    // Timeline objects need special handling during interpolation - just return self
    let timeline_transform_id = vm.add_apply_transform_fn(|_vm, object| {
        // Timeline objects are handled specially in interpolate_value
        object.into()
    });

    // Register the 'timeline' function on animator module
    // Usage: timeline(t0 v0 t1 v1 ...) where each pair is (time, value)
    // Times should be in range 0-1 (representing animation progress)
    vm.add_method(animator, id!(timeline), script_args!(), move |vm, args| {
        // Get positional args from the vec part
        let len = vm.bx.heap.vec_len(args);

        // Error if not mod 2 (must be pairs of time/value)
        if len % 2 != 0 {
            log!(
                "timeline: expected even number of arguments (time/value pairs), got {}",
                len
            );
            return ScriptValue::NIL;
        }

        if len == 0 {
            log!("timeline: expected at least one time/value pair");
            return ScriptValue::NIL;
        }

        // Create timeline object
        let timeline_obj = vm.bx.heap.new_object();

        // Create a keyframes array to store the time/value pairs
        let keyframes = vm.bx.heap.new_object();

        // Copy keyframes from args
        for i in 0..len {
            let value = vm.bx.heap.vec_value(args, i, NoTrap);
            vm.bx
                .heap
                .vec_push(keyframes, ScriptValue::NIL, value, NoTrap);
        }

        // Store keyframes on timeline object
        vm.bx
            .heap
            .set_value_def(timeline_obj, id!(keyframes).into(), keyframes.into());
        vm.bx
            .heap
            .set_object_apply_transform(timeline_obj, timeline_transform_id);

        timeline_obj.into()
    });

    script_mod! {
        use mod.animator;
        use mod.std.*;
        animator.Animator = set_type_default() do #(Animator::script_ext(vm)){}
        animator.AnimatorGroup = #(AnimatorGroup::script_ext(vm)){}
        animator.AnimatorState =  #(AnimatorState::script_api(vm)){}
        animator.Play =  #(Play::script_api(vm))
        animator.Ease = #(Ease::script_api(vm))
    };
    script_mod(vm);
}

pub trait AnimatorImpl {
    fn animator_cut(&mut self, cx: &mut Cx, state: &[LiveId; 2]) {
        self.animator_cut_scoped(cx, state, &mut Scope::empty())
    }
    fn animator_play(&mut self, cx: &mut Cx, state: &[LiveId; 2]) {
        self.animator_play_scoped(cx, state, &mut Scope::empty())
    }
    fn animator_toggle_scoped(
        &mut self,
        cx: &mut Cx,
        is_state_1: bool,
        animate: Animate,
        state1: &[LiveId; 2],
        state2: &[LiveId; 2],
        scope: &mut Scope,
    ) {
        if is_state_1 {
            if let Animate::Yes = animate {
                self.animator_play_scoped(cx, state1, scope)
            } else {
                self.animator_cut_scoped(cx, state1, scope)
            }
        } else {
            if let Animate::Yes = animate {
                self.animator_play_scoped(cx, state2, scope)
            } else {
                self.animator_cut_scoped(cx, state2, scope)
            }
        }
    }
    fn animator_toggle(
        &mut self,
        cx: &mut Cx,
        is_state_1: bool,
        animate: Animate,
        state1: &[LiveId; 2],
        state2: &[LiveId; 2],
    ) {
        self.animator_toggle_scoped(cx, is_state_1, animate, state1, state2, &mut Scope::empty())
    }

    fn animator_handle_event(&mut self, cx: &mut Cx, event: &Event) -> AnimatorAction {
        self.animator_handle_event_scoped(cx, event, &mut Scope::empty())
    }

    // implemented by proc macro
    fn animator_cut_scoped(&mut self, cx: &mut Cx, state: &[LiveId; 2], scope: &mut Scope);
    fn animator_play_scoped(&mut self, cx: &mut Cx, state: &[LiveId; 2], scope: &mut Scope);
    fn animator_in_state(&self, cx: &Cx, check_state_pair: &[LiveId; 2]) -> bool;
    fn animator_handle_event_scoped(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
    ) -> AnimatorAction;
}

#[derive(Debug, Clone, Copy)]
pub enum Animate {
    Yes,
    No,
}

/// Container for all states within a state group (e.g., hover: {off, on, drag})
#[derive(Default, Script)]
pub struct AnimatorGroup {
    #[live]
    default: LiveId,
    #[rust]
    states: LiveIdMap<LiveId, AnimatorState>,
}

impl ScriptHook for AnimatorGroup {
    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        let Some(obj) = value.as_object() else {
            return false;
        };

        vm.map_mut_with(obj, |vm, map| {
            for (key, map_value) in map.iter() {
                if let Some(key_id) = key.as_id() {
                    if key_id == id!(default) {
                        if let Some(default_id) = map_value.value.as_id() {
                            self.default = default_id;
                        }
                    } else {
                        let state = AnimatorState::script_from_value(vm, map_value.value);
                        self.states.insert(key_id, state);
                    }
                }
            }
        });

        true
    }
}

/// A single animation state (e.g., off, on, drag)
#[derive(Default, Script, ScriptHook)]
pub struct AnimatorState {
    #[live]
    pub cursor: Option<MouseCursor>,
    #[live]
    pub ease: Option<Ease>,
    #[live]
    pub from: LiveIdMap<LiveId, Play>,
    #[live]
    pub apply: Option<ScriptObject>,
    #[live]
    pub redraw: bool,
}

/// Runtime state for a single animation track
#[derive(Clone)]
struct AnimatorTrack {
    /// The state group this track belongs to (e.g., "hover")
    group_id: LiveId,
    /// The current state id (e.g., "off", "on", "drag")
    state_id: LiveId,
    /// When the animation started
    start_time: f64,
    /// The Play mode for this animation
    play: Play,
    /// The ease function
    ease: Ease,
    /// The target apply object (what we're animating to)
    target_apply: ScriptObject,
    /// The starting values SNAPSHOT (captured/copied when animation begins)
    /// This is a SEPARATE object from state_object - it must not be mutated during animation
    from_snapshot: ScriptObject,
    /// Whether this track needs redraw
    redraw: bool,
}

#[derive(Default, Script)]
pub struct Animator {
    #[rust]
    pub is_defined: bool,
    #[rust]
    pub next_frame: NextFrame,
    #[rust]
    pub groups: LiveIdMap<LiveId, AnimatorGroup>,
    /// Runtime: Current state for each state group
    #[rust]
    current_states: LiveIdMap<LiveId, LiveId>,
    /// Runtime: Active animation tracks (one per state group that's animating)
    #[rust]
    tracks: Vec<AnimatorTrack>,
    /// Runtime: The shared state object containing current values from ALL animation groups
    /// This is the source of truth for "from" values when starting new animations
    #[rust]
    state_object: Option<ScriptObject>,
}

impl ScriptHook for Animator {
    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        let Some(obj) = value.as_object() else {
            return false;
        };

        vm.map_mut_with(obj, |vm, map| {
            for (key, map_value) in map.iter() {
                if let Some(group_id) = key.as_id() {
                    let group = AnimatorGroup::script_from_value(vm, map_value.value);
                    self.groups.insert(group_id, group);
                }
            }
        });
        self.is_defined = true;
        true
    }
}

impl ScriptApplyDefault for Animator {
    fn script_apply_default(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) -> Option<ScriptValue> {
        if !apply.is_new() {
            return None;
        }
        let index = apply.as_default().map_or(0, |x| x + 1);
        let (_, group) = self.groups.iter().nth(index)?;
        let state = group.states.get(&group.default)?;

        // Return the apply value from that state
        let apply = state.apply?.into();
        Some(apply)
    }
}

#[derive(Copy, Clone)]
pub enum AnimatorAction {
    Animating { redraw: bool },
    None,
}

impl AnimatorAction {
    pub fn must_redraw(&self) -> bool {
        match self {
            Self::Animating { redraw } => *redraw,
            Self::None => false,
        }
    }
}

impl Animator {
    /// Start animating to a new state
    pub fn play(&mut self, cx: &mut Cx, state: &[LiveId; 2]) -> Option<ScriptValue> {
        let group_id = state[0];
        let target_state_id = state[1];

        // Get the state group
        let group = self.groups.get(&group_id)?;

        // Get the target state
        let target_state = group.states.get(&target_state_id)?;

        // Get the apply object
        let target_apply = target_state.apply?;

        // Find existing track for this group (if any)
        let existing_track_idx = self.tracks.iter().position(|t| t.group_id == group_id);

        // Get the current state for this group (from current_states or default)
        let current_state_id = self
            .current_states
            .get(&group_id)
            .copied()
            .unwrap_or(group.default);

        // Determine from_state_id for Play mode lookup:
        // If there's an active track, we're coming from that track's target state
        let from_state_id = existing_track_idx
            .map(|idx| self.tracks[idx].state_id)
            .unwrap_or(current_state_id);

        // If we're already in this state and not animating, do nothing
        if current_state_id == target_state_id && existing_track_idx.is_none() {
            return None;
        }

        // Determine the Play mode from the target state's `from` map
        let play = target_state
            .from
            .get(&from_state_id)
            .or_else(|| target_state.from.get(&id!(all)))
            .copied()
            .unwrap_or(Play::Forward { duration: 0.3 });

        // Get the ease from the target state, default to Linear
        let ease = target_state.ease.unwrap_or(Ease::Linear);

        // Set cursor if specified
        if let Some(cursor) = target_state.cursor {
            cx.set_cursor(cursor);
        }

        // For snap, we don't animate - just apply immediately
        if matches!(play, Play::Snap) {
            // Remove any existing track for this group
            self.tracks.retain(|t| t.group_id != group_id);
            // Update current state
            self.current_states.insert(group_id, target_state_id);
            // Merge target values into state_object
            cx.with_vm(|vm| {
                let state_obj = self
                    .state_object
                    .get_or_insert_with(|| vm.bx.heap.new_object());
                Self::merge_object(vm, *state_obj, target_apply);
            });
            // Request next frame so the apply gets processed
            self.next_frame = cx.new_next_frame();
            // Return the apply object directly
            return Some(target_apply.into());
        }

        // Remove any existing track for this group
        self.tracks.retain(|t| t.group_id != group_id);

        // Create a SNAPSHOT of the current "from" values at animation START.
        // This is the only place we allocate objects for from_snapshot - it happens
        // once per play() call, NOT during animation frames.
        // The snapshot must be a separate object that won't be mutated during animation.
        // We sample from state_object (current animated values) or fall back to static state apply.
        let from_snapshot = cx.with_vm(|vm| {
            let snapshot = vm.bx.heap.new_object();

            // Get the default state's apply for fallback values
            let default_state_id = group.default;
            let default_apply = group.states.get(&default_state_id).and_then(|s| s.apply);

            // For each key in target_apply, get the "from" value:
            // 1. First try state_object (current animated values)
            // 2. Fall back to default state's apply if not in state_object
            vm.map_mut_with(target_apply, |vm, target_map| {
                for (key, _) in target_map.iter() {
                    // Try state_object first, fall back to default_apply
                    let from_val = if let Some(state_obj) = self.state_object {
                        let v = vm.bx.heap.value(state_obj, *key, NoTrap);
                        if !v.is_nil() && !v.is_err() {
                            v
                        } else if let Some(default_obj) = default_apply {
                            vm.bx.heap.value(default_obj, *key, NoTrap)
                        } else {
                            ScriptValue::NIL
                        }
                    } else if let Some(default_obj) = default_apply {
                        vm.bx.heap.value(default_obj, *key, NoTrap)
                    } else {
                        ScriptValue::NIL
                    };

                    if !from_val.is_nil() && !from_val.is_err() {
                        // Deep copy if it's an object
                        if let Some(from_obj) = from_val.as_object() {
                            let new_obj = vm.bx.heap.new_object();
                            Self::deep_copy_object(vm, new_obj, from_obj);
                            vm.bx.heap.set_value_def(snapshot, *key, new_obj.into());
                        } else {
                            vm.bx.heap.set_value_def(snapshot, *key, from_val);
                        }
                    }
                }
            });

            snapshot
        });

        // Create new track
        // Use NEG_INFINITY as marker - will be set to actual time on first NextFrame
        let track = AnimatorTrack {
            group_id,
            state_id: target_state_id,
            start_time: f64::NEG_INFINITY,
            play,
            ease,
            target_apply,
            from_snapshot,
            redraw: target_state.redraw,
        };

        self.tracks.push(track);

        // Request next frame
        self.next_frame = cx.new_next_frame();

        // Return the snapshot (from values at t=0)
        Some(from_snapshot.into())
    }

    /// Immediately cut to a state without animation
    pub fn cut(&mut self, cx: &mut Cx, state: &[LiveId; 2]) -> Option<ScriptValue> {
        let group_id = state[0];
        let target_state_id = state[1];

        // Get the state group
        let group = self.groups.get(&group_id)?;

        // Get the target state
        let target_state = group.states.get(&target_state_id)?;

        // Get the apply object
        let target_apply = target_state.apply?;

        // Set cursor if specified
        if let Some(cursor) = target_state.cursor {
            cx.set_cursor(cursor);
        }

        // Remove any existing track for this group
        self.tracks.retain(|t| t.group_id != group_id);

        // Update current state
        self.current_states.insert(group_id, target_state_id);

        // Merge target values into state_object
        cx.with_vm(|vm| {
            let state_obj = self
                .state_object
                .get_or_insert_with(|| vm.bx.heap.new_object());
            Self::merge_object(vm, *state_obj, target_apply);
        });

        // Return the apply object directly
        Some(target_apply.into())
    }

    /// Check if the animator is in a specific state
    pub fn in_state(&self, _cx: &Cx, state: &[LiveId; 2]) -> bool {
        let group_id = state[0];
        let state_id = state[1];

        // If there's an active track for this group, check its target state
        // This matches the old animator behavior where in_state returns true
        // for the state we're animating TO, not the state we came FROM
        if let Some(track) = self.tracks.iter().find(|t| t.group_id == group_id) {
            return track.state_id == state_id;
        }

        // Check current state (only set after animation completes)
        if let Some(current) = self.current_states.get(&group_id) {
            return *current == state_id;
        }

        // Fall back to default
        if let Some(group) = self.groups.get(&group_id) {
            return group.default == state_id;
        }

        false
    }

    /// Check if any animation tracks are currently active
    pub fn is_animating(&self) -> bool {
        !self.tracks.is_empty()
    }

    /// Check if a specific track (state group) is currently animating
    pub fn is_track_animating(&self, track_id: LiveId) -> bool {
        self.tracks.iter().any(|t| t.group_id == track_id)
    }

    /// Handle animation events (NextFrame)
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        act: &mut AnimatorAction,
    ) -> Option<ScriptValue> {
        if let Event::NextFrame(nf) = event {
            if !nf.set.contains(&self.next_frame) {
                return None;
            }

            if self.tracks.is_empty() {
                return None;
            }

            let current_time = nf.time;

            // Initialize start_time for tracks that just started
            for track in &mut self.tracks {
                if track.start_time == f64::NEG_INFINITY {
                    track.start_time = current_time;
                }
            }

            let mut any_animating = false;
            let mut any_redraw = false;

            // Process tracks and interpolate into the shared state_object.
            // NOTE: After the first frame, no new objects should be allocated here.
            // The state_object structure is populated on first frame and reused thereafter.
            let result = cx.with_vm(|vm| {
                // Get or create the shared state object (created once, reused forever)
                let state_obj = self
                    .state_object
                    .get_or_insert_with(|| vm.bx.heap.new_object());
                let state_obj = *state_obj;

                for track in &self.tracks {
                    let elapsed = current_time - track.start_time;
                    let (ended, time) = track.play.get_ended_time(elapsed);
                    let mix = if ended { 1.0 } else { track.ease.map(time) };

                    if !ended {
                        any_animating = true;
                    }
                    if track.redraw {
                        any_redraw = true;
                    }

                    // Interpolate from the snapshot (frozen, read-only) to target (read-only),
                    // writing results into state_obj. Reuses nested objects in state_obj.
                    Self::interpolate_object(
                        vm,
                        state_obj,
                        track.from_snapshot,
                        track.target_apply,
                        mix,
                    );
                }
                state_obj
            });

            // Update state and remove ended tracks
            self.tracks.retain(|track| {
                let elapsed = current_time - track.start_time;
                let (ended, _) = track.play.get_ended_time(elapsed);
                if ended {
                    self.current_states.insert(track.group_id, track.state_id);
                    false
                } else {
                    true
                }
            });

            // Request next frame if still animating
            if any_animating {
                self.next_frame = cx.new_next_frame();
            }

            *act = AnimatorAction::Animating { redraw: any_redraw };

            return Some(result.into());
        }
        None
    }

    /// Recursively interpolate between two objects, writing results into `result`.
    ///
    /// IMPORTANT: This function reuses nested objects from `result` (state_object) to avoid
    /// allocating new objects on every animation frame. After the first frame populates
    /// the structure, subsequent frames reuse the same nested objects.
    ///
    /// - `result`: The state_object we write interpolated values into (reuses nested objects)
    /// - `from`: The from_snapshot (read-only, frozen at animation start)
    /// - `to`: The target_apply template (read-only, never mutated)
    fn interpolate_object(
        vm: &mut ScriptVm,
        result: ScriptObject,
        from: ScriptObject,
        to: ScriptObject,
        mix: f64,
    ) {
        // Iterate over the 'to' object's properties (read-only)
        vm.map_mut_with(to, |vm, to_map| {
            for (key, to_value) in to_map.iter() {
                let to_val = to_value.value;

                // Read from 'from' snapshot (never mutated)
                let from_val = vm.bx.heap.value(from, *key, NoTrap);

                // Get existing value at this key in result for reusing nested objects
                // After first frame, this should always find existing objects
                let existing = vm.bx.heap.value(result, *key, NoTrap);

                // Interpolate - reuses `existing` nested objects from result
                let interpolated = Self::interpolate_value(vm, from_val, to_val, mix, existing);

                // Write to result (state_object)
                vm.bx.heap.set_value_def(result, *key, interpolated);
            }
        });
    }

    /// Merge source object's values into target object (used for snap/cut operations)
    /// IMPORTANT: This deep-copies objects from source to avoid mutating templates
    fn merge_object(vm: &mut ScriptVm, target: ScriptObject, source: ScriptObject) {
        vm.map_mut_with(source, |vm, source_map| {
            for (key, source_value) in source_map.iter() {
                let source_val = source_value.value;

                // Get existing value at this key in target
                let existing = vm.bx.heap.value(target, *key, NoTrap);

                if let Some(source_obj) = source_val.as_object() {
                    // Source is an object - we need to handle it carefully
                    if let Some(existing_obj) = existing.as_object() {
                        // Both exist as objects - recursively merge into existing
                        Self::merge_object(vm, existing_obj, source_obj);
                    } else {
                        // Target doesn't have this as object yet - create a COPY, don't reference
                        let new_obj = vm.bx.heap.new_object();
                        Self::deep_copy_object(vm, new_obj, source_obj);
                        vm.bx.heap.set_value_def(target, *key, new_obj.into());
                    }
                } else {
                    // Primitive value - just copy it directly (primitives are value types)
                    vm.bx.heap.set_value_def(target, *key, source_val);
                }
            }
        });
    }

    /// Deep copy all values from source object to dest object
    /// This ensures we never share object references with templates
    fn deep_copy_object(vm: &mut ScriptVm, dest: ScriptObject, source: ScriptObject) {
        vm.map_mut_with(source, |vm, source_map| {
            for (key, source_value) in source_map.iter() {
                let source_val = source_value.value;

                if let Some(source_obj) = source_val.as_object() {
                    // Recursively copy nested objects
                    let new_obj = vm.bx.heap.new_object();
                    Self::deep_copy_object(vm, new_obj, source_obj);
                    vm.bx.heap.set_value_def(dest, *key, new_obj.into());
                } else {
                    // Primitive - copy directly
                    vm.bx.heap.set_value_def(dest, *key, source_val);
                }
            }
        });
    }

    /// Interpolate between two ScriptValues
    /// `existing` is the current value at this key in the result object (for reusing nested objects)
    fn interpolate_value(
        vm: &mut ScriptVm,
        from: ScriptValue,
        to: ScriptValue,
        mix: f64,
        existing: ScriptValue,
    ) -> ScriptValue {
        // If from is NIL or error (no starting value), just return target value
        // This happens when state_object doesn't have a value for this key yet
        // (e.g., hover state exists but opened state hasn't been animated before)
        if from.is_nil() || from.is_err() {
            return to;
        }

        // Handle apply_transform objects (snap and timeline)
        if let Some(to_obj) = to.as_object() {
            if vm.bx.heap.has_apply_transform(to) {
                // Check if it's a timeline object (has keyframes property)
                let keyframes = vm.bx.heap.value(to_obj, id!(keyframes).into(), NoTrap);
                if let Some(kf_obj) = keyframes.as_object() {
                    return Self::interpolate_timeline(vm, from, kf_obj, mix, existing);
                }
                // Otherwise it's a snap object - return the stored value
                return vm.bx.heap.value(to_obj, id!(value).into(), NoTrap);
            }

            // If it's an object, recursively interpolate
            if let Some(from_obj) = from.as_object() {
                // Reuse existing nested object from state_object if available.
                // Only creates new object on first frame when state_object structure is empty.
                // After first frame, `existing` will always be an object and we reuse it.
                let result_obj = existing
                    .as_object()
                    .unwrap_or_else(|| vm.bx.heap.new_object());
                Self::interpolate_object(vm, result_obj, from_obj, to_obj, mix);
                return result_obj.into();
            } else {
                // Can't interpolate different types, return 'to' at mix >= 0.5
                return if mix >= 0.5 { to } else { from };
            }
        }

        // Numbers (f64)
        if let (Some(from_f), Some(to_f)) = (from.as_number(), to.as_number()) {
            let result = from_f + (to_f - from_f) * mix;
            return ScriptValue::from_f64(result);
        }

        // Colors
        if let (Some(from_c), Some(to_c)) = (from.as_color(), to.as_color()) {
            let from_vec = Vec4f::from_u32(from_c);
            let to_vec = Vec4f::from_u32(to_c);
            let mix_f = mix as f32;
            let result = Vec4f {
                x: from_vec.x + (to_vec.x - from_vec.x) * mix_f,
                y: from_vec.y + (to_vec.y - from_vec.y) * mix_f,
                z: from_vec.z + (to_vec.z - from_vec.z) * mix_f,
                w: from_vec.w + (to_vec.w - from_vec.w) * mix_f,
            };
            return ScriptValue::from_color(result.to_u32());
        }

        // Pods (vectors)
        if let (Some(from_pod), Some(to_pod)) = (from.as_pod(), to.as_pod()) {
            return Self::interpolate_pod(vm, from_pod, to_pod, mix);
        }

        // Booleans - snap at 0.5
        if from.is_bool() || to.is_bool() {
            return if mix >= 0.5 { to } else { from };
        }

        // IDs - snap at 0.5
        if from.is_id() || to.is_id() {
            return if mix >= 0.5 { to } else { from };
        }

        // Default: return 'to' at mix >= 0.5
        if mix >= 0.5 {
            to
        } else {
            from
        }
    }

    /// Interpolate between two pod values (vectors)
    /// For now, pods snap at mix >= 0.5 since creating interpolated pods requires
    /// internal APIs. Most animation use cases are for f32/f64/colors which are handled above.
    fn interpolate_pod(
        _vm: &mut ScriptVm,
        from: ScriptPod,
        to: ScriptPod,
        mix: f64,
    ) -> ScriptValue {
        // Pods (vectors, matrices, etc) snap at 0.5
        // The common animation case is f32/f64 numbers and colors, which are handled separately
        if mix >= 0.5 {
            to.into()
        } else {
            from.into()
        }
    }

    /// Interpolate using a timeline of keyframes
    /// keyframes is an object with vec storage containing pairs [t0, v0, t1, v1, ...]
    /// `from` is the current value to use if timeline doesn't start at time 0
    /// Assumes keyframes are already sorted by time
    fn interpolate_timeline(
        vm: &mut ScriptVm,
        from: ScriptValue,
        keyframes: ScriptObject,
        mix: f64,
        existing: ScriptValue,
    ) -> ScriptValue {
        let len = vm.bx.heap.vec_len(keyframes);
        let num_pairs = len / 2;

        if num_pairs == 0 {
            return from;
        }

        // Get first keyframe
        let first_time = vm
            .bx
            .heap
            .vec_value(keyframes, 0, NoTrap)
            .as_f64()
            .unwrap_or(0.0);
        let first_value = vm.bx.heap.vec_value(keyframes, 1, NoTrap);

        // If mix is before first keyframe, interpolate from `from` to first keyframe value
        if mix <= first_time {
            if first_time <= 0.0 {
                return first_value;
            }
            let local_mix = mix / first_time;
            return Self::interpolate_value(vm, from, first_value, local_mix, existing);
        }

        // Get last keyframe
        let last_time = vm
            .bx
            .heap
            .vec_value(keyframes, (num_pairs - 1) * 2, NoTrap)
            .as_f64()
            .unwrap_or(1.0);
        let last_value = vm
            .bx
            .heap
            .vec_value(keyframes, (num_pairs - 1) * 2 + 1, NoTrap);

        // If mix is at or after last keyframe, return last value
        if mix >= last_time {
            return last_value;
        }

        // Find the two keyframes that bracket mix
        let mut t0 = first_time;
        let mut v0 = first_value;

        for i in 1..num_pairs {
            let t1 = vm
                .bx
                .heap
                .vec_value(keyframes, i * 2, NoTrap)
                .as_f64()
                .unwrap_or(1.0);
            let v1 = vm.bx.heap.vec_value(keyframes, i * 2 + 1, NoTrap);

            if mix <= t1 {
                // Interpolate between t0,v0 and t1,v1
                let segment_duration = t1 - t0;
                if segment_duration <= 0.0 {
                    return v1;
                }
                let local_mix = (mix - t0) / segment_duration;
                return Self::interpolate_value(vm, v0, v1, local_mix, existing);
            }

            t0 = t1;
            v0 = v1;
        }

        // Fallback
        last_value
    }
}

// deserialisable DSL structure
#[derive(Debug, Clone, Script, ScriptHook)]
pub struct KeyFrame {
    #[live(Ease::Linear)]
    pub ease: Ease,

    #[live(1.0)]
    pub time: f64,

    #[live(NIL)]
    pub value: ScriptValue,
}

#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook)]
pub enum Play {
    #[pick {duration: 1.0}]
    Forward {
        duration: f64,
    },

    Snap,

    #[live {duration: 1.0, end: 1.0}]
    Reverse {
        duration: f64,
        end: f64,
    },

    #[live {duration: 1.0, end: 1.0}]
    Loop {
        duration: f64,
        end: f64,
    },

    #[live {duration: 1.0, end: 1.0}]
    ReverseLoop {
        duration: f64,
        end: f64,
    },

    #[live {duration: 1.0, end: 1.0}]
    BounceLoop {
        duration: f64,
        end: f64,
    },
}

impl Play {
    /*
    pub fn duration(&self) -> f64 {
        match self {
            Self::Forward {duration, ..} => *duration,
            Self::Reverse {duration, ..} => *duration,
            Self::Loop {duration, ..} => *duration,
            Self::ReverseLoop {duration, ..} => *duration,
            Self::BounceLoop {duration, ..} => *duration,
        }
    }*/

    pub fn get_ended_time(&self, time: f64) -> (bool, f64) {
        match self {
            Self::Snap => (true, 1.0),
            Self::Forward { duration } => {
                if *duration == 0.0 {
                    return (true, 1.0);
                }
                (time > *duration, time.min(*duration) / duration)
            }
            Self::Reverse { duration, end } => {
                if *duration == 0.0 {
                    return (true, 1.0);
                }
                (time > *duration, end - (time.min(*duration) / duration))
            }
            Self::Loop { duration, end } => {
                if *duration == 0.0 {
                    return (true, 1.0);
                }
                (false, (time / duration) % end)
            }
            Self::ReverseLoop { end, duration } => {
                if *duration == 0.0 {
                    return (true, 1.0);
                }
                (false, end - (time / duration) % end)
            }
            Self::BounceLoop { end, duration } => {
                if *duration == 0.0 {
                    return (true, 1.0);
                }
                let mut local_time = (time / duration) % (end * 2.0);
                if local_time > *end {
                    local_time = 2.0 * end - local_time;
                };
                (false, local_time)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
pub enum Ease {
    #[pick]
    Linear,
    #[live]
    None,
    #[live(1.0)]
    Constant(f64),
    #[live]
    InQuad,
    #[live]
    OutQuad,
    #[live]
    InOutQuad,
    #[live]
    InCubic,
    #[live]
    OutCubic,
    #[live]
    InOutCubic,
    #[live]
    InQuart,
    #[live]
    OutQuart,
    #[live]
    InOutQuart,
    #[live]
    InQuint,
    #[live]
    OutQuint,
    #[live]
    InOutQuint,
    #[live]
    InSine,
    #[live]
    OutSine,
    #[live]
    InOutSine,
    #[live]
    InExp,
    #[live]
    OutExp,
    #[live]
    InOutExp,
    #[live]
    InCirc,
    #[live]
    OutCirc,
    #[live]
    InOutCirc,
    #[live]
    InElastic,
    #[live]
    OutElastic,
    #[live]
    InOutElastic,
    #[live]
    InBack,
    #[live]
    OutBack,
    #[live]
    InOutBack,
    #[live]
    InBounce,
    #[live]
    OutBounce,
    #[live]
    InOutBounce,
    #[live {d1: 0.82, d2: 0.97, max: 100}]
    ExpDecay { d1: f64, d2: f64, max: usize },

    #[live {begin: 0.0, end: 1.0}]
    Pow { begin: f64, end: f64 },
    #[live {cp0: 0.0, cp1: 0.0, cp2: 1.0, cp3: 1.0}]
    Bezier {
        cp0: f64,
        cp1: f64,
        cp2: f64,
        cp3: f64,
    },
}

impl Ease {
    pub fn map(&self, t: f64) -> f64 {
        match self {
            Self::ExpDecay { d1, d2, max } => {
                // there must be a closed form for this
                if t > 0.999 {
                    return 1.0;
                }

                // first we count the number of steps we'd need to decay
                let mut di = *d1;
                let mut dt = 1.0;
                let max_steps = (*max).min(1000);
                let mut steps = 0;
                // for most of the settings we use this takes max 15 steps or so
                while dt > 0.001 && steps < max_steps {
                    steps = steps + 1;
                    dt = dt * di;
                    di *= d2;
                }
                // then we know how to find the step, and lerp it
                let step = t * (steps as f64);
                let mut di = *d1;
                let mut dt = 1.0;
                let max_steps = max_steps as f64;
                let mut steps = 0.0;
                while dt > 0.001 && steps < max_steps {
                    steps += 1.0;
                    if steps >= step {
                        // right step
                        let fac = steps - step;
                        return 1.0 - (dt * fac + (dt * di) * (1.0 - fac));
                    }
                    dt = dt * di;
                    di *= d2;
                }
                1.0
            }
            Self::Linear => {
                return t.max(0.0).min(1.0);
            }
            Self::Constant(t) => {
                return t.max(0.0).min(1.0);
            }
            Self::None => {
                return 1.0;
            }
            Self::Pow { begin, end } => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }
                let a = -1. / (begin * begin).max(1.0);
                let b = 1. + 1. / (end * end).max(1.0);
                let t2 = (((a - 1.) * -b) / (a * (1. - b))).powf(t);
                return (-a * b + b * a * t2) / (a * t2 - b);
            }

            Self::InQuad => {
                return t * t;
            }
            Self::OutQuad => {
                return t * (2.0 - t);
            }
            Self::InOutQuad => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t;
                } else {
                    let t = t - 1.;
                    return -0.5 * (t * (t - 2.) - 1.);
                }
            }
            Self::InCubic => {
                return t * t * t;
            }
            Self::OutCubic => {
                let t2 = t - 1.0;
                return t2 * t2 * t2 + 1.0;
            }
            Self::InOutCubic => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t;
                } else {
                    let t = t - 2.;
                    return 1. / 2. * (t * t * t + 2.);
                }
            }
            Self::InQuart => return t * t * t * t,
            Self::OutQuart => {
                let t = t - 1.;
                return -(t * t * t * t - 1.);
            }
            Self::InOutQuart => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t;
                } else {
                    let t = t - 2.;
                    return -0.5 * (t * t * t * t - 2.);
                }
            }
            Self::InQuint => {
                return t * t * t * t * t;
            }
            Self::OutQuint => {
                let t = t - 1.;
                return t * t * t * t * t + 1.;
            }
            Self::InOutQuint => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t * t;
                } else {
                    let t = t - 2.;
                    return 0.5 * (t * t * t * t * t + 2.);
                }
            }
            Self::InSine => {
                return -(t * PI * 0.5).cos() + 1.;
            }
            Self::OutSine => {
                return (t * PI * 0.5).sin();
            }
            Self::InOutSine => {
                return -0.5 * ((t * PI).cos() - 1.);
            }
            Self::InExp => {
                if t < 0.001 {
                    return 0.;
                } else {
                    return 2.0f64.powf(10. * (t - 1.));
                }
            }
            Self::OutExp => {
                if t > 0.999 {
                    return 1.;
                } else {
                    return -(2.0f64.powf(-10. * t)) + 1.;
                }
            }
            Self::InOutExp => {
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * 2.0f64.powf(10. * (t - 1.));
                } else {
                    let t = t - 1.;
                    return 0.5 * (-(2.0f64.powf(-10. * t)) + 2.);
                }
            }
            Self::InCirc => {
                return -((1. - t * t).sqrt() - 1.);
            }
            Self::OutCirc => {
                let t = t - 1.;
                return (1. - t * t).sqrt();
            }
            Self::InOutCirc => {
                let t = t * 2.;
                if t < 1. {
                    return -0.5 * ((1. - t * t).sqrt() - 1.);
                } else {
                    let t = t - 2.;
                    return 0.5 * ((1. - t * t).sqrt() + 1.);
                }
            }
            Self::InElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t - 1.0;
                return -(2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
            }
            Self::OutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0

                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                return 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
            }
            Self::InOutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    let t = t - 1.0;
                    return -0.5 * (2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
                } else {
                    let t = t - 1.0;
                    return 0.5 * 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
                }
            }
            Self::InBack => {
                let s = 1.70158;
                return t * t * ((s + 1.) * t - s);
            }
            Self::OutBack => {
                let s = 1.70158;
                let t = t - 1.;
                return t * t * ((s + 1.) * t + s) + 1.;
            }
            Self::InOutBack => {
                let s = 1.70158;
                let t = t * 2.0;
                if t < 1. {
                    let s = s * 1.525;
                    return 0.5 * (t * t * ((s + 1.) * t - s));
                } else {
                    let t = t - 2.;
                    return 0.5 * (t * t * ((s + 1.) * t + s) + 2.);
                }
            }
            Self::InBounce => {
                return 1.0 - Self::OutBounce.map(1.0 - t);
            }
            Self::OutBounce => {
                if t < (1. / 2.75) {
                    return 7.5625 * t * t;
                }
                if t < (2. / 2.75) {
                    let t = t - (1.5 / 2.75);
                    return 7.5625 * t * t + 0.75;
                }
                if t < (2.5 / 2.75) {
                    let t = t - (2.25 / 2.75);
                    return 7.5625 * t * t + 0.9375;
                }
                let t = t - (2.625 / 2.75);
                return 7.5625 * t * t + 0.984375;
            }
            Self::InOutBounce => {
                if t < 0.5 {
                    return Self::InBounce.map(t * 2.) * 0.5;
                } else {
                    return Self::OutBounce.map(t * 2. - 1.) * 0.5 + 0.5;
                }
            }
            Self::Bezier { cp0, cp1, cp2, cp3 } => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }

                if (cp0 - cp1).abs() < 0.001 && (cp2 - cp3).abs() < 0.001 {
                    return t;
                }

                let epsilon = 1.0 / 200.0 * t;
                let cx = 3.0 * cp0;
                let bx = 3.0 * (cp2 - cp0) - cx;
                let ax = 1.0 - cx - bx;
                let cy = 3.0 * cp1;
                let by = 3.0 * (cp3 - cp1) - cy;
                let ay = 1.0 - cy - by;
                let mut u = t;

                for _i in 0..6 {
                    let x = ((ax * u + bx) * u + cx) * u - t;
                    if x.abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }
                    let d = (3.0 * ax * u + 2.0 * bx) * u + cx;
                    if d.abs() < 1e-6 {
                        break;
                    }
                    u = u - x / d;
                }

                if t > 1. {
                    return (ay + by) + cy;
                }
                if t < 0. {
                    return 0.0;
                }

                let mut w = 0.0;
                let mut v = 1.0;
                u = t;
                for _i in 0..8 {
                    let x = ((ax * u + bx) * u + cx) * u;
                    if (x - t).abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }

                    if t > x {
                        w = u;
                    } else {
                        v = u;
                    }
                    u = (v - w) * 0.5 + w;
                }

                return ((ay * u + by) * u + cy) * u;
            }
        }
    }
}
