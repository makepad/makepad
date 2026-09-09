//! MIDI LEARN — map any learnable dial/slider to any MIDI CC, live.
//!
//! Two entry paths, one state machine:
//!   * ALT-CLICK a learnable control — it arms directly;
//!   * the global LEARN button — enters PICK mode, where every learnable
//!     control shows a hint outline and the next click on one arms it.
//! An ARMED control pulses until the next CC arrives on any port, which
//! binds it: from then on that (channel, cc) drives the control. Re-arm to
//! re-learn; ALT-CLICK an armed control again to CLEAR its binding; Esc (or
//! LEARN again) backs out of pick/armed. Bindings persist gen-panel style
//! (`midi-map.txt`: `control channel cc` lines) and restore at boot. A
//! learned CC OVERRIDES any hardwired APC meaning for that message —
//! per-control, everything else on the hardware keeps working.
//!
//! Two pieces:
//!   * [`MidiLearn`] — the pure, tested state machine (no Cx anywhere).
//!   * [`VjLearnWrap`] — the wrapper WIDGET: host any slider/dial inside it
//!     and it becomes learnable — alt-click hit handling that leaves the
//!     inner control's normal drag alone, the LEARN-MODE OUTLINE (hint +
//!     pulsing armed ring, unmistakable at stage distance), and a small
//!     mapped tick when bound. Wrap a control, add one row to the app's
//!     learnable table — nothing else.

use crate::midi_binding::{Behaviour, Binding, Motion, Source, Transform};
use makepad_widgets::*;
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The learn overlay: hint outline (pick mode), pulsing armed ring, and
    // the mapped tick. Drawn OVER the wrapped control only while a learn
    // state is active (so it never steals clicks at rest).
    set_type_default() do #(DrawLearnRing::script_shader(vm)){
        ..mod.draw.DrawQuad

        pixel: fn() {
            let w = self.rect_size.x
            let h = self.rect_size.y
            let p = self.pos * self.rect_size
            let sdf = Sdf2d.viewport(p)
            let acc = vec4(0.243, 0.878, 0.690, 1.0)
            let r = 6.0
            // Armed: a bright pulsing double ring nobody can miss.
            if self.mode > 1.5 {
                let pulse = 0.55 + 0.45 * sin(self.t * 7.0)
                sdf.box(1.0, 1.0, w - 2.0, h - 2.0, r)
                sdf.stroke(vec4(acc.x, acc.y, acc.z, pulse), 2.5)
                sdf.box(4.5, 4.5, w - 9.0, h - 9.0, r - 3.0)
                sdf.stroke(vec4(1.0, 1.0, 1.0, pulse * 0.55), 1.0)
                return sdf.result
            }
            // Pick-mode hint: a soft steady outline saying "clickable".
            sdf.box(1.0, 1.0, w - 2.0, h - 2.0, r)
            sdf.stroke(vec4(acc.x, acc.y, acc.z, 0.45), 1.5)
            return sdf.result
        }
    }

    // The mapped tick: a small accent square, corner-sized.
    set_type_default() do #(DrawLearnTick::script_shader(vm)){
        ..mod.draw.DrawQuad

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.4)
            sdf.fill(vec4(0.243, 0.878, 0.690, 0.9))
            return sdf.result
        }
    }

    mod.widgets.VjLearnWrapBase = #(VjLearnWrap::register_widget(vm))
    mod.widgets.Learn = set_type_default() do mod.widgets.VjLearnWrapBase{
        width: Fit
        height: Fit
    }
}

// ---------------------------------------------------------------------------
// the pure state machine
// ---------------------------------------------------------------------------

/// What a MIDI message meant to the learn layer.
#[derive(Clone, Debug, PartialEq)]
pub enum LearnEvent {
    /// The armed control just bound to this CC.
    Bound { control: String, channel: u8, cc: u8 },
    /// A bound control's new value, 0..1, and the CC it came from.
    ///
    /// The source travels with the value because a button's edge detector
    /// has to know whether the level it remembers was set by the control
    /// that is speaking now. Re-learn a pad onto a different button and the
    /// old level is not a previous state, it is a different pad.
    Value { control: String, channel: u8, cc: u8, value: f32 },
    /// A bound control's turn -- from an encoder that sends changes rather
    /// than positions -- in 0..1 units, signed. The caller knows where the
    /// control is; this layer does not.
    Turn { control: String, channel: u8, cc: u8, delta: f32 },
}

