//! BlackShark V3 Pro's on-device controls (USB 1532:0577, HID interface 5).
//!
//! The report layout and setters come from captures of Synapse 4 with
//! headset/receiver firmware 1.4.4. This module holds the wire format and
//! the settings model only; the Linux receiver I/O is `device_linux`, the
//! microphone gain (an ALSA mixer control, not a vendor command) is
//! `mixer_linux`, and the panel is `panel_linux` (a child of the shell's
//! panels). A successful USB write alone is never an acknowledgement from
//! the headset: every setting is read back before it counts as applied.

#[cfg(target_os = "linux")]
pub mod device_linux;
#[cfg(target_os = "linux")]
pub mod mixer_linux;

use std::collections::BTreeMap;

pub const REPORT_LEN: usize = 64;
pub const EQ_FREQUENCIES: [&str; 10] = ["31", "63", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];
/// Headphone EQ slots with a verified apply sequence (Default, Game, Movie,
/// Music, Esports). Synapse also reads slots 5..=8; they are read for the
/// active flag only and are never written.
pub const HEADPHONE_EDITABLE_SLOTS: u8 = 5;
pub const HEADPHONE_READ_SLOTS: u8 = 9;
/// The microphone EQ preset whose bands may be edited.
pub const MIC_CUSTOM_PRESET: u8 = 0x20;
pub const SLEEP_CHOICES: [u8; 5] = [0, 15, 30, 45, 60];
pub const SIDETONE_MAX: u8 = 15;
pub const EQ_LIMIT: i8 = 6;
pub type Report = [u8; REPORT_LEN];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    Receiver,
    Headset,
}

impl Destination {
    fn byte(self) -> u8 {
        match self {
            Self::Receiver => 0,
            Self::Headset => 0x80,
        }
    }
}

/// The payload must fit before the checksum at byte 62. Callers construct
/// commands from complete values; never truncate an EQ curve or argument.
pub fn command(destination: Destination, function: u8, arguments: &[u8]) -> Result<Report, &'static str> {
    if arguments.len() > 49 {
        return Err("Headset command exceeds the HID report payload");
    }
    let mut report = [0; REPORT_LEN];
    report[0] = 2;
    report[2] = 0x60;
    report[6] = 4 + arguments.len() as u8;
    report[9] = destination.byte();
    report[10] = function;
    report[12] = arguments.len() as u8;
    report[13..13 + arguments.len()].copy_from_slice(arguments);
    report[62] = checksum(&report);
    Ok(report)
}

/// A getter: no arguments, or the slot it reads.
pub fn query(destination: Destination, function: u8, slot: Option<u8>) -> Report {
    // One optional byte always fits; `command` cannot fail here.
    let arguments: &[u8] = match &slot {
        Some(slot) => std::slice::from_ref(slot),
        None => &[],
    };
    command(destination, function, arguments).unwrap_or([0; REPORT_LEN])
}

fn checksum(report: &Report) -> u8 {
    report[..62].iter().fold(0, |crc, byte| crc ^ byte)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyKind {
    Response,
    Notification,
}

/// A complete vendor report from the receiver, of any status. Consumer
/// keys, generic Razer commands (their command sits in byte 8), requests
/// (sub 0) and damaged reports are not frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    /// `2` is success; anything else is the device refusing or failing.
    pub status: u8,
    pub destination: Destination,
    pub function: u8,
    pub kind: ReplyKind,
    pub arguments: &'a [u8],
}

