//! The BlackShark V3 Pro's settings view inside Controllers & devices: the
//! headset's row in the device list opens it, "Controllers & devices" goes
//! back. Drawn from the worker's snapshot (`system.headset`, what the
//! headset last reported) and the edits still waiting for it
//! (`headset_pending`): a waiting edit shows its value with "Applying…"
//! (or "Waiting for headset"), a failure says so under the controls, and a
//! control whose value the headset has not reported is drawn disabled.
//!
//! The view is a column of sections under a fixed hero and back row. It is
//! as tall as its sections; on a short screen the card is clamped and the
//! wheel scrolls whole sections. Sliders and EQ bands follow the pointer
//! locally and are sent on release (the microphone gain, a mixer control,
//! also while dragged); the worker coalesces whatever arrives meanwhile.

use super::super::ui::HAlign;
use super::*;
use crate::linux_controls::ControlAction;
use crate::shell::blackshark::{
    device_linux::describe, firmware_label, Control, Equalizer, HeadphonePreset, HeadsetEdit, HeadsetSnapshot,
    LinkState, NoiseControl, NoiseMode, Settings, EQ_FREQUENCIES, EQ_LIMIT, HEADPHONE_EDITABLE_SLOTS,
    MIC_CUSTOM_PRESET, SIDETONE_MAX, SLEEP_CHOICES,
};

const NAME: &str = "BlackShark V3 Pro";
const HERO_H: f64 = 40.0;
const SEP_H: f64 = 12.0;
const BACK_H: f64 = 26.0;
const HEADER_H: f64 = 22.0;
const ROW_H: f64 = 28.0;
const ROW_GAP: f64 = 6.0;
const SLIDER_H: f64 = 34.0;
const EQ_H: f64 = 110.0;
/// Room above and below an EQ band's travel inside its column.
const EQ_PAD: f64 = 8.0;
const FREQ_H: f64 = 14.0;
const LINE_H: f64 = 18.0;
const HINT_H: f64 = 18.0;
const BUTTON_GAP: f64 = 6.0;
const STRENGTH_LABEL_W: f64 = 72.0;

/// A hit target of the headset row and view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::shell) enum HeadsetHit {
    Open,
    Back,
    Noise(NoiseMode),
    AncLevel(u8),
    Sidetone,
    MicGain,
    Sleep(u8),
    Preset(u8),
    HeadphoneBand(u8),
    MicBand(u8),
}

/// A slider or EQ band under the pointer: the value it shows until release.
#[derive(Clone, Copy, Debug)]
pub(in crate::shell) struct HeadsetDrag {
    hit: HeadsetHit,
    /// Sidetone level, or the microphone gain in hundredths of a dB.
    value: i64,
    /// The whole curve being edited, with the dragged band changed.
    bands: [i8; 10],
    /// The headphone preset whose curve is edited.
    slot: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Noise,
    Monitoring,
    Gain,
    Sleep,
    Headphone,
    Microphone,
    About,
}

const SECTIONS: [Section; 7] = [
    Section::Noise,
    Section::Monitoring,
    Section::Gain,
    Section::Sleep,
    Section::Headphone,
    Section::Microphone,
    Section::About,
];

/// `r` split into `n` equal cells `gap` apart.
fn cells(r: Rect, n: usize, gap: f64) -> Vec<Rect> {
    let n = n.max(1);
    let w = ((r.size.x - gap * (n - 1) as f64) / n as f64).max(0.0);
    (0..n)
        .map(|i| rect(r.pos.x + i as f64 * (w + gap), r.pos.y, w, r.size.y))
        .collect()
}

fn db_label(db100: i64) -> String {
    format!("{:.1} dB", db100 as f64 / 100.0).replace('-', "\u{2212}")
}

fn sleep_label(minutes: u8) -> String {
    if minutes == 0 {
        "Off".into()
    } else {
        format!("{minutes} min")
    }
}

impl ShellPanel {
    fn headset(&self) -> &HeadsetSnapshot {
        &self.system.headset
    }

    /// Rows the headset adds to the device list.
    pub(in crate::shell) fn headset_rows(&self) -> usize {
        usize::from(self.headset().present())
    }