#[derive(Default)]
pub struct MidiLearn {
    /// PICK mode: the next click on a learnable control arms it.
    pub picking: bool,
    /// The control waiting for its CC.
    pub armed: Option<String>,
    /// Each learned control's record: its source, and what it makes of
    /// what arrives (midi_binding.rs).
    bindings: HashMap<String, Binding>,
    reverse: HashMap<Source, String>,
}

impl MidiLearn {
    /// The global LEARN button: enter pick mode, or back out of whatever
    /// learn state is active. Returns whether learn is now active.
    pub fn toggle_pick(&mut self) -> bool {
        if self.picking || self.armed.is_some() {
            self.picking = false;
            self.armed = None;
            false
        } else {
            self.picking = true;
            true
        }
    }

    /// Esc: back out of pick/armed (bindings untouched).
    pub fn escape(&mut self) -> bool {
        let was = self.picking || self.armed.is_some();
        self.picking = false;
        self.armed = None;
        was
    }

    pub fn active(&self) -> bool {
        self.picking || self.armed.is_some()
    }

    /// A learnable control was clicked. In pick mode (any click) or on an
    /// alt-click, that arms it; an ALT-CLICK on the control already armed
    /// CLEARS its binding and disarms. Returns whether the click belonged
    /// to the learn layer.
    pub fn control_clicked(&mut self, control: &str, alt: bool) -> bool {
        if self.armed.as_deref() == Some(control) {
            // Second touch on the armed control: alt clears the binding,
            // a plain pick-click just disarms.
            if alt {
                self.clear(control);
            }
            self.armed = None;
            return true;
        }
        if self.picking || alt {
            self.picking = false;
            self.armed = Some(control.to_string());
            return true;
        }
        false
    }

    pub fn is_armed(&self, control: &str) -> bool {
        self.armed.as_deref() == Some(control)
    }

    pub fn is_bound(&self, control: &str) -> bool {
        self.bindings.contains_key(control)
    }

    /// The source a control is driven from.
    pub fn binding(&self, control: &str) -> Option<(u8, u8)> {
        self.bindings.get(control).map(|binding| binding.source)
    }

    /// The whole record: the source and what the control makes of it.
    pub fn record(&self, control: &str) -> Option<Binding> {
        self.bindings.get(control).copied()
    }

    /// How a bound control reads its number. False when it is not bound.
    pub fn set_transform(&mut self, control: &str, transform: Transform) -> bool {
        match self.bindings.get_mut(control) {
            Some(binding) => {
                binding.transform = transform;
                true
            }
            None => false,
        }
    }

    pub fn set_sensitivity(&mut self, control: &str, sensitivity: f32) -> bool {
        match self.bindings.get_mut(control) {
            Some(binding) if sensitivity.is_finite() && sensitivity > 0.0 => {
                binding.sensitivity = sensitivity;
                true
            }
            _ => false,
        }
    }

