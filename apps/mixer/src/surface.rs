//! Surface binder: connects the mixer model to whatever splash layout is
//! loaded, by widget NAME (the slot contract documented in
//! layouts/lr_mix.splash). The layout owns look and arrangement; this file
//! owns meaning — which slot maps to which whitelisted parameter — and the
//! gesture-to-command translation. A layout that omits a child simply
//! doesn't show that control; a layout cannot name an OSC address at all.

use makepad_mixer::model::{meters1_slots, scribble_rgb, MixerModel, StripId};
use makepad_mixer::safety::{BusN, Ch};
use makepad_mixer::safety::{
    DynLeaf, EqBand, EqBand6, EqLeaf, GateLeaf, PVal, Param,
};
use makepad_mixer::units::{
    format_level_db, format_pan, format_signed_db, lin_to_unit, DYN_RATIOS, EQ_GAIN_MAX,
    EQ_GAIN_MIN,
};
use makepad_widgets::*;
use std::time::Instant;

pub const MAX_SLOTS: usize = 16;
const SEND_THROTTLE_MS: u128 = 40;
/// Strip widths (in points) that the printed fader scale needs.
/// Wire distance that counts as "flat" on the +/-15 dB EQ gain slider: half
/// a dB, wide enough to find by hand on a small strip.
const EQ_DETENT: f32 = 0.5 / 30.0;
const SCALE_HIDE_BELOW: f64 = 84.0;
const SCALE_SHOW_ABOVE: f64 = 92.0;