    // ---- what is shown: a waiting edit, else what the headset reported --

    /// What this connection read: the only values controls edit from.
    fn live(&self) -> &Settings {
        &self.headset().settings
    }

    /// Earlier values, shown disabled until read again.
    fn known(&self) -> &Settings {
        &self.headset().last_known
    }

    fn shown_noise(&self) -> Option<NoiseControl> {
        self.headset_pending.noise.or(self.live().noise).or(self.known().noise)
    }

    fn shown_sidetone(&self) -> Option<u8> {
        match self.headset_drag {
            Some(drag) if drag.hit == HeadsetHit::Sidetone => Some(drag.value as u8),
            _ => self
                .headset_pending
                .sidetone
                .or(self.live().sidetone)
                .or(self.known().sidetone),
        }
    }

    fn shown_sleep(&self) -> Option<u8> {
        self.headset_pending
            .sleep_minutes
            .or(self.live().sleep_minutes)
            .or(self.known().sleep_minutes)
    }

    fn shown_gain(&self) -> Option<(i64, i64, i64)> {
        let gain = self.headset().mic_gain?;
        let value = match self.headset_drag {
            Some(drag) if drag.hit == HeadsetHit::MicGain => drag.value,
            _ => self.headset_pending.mic_gain_db100.unwrap_or(gain.db100),
        };
        Some((value, gain.min_db100, gain.max_db100))
    }

    fn shown_preset(&self) -> Option<u8> {
        self.headset_pending
            .headphone_active
            .or(self.live().active_headphone_preset())
            .or(self.known().active_headphone_preset())
    }

    fn shown_headphone_bands(&self, slot: u8) -> Option<[i8; 10]> {
        if let Some(drag) = self
            .headset_drag
            .filter(|d| matches!(d.hit, HeadsetHit::HeadphoneBand(_)) && d.slot == slot)
        {
            return Some(drag.bands);
        }
        self.headset_pending
            .headphone_bands
            .get(&slot)
            .copied()
            .or_else(|| self.live().headphone_presets.get(&slot).and_then(|p| p.eq))
            .or_else(|| self.known().headphone_presets.get(&slot).and_then(|p| p.eq))
            .map(|eq| eq.bands)
    }

    fn shown_mic_bands(&self) -> Option<[i8; 10]> {
        if let Some(drag) = self.headset_drag.filter(|d| matches!(d.hit, HeadsetHit::MicBand(_))) {
            return Some(drag.bands);
        }
        self.headset_pending
            .mic_eq
            .or(self.live().mic_eq)
            .or(self.known().mic_eq)
            .map(|eq| eq.bands)
    }

    /// A preset can be chosen once its curve and metadata were read on this
    /// connection.
    fn preset_ready(&self, slot: u8) -> bool {
        self.live()
            .headphone_presets
            .get(&slot)
            .is_some_and(|p| p.eq.is_some() && p.metadata.is_some())
    }

    /// A section header's right-hand value: a waiting edit, a failure, or
    /// the value itself, marked when it was not read on this connection.
    fn headset_value(&self, control: Control, value: Option<String>, live: bool) -> String {
        if self.headset_pending.pending(control) {
            let reachable = self.headset().connected() || control == Control::MicGain && self.headset().present();
            return if reachable {
                "Applying\u{2026}".into()
            } else {
                "Waiting for headset".into()
            };
        }
        if self.headset().problem(control).is_some() {
            return "Not applied".into();
        }
        match value {
            Some(value) if live => value,
            Some(value) => format!("{value} \u{b7} last known"),
            None => "\u{2014}".into(),
        }
    }

    // ---- layout ---------------------------------------------------------

    fn headset_top_h() -> f64 {
        HERO_H + SEP_H + BACK_H
    }