    /// What a press does on a bound button; None is the button's own.
    pub fn set_press(&mut self, control: &str, press: Option<Behaviour>) -> bool {
        match self.bindings.get_mut(control) {
            Some(binding) => {
                binding.press = press;
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self, control: &str) {
        if let Some(binding) = self.bindings.remove(control) {
            self.reverse.remove(&binding.source);
        }
    }

    /// Drop every binding whose control is not one the app still has.
    ///
    /// The map is a file, and a file outlives the build that wrote it. A
    /// control that has since been renamed or removed leaves a line naming
    /// it, and that line is not merely dead: the reverse map still holds
    /// its CC, so `midi` CLAIMS every message on that CC and the caller
    /// consumes it -- a knob that silently does nothing, on a binding no
    /// surface can show and therefore no gesture can clear.
    ///
    /// Dropped in memory only. The file is left exactly as it was, because
    /// a build that does not know a control today may be an older build
    /// than the one that wrote it, and quietly deleting the newer build's
    /// work would be worse than ignoring it.
    pub fn retain_controls(&mut self, known: impl Fn(&str) -> bool) {
        self.bindings.retain(|control, _| known(control));
        self.reverse.retain(|_, control| known(control));
    }

    /// A raw MIDI message. CC messages bind the armed control or drive a
    /// bound one; everything else is not ours. The caller CONSUMES a
    /// message this returns Some for — a learned CC overrides whatever the
    /// hardwired surface would have done with it.
    pub fn midi(&mut self, data: [u8; 3]) -> Option<LearnEvent> {
        if data[0] & 0xf0 != 0xb0 {
            return None;
        }
        let channel = data[0] & 0x0f;
        let cc = data[1] & 0x7f;
        if let Some(control) = self.armed.take() {
            // Re-learn moves the control's old CC and keeps what the
            // control was told about reading it; stealing a CC another
            // control held moves it here whole (last learn wins).
            let kept = self.bindings.remove(&control);
            if let Some(old) = kept {
                self.reverse.remove(&old.source);
            }
            if let Some(previous) = self.reverse.remove(&(channel, cc)) {
                self.bindings.remove(&previous);
            }
            let binding = Binding { source: (channel, cc), ..kept.unwrap_or(Binding::new((channel, cc))) };
            self.bindings.insert(control.clone(), binding);
            self.reverse.insert((channel, cc), control.clone());
            return Some(LearnEvent::Bound { control, channel, cc });
        }
        let control = self.reverse.get(&(channel, cc))?.clone();
        // The one seam: what the number MEANS is the binding's to say.
        let binding = self.bindings[&control];
        match binding.transform.read(data[2], binding.sensitivity) {
            Motion::At(value) => Some(LearnEvent::Value { control, channel, cc, value }),
            Motion::By(delta) => Some(LearnEvent::Turn { control, channel, cc, delta }),
        }
    }

    /// `midi-map.txt` body: one `control channel cc` line per binding,
    /// followed by the words its record adds -- none at the defaults, so
    /// a binding that was never told anything writes the line it always
    /// wrote. The header stays `v1`: an older build reads the three
    /// tokens and ignores the rest.
    pub fn encode(&self) -> String {
        let mut lines: Vec<String> = self
            .bindings
            .iter()
            .map(|(control, binding)| {
                let (channel, cc) = binding.source;
                let words = binding.words();
                match words.is_empty() {
                    true => format!("{control} {channel} {cc}"),
                    false => format!("{control} {channel} {cc} {words}"),
                }
            })
            .collect();
        lines.sort();
        let mut out = String::from("v1\n");
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    pub fn decode(body: &str) -> MidiLearn {
        let mut out = MidiLearn::default();
        let mut lines = body.lines();
        if lines.next() != Some("v1") {
            return out;
        }
        for line in lines {
            let mut it = line.split_whitespace();
            let (Some(control), Some(channel), Some(cc)) = (it.next(), it.next(), it.next())
            else {
                continue;
            };
            let (Ok(channel), Ok(cc)) = (channel.parse::<u8>(), cc.parse::<u8>()) else {
                continue;
            };
            if channel > 15 || cc > 127 {
                continue;
            }
            let binding = Binding::new((channel, cc)).with_words(it);
            out.bindings.insert(control.to_string(), binding);
            out.reverse.insert((channel, cc), control.to_string());
        }
        out
    }
}

// ---------------------------------------------------------------------------
// the wrapper widget
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum LearnWrapAction {
    #[default]
    None,
    /// The wrapper was clicked as a LEARN gesture (pick-mode click, or an
    /// alt-click in any mode). `alt` distinguishes the clear gesture.
    Clicked { alt: bool },
}

/// Learn-overlay quad. Instance layout law: shader inputs after the deref.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLearnRing {
    #[deref]
    pub draw_super: DrawQuad,
    /// 1 = pick-mode hint, 2 = armed (pulsing).
    #[live]
    pub mode: f32,
    /// Seconds, for the armed pulse.
    #[live]
    pub t: f32,
}

/// Small mapped-indicator quad (drawn at rest, corner-sized so it can never
/// swallow the control's clicks).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLearnTick {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjLearnWrap {
    /// Hosts the wrapped control (children flow straight through, and
    /// `ids!()` lookups resolve through the wrapper unchanged).
    #[deref]
    view: View,
    #[live]
    draw_ring: DrawLearnRing,
    #[live]
    draw_tick: DrawLearnTick,
    #[rust]
    next_frame: NextFrame,
    /// 0 idle, 1 pick-hint, 2 armed — pushed by the app.
    #[rust]
    mode: u8,
    #[rust]
    mapped: bool,
}

impl VjLearnWrap {
    pub fn set_learn_state(&mut self, cx: &mut Cx, mode: u8, mapped: bool) {
        if self.mode == mode && self.mapped == mapped {
            return;
        }
        self.mode = mode;
        self.mapped = mapped;
        if mode == 2 {
            self.next_frame = cx.new_next_frame();
        }
        self.view.redraw(cx);
    }
}

impl Widget for VjLearnWrap {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The armed pulse animates on its own clock.
        if self.next_frame.is_event(event).is_some() && self.mode == 2 {
            self.view.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
        // In hint/armed mode the OVERLAY owns the click (that is the pick
        // gesture); the ring quad is topmost, so its hits arrive here.
        match event.hits(cx, self.draw_ring.area()) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.widget_action(
                    self.widget_uid(),
                    LearnWrapAction::Clicked { alt: fe.modifiers.alt },
                );
                return;
            }
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            _ => {}
        }
        // At rest the inner control owns every hit; an ALT-press inside the
        // rect is the direct arm gesture. (The inner control may see the
        // same press start a drag — releasing without a CC leaves nothing
        // bound, and the wiggle is undone by the next CC anyway.)
        if self.mode == 0 {
            if let Event::MouseDown(e) = event {
                if e.modifiers.alt
                    && self.view.area().rect(cx).contains(e.abs)
                {
                    cx.widget_action(
                        self.widget_uid(),
                        LearnWrapAction::Clicked { alt: true },
                    );
                }
            }
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        if step.is_done() {
            let rect = self.view.area().rect(cx);
            if rect.size.x > 1.0 {
                if self.mode > 0 {
                    self.draw_ring.mode = self.mode as f32;
                    self.draw_ring.t = cx.cx.seconds_since_app_start() as f32;
                    self.draw_ring.draw_abs(cx, rect);
                } else if self.mapped {
                    // A quiet accent tick in the corner: "this one is mapped".
                    self.draw_tick.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(rect.pos.x + rect.size.x - 7.0, rect.pos.y + 1.0),
                            size: dvec2(6.0, 6.0),
                        },
                    );
                }
            }
        }
        step
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(ch: u8, cc_: u8, v: u8) -> [u8; 3] {
        [0xb0 | ch, cc_, v]
    }

    #[test]
    fn pick_mode_arms_on_the_next_click_and_a_cc_binds() {
        let mut m = MidiLearn::default();
        assert!(m.toggle_pick(), "LEARN enters pick mode");
        assert!(m.picking);
        assert!(m.control_clicked("video_fade", false), "pick click arms");
        assert!(!m.picking, "pick mode is one-shot");
        assert!(m.is_armed("video_fade"));
        // The wiggle binds.
        assert_eq!(
            m.midi(cc(2, 48, 64)),
            Some(LearnEvent::Bound { control: "video_fade".into(), channel: 2, cc: 48 })
        );
        assert!(m.is_bound("video_fade"));
        // From now on that CC drives the control.
        assert_eq!(
            m.midi(cc(2, 48, 127)),
            Some(LearnEvent::Value {
                control: "video_fade".into(),
                channel: 2,
                cc: 48,
                value: 1.0
            })
        );
        // Other messages are not ours.
        assert_eq!(m.midi(cc(2, 49, 10)), None);
        assert_eq!(m.midi([0x90, 60, 100]), None, "notes pass through");
    }

    #[test]
    fn alt_click_arms_directly_and_alt_click_again_clears() {
        let mut m = MidiLearn::default();
        assert!(m.control_clicked("xfader", true), "alt-click arms");
        assert!(m.is_armed("xfader"));
        m.midi(cc(0, 7, 0));
        assert!(m.is_bound("xfader"));
        // Re-learn: alt-click again arms again…
        assert!(m.control_clicked("xfader", true));
        // …and alt-clicking the ARMED control clears the binding.
        assert!(m.control_clicked("xfader", true));
        assert!(!m.is_bound("xfader"), "cleared");
        assert!(!m.is_armed("xfader"));
        // A plain click without pick mode is not a learn gesture.
        assert!(!m.control_clicked("xfader", false));
    }

    #[test]
    fn last_learn_wins_conflicts_and_relearn_moves_the_control() {
        let mut m = MidiLearn::default();
        m.control_clicked("a", true);
        m.midi(cc(0, 20, 0));
        m.control_clicked("b", true);
        m.midi(cc(0, 20, 0));
        assert!(m.is_bound("b"), "b stole cc 20");
        assert!(!m.is_bound("a"), "a lost it");
        // Re-learn b onto another CC: the old CC is freed.
        m.control_clicked("b", true);
        m.midi(cc(0, 21, 0));
        assert_eq!(m.binding("b"), Some((0, 21)));
        assert_eq!(m.midi(cc(0, 20, 64)), None, "cc 20 drives nothing now");
    }

    #[test]
    fn escape_and_the_learn_button_back_out() {
        let mut m = MidiLearn::default();
        m.toggle_pick();
        assert!(m.escape(), "esc leaves pick mode");
        assert!(!m.active());
        m.control_clicked("a", true);
        assert!(m.toggle_pick() == false, "LEARN while armed backs out");
        assert!(!m.active());
        assert!(!m.escape(), "nothing to leave");
    }

    #[test]
    fn persistence_round_trips_and_junk_is_ignored() {
        let mut m = MidiLearn::default();
        m.control_clicked("video_fade", true);
        m.midi(cc(3, 74, 0));
        m.control_clicked("fx_a_spd", true);
        m.midi(cc(0, 16, 0));
        let decoded = MidiLearn::decode(&m.encode());
        assert_eq!(decoded.binding("video_fade"), Some((3, 74)));
        assert_eq!(decoded.binding("fx_a_spd"), Some((0, 16)));
        // The reverse map came back too: values route.
        let mut decoded = decoded;
        assert_eq!(
            decoded.midi(cc(0, 16, 127)),
            Some(LearnEvent::Value {
                control: "fx_a_spd".into(),
                channel: 0,
                cc: 16,
                value: 1.0
            })
        );
        assert!(MidiLearn::decode("junk\nx y z").binding("x").is_none());
        assert!(MidiLearn::decode("v1\nbad 99 300\n").binding("bad").is_none());
    }
    /// A value says which CC it came from, so a re-learn is visible to a
    /// caller that caches anything per control.
    #[test]
    fn a_value_names_the_source_it_came_from() {
        let mut m = MidiLearn::default();
        m.control_clicked("autofade", true);
        m.midi(cc(0, 20, 0));
        assert_eq!(
            m.midi(cc(0, 20, 127)),
            Some(LearnEvent::Value {
                control: "autofade".into(),
                channel: 0,
                cc: 20,
                value: 1.0
            })
        );
        // Re-learn onto another pad: the same control, a different source.
        m.control_clicked("autofade", true);
        m.midi(cc(0, 21, 0));
        assert_eq!(
            m.midi(cc(0, 21, 127)),
            Some(LearnEvent::Value {
                control: "autofade".into(),
                channel: 0,
                cc: 21,
                value: 1.0
            })
        );
        assert_eq!(m.midi(cc(0, 20, 127)), None, "the old pad is nobody's now");
    }

    /// A line naming a control this build does not have is not merely dead:
    /// its CC is still claimed, so the knob does nothing and there is no
    /// surface on which to clear it.
    #[test]
    fn a_binding_for_a_control_that_is_gone_stops_eating_its_cc() {
        let mut m = MidiLearn::decode("v1\nvideo_fade 0 20\nghost_control 0 21\n");
        assert_eq!(m.binding("ghost_control"), Some((0, 21)));
        assert!(
            matches!(m.midi(cc(0, 21, 127)), Some(LearnEvent::Value { .. })),
            "without the sweep the ghost claims the message"
        );
        m.retain_controls(|control| control == "video_fade");
        assert_eq!(m.binding("ghost_control"), None);
        assert_eq!(m.midi(cc(0, 21, 127)), None, "the CC is free for the surface");
        assert_eq!(m.binding("video_fade"), Some((0, 20)), "the real one is untouched");
    }

    /// The number a bound control is sent means what its record says.
    #[test]
    fn a_binding_reads_through_its_transform() {
        let mut m = MidiLearn::default();
        m.control_clicked("master", true);
        m.midi(cc(0, 14, 0));
        assert!(m.set_transform("master", Transform::Invert));
        assert!(!m.set_transform("nobody", Transform::Invert), "an unbound control has no record");
        assert_eq!(
            m.midi(cc(0, 14, 127)),
            Some(LearnEvent::Value { control: "master".into(), channel: 0, cc: 14, value: 0.0 })
        );
        assert_eq!(m.record("master").map(|b| b.transform), Some(Transform::Invert));
    }

    #[test]
    fn a_turn_is_not_a_value() {
        let mut m = MidiLearn::default();
        m.control_clicked("master", true);
        m.midi(cc(0, 14, 0));
        m.set_transform("master", Transform::Relative);
        match m.midi(cc(0, 14, 1)) {
            Some(LearnEvent::Turn { control, channel, cc: number, delta }) => {
                assert_eq!((control.as_str(), channel, number), ("master", 0, 14));
                assert!((delta - 1.0 / 127.0).abs() < 1e-6);
            }
            other => panic!("a relative binding sends a turn, got {other:?}"),
        }
        assert!(m.set_sensitivity("master", 0.5));
        assert!(!m.set_sensitivity("master", f32::NAN), "a sensitivity that is not a number is refused");
        match m.midi(cc(0, 14, 1)) {
            Some(LearnEvent::Turn { delta, .. }) => assert!((delta - 0.5 / 127.0).abs() < 1e-6),
            other => panic!("{other:?}"),
        }
    }

    /// Re-learning moves the source and keeps what the control was told;
    /// a steal drops the loser's record whole.
    #[test]
    fn a_relearn_moves_the_source_and_keeps_what_the_control_was_told() {
        let mut m = MidiLearn::default();
        m.control_clicked("master", true);
        m.midi(cc(0, 14, 0));
        m.set_transform("master", Transform::Relative64);
        m.set_sensitivity("master", 2.0);
        m.set_press("master", Some(Behaviour::Trigger));
        m.control_clicked("master", true);
        m.midi(cc(1, 20, 0));
        let record = m.record("master").expect("still bound");
        assert_eq!(record.source, (1, 20));
        assert_eq!(record.transform, Transform::Relative64);
        assert_eq!(record.sensitivity, 2.0);
        assert_eq!(record.press, Some(Behaviour::Trigger));
        assert_eq!(m.midi(cc(0, 14, 64)), None, "the old source is nobody's");
    }

    #[test]
    fn a_steal_drops_the_losers_record() {
        let mut m = MidiLearn::default();
        m.control_clicked("master", true);
        m.midi(cc(0, 14, 0));
        m.set_transform("master", Transform::Invert);
        m.control_clicked("xfader", true);
        m.midi(cc(0, 14, 0));
        assert_eq!(m.record("master"), None, "the loser's record is gone, not orphaned");
        assert_eq!(m.record("xfader").map(|b| b.transform), Some(Transform::Plain), "the winner starts fresh");
    }

    /// The words ride the map line after the three tokens, and a line
    /// without them is the binding it always was.
    #[test]
    fn the_map_carries_the_words_and_reads_lines_without_them() {
        let mut m = MidiLearn::default();
        m.control_clicked("master", true);
        m.midi(cc(0, 14, 0));
        m.set_transform("master", Transform::Relative64);
        m.set_sensitivity("master", 0.5);
        m.control_clicked("xfader", true);
        m.midi(cc(0, 15, 0));
        let text = m.encode();
        assert!(text.contains("master 0 14 read=relative64 sens=0.5\n"), "{text}");
        assert!(text.contains("xfader 0 15\n"), "{text}");
        let back = MidiLearn::decode(&text);
        assert_eq!(back.record("master"), m.record("master"));
        assert_eq!(back.record("xfader"), Some(Binding::new((0, 15))));
    }

    /// Today's reader -- the three tokens, the rest ignored -- reads every
    /// CC line the new build writes as the plain binding it names.
    #[test]
    fn todays_reader_reads_every_cc_line_the_new_build_writes() {
        let mut m = MidiLearn::default();
        m.control_clicked("master", true);
        m.midi(cc(2, 14, 0));
        m.set_transform("master", Transform::Spread64);
        m.set_press("master", Some(Behaviour::TapHold));
        // A copy of the v1 parse as it stood before records had words.
        let mut old: HashMap<String, (u8, u8)> = HashMap::new();
        let text = m.encode();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("v1"));
        for line in lines {
            let mut it = line.split_whitespace();
            let (Some(control), Some(channel), Some(number)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            let (Ok(channel), Ok(number)) = (channel.parse::<u8>(), number.parse::<u8>()) else {
                continue;
            };
            old.insert(control.to_string(), (channel, number));
        }
        assert_eq!(old.get("master"), Some(&(2, 14)));
    }
}