impl<'a> Frame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        let report: &Report = bytes.try_into().ok()?;
        if report[0] != 2
            || report[2] != 0x60
            || report[3..6] != [0, 0, 0]
            || report[7..9] != [0, 0]
            || report[12] > 49
            || report[6] != 4 + report[12]
            || checksum(report) != report[62]
            || report[63] != 0
        {
            return None;
        }
        let destination = match report[9] {
            0 => Destination::Receiver,
            0x80 => Destination::Headset,
            _ => return None,
        };
        let kind = match report[11] {
            1 => ReplyKind::Response,
            2 => ReplyKind::Notification,
            _ => return None,
        };
        Some(Self {
            status: report[1],
            destination,
            function: report[10],
            kind,
            arguments: &report[13..13 + report[12] as usize],
        })
    }

    pub fn succeeded(&self) -> bool {
        self.status == 2
    }

    /// This is the response to `request`: same destination and function,
    /// and for the slot getters (headphone EQ bands and metadata) the same
    /// slot, so a late answer for another slot never fills this one. The
    /// transaction byte is always 0x60, so the caller also keeps a single
    /// request in flight and drains late answers after a timeout.
    pub fn answers(&self, request: &Report) -> bool {
        if self.kind != ReplyKind::Response || self.destination.byte() != request[9] || self.function != request[10] {
            return false;
        }
        match request[10] {
            0x15 | 0x60 if request[12] == 1 => self.arguments.first() == Some(&request[13]),
            _ => true,
        }
    }

    /// A setter's acknowledgement: success with the single argument 0.
    pub fn acknowledged(&self) -> bool {
        self.succeeded() && self.arguments == [0]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseMode {
    Off,
    Anc,
    Ambient,
}

impl NoiseMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Anc => "Noise cancelling",
            Self::Ambient => "Ambient",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoiseControl {
    pub mode: NoiseMode,
    /// Retained ANC intensity, including when ANC is off or ambient is on.
    pub anc_level: u8,
}

impl NoiseControl {
    pub fn write(self) -> Result<Report, &'static str> {
        if !(1..=4).contains(&self.anc_level) {
            return Err("ANC level must be between 1 and 4");
        }
        let args = match self.mode {
            NoiseMode::Off => [0, self.anc_level],
            NoiseMode::Anc => [1, self.anc_level],
            // Synapse writes ambient as 50 01; the subsequent getter keeps
            // the previous ANC level. This is not an ambient-level slider.
            NoiseMode::Ambient => [0x50, 1],
        };
        command(Destination::Headset, 0x92, &args)
    }

    /// Whether a read-back shows this request applied. Ambient keeps the
    /// earlier ANC level in the getter, so only its mode is compared.
    pub fn applied_in(self, read: NoiseControl) -> bool {
        read.mode == self.mode && (self.mode == NoiseMode::Ambient || read.anc_level == self.anc_level)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Equalizer {
    /// Whole decibels, -6..=6, in EQ_FREQUENCIES order.
    pub bands: [i8; 10],
}

impl Equalizer {
    fn decode(bytes: &[u8]) -> Option<Self> {
        let raw: &[u8; 10] = bytes.try_into().ok()?;
        let mut bands = [0; 10];
        for (value, raw) in bands.iter_mut().zip(raw) {
            let magnitude = (raw & 0x7f) as i8;
            if magnitude > EQ_LIMIT {
                return None;
            }
            *value = if raw & 0x80 != 0 { -magnitude } else { magnitude };
        }
        Some(Self { bands })
    }

    fn encode(self) -> Result<[u8; 10], &'static str> {
        let mut raw = [0; 10];
        for (raw, value) in raw.iter_mut().zip(self.bands) {
            if !(-EQ_LIMIT..=EQ_LIMIT).contains(&value) {
                return Err("EQ bands must be between -6 and +6 dB");
            }
            *raw = if value < 0 { 0x80 | (-value as u8) } else { value as u8 };
        }
        Ok(raw)
    }

    pub fn microphone_write(self, preset: u8) -> Result<[Report; 2], &'static str> {
        // Only the custom preset was verified for editing. Do not silently
        // overwrite another device preset using guessed preset identifiers.
        if preset != MIC_CUSTOM_PRESET {
            return Err("Select the custom microphone EQ before editing its bands");
        }
        Ok([
            command(Destination::Headset, 0x96, &[preset])?,
            command(Destination::Headset, 0x97, &self.encode()?)?,
        ])
    }
}

pub fn sidetone_write(level: u8) -> Result<[Report; 2], &'static str> {
    if level > SIDETONE_MAX {
        return Err("Mic monitoring level must be between 0 and 15");
    }
    Ok([
        command(Destination::Headset, 0x98, &[1])?,
        command(Destination::Headset, 0x99, &[level])?,
    ])
}

pub fn sleep_write(minutes: u8) -> Result<Report, &'static str> {
    if !SLEEP_CHOICES.contains(&minutes) {
        return Err("Sleep timer must be Off, 15, 30, 45 or 60 minutes");
    }
    command(Destination::Headset, 0xac, &[minutes])
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeadphonePreset {
    pub eq: Option<Equalizer>,
    /// Preserve the device's six metadata bytes, including its game variant.
    pub metadata: Option<[u8; 6]>,
}