    /// The lines under the controls: what to do, firmware, problems.
    fn headset_about_lines(&self) -> Vec<String> {
        let hs = self.headset();
        let mut lines = Vec::new();
        let advice = match hs.state {
            LinkState::Absent => Some("Plug in the BlackShark receiver to change its settings."),
            LinkState::NoAccess => Some("This account can't open the receiver's settings."),
            LinkState::ReceiverSilent => {
                Some("The receiver isn't answering. Unplug it for a few seconds and plug it back in.")
            }
            LinkState::HeadsetAway => Some("Turn the headset on to change its settings."),
            LinkState::Checking => Some("Reading the headset's settings\u{2026}"),
            LinkState::Connected if !hs.fresh && hs.busy => Some("Reading the headset's settings\u{2026}"),
            LinkState::Connected if !hs.fresh => {
                Some("Some settings couldn't be read yet; they show their last known value.")
            }
            LinkState::Connected => None,
        };
        lines.extend(advice.map(str::to_string));
        let mut firmware = Vec::new();
        if let Some(receiver) = hs.receiver_firmware {
            firmware.push(format!("Receiver {}", firmware_label(&receiver)));
        }
        if let Some(headset) = hs.headset_firmware {
            firmware.push(format!("Headset {}", firmware_label(&headset)));
        }
        if !firmware.is_empty() {
            lines.push(format!("Firmware: {}", firmware.join(" \u{b7} ")));
        }
        lines.extend(hs.problems.iter().map(|(_, message)| message.clone()));
        if !self.system_notice.is_empty() {
            lines.push(self.system_notice.clone());
        }
        lines
    }

    fn headset_section_h(&self, section: Section) -> f64 {
        SEP_H
            + match section {
                Section::Noise => HEADER_H + ROW_H + ROW_GAP + ROW_H,
                Section::Monitoring | Section::Gain => HEADER_H + SLIDER_H,
                Section::Sleep => HEADER_H + ROW_H,
                Section::Headphone => HEADER_H + ROW_H + ROW_GAP + EQ_H + FREQ_H,
                Section::Microphone => HEADER_H + EQ_H + FREQ_H,
                Section::About => LINE_H * self.headset_about_lines().len().max(1) as f64,
            }
    }

    /// The whole view; the card is clamped to the screen and scrolls.
    pub(in crate::shell) fn headset_height(&self) -> f64 {
        Self::headset_top_h() + SECTIONS.iter().map(|s| self.headset_section_h(*s)).sum::<f64>()
    }

    pub(in crate::shell) fn headset_scroll_by(&mut self, cx: &mut Cx, dy: f64) {
        if dy > 0.0 {
            self.headset_scroll = (self.headset_scroll + 1).min(SECTIONS.len() - 1);
        } else if dy < 0.0 {
            self.headset_scroll = self.headset_scroll.saturating_sub(1);
        }
        self.redraw(cx);
    }

    // ---- drawing ----------------------------------------------------------

    /// The headset's row in the device list: it opens the view.
    pub(in crate::shell) fn draw_headset_row(&mut self, cx: &mut Cx2d, row: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let hit = Hit::Headset(HeadsetHit::Open);
        self.d
            .cursor_surface(cx, row, &tok.controls, self.hot == Some(hit), false);
        let summary = self.headset().summary();
        self.d
            .panel_hero(cx, row, &tok, fg, Ico::Headphone, NAME, &summary, 26.0);
        self.d.icon_centered(
            cx,
            Ico::ChevronRight,
            rect(row.pos.x + row.size.x - 22.0, row.pos.y, 18.0, row.size.y),
            12.0,
            fg,
        );
        self.hits.push((hit, row));
    }