/// Cached refs for one strip slot. All optional-by-layout; empty refs are
/// simply skipped.
struct Slot {
    root: WidgetRef,
    scale: WidgetRef,
    eq: WidgetRef,
    eq_v: ViewRef,
    dynv: WidgetRef,
    meter: WidgetRef,
    mute: ViewRef,
    name_plate: ViewRef,
    name_lbl: LabelRef,
    gain_row: WidgetRef,
    gain_lbl: LabelRef,
    gain_sl: SliderRef,
    eq_lbl: LabelRef,
    eq_sl: SliderRef,
    pan_lbl: LabelRef,
    pan_sl: SliderRef,
    fader: SliderRef,
    fader_db: LabelRef,
    strip_lbl: LabelRef,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Ctl {
    Fader,
    Pan,
    Gain,
    Eq,
}

pub struct SurfaceBinder {
    slots: Vec<Slot>,
    strips: Vec<StripId>,
    /// Which EQ band each strip's gain slider drives (click the curve to
    /// change it). Band 2 — the low-mid — is where hands go first.
    sel_band: Vec<u8>,
    /// Strips too narrow for a full "B2 +0.0 dB" readout drop the unit and
    /// then the band prefix, rather than clipping the number itself.
    compact: Vec<u8>,
    peaks: Vec<f32>,
    last_meter: Option<Instant>,
    throttle: std::collections::HashMap<(usize, Ctl), Instant>,
    note: LabelRef,
}

impl Default for SurfaceBinder {
    fn default() -> Self {
        SurfaceBinder {
            slots: Vec::new(),
            strips: Vec::new(),
            sel_band: vec![2; MAX_SLOTS],
            compact: vec![0; MAX_SLOTS],
            peaks: vec![f32::NEG_INFINITY; MAX_SLOTS],
            last_meter: None,
            throttle: Default::default(),
            note: Default::default(),
        }
    }
}

/// Sets a script-declared shader instance on a View's background, both in
/// the persistent dyn-instance store (survives redraw) and directly on the
/// already-drawn area (live update without relayout).
fn set_view_instance(cx: &mut Cx, w: &WidgetRef, name: &str, vals: &[f32]) {
    if let Some(mut v) = w.borrow_mut::<View>() {
        let id = LiveId::from_str(name);
        v.draw_bg.draw_vars.set_dyn_instance(cx, id, vals);
        v.draw_bg.draw_vars.set_instance_on_area(cx, id, vals);
    }
}

fn set_label_color(cx: &mut Cx, l: &LabelRef, rgb: [f32; 3]) {
    if let Some(mut inner) = l.borrow_mut() {
        inner.draw_text.color = Vec4f {
            x: rgb[0],
            y: rgb[1],
            z: rgb[2],
            w: 1.0,
        };
        inner.draw_text.redraw(cx);
    }
}

fn slider_dragging(s: &SliderRef) -> bool {
    s.borrow().map(|s| s.dragging.is_some()).unwrap_or(false)
}

impl SurfaceBinder {
    /// Re-resolves every slot ref inside the (re)loaded layout.
    pub fn rebind(&mut self, cx: &mut Cx, surface: &WidgetRef) {
        self.slots.clear();
        self.note = surface.label(cx, &[LiveId::from_str("surface_note")]);
        for i in 0..MAX_SLOTS {
            let sid = [LiveId::from_str(&format!("strip_{i}"))];
            let root = surface.widget(cx, &sid);
            if root.is_empty() {
                continue;
            }
            let child = |cx: &mut Cx, name: &str| -> WidgetRef {
                root.widget(cx, &[LiveId::from_str(name)])
            };
            let slot = Slot {
                scale: child(cx, "scale"),
                eq: child(cx, "eq"),
                dynv: child(cx, "dyn"),
                meter: child(cx, "meter"),
                mute: root.view(cx, &[LiveId::from_str("mute")]),
                name_plate: root.view(cx, &[LiveId::from_str("name_plate")]),
                name_lbl: root.label(cx, &[LiveId::from_str("name_lbl")]),
                gain_row: child(cx, "gain_row"),
                gain_lbl: root.label(cx, &[LiveId::from_str("gain_lbl")]),
                gain_sl: root.slider(cx, &[LiveId::from_str("gain_sl")]),
                eq_v: root.view(cx, &[LiveId::from_str("eq")]),
                eq_lbl: root.label(cx, &[LiveId::from_str("eq_lbl")]),
                eq_sl: root.slider(cx, &[LiveId::from_str("eq_sl")]),
                pan_lbl: root.label(cx, &[LiveId::from_str("pan_lbl")]),
                pan_sl: root.slider(cx, &[LiveId::from_str("pan_sl")]),
                fader: root.slider(cx, &[LiveId::from_str("fader")]),
                fader_db: root.label(cx, &[LiveId::from_str("fader_db")]),
                strip_lbl: root.label(cx, &[LiveId::from_str("strip_lbl")]),
                root,
            };
            self.slots.push(slot);
        }
        self.throttle.clear();
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Strips get narrow when the window does. Below the width where the
    /// printed dB scale can still render "-50", the column is dropped and
    /// its space goes to the fader and meter: a scale clipped to ")" and
    /// ";" is worse than no scale at all, and the fader keeps its own tick
    /// marks either way. Hysteresis so a drag across the threshold does not
    /// flicker.
    pub fn fit_density(&mut self, cx: &mut Cx, model: &MixerModel) {
        let mut redraw_labels: Vec<usize> = Vec::new();
        for (i, slot) in self.slots.iter().enumerate() {
            if slot.scale.is_empty() || !slot.root.visible() {
                continue;
            }
            let w = slot.root.area().rect(cx).size.x;
            let showing = slot.scale.visible();
            let show = if showing { w >= SCALE_HIDE_BELOW } else { w >= SCALE_SHOW_ABOVE };
            if show != showing {
                slot.scale.set_visible(cx, show);
                slot.root.redraw(cx);
            }
            let level = if w >= 96.0 {
                0
            } else if w >= 76.0 {
                1
            } else {
                2
            };
            if self.compact.get(i).copied() != Some(level) {
                self.compact[i] = level;
                redraw_labels.push(i);
            }
        }
        for i in redraw_labels {
            self.sync_strip(cx, model, i);
        }
    }

    /// The layout's one status line. The host writes session state here
    /// (searching, reading state, lost the console); the binder writes
    /// surface state (a layout too small for the desk).
    pub fn set_note(&self, cx: &mut Cx, text: &str) {
        self.note.set_text(cx, text);
        // An empty label still costs a line of height at the top of the
        // surface; the strips get that space back instead.
        self.note.set_visible(cx, !text.is_empty());
    }

    /// Applies the model's strip list to the slots: visibility, labels,
    /// plate colours, and every currently-known value.
    pub fn sync_all(&mut self, cx: &mut Cx, model: &MixerModel) {
        self.strips = model.strips();
        // Only the binder's own bad news goes here; session status is the
        // host's line (see set_note) and must not be stomped on.
        if self.strips.len() > self.slots.len() {
            let msg = format!(
                "layout shows {} of {} strips",
                self.slots.len(),
                self.strips.len()
            );
            self.set_note(cx, &msg);
        }
        for (i, slot) in self.slots.iter().enumerate() {
            let visible = i < self.strips.len();
            slot.root.set_visible(cx, visible);
            if !visible {
                continue;
            }
            let strip = self.strips[i];
            slot.strip_lbl.set_text(cx, &strip.label());
            self.sync_strip(cx, model, i);
        }
    }

    fn strip_at(&self, i: usize) -> Option<StripId> {
        self.strips.get(i).copied()
    }

    /// Pushes every known value of one strip into its slot widgets.
    fn sync_strip(&self, cx: &mut Cx, model: &MixerModel, i: usize) {
        let Some(strip) = self.strip_at(i) else { return };
        let slot = &self.slots[i];
        let level = self.compact.get(i).copied().unwrap_or(0);

        // Name plate: the console's own scribble text, verbatim (this desk
        // numbers its own names: "1 TV"), falling back to "Ch 1" until the
        // name arrives. Fill follows the console's colour flag: plain
        // colours paint the strip, "inverted" ones tint text and border.
        slot.name_lbl.set_text(cx, &model.strip_name(strip));
        let idx = model.strip_color(strip);
        let filled = match idx {
            Some(i) if makepad_mixer::model::scribble_inverted(i) => 0.0,
            Some(_) => 1.0,
            None => 0.0,
        };
        if let Some(idx) = idx {
            let rgb = scribble_rgb(idx);
            set_view_instance(cx, &slot.name_plate, "plate_rgb", &rgb);
            set_view_instance(cx, &slot.name_plate, "plate_filled", &[filled]);
            let text_rgb = if filled > 0.5 { [0.04, 0.05, 0.07] } else { rgb };
            set_label_color(cx, &slot.name_lbl, text_rgb);
        }

        // Fader.
        if let Some(f) = model.get_f(fader_param(strip)) {
            if !slider_dragging(&slot.fader) {
                slot.fader.set_value(cx, f as f64);
            }
            slot.fader_db.set_text(cx, &format_level_db(f));
        } else {
            slot.fader_db.set_text(cx, "—");
        }

        // Mute (mix/on is inverted: 0 = muted).
        if let Some(on) = model.get_i(on_param(strip)) {
            set_view_instance(
                cx,
                &slot.mute,
                "muted",
                &[if on == 0 { 1.0 } else { 0.0 }],
            );
        }

        // Pan. When an unlinked pair's halves really do sit hard L/R, the
        // pan IS the stereo image and a single strip gesture cannot express
        // it — show "LR" instead of the odd half's L100. Two halves sitting
        // anywhere else pan together like one channel.
        if pan_is_stereo_image(model, strip) {
            if !slider_dragging(&slot.pan_sl) {
                slot.pan_sl.set_value(cx, 0.5);
            }
            slot.pan_lbl.set_text(cx, "LR");
        } else if let Some(p) = model.get_f(pan_param(strip)) {
            if !slider_dragging(&slot.pan_sl) {
                slot.pan_sl.set_value(cx, p as f64);
            }
            slot.pan_lbl.set_text(cx, &format_pan(p));
        } else {
            slot.pan_lbl.set_text(cx, "—");
        }

        // Gain row: preamp gain on input strips, hidden elsewhere.
        match strip {
            StripId::Ch { base, .. } => {
                slot.gain_row.set_visible(cx, true);
                if let Some(g) = model.get_f(Param::HeadampGain(base)) {
                    if !slider_dragging(&slot.gain_sl) {
                        slot.gain_sl.set_value(cx, g as f64);
                    }
                    let db = lin_to_unit(-12.0, 60.0, g);
                    slot.gain_lbl.set_text(cx, &db_readout(db, level));
                } else {
                    slot.gain_lbl.set_text(cx, "—");
                }
            }
            _ => {
                slot.gain_row.set_visible(cx, false);
            }
        }

        // EQ gain row: the selected band's boost/cut — the slider that
        // bends the curve drawn above it.
        let band = self.band_at(i, strip);
        if let Some(g) = model.get_f(eq_param(strip, band, EqLeaf::G)) {
            if !slider_dragging(&slot.eq_sl) {
                slot.eq_sl.set_value(cx, g as f64);
            }
            let db = lin_to_unit(EQ_GAIN_MIN, EQ_GAIN_MAX, g);
            slot.eq_lbl.set_text(cx, &eq_readout(band, db, level));
        } else {
            slot.eq_lbl.set_text(cx, &format!("B{band} —"));
        }

        self.sync_eq(cx, model, i);
        self.sync_dyn(cx, model, i);
    }

    /// The EQ band this strip's gain slider drives, clamped to the bands
    /// the strip actually has (4 on inputs, 6 on buses and main).
    fn band_at(&self, i: usize, strip: StripId) -> u8 {
        self.sel_band
            .get(i)
            .copied()
            .unwrap_or(2)
            .clamp(1, eq_band_count(strip))
    }

    fn sync_eq(&self, cx: &mut Cx, model: &MixerModel, i: usize) {
        let Some(strip) = self.strip_at(i) else { return };
        let slot = &self.slots[i];
        if slot.eq.is_empty() {
            return;
        }
        let on = match strip {
            StripId::Ch { base, .. } => model.get_i(Param::ChEqOn(base)),
            StripId::Bus { base, .. } => model.get_i(Param::BusEqOn(base)),
            StripId::Main => model.get_i(Param::LrEqOn),
        };
        // Unknown EQ state shows DIM, not as a confident flat curve.
        set_view_instance(cx, &slot.eq, "eq_on", &[on.map(|v| v as f32).unwrap_or(0.0)]);
        let n = eq_band_count(strip);
        let sel = self.band_at(i, strip);
        for band in 1..=6u8 {
            if band > n {
                // A band this strip does not have: park it flat and out of
                // the way so it adds nothing to the drawn curve.
                set_view_instance(cx, &slot.eq, &format!("b{band}"), &[2.0, 1.2, 0.5, 0.5]);
                continue;
            }
            let (t, f, g, q) = eq_band_values(model, strip, band);
            set_view_instance(cx, &slot.eq, &format!("b{band}"), &[t, f, g, q]);
            if band == sel {
                set_view_instance(cx, &slot.eq, "sel_f", &[f]);
            }
        }
        slot.eq.redraw(cx);
    }

    fn sync_dyn(&self, cx: &mut Cx, model: &MixerModel, i: usize) {
        let Some(strip) = self.strip_at(i) else { return };
        let slot = &self.slots[i];
        if slot.dynv.is_empty() {
            return;
        }
        let dynp = |l: DynLeaf| -> Param {
            match strip {
                StripId::Ch { base, .. } => Param::ChDyn(base, l),
                StripId::Bus { base, .. } => Param::BusDyn(base, l),
                StripId::Main => Param::LrDyn(l),
            }
        };
        let comp_on = model.get_i(dynp(DynLeaf::On)).unwrap_or(0) as f32;
        let thr_db = model
            .get_f(dynp(DynLeaf::Thr))
            .map(|t| lin_to_unit(-60.0, 0.0, t))
            .unwrap_or(0.0);
        let ratio = model
            .get_i(dynp(DynLeaf::Ratio))
            .and_then(|r| DYN_RATIOS.get(r as usize).copied())
            .unwrap_or(1.0);
        set_view_instance(cx, &slot.dynv, "comp_on", &[comp_on]);
        set_view_instance(cx, &slot.dynv, "thr_db", &[thr_db]);
        set_view_instance(cx, &slot.dynv, "ratio", &[ratio]);
        if let StripId::Ch { base, .. } = strip {
            let gate_on = model.get_i(Param::ChGate(base, GateLeaf::On)).unwrap_or(0) as f32;
            let gate_thr = model
                .get_f(Param::ChGate(base, GateLeaf::Thr))
                .map(|t| lin_to_unit(-80.0, 0.0, t))
                .unwrap_or(-80.0);
            set_view_instance(cx, &slot.dynv, "gate_on", &[gate_on]);
            set_view_instance(cx, &slot.dynv, "gate_thr_db", &[gate_thr.max(-60.0)]);
        }
        slot.dynv.redraw(cx);
    }

}

fn fader_param(s: StripId) -> Param {
    match s {
        StripId::Ch { base, .. } => Param::ChMixFader(base),
        StripId::Bus { base, .. } => Param::BusMixFader(base),
        StripId::Main => Param::LrMixFader,
    }
}

fn on_param(s: StripId) -> Param {
    match s {
        StripId::Ch { base, .. } => Param::ChMixOn(base),
        StripId::Bus { base, .. } => Param::BusMixOn(base),
        StripId::Main => Param::LrMixOn,
    }
}

fn pan_param(s: StripId) -> Param {
        match s {
            StripId::Ch { base, .. } => Param::ChMixPan(base),
            StripId::Bus { base, .. } => Param::BusMixPan(base),
            StripId::Main => Param::LrMixPan,
        }
    }

/// The even-half equivalent of an odd-half parameter, for VIRTUAL pairs
/// (console link off, stereo by naming convention): gestures drive both.
fn twin(p: Param) -> Option<Param> {
    use makepad_mixer::safety::{BusN, Ch};
    let ch = |c: Ch| Ch::new(c.get() + 1);
    let bus = |b: BusN| BusN::new(b.get() + 1);
    match p {
        Param::ChMixFader(c) => ch(c).map(Param::ChMixFader),
        Param::ChMixOn(c) => ch(c).map(Param::ChMixOn),
        Param::HeadampGain(c) => ch(c).map(Param::HeadampGain),
        Param::ChGate(c, l) => ch(c).map(|c| Param::ChGate(c, l)),
        Param::ChDyn(c, l) => ch(c).map(|c| Param::ChDyn(c, l)),
        Param::ChEq(c, b, l) => ch(c).map(|c| Param::ChEq(c, b, l)),
        Param::ChConfigName(c) => ch(c).map(Param::ChConfigName),
        Param::BusMixFader(b) => bus(b).map(Param::BusMixFader),
        Param::BusMixOn(b) => bus(b).map(Param::BusMixOn),
        Param::BusDyn(b, l) => bus(b).map(|b| Param::BusDyn(b, l)),
        Param::BusEq(b, band, l) => bus(b).map(|b| Param::BusEq(b, band, l)),
        Param::BusConfigName(b) => bus(b).map(Param::BusConfigName),
        _ => None,
    }
}

/// The even half's pan parameter, for pairs the console is not mirroring.
fn pan_twin(s: StripId) -> Option<Param> {
    use makepad_mixer::safety::{BusN, Ch};
    match s {
        StripId::Ch { base, .. } => Ch::new(base.get() + 1).map(Param::ChMixPan),
        StripId::Bus { base, .. } => BusN::new(base.get() + 1).map(Param::BusMixPan),
        StripId::Main => None,
    }
}

/// True when an unlinked pair's two halves sit hard left and hard right —
/// i.e. the pan controls the stereo image, not a position.
fn pan_is_stereo_image(model: &MixerModel, s: StripId) -> bool {
    if !s.is_virtual_pair() {
        return false;
    }
    let (Some(odd), Some(even)) = (
        model.get_f(pan_param(s)),
        pan_twin(s).and_then(|t| model.get_f(t)),
    ) else {
        // Unknown pans: treat as a normal pair rather than locking the
        // control on a guess.
        return false;
    };
    odd <= 0.02 && even >= 0.98
}

/// EQ bands per strip: input channels have 4, buses and main have 6.
fn eq_band_count(s: StripId) -> u8 {
    match s {
        StripId::Ch { .. } => 4,
        _ => 6,
    }
}

/// One EQ leaf of one band. `band` must be within [`eq_band_count`].
fn eq_param(s: StripId, band: u8, l: EqLeaf) -> Param {
    match s {
        StripId::Ch { base, .. } => {
            Param::ChEq(base, EqBand::new(band.clamp(1, 4)).unwrap(), l)
        }
        StripId::Bus { base, .. } => {
            Param::BusEq(base, EqBand6::new(band.clamp(1, 6)).unwrap(), l)
        }
        StripId::Main => Param::LrEq(EqBand6::new(band.clamp(1, 6)).unwrap(), l),
    }
}

/// Readouts shrink with the strip: full, then no unit, then value only.
fn db_readout(db: f32, level: u8) -> String {
    if level == 0 {
        format_signed_db(db)
    } else {
        format!("{:+.1}", db)
    }
}

fn eq_readout(band: u8, db: f32, level: u8) -> String {
    match level {
        0 => format!("B{band} {}", format_signed_db(db)),
        1 => format!("B{band} {:+.1}", db),
        _ => format!("{:+.1}", db),
    }
}

fn name_param(s: StripId) -> Param {
    match s {
        StripId::Ch { base, .. } => Param::ChConfigName(base),
        StripId::Bus { base, .. } => Param::BusConfigName(base),
        StripId::Main => Param::LrConfigName,
    }
}

fn color_param(s: StripId) -> Param {
    match s {
        StripId::Ch { base, .. } => Param::ChConfigColor(base),
        StripId::Bus { base, .. } => Param::BusConfigColor(base),
        StripId::Main => Param::LrConfigColor,
    }
}

fn eq_on_param(s: StripId) -> Param {
    match s {
        StripId::Ch { base, .. } => Param::ChEqOn(base),
        StripId::Bus { base, .. } => Param::BusEqOn(base),
        StripId::Main => Param::LrEqOn,
    }
}

/// Where a band sits before the console has answered — spread across the
/// span so an unread strip still draws a plausible flat curve.
fn default_band_f(band: u8, n: u8) -> f32 {
    if n <= 4 {
        [0.15, 0.4, 0.65, 0.88][(band.clamp(1, 4) - 1) as usize]
    } else {
        [0.08, 0.26, 0.42, 0.58, 0.74, 0.92][(band.clamp(1, 6) - 1) as usize]
    }
}

/// (type, freq01, gain01, q01) of one band, with display fallbacks.
fn eq_band_values(model: &MixerModel, s: StripId, band: u8) -> (f32, f32, f32, f32) {
    let n = eq_band_count(s);
    let t = model
        .get_i(eq_param(s, band, EqLeaf::Type))
        .map(|v| v as f32)
        .unwrap_or(2.0);
    let f = model
        .get_f(eq_param(s, band, EqLeaf::F))
        .unwrap_or_else(|| default_band_f(band, n));
    let g = model.get_f(eq_param(s, band, EqLeaf::G)).unwrap_or(0.5);
    let q = model.get_f(eq_param(s, band, EqLeaf::Q)).unwrap_or(0.5);
    (t, f, g, q)
}

impl SurfaceBinder {