impl HeadphonePreset {
    pub fn name(slot: u8) -> String {
        match slot {
            0 => "Default".into(),
            1 => "Game".into(),
            2 => "Movie".into(),
            3 => "Music".into(),
            4 => "Esports".into(),
            _ => format!("Preset {}", slot as u16 + 1),
        }
    }

    /// The worker must acknowledge every step, in order, then read the bands
    /// and metadata back. Do not interleave another edit inside this sequence.
    pub fn write(&self, eq: Equalizer) -> Result<[Report; 5], &'static str> {
        let mut metadata = self.metadata.ok_or("Headphone preset has not been read yet")?;
        let slot = metadata[0];
        if slot >= HEADPHONE_EDITABLE_SLOTS {
            return Err("This additional headset preset has no verified apply command yet");
        }
        metadata[2] = 1;
        metadata[3] = 1;
        let mut bands = [0; 11];
        bands[0] = slot;
        bands[1..].copy_from_slice(&eq.encode()?);
        let mut commit = [0; 11];
        commit[0] = slot;
        commit[3] = 1;
        commit[4] = 1;
        commit[5] = u8::from(slot == 4);
        Ok([
            command(Destination::Headset, 0xe1, &[1])?,
            command(Destination::Headset, 0x95, &bands)?,
            command(Destination::Headset, 0xe0, &metadata)?,
            command(Destination::Headset, 0xe1, &[2])?,
            command(Destination::Headset, 0xeb, &commit)?,
        ])
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub battery_percent: Option<u8>,
    pub charging: Option<bool>,
    pub noise: Option<NoiseControl>,
    pub sidetone: Option<u8>,
    pub sleep_minutes: Option<u8>,
    pub mic_preset: Option<u8>,
    pub mic_eq: Option<Equalizer>,
    pub headphone_presets: BTreeMap<u8, HeadphonePreset>,
}

impl Settings {
    /// Invalid and unavailable values leave the last confirmed value intact.
    /// Connection/freshness is tracked separately by the I/O owner, so cached
    /// values are never presented as a new successful read after a timeout.
    /// Returns whether the reply carried a value this model accepted.
    pub fn observe(&mut self, reply: Frame<'_>) -> bool {
        if reply.destination != Destination::Headset || !reply.succeeded() {
            return false;
        }
        match (reply.function, reply.arguments) {
            (0x21, [value]) if *value <= 100 => self.battery_percent = Some(*value),
            (0x2a, [value]) if *value <= 1 => self.charging = Some(*value != 0),
            (0x12, [mode, level]) if (1..=4).contains(level) => {
                let mode = match mode {
                    0 => NoiseMode::Off,
                    1 => NoiseMode::Anc,
                    0x50 => NoiseMode::Ambient,
                    _ => return false,
                };
                self.noise = Some(NoiseControl {
                    mode,
                    anc_level: *level,
                });
            }
            (0x19, [value]) if *value <= SIDETONE_MAX => self.sidetone = Some(*value),
            (0x2c, [value]) if SLEEP_CHOICES.contains(value) => {
                self.sleep_minutes = Some(*value);
            }
            (0x16, [preset]) => self.mic_preset = Some(*preset),
            (0x17, bands) => match Equalizer::decode(bands) {
                Some(eq) => self.mic_eq = Some(eq),
                None => return false,
            },
            (0x15, [slot, bands @ ..]) if *slot < HEADPHONE_READ_SLOTS => match Equalizer::decode(bands) {
                Some(eq) => self.headphone_presets.entry(*slot).or_default().eq = Some(eq),
                None => return false,
            },
            (0x60, metadata) if metadata.len() == 6 && metadata[0] < HEADPHONE_READ_SLOTS => {
                let slot = metadata[0];
                self.headphone_presets.entry(slot).or_default().metadata = metadata.try_into().ok();
            }
            _ => return false,
        }
        true
    }

    /// The preset whose metadata carries the active flag (byte 3). Every
    /// slot's flag is read back together, so a stale flag never survives a
    /// change; two flags (an interrupted read) mean nothing is claimed.
    pub fn active_headphone_preset(&self) -> Option<u8> {
        let mut active = self
            .headphone_presets
            .iter()
            .filter(|(_, preset)| preset.metadata.is_some_and(|m| m[3] == 1))
            .map(|(slot, _)| *slot);
        let first = active.next();
        if active.next().is_some() {
            None
        } else {
            first
        }
    }