    pub(in crate::shell) fn draw_headset(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let summary = self.headset().summary();
        let (hero, rest) = cut_top(body, HERO_H);
        self.d
            .panel_hero(cx, hero, &tok, fg, Ico::Headphone, NAME, &summary, 0.0);
        let (sep, rest) = cut_top(rest, SEP_H);
        self.d.separator(cx, sep, fg, 0.12);
        let (back, mut rest) = cut_top(rest, BACK_H);
        let hit = Hit::Headset(HeadsetHit::Back);
        self.d
            .cursor_surface(cx, back, &tok.controls, self.hot == Some(hit), false);
        self.d.icon_centered(
            cx,
            Ico::ChevronLeft,
            rect(back.pos.x, back.pos.y, 22.0, back.size.y),
            12.0,
            fg,
        );
        self.d.label(
            cx,
            rect(
                back.pos.x + 22.0,
                back.pos.y,
                (back.size.x - 22.0).max(0.0),
                back.size.y,
            ),
            false,
            tok.font.body,
            fg,
            HAlign::Left,
            "Controllers & devices",
        );
        self.hits.push((hit, back));

        // Whole sections from the scroll position; the rest scrolls.
        let heights: Vec<f64> = SECTIONS.iter().map(|s| self.headset_section_h(*s)).collect();
        let total: f64 = heights.iter().sum();
        let overflow = total > rest.size.y + 0.5;
        let room = if overflow { rest.size.y - HINT_H } else { rest.size.y };
        let tail = |start: usize| heights[start..].iter().sum::<f64>();
        let last_start = (0..SECTIONS.len())
            .find(|&s| tail(s) <= room + 0.5)
            .unwrap_or(SECTIONS.len() - 1);
        self.headset_scroll = if overflow {
            self.headset_scroll.min(last_start)
        } else {
            0
        };
        let start = self.headset_scroll;
        let mut used = 0.0;
        let mut end = start;
        for (index, section) in SECTIONS.iter().enumerate().skip(start) {
            let h = heights[index];
            if used + h > room + 0.5 && end > start {
                break;
            }
            let (area, next) = cut_top(rest, h);
            rest = next;
            used += h;
            end = index + 1;
            self.draw_headset_section(cx, *section, area);
        }
        if overflow {
            let below = SECTIONS.len() - end;
            let hint = match (start, below) {
                (0, _) => "Scroll for more settings".to_string(),
                (_, 0) => "Scroll up for more settings".to_string(),
                _ => "Scroll for more settings".to_string(),
            };
            let (row, _) = cut_top(rest, HINT_H);
            self.d
                .label_elided(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Left, &hint);
        }
    }