    /// Routes a single console-reported change to the affected strip.
    pub fn push_param(&mut self, cx: &mut Cx, model: &MixerModel, p: Param) {
        use Param::*;
        // Link and NAME changes can reshape the whole surface (virtual
        // pairs are recognised by the even half's "<odd> R" name).
        match p {
            ChLink(_) | BusLink(_) | ChConfigName(_) | BusConfigName(_) => {
                self.sync_all(cx, model);
                return;
            }
            _ => {}
        }
        for i in 0..self.strips.len().min(self.slots.len()) {
            let strip = self.strips[i];
            let hit = match (strip, p) {
                (StripId::Ch { base, paired, .. }, p) => {
                    let mine = |c: Ch| {
                        c == base || (paired && c.get() == base.get() + 1)
                    };
                    match p {
                        ChMixFader(c) | ChMixOn(c) | ChMixPan(c) | ChConfigName(c)
                        | ChConfigColor(c) | ChEqOn(c) | HeadampGain(c) => mine(c),
                        ChEq(c, _, _) | ChGate(c, _) | ChDyn(c, _) => mine(c),
                        _ => false,
                    }
                }
                (StripId::Bus { base, paired, .. }, p) => {
                    let mine = |b: BusN| {
                        b == base || (paired && b.get() == base.get() + 1)
                    };
                    match p {
                        BusMixFader(b) | BusMixOn(b) | BusMixPan(b) | BusConfigName(b)
                        | BusConfigColor(b) | BusEqOn(b) => mine(b),
                        BusEq(b, _, _) | BusDyn(b, _) => mine(b),
                        _ => false,
                    }
                }
                (StripId::Main, p) => matches!(
                    p,
                    LrMixFader
                        | LrMixOn
                        | LrMixPan
                        | LrConfigName
                        | LrConfigColor
                        | LrEqOn
                        | LrEq(_, _)
                        | LrDyn(_)
                ),
            };
            if hit {
                self.sync_strip(cx, model, i);
            }
        }
    }