    /// Copy every value this read holds over `older`, keeping what it
    /// lacks: the last known values for display.
    pub fn remember_into(&self, older: &mut Settings) {
        fn keep<T: Copy>(newer: Option<T>, older: &mut Option<T>) {
            if newer.is_some() {
                *older = newer;
            }
        }
        keep(self.battery_percent, &mut older.battery_percent);
        keep(self.charging, &mut older.charging);
        keep(self.noise, &mut older.noise);
        keep(self.sidetone, &mut older.sidetone);
        keep(self.sleep_minutes, &mut older.sleep_minutes);
        keep(self.mic_preset, &mut older.mic_preset);
        keep(self.mic_eq, &mut older.mic_eq);
        for (slot, preset) in &self.headphone_presets {
            let entry = older.headphone_presets.entry(*slot).or_default();
            keep(preset.eq, &mut entry.eq);
            keep(preset.metadata, &mut entry.metadata);
        }
    }

    /// Drop what this read says about `control`: an interrupted edit left
    /// it unknown until it is read again.
    pub fn forget(&mut self, control: Control) {
        match control {
            Control::Noise => self.noise = None,
            Control::Sidetone => self.sidetone = None,
            Control::Sleep => self.sleep_minutes = None,
            Control::MicEq => {
                self.mic_preset = None;
                self.mic_eq = None;
            }
            // Every slot: the active flags of all of them may have moved.
            Control::Headphone => self.headphone_presets.clear(),
            Control::MicGain => {}
        }
    }
}

// ======================================================================
// What the panel shows and asks for
// ======================================================================

/// Where the receiver and headset stand, as the worker last observed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LinkState {
    /// No receiver plugged in.
    #[default]
    Absent,
    /// The receiver is there but its settings interface cannot be opened.
    NoAccess,
    /// Opened; the first answers are on their way.
    Checking,
    /// The receiver takes commands but never answers them.
    ReceiverSilent,
    /// The receiver answers; the headset does not (asleep, off or out of
    /// range — the device does not say which).
    HeadsetAway,
    /// The headset answers.
    Connected,
}

/// One control the person can change, for pending and problem reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Control {
    Noise,
    Sidetone,
    Sleep,
    MicEq,
    Headphone,
    MicGain,
}

/// The microphone's hardware capture gain, in hundredths of a dB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MicGain {
    pub db100: i64,
    pub min_db100: i64,
    pub max_db100: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeadsetSnapshot {
    pub state: LinkState,
    /// What the headset reported on this connection: the only values a
    /// control may be edited from or an edit verified against. Emptied when
    /// the connection ends, and per control when an edit is interrupted.
    pub settings: Settings,
    /// Earlier values, shown (labelled, disabled) until read again.
    pub last_known: Settings,
    /// A full read completed on this connection with every value read.
    pub fresh: bool,
    /// `Cx::monotonic_now()` of the last battery reading.
    pub battery_at: Option<f64>,
    pub receiver_firmware: Option<[u8; 4]>,
    pub headset_firmware: Option<[u8; 8]>,
    pub mic_gain: Option<MicGain>,
    /// A device exchange is under way (a read or an edit being applied).
    pub busy: bool,
    /// The latest failure per control, cleared once that control applies.
    pub problems: Vec<(Control, String)>,
}

impl HeadsetSnapshot {
    pub fn present(&self) -> bool {
        self.state != LinkState::Absent
    }

    pub fn connected(&self) -> bool {
        self.state == LinkState::Connected
    }

    pub fn problem(&self, control: Control) -> Option<&str> {
        self.problems
            .iter()
            .find(|(c, _)| *c == control)
            .map(|(_, m)| m.as_str())
    }