    fn draw_headset_section(&mut self, cx: &mut Cx2d, section: Section, area: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let (sep, area) = cut_top(area, SEP_H);
        self.d.separator(cx, sep, fg, 0.12);
        let live = self.headset().connected();
        match section {
            Section::Noise => {
                let shown = self.shown_noise();
                let editable = live && self.live().noise.is_some();
                let value = self.headset_value(
                    Control::Noise,
                    shown.map(|n| match n.mode {
                        NoiseMode::Anc => format!("{} \u{b7} {}", n.mode.label(), n.anc_level),
                        _ => n.mode.label().to_string(),
                    }),
                    editable || self.headset_pending.noise.is_some(),
                );
                let (header, area) = cut_top(area, HEADER_H);
                self.section_header(cx, header, "NOISE CONTROL", &value);
                let (row, area) = cut_top(area, ROW_H);
                let modes = [
                    (NoiseMode::Off, "Off"),
                    (NoiseMode::Anc, "ANC"),
                    (NoiseMode::Ambient, "Ambient"),
                ];
                for (cell, (mode, label)) in cells(row, modes.len(), BUTTON_GAP).into_iter().zip(modes) {
                    let hit = editable.then_some(Hit::Headset(HeadsetHit::Noise(mode)));
                    self.small_button(cx, cell, hit, shown.is_some_and(|n| n.mode == mode), None, label);
                }
                let (_, area) = cut_top(area, ROW_GAP);
                let (row, _) = cut_top(area, ROW_H);
                let anc = shown.is_some_and(|n| n.mode == NoiseMode::Anc);
                let dim = if anc && editable { fg } else { darker(fg, 1.6) };
                self.d.label(
                    cx,
                    rect(row.pos.x, row.pos.y, STRENGTH_LABEL_W, row.size.y),
                    false,
                    tok.font.body_small,
                    dim,
                    HAlign::Left,
                    "Strength",
                );
                let levels = rect(
                    row.pos.x + STRENGTH_LABEL_W,
                    row.pos.y,
                    (row.size.x - STRENGTH_LABEL_W).max(0.0),
                    row.size.y,
                );
                for (index, cell) in cells(levels, 4, BUTTON_GAP).into_iter().enumerate() {
                    let level = index as u8 + 1;
                    let hit = (editable && anc).then_some(Hit::Headset(HeadsetHit::AncLevel(level)));
                    let selected = anc && shown.is_some_and(|n| n.anc_level == level);
                    self.small_button(cx, cell, hit, selected, None, &level.to_string());
                }
            }
            Section::Monitoring => {
                let shown = self.shown_sidetone();
                let editable = live && self.live().sidetone.is_some();
                let value = self.headset_value(
                    Control::Sidetone,
                    shown.map(|l| if l == 0 { "Off".into() } else { l.to_string() }),
                    editable || self.headset_pending.sidetone.is_some(),
                );
                let (header, area) = cut_top(area, HEADER_H);
                self.section_header(cx, header, "MIC MONITORING", &value);
                let (row, _) = cut_top(area, SLIDER_H);
                let progress = shown.map_or(0.0, |l| l as f64 / SIDETONE_MAX as f64);
                self.slider_row(cx, row, Hit::Headset(HeadsetHit::Sidetone), progress, editable);
            }
            Section::Gain => {
                let shown = self.shown_gain();
                let value = self.headset_value(Control::MicGain, shown.map(|(db, _, _)| db_label(db)), true);
                let (header, area) = cut_top(area, HEADER_H);
                self.section_header(cx, header, "MICROPHONE GAIN", &value);
                let (row, _) = cut_top(area, SLIDER_H);
                let progress = shown.map_or(0.0, |(db, min, max)| (db - min) as f64 / (max - min).max(1) as f64);
                // The last known gain stays visible; it can be changed while
                // the receiver is plugged in (the headset may be asleep).
                let enabled = shown.is_some() && self.headset().present();
                self.slider_row(cx, row, Hit::Headset(HeadsetHit::MicGain), progress, enabled);
            }
            Section::Sleep => {
                let shown = self.shown_sleep();
                let editable = live && self.live().sleep_minutes.is_some();
                let value = self.headset_value(
                    Control::Sleep,
                    shown.map(|m| {
                        if m == 0 {
                            "Never".into()
                        } else {
                            format!("After {m} min")
                        }
                    }),
                    editable || self.headset_pending.sleep_minutes.is_some(),
                );
                let (header, area) = cut_top(area, HEADER_H);
                self.section_header(cx, header, "AUTO SLEEP", &value);
                let (row, _) = cut_top(area, ROW_H);
                for (cell, minutes) in cells(row, SLEEP_CHOICES.len(), BUTTON_GAP)
                    .into_iter()
                    .zip(SLEEP_CHOICES)
                {
                    let hit = editable.then_some(Hit::Headset(HeadsetHit::Sleep(minutes)));
                    self.small_button(cx, cell, hit, shown == Some(minutes), None, &sleep_label(minutes));
                }
            }
            Section::Headphone => {
                let active = self.shown_preset();
                let dragged = self
                    .headset_drag
                    .filter(|d| matches!(d.hit, HeadsetHit::HeadphoneBand(_)));
                let value = match dragged {
                    Some(drag) => band_readout(drag),
                    None => {
                        let read = self.live().active_headphone_preset().is_some()
                            || self.headset_pending.pending(Control::Headphone);
                        self.headset_value(Control::Headphone, active.map(HeadphonePreset::name), read)
                    }
                };
                let (header, area) = cut_top(area, HEADER_H);
                self.section_header(cx, header, "HEADPHONE EQ", &value);
                let (row, area) = cut_top(area, ROW_H);
                for (index, cell) in cells(row, HEADPHONE_EDITABLE_SLOTS as usize, BUTTON_GAP)
                    .into_iter()
                    .enumerate()
                {
                    let slot = index as u8;
                    let hit = (live && self.preset_ready(slot)).then_some(Hit::Headset(HeadsetHit::Preset(slot)));
                    self.small_button(cx, cell, hit, active == Some(slot), None, &HeadphonePreset::name(slot));
                }
                let (_, area) = cut_top(area, ROW_GAP);
                let slot = active.filter(|s| *s < HEADPHONE_EDITABLE_SLOTS);
                let bands = slot.and_then(|s| self.shown_headphone_bands(s));
                let editable = live && slot.is_some_and(|s| self.preset_ready(s));
                self.draw_eq(cx, area, bands, editable, HeadsetHit::HeadphoneBand);
            }
            Section::Microphone => {
                let pending = self.headset_pending.mic_eq.is_some();
                let read = self.live().mic_preset.or(self.known().mic_preset);
                let custom = pending || read == Some(MIC_CUSTOM_PRESET);
                let known = read.is_some() || pending;
                let editable = live && self.live().mic_eq.is_some();
                let dragged = self.headset_drag.filter(|d| matches!(d.hit, HeadsetHit::MicBand(_)));
                let value = match dragged {
                    Some(drag) => band_readout(drag),
                    None => self.headset_value(
                        Control::MicEq,
                        known.then(|| if custom { "Custom".into() } else { "Built-in".into() }),
                        pending || self.live().mic_preset.is_some(),
                    ),
                };
                let (header, area) = cut_top(area, HEADER_H);
                self.section_header(cx, header, "MICROPHONE EQ", &value);
                let bands = self.shown_mic_bands();
                self.draw_eq(cx, area, bands, editable, HeadsetHit::MicBand);
            }
            Section::About => {
                let dim = darker(fg, 1.4);
                let mut area = area;
                for line in self.headset_about_lines() {
                    let (row, next) = cut_top(area, LINE_H);
                    area = next;
                    self.d
                        .label_elided(cx, row, false, tok.font.caption, dim, HAlign::Left, &line);
                }
            }
        }
    }