    /// Live meter frame. Bank 1 drives the strip meters (with peak hold);
    /// bank 6 drives the gain-reduction bar on the dyn thumbnails.
    pub fn push_meters(&mut self, cx: &mut Cx, model: &MixerModel, bank: u8, vals: &[f32]) {
        if bank == 1 {
            self.fit_density(cx, model);
            let now = Instant::now();
            let dt = self
                .last_meter
                .map(|t| now.duration_since(t).as_secs_f32())
                .unwrap_or(0.05)
                .min(0.5);
            self.last_meter = Some(now);
            for i in 0..self.strips.len().min(self.slots.len()) {
                let (l, r) = meters1_slots(self.strips[i]);
                let level = match (vals.get(l), vals.get(r)) {
                    (Some(a), Some(b)) => a.max(*b),
                    _ => continue,
                };
                let peak = &mut self.peaks[i];
                *peak = (*peak - 18.0 * dt).max(level);
                let slot = &self.slots[i];
                if !slot.meter.is_empty() {
                    set_view_instance(cx, &slot.meter, "level_db", &[level.max(-90.0)]);
                    set_view_instance(cx, &slot.meter, "peak_db", &[peak.max(-90.0)]);
                    // Belt: the on-area write repaints when the cached area
                    // is current, but a stale area (mid-relayout) would
                    // silently freeze the meter — schedule a real redraw so
                    // the dyn-instance values always land.
                    slot.meter.redraw(cx);
                }
            }
        } else if bank == 6 {
            for i in 0..self.strips.len().min(self.slots.len()) {
                let slot = &self.slots[i];
                if slot.dynv.is_empty() {
                    continue;
                }
                let gr = match self.strips[i] {
                    StripId::Ch { base, .. } => vals.get(16 + (base.get() - 1) as usize),
                    StripId::Bus { base, .. } => vals.get(32 + (base.get() - 1) as usize),
                    StripId::Main => vals.get(38),
                };
                if let Some(gr) = gr {
                    let gr = if gr.is_finite() { gr.min(0.0) } else { 0.0 };
                    set_view_instance(cx, &slot.dynv, "gr_db", &[gr]);
                }
            }
        }
    }