    /// The device row's second line in the Controllers & devices list.
    pub fn summary(&self) -> String {
        let battery = self
            .settings
            .battery_percent
            .or(self.last_known.battery_percent)
            .map(|p| format!("{p}%"));
        match self.state {
            LinkState::Absent => "Receiver not connected".into(),
            LinkState::NoAccess => "Settings unavailable".into(),
            LinkState::Checking => "Checking headset\u{2026}".into(),
            LinkState::ReceiverSilent => "Receiver not responding".into(),
            LinkState::HeadsetAway => match battery {
                Some(b) => format!("Headset asleep or off \u{b7} last {b}"),
                None => "Headset asleep or off".into(),
            },
            LinkState::Connected => {
                let live = self.settings.battery_percent.map(|p| format!("{p}%"));
                let mut text = live.unwrap_or_else(|| "Battery unknown".into());
                if self.settings.charging == Some(true) {
                    text.push_str(" \u{b7} Charging");
                }
                text
            }
        }
    }
}

/// Edits the person asked for and the headset has not confirmed yet. The
/// panel overwrites a setting's entry with its newest value; the worker
/// clears an entry only once the headset reads back exactly that value (or
/// refuses it outright), so nothing typed is ever dropped and a burst of
/// slider moves becomes one exchange per setting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeadsetEdits {
    pub noise: Option<NoiseControl>,
    pub sidetone: Option<u8>,
    pub sleep_minutes: Option<u8>,
    pub mic_eq: Option<Equalizer>,
    /// New bands per headphone slot (an edit of a preset's curve).
    pub headphone_bands: BTreeMap<u8, Equalizer>,
    /// The preset to end up active. Applying a slot also activates it, so
    /// this one goes last.
    pub headphone_active: Option<u8>,
    pub mic_gain_db100: Option<i64>,
    /// Re-read everything (the panel opened). Not an edit.
    pub refresh: bool,
}

impl HeadsetEdits {
    /// Nothing the person asked for is waiting.
    pub fn is_empty(&self) -> bool {
        self.noise.is_none()
            && self.sidetone.is_none()
            && self.sleep_minutes.is_none()
            && self.mic_eq.is_none()
            && self.headphone_bands.is_empty()
            && self.headphone_active.is_none()
            && self.mic_gain_db100.is_none()
    }

    pub fn pending(&self, control: Control) -> bool {
        match control {
            Control::Noise => self.noise.is_some(),
            Control::Sidetone => self.sidetone.is_some(),
            Control::Sleep => self.sleep_minutes.is_some(),
            Control::MicEq => self.mic_eq.is_some(),
            Control::Headphone => self.headphone_active.is_some() || !self.headphone_bands.is_empty(),
            Control::MicGain => self.mic_gain_db100.is_some(),
        }
    }

    pub fn apply(&mut self, edit: HeadsetEdit) {
        match edit {
            HeadsetEdit::Noise(noise) => self.noise = Some(noise),
            HeadsetEdit::Sidetone(level) => self.sidetone = Some(level.min(SIDETONE_MAX)),
            HeadsetEdit::Sleep(minutes) if SLEEP_CHOICES.contains(&minutes) => self.sleep_minutes = Some(minutes),
            HeadsetEdit::Sleep(_) => {}
            HeadsetEdit::MicEq(eq) => self.mic_eq = Some(eq),
            HeadsetEdit::HeadphonePreset(slot) if slot < HEADPHONE_EDITABLE_SLOTS => self.headphone_active = Some(slot),
            HeadsetEdit::HeadphonePreset(_) => {}
            HeadsetEdit::HeadphoneBands(slot, eq) if slot < HEADPHONE_EDITABLE_SLOTS => {
                self.headphone_bands.insert(slot, eq);
                self.headphone_active = Some(slot);
            }
            HeadsetEdit::HeadphoneBands(..) => {}
            HeadsetEdit::MicGain(db100) => self.mic_gain_db100 = Some(db100),
            HeadsetEdit::Refresh => self.refresh = true,
        }
    }
}

/// One request from the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum HeadsetEdit {
    Noise(NoiseControl),
    Sidetone(u8),
    Sleep(u8),
    /// The custom microphone EQ's bands (writing them selects that preset).
    MicEq(Equalizer),
    HeadphonePreset(u8),
    HeadphoneBands(u8, Equalizer),
    MicGain(i64),
    Refresh,
}

/// "1.4.4" from a firmware reply's leading bytes.
pub fn firmware_label(bytes: &[u8]) -> String {
    match bytes {
        [major, minor, patch, ..] => format!("{major}.{minor}.{patch}"),
        _ => "unknown".into(),
    }
}