    /// Ten bands, -6..=+6 dB, as columns around a zero line; each column is
    /// its band's hit while `editable`.
    fn draw_eq(
        &mut self,
        cx: &mut Cx2d,
        area: Rect,
        bands: Option<[i8; 10]>,
        editable: bool,
        hit: fn(u8) -> HeadsetHit,
    ) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let (graph, labels) = cut_top(area, EQ_H);
        let travel_top = graph.pos.y + EQ_PAD;
        let travel = (graph.size.y - EQ_PAD * 2.0).max(1.0);
        let zero = travel_top + travel * 0.5;
        self.d
            .solid(cx, rect(graph.pos.x, zero, graph.size.x, 1.0), alpha(fg, 0.18));
        let columns = cells(graph, EQ_FREQUENCIES.len(), 0.0);
        let label_cells = cells(
            rect(labels.pos.x, labels.pos.y, labels.size.x, FREQ_H),
            EQ_FREQUENCIES.len(),
            0.0,
        );
        let color = if editable { tok.bar.active } else { alpha(fg, 0.35) };
        for (index, column) in columns.into_iter().enumerate() {
            let this = Hit::Headset(hit(index as u8));
            let hot = editable && (self.hot == Some(this) || self.dragging == Some(this));
            if hot {
                self.d.cursor_surface(cx, column, &tok.controls, true, false);
            }
            let center = column.pos.x + column.size.x * 0.5;
            self.d
                .solid(cx, rect(center - 1.0, travel_top, 2.0, travel), alpha(fg, 0.12));
            if let Some(bands) = bands {
                let v = bands[index].clamp(-EQ_LIMIT, EQ_LIMIT) as f64;
                let y = travel_top + (EQ_LIMIT as f64 - v) / (2.0 * EQ_LIMIT as f64) * travel;
                self.d
                    .solid(cx, rect(center - 2.0, y.min(zero), 4.0, (y - zero).abs()), color);
                let knob = if editable { fg } else { alpha(fg, 0.45) };
                self.d.solid(cx, rect(center - 6.0, y - 3.0, 12.0, 6.0), knob);
            }
            if editable {
                self.hits.push((this, column));
            }
        }
        for (cell, frequency) in label_cells.into_iter().zip(EQ_FREQUENCIES) {
            self.d.label(
                cx,
                cell,
                false,
                tok.font.caption,
                darker(fg, 1.4),
                HAlign::Center,
                frequency,
            );
        }
    }

    // ---- input --------------------------------------------------------------

    fn headset_send(&mut self, cx: &mut Cx, edit: HeadsetEdit) {
        self.system_action(cx, ControlAction::Headset(edit));
    }

    /// The band value under `p` in the column of `hit`, -6..=6.
    fn band_at(&self, hit: HeadsetHit, p: Vec2d) -> Option<i8> {
        let (_, column) = self.hits.iter().find(|(h, _)| *h == Hit::Headset(hit))?;
        let travel = (column.size.y - EQ_PAD * 2.0).max(1.0);
        let t = ((p.y - column.pos.y - EQ_PAD) / travel).clamp(0.0, 1.0);
        Some((EQ_LIMIT as f64 - t * 2.0 * EQ_LIMIT as f64).round() as i8)
    }

    fn gain_at(&self, p: Vec2d) -> Option<i64> {
        let gain = self.headset().mic_gain?;
        let v = self.slider_value(Hit::Headset(HeadsetHit::MicGain), p);
        Some(gain.min_db100 + ((gain.max_db100 - gain.min_db100) as f64 * v).round() as i64)
    }

    pub(in crate::shell) fn headset_press(&mut self, cx: &mut Cx, hit: HeadsetHit, p: Vec2d) {
        match hit {
            HeadsetHit::Open => {
                self.headset_view = true;
                self.headset_scroll = 0;
                self.headset_drag = None;
                // Opening reads the headset again, so the view is current.
                self.headset_send(cx, HeadsetEdit::Refresh);
            }
            HeadsetHit::Back => {
                self.headset_view = false;
                self.headset_drag = None;
                self.dragging = None;
            }
            HeadsetHit::Noise(mode) => {
                if let Some(shown) = self.shown_noise().filter(|n| n.mode != mode) {
                    self.headset_send(
                        cx,
                        HeadsetEdit::Noise(NoiseControl {
                            mode,
                            anc_level: shown.anc_level,
                        }),
                    );
                }
            }
            HeadsetHit::AncLevel(level) => {
                if self
                    .shown_noise()
                    .is_some_and(|n| n.mode == NoiseMode::Anc && n.anc_level != level)
                {
                    self.headset_send(
                        cx,
                        HeadsetEdit::Noise(NoiseControl {
                            mode: NoiseMode::Anc,
                            anc_level: level,
                        }),
                    );
                }
            }
            HeadsetHit::Sleep(minutes) => {
                if self.shown_sleep() != Some(minutes) {
                    self.headset_send(cx, HeadsetEdit::Sleep(minutes));
                }
            }
            HeadsetHit::Preset(slot) => {
                if self.shown_preset() != Some(slot) {
                    self.headset_send(cx, HeadsetEdit::HeadphonePreset(slot));
                }
            }
            HeadsetHit::Sidetone => {
                self.dragging = Some(Hit::Headset(hit));
                self.headset_drag = Some(HeadsetDrag {
                    hit,
                    value: 0,
                    bands: [0; 10],
                    slot: 0,
                });
                self.headset_drag_to(cx, hit, p);
            }
            HeadsetHit::MicGain => {
                // The press itself is a request, wherever it lands; moves
                // then send only what changed.
                if let Some(value) = self.gain_at(p) {
                    self.dragging = Some(Hit::Headset(hit));
                    self.headset_drag = Some(HeadsetDrag {
                        hit,
                        value,
                        bands: [0; 10],
                        slot: 0,
                    });
                    self.headset_send(cx, HeadsetEdit::MicGain(value));
                }
            }
            HeadsetHit::HeadphoneBand(_) => {
                let slot = self.shown_preset().filter(|s| *s < HEADPHONE_EDITABLE_SLOTS);
                if let Some((slot, bands)) = slot.and_then(|s| self.shown_headphone_bands(s).map(|b| (s, b))) {
                    self.dragging = Some(Hit::Headset(hit));
                    self.headset_drag = Some(HeadsetDrag {
                        hit,
                        value: 0,
                        bands,
                        slot,
                    });
                    self.headset_drag_to(cx, hit, p);
                }
            }
            HeadsetHit::MicBand(_) => {
                if let Some(bands) = self.shown_mic_bands() {
                    self.dragging = Some(Hit::Headset(hit));
                    self.headset_drag = Some(HeadsetDrag {
                        hit,
                        value: 0,
                        bands,
                        slot: 0,
                    });
                    self.headset_drag_to(cx, hit, p);
                }
            }
        }
        self.redraw(cx);
    }

    pub(in crate::shell) fn headset_drag_to(&mut self, cx: &mut Cx, hit: HeadsetHit, p: Vec2d) {
        let Some(mut drag) = self.headset_drag.filter(|d| d.hit == hit) else {
            return;
        };
        match hit {
            HeadsetHit::Sidetone => {
                let v = self.slider_value(Hit::Headset(hit), p);
                drag.value = (v * SIDETONE_MAX as f64).round() as i64;
            }
            HeadsetHit::MicGain => {
                let Some(value) = self.gain_at(p) else {
                    return;
                };
                if value != drag.value {
                    drag.value = value;
                    // A mixer write is cheap and local: the gain follows
                    // the hand, coalesced by the worker.
                    self.headset_send(cx, HeadsetEdit::MicGain(value));
                }
            }
            HeadsetHit::HeadphoneBand(band) | HeadsetHit::MicBand(band) => {
                if let Some(value) = self.band_at(hit, dvec2(p.x, p.y)) {
                    drag.bands[band as usize] = value;
                }
            }
            _ => return,
        }
        self.headset_drag = Some(drag);
        self.redraw(cx);
    }

    /// The drag ends: its value goes to the headset if it changed anything.
    pub(in crate::shell) fn headset_release(&mut self, cx: &mut Cx) {
        let Some(drag) = self.headset_drag.take() else {
            return;
        };
        match drag.hit {
            HeadsetHit::Sidetone => {
                let level = drag.value.clamp(0, SIDETONE_MAX as i64) as u8;
                if self.headset_pending.sidetone.or(self.live().sidetone) != Some(level) {
                    self.headset_send(cx, HeadsetEdit::Sidetone(level));
                }
            }
            HeadsetHit::MicGain => {} // sent while dragged
            HeadsetHit::HeadphoneBand(_) => {
                if self.shown_headphone_bands(drag.slot) != Some(drag.bands) {
                    self.headset_send(
                        cx,
                        HeadsetEdit::HeadphoneBands(drag.slot, Equalizer { bands: drag.bands }),
                    );
                }
            }
            HeadsetHit::MicBand(_) => {
                if self.shown_mic_bands() != Some(drag.bands) {
                    self.headset_send(cx, HeadsetEdit::MicEq(Equalizer { bands: drag.bands }));
                }
            }
            _ => {}
        }
        self.redraw(cx);
    }

    /// What a remote snapshot reads: the device row, or the view's state,
    /// the edits waiting and every hit target, so a proof can drive it.
    pub(in crate::shell) fn headset_text(&self) -> String {
        let mut text = format!("{NAME}: {}", self.headset().summary());
        if self.headset_view {
            text.push_str("\nview=headset");
            text.push('\n');
            text.push_str(&describe(self.headset(), &self.headset_pending));
        }
        for (hit, r) in &self.hits {
            if let Hit::Headset(hit) = hit {
                text.push_str(&format!(
                    "\nhit={hit:?} x={} y={} w={} h={}",
                    r.pos.x, r.pos.y, r.size.x, r.size.y
                ));
            }
        }
        text
    }
}

/// "125 Hz +3 dB" while a band is dragged.
fn band_readout(drag: HeadsetDrag) -> String {
    let band = match drag.hit {
        HeadsetHit::HeadphoneBand(band) | HeadsetHit::MicBand(band) => band as usize,
        _ => return String::new(),
    };
    let frequency = EQ_FREQUENCIES.get(band).copied().unwrap_or("");
    let value = drag.bands[band];
    let sign = if value > 0 {
        "+"
    } else if value < 0 {
        "\u{2212}"
    } else {
        ""
    };
    format!("{frequency} Hz {sign}{} dB", value.unsigned_abs())
}