    /// Turns user gestures into whitelisted (Param, value) commands.
    /// Continuous drags are throttled; the final position always sends.
    pub fn handle_actions(
        &mut self,
        cx: &mut Cx,
        actions: &Actions,
        model: &MixerModel,
    ) -> Vec<(Param, PVal)> {
        let mut out = Vec::new();
        let mut resync: Vec<usize> = Vec::new();
        for i in 0..self.strips.len().min(self.slots.len()) {
            let strip = self.strips[i];
            let mut mine: Vec<(Param, PVal)> = Vec::new();

            let slider_cmd = |ctl: Ctl,
                                  sl: &SliderRef,
                                  param: Param,
                                  out: &mut Vec<(Param, PVal)>,
                                  throttle: &mut std::collections::HashMap<(usize, Ctl), Instant>| {
                if sl.is_empty() {
                    return None;
                }
                if let Some(v) = sl.slided(actions) {
                    let now = Instant::now();
                    let due = throttle
                        .get(&(i, ctl))
                        .map(|t| now.duration_since(*t).as_millis() >= SEND_THROTTLE_MS)
                        .unwrap_or(true);
                    if due {
                        throttle.insert((i, ctl), now);
                        out.push((param, PVal::F(v as f32)));
                    }
                    return Some(v as f32);
                }
                if let Some(v) = sl.end_slide(actions) {
                    throttle.remove(&(i, ctl));
                    out.push((param, PVal::F(v as f32)));
                    return Some(v as f32);
                }
                None
            };

            let slot = &self.slots[i];
            let mut picked: Option<u8> = None;
            if let Some(v) = slider_cmd(
                Ctl::Fader,
                &slot.fader,
                fader_param(strip),
                &mut mine,
                &mut self.throttle,
            ) {
                // Optimistic readout while dragging; the echo confirms.
                slot.fader_db.set_text(cx, &format_level_db(v));
            }
            // A hard-panned pair's pan is the stereo image itself — a single
            // strip gesture cannot express it, so it does not transmit and
            // the strip snaps back.
            if pan_is_stereo_image(model, strip) {
                if slot.pan_sl.slided(actions).is_some()
                    || slot.pan_sl.end_slide(actions).is_some()
                {
                    resync.push(i);
                }
            } else if let Some(v) = slider_cmd(
                Ctl::Pan,
                &slot.pan_sl,
                pan_param(strip),
                &mut mine,
                &mut self.throttle,
            ) {
                slot.pan_lbl.set_text(cx, &format_pan(v));
                // An unlinked pair pans on both halves (twin() leaves pan
                // out precisely so this stays a deliberate decision).
                if strip.is_virtual_pair() {
                    if let Some(t) = pan_twin(strip) {
                        mine.push((t, PVal::F(v)));
                    }
                }
            }
            if let StripId::Ch { base, .. } = strip {
                if let Some(v) = slider_cmd(
                    Ctl::Gain,
                    &slot.gain_sl,
                    Param::HeadampGain(base),
                    &mut mine,
                    &mut self.throttle,
                ) {
                    let level = self.compact.get(i).copied().unwrap_or(0);
                    slot.gain_lbl
                        .set_text(cx, &db_readout(lin_to_unit(-12.0, 60.0, v), level));
                }
            }
            // EQ gain of the selected band. Flat (0 dB) is a detent: the
            // slider snaps to it from within a fifth of a dB, so a band can
            // always be put back exactly where it was. The curve above the
            // slider is
            // redrawn from the dragged value on the spot — it does not wait
            // for the console to echo.
            let band = self.band_at(i, strip);
            if let Some(v) = slider_cmd(
                Ctl::Eq,
                &slot.eq_sl,
                eq_param(strip, band, EqLeaf::G),
                &mut mine,
                &mut self.throttle,
            ) {
                let v = if (v - 0.5).abs() < EQ_DETENT { 0.5 } else { v };
                if let Some(last) = mine.last_mut() {
                    if last.0 == eq_param(strip, band, EqLeaf::G) {
                        last.1 = PVal::F(v);
                    }
                }
                if !slider_dragging(&slot.eq_sl) {
                    slot.eq_sl.set_value(cx, v as f64);
                }
                let level = self.compact.get(i).copied().unwrap_or(0);
                slot.eq_lbl.set_text(
                    cx,
                    &eq_readout(band, lin_to_unit(EQ_GAIN_MIN, EQ_GAIN_MAX, v), level),
                );
                if !slot.eq.is_empty() {
                    let (t, f, _, q) = eq_band_values(model, strip, band);
                    set_view_instance(cx, &slot.eq, &format!("b{band}"), &[t, f, v, q]);
                    set_view_instance(cx, &slot.eq, "sel_f", &[f]);
                    slot.eq.redraw(cx);
                }
            }

            // Clicking the curve picks the band the slider drives.
            if !slot.eq_v.is_empty() {
                if let Some(fd) = slot.eq_v.finger_down(actions) {
                    let w = fd.rect.size.x.max(1.0);
                    let x01 = (((fd.abs.x - fd.rect.pos.x) / w) as f32).clamp(0.0, 1.0);
                    let n = eq_band_count(strip);
                    let mut best = 1u8;
                    let mut best_d = f32::MAX;
                    for b in 1..=n {
                        let (_, f, _, _) = eq_band_values(model, strip, b);
                        let d = (f - x01).abs();
                        if d < best_d {
                            best_d = d;
                            best = b;
                        }
                    }
                    picked = Some(best);
                }
            }

            // Mute: toggle the console's mix/on (inverted). Only when the
            // current state is KNOWN — we never guess at hardware state.
            if !slot.mute.is_empty() && slot.mute.finger_down(actions).is_some() {
                if let Some(on) = model.get_i(on_param(strip)) {
                    mine.push((on_param(strip), PVal::I(if on == 0 { 1 } else { 0 })));
                }
            }

            if let Some(b) = picked {
                if self.sel_band.get(i).copied() != Some(b) {
                    self.sel_band[i] = b;
                    resync.push(i);
                }
            }

            // A virtual pair drives BOTH halves with every gesture.
            let expand = strip.is_virtual_pair();
            for (p, v) in mine {
                if expand {
                    if let Some(t) = twin(p) {
                        out.push((t, v.clone()));
                    }
                }
                out.push((p, v));
            }
        }
        for i in resync {
            self.sync_strip(cx, model, i);
        }
        out
    }

    /// Parameters the current strips display that the console has not yet
    /// answered — the re-request pass queries exactly these.
    pub fn missing_params(&self, model: &MixerModel) -> Vec<Param> {
        let mut out = Vec::new();
        for strip in &self.strips {
            let mut want: Vec<Param> = vec![fader_param(*strip), on_param(*strip), pan_param(*strip)];
            want.push(eq_on_param(*strip));
            for band in 1..=eq_band_count(*strip) {
                for l in EqLeaf::ALL {
                    want.push(eq_param(*strip, band, l));
                }
            }
            want.push(name_param(*strip));
            want.push(color_param(*strip));
            if let StripId::Ch { base, .. } = strip {
                want.push(Param::HeadampGain(*base));
            }
            // A pair's even half decides how the strip is drawn (meters,
            // stereo image) — read it too.
            if strip.is_virtual_pair() {
                if let Some(t) = pan_twin(*strip) {
                    want.push(t);
                }
                if let Some(t) = twin(name_param(*strip)) {
                    want.push(t);
                }
            }
            for p in want {
                if model.get(p).is_none() {
                    out.push(p);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_mixer::safety::{BusN, Ch};

    #[test]
    fn twin_maps_odd_params_to_even_halves() {
        let c1 = Ch::new(1).unwrap();
        assert_eq!(
            twin(Param::ChMixFader(c1)),
            Some(Param::ChMixFader(Ch::new(2).unwrap()))
        );
        assert_eq!(
            twin(Param::HeadampGain(Ch::new(15).unwrap())),
            Some(Param::HeadampGain(Ch::new(16).unwrap()))
        );
        assert_eq!(
            twin(Param::BusMixOn(BusN::new(3).unwrap())),
            Some(Param::BusMixOn(BusN::new(4).unwrap()))
        );
        // pan deliberately has no twin: a virtual pair's pans ARE the image
        assert_eq!(twin(Param::ChMixPan(c1)), None);
        // and the top of the range cannot overflow
        assert_eq!(twin(Param::ChMixFader(Ch::new(16).unwrap())), None);
        assert_eq!(twin(Param::BusMixFader(BusN::new(6).unwrap())), None);
    }

    #[test]
    fn eq_moves_on_both_halves_of_an_unlinked_pair() {
        // The EQ slider is a strip control: on a pair the console is not
        // mirroring, both halves have to get it or the pair goes lopsided.
        let b2 = makepad_mixer::safety::EqBand::new(2).unwrap();
        assert_eq!(
            twin(Param::ChEq(Ch::new(3).unwrap(), b2, EqLeaf::G)),
            Some(Param::ChEq(Ch::new(4).unwrap(), b2, EqLeaf::G))
        );
        let b5 = makepad_mixer::safety::EqBand6::new(5).unwrap();
        assert_eq!(
            twin(Param::BusEq(BusN::new(1).unwrap(), b5, EqLeaf::G)),
            Some(Param::BusEq(BusN::new(2).unwrap(), b5, EqLeaf::G))
        );
    }

    #[test]
    fn eq_bands_are_four_on_inputs_and_six_on_buses() {
        let ch = StripId::Ch { base: Ch::new(1).unwrap(), paired: true, linked: false };
        let bus = StripId::Bus { base: BusN::new(1).unwrap(), paired: false, linked: false };
        assert_eq!(eq_band_count(ch), 4);
        assert_eq!(eq_band_count(bus), 6);
        assert_eq!(eq_band_count(StripId::Main), 6);
        // a band number out of the strip's range is clamped, never a panic
        assert_eq!(eq_param(ch, 6, EqLeaf::G).addr(), "/ch/01/eq/4/g");
        assert_eq!(eq_param(bus, 6, EqLeaf::G).addr(), "/bus/1/eq/6/g");
        assert_eq!(eq_param(StripId::Main, 0, EqLeaf::F).addr(), "/lr/eq/1/f");
    }

    #[test]
    fn only_a_hard_panned_pair_counts_as_a_stereo_image() {
        let c1 = Ch::new(1).unwrap();
        let c2 = Ch::new(2).unwrap();
        let virt = StripId::Ch { base: c1, paired: true, linked: false };
        let linked = StripId::Ch { base: c1, paired: true, linked: true };
        let mut m = MixerModel::default();
        // unknown pans: the control stays usable rather than locking on a guess
        assert!(!pan_is_stereo_image(&m, virt));
        m.apply(Param::ChMixPan(c1), PVal::F(0.0));
        m.apply(Param::ChMixPan(c2), PVal::F(1.0));
        assert!(pan_is_stereo_image(&m, virt));
        // a pair sitting anywhere else pans together like one channel
        m.apply(Param::ChMixPan(c2), PVal::F(0.5));
        assert!(!pan_is_stereo_image(&m, virt));
        // a console-linked pair mirrors itself; never an image
        m.apply(Param::ChMixPan(c2), PVal::F(1.0));
        assert!(!pan_is_stereo_image(&m, linked));
        assert_eq!(pan_twin(virt), Some(Param::ChMixPan(c2)));
    }
}
