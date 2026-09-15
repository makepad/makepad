//! Automate APC40 lighting desk, shared with the VJ console.
//!
//! The clip-launch grid (notes 0–39) and the mixer crossfader (CC 15) stay
//! with VJ. Everything else on the same APC40 MkII is the room lighting
//! surface that used to live in `examples/automate`:
//!
//! - track faders CC 7 / channels 0–8 (`fade[0..=8]`, 8 = master)
//! - device knobs CC 16–23 / channels 0–7 (`dial_0` … `dial_7`)
//! - top knobs CC 48–55 / channel 0 (`dial_top`)
//! - tempo encoder CC 13
//! - clip-stop row note 52 / channels 0–7 → scenes 0–7
//! - scene-launch notes 82–86 → scenes 8–12
//! - note 81 write-preset, note 89 power
//!
//! Output is the original [`crate::apply_dmx_mapping`] room patch.

use crate::{
    apply_dmx_mapping, clamp01, ArtNetPacket, ControllerButtons, ControllerState, ARTNET_BIND_ADDR,
    ARTNET_BROADCAST_ADDR, CONTROLLER_INSTANCE_LOCK_ADDR, DMX_FRAME_DT, DMX_FRAME_HZ,
};
use makepad_micro_serde::*;
use makepad_platform::Cx;
use std::{
    net::UdpSocket,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::JoinHandle;

/// Clip grid + VJ mixer slider. Lighting must not consume these.
pub const VJ_CLIP_NOTE_MAX: u8 = 39;
pub const VJ_CROSSFADER_CC: u8 = 0x0f;

pub const CC_TEMPO: u8 = 13;
pub const CC_FADER: u8 = 7;
pub const CC_DEVICE_KNOB_LO: u8 = 16;
pub const CC_DEVICE_KNOB_HI: u8 = 23;
pub const CC_TOP_KNOB_LO: u8 = 48;
pub const CC_TOP_KNOB_HI: u8 = 55;
pub const NOTE_SCENE_STOP: u8 = 52;
pub const NOTE_SCENE_LAUNCH_LO: u8 = 82;
pub const NOTE_SCENE_LAUNCH_HI: u8 = 86;
pub const NOTE_WRITE: u8 = 81;
pub const NOTE_POWER: u8 = 89;

const SCENE_UI_COOLDOWN_SECS: f64 = 0.350;
pub const SCENE_COUNT: usize = 16;

const DEFAULT_PRESET_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../local/vj/dmx/2025-04-30"
);

// Original numbered scenes only; the recovered dmx.ron current state is excluded.
const RECOVERED_PRESETS: [&str; SCENE_COUNT] = [
    include_str!("../resources/dmx/2025-04-30/dmx0.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx1.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx2.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx3.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx4.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx5.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx6.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx7.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx8.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx9.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx10.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx11.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx12.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx13.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx14.ron"),
    include_str!("../resources/dmx/2025-04-30/dmx15.ron"),
];

/// True when this MIDI message belongs to the VJ clip grid or crossfader.
pub fn is_vj_reserved_midi(data: [u8; 3]) -> bool {
    let status = data[0] >> 4;
    if status == 0x8 || status == 0x9 {
        return data[1] <= VJ_CLIP_NOTE_MAX;
    }
    status == 0xb && data[1] == VJ_CROSSFADER_CC
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeskEvent {
    Ignored,
    Continuous,
    Tempo,
    Power(bool),
    Write(bool),
    SceneLoad(usize),
    SceneSave(usize),
    SceneMissing(usize),
}

#[derive(Clone, Debug)]
pub struct DeskState {
    pub state: ControllerState,
    pub buttons: ControllerButtons,
    pub last_scene: Option<usize>,
    write_preset: bool,
    scene_cooldown_until: f64,
}

impl Default for DeskState {
    fn default() -> Self {
        let mut buttons = ControllerButtons::default();
        buttons.power = true;
        Self {
            state: ControllerState::default(),
            buttons,
            last_scene: None,
            write_preset: false,
            scene_cooldown_until: Cx::monotonic_now(),
        }
    }
}

impl DeskState {
    pub fn handle_midi(&mut self, data: [u8; 3], presets: &PresetBank) -> DeskEvent {
        if is_vj_reserved_midi(data) {
            return DeskEvent::Ignored;
        }
        let status = data[0] >> 4;
        let channel = (data[0] & 0x0f) as usize;
        let data1 = data[1];
        let data2 = data[2];
        if status == 0xb {
            return self.handle_cc(channel, data1, data2);
        }
        if status == 0x8 || status == 0x9 {
            let on = status == 0x9 && data2 != 0;
            return self.handle_note(channel, data1, on, presets);
        }
        DeskEvent::Ignored
    }

    fn handle_cc(&mut self, channel: usize, param: u8, value: u8) -> DeskEvent {
        if param == CC_TEMPO {
            if value == 1 {
                self.state.tempo = (self.state.tempo + 0.02).min(1.0);
            } else {
                self.state.tempo = (self.state.tempo - 0.02).max(0.0);
            }
            return DeskEvent::Tempo;
        }
        let norm = value as f32 / 127.0;
        if param == CC_FADER && channel < self.state.fade.len() {
            self.state.fade[channel] = norm;
            return DeskEvent::Continuous;
        }
        if (CC_DEVICE_KNOB_LO..=CC_DEVICE_KNOB_HI).contains(&param) {
            let index = (param - CC_DEVICE_KNOB_LO) as usize;
            match channel {
                0 => self.state.dial_0[index] = norm,
                1 => self.state.dial_1[index] = norm,
                2 => self.state.dial_2[index] = norm,
                3 => self.state.dial_3[index] = norm,
                4 => self.state.dial_4[index] = norm,
                5 => self.state.dial_5[index] = norm,
                6 => self.state.dial_6[index] = norm,
                7 => self.state.dial_7[index] = norm,
                _ => return DeskEvent::Ignored,
            }
            return DeskEvent::Continuous;
        }
        if channel == 0 && (CC_TOP_KNOB_LO..=CC_TOP_KNOB_HI).contains(&param) {
            self.state.dial_top[(param - CC_TOP_KNOB_LO) as usize] = norm;
            return DeskEvent::Continuous;
        }
        DeskEvent::Ignored
    }

    fn handle_note(
        &mut self,
        channel: usize,
        note: u8,
        on: bool,
        presets: &PresetBank,
    ) -> DeskEvent {
        match note {
            NOTE_WRITE => {
                self.write_preset = on;
                self.buttons.write_preset = on;
                DeskEvent::Write(on)
            }
            NOTE_POWER => {
                self.buttons.power = on;
                DeskEvent::Power(on)
            }
            NOTE_SCENE_STOP if channel < 8 => self.scene_note(channel, on, presets),
            NOTE_SCENE_LAUNCH_LO..=NOTE_SCENE_LAUNCH_HI => {
                let index = (note - NOTE_SCENE_LAUNCH_LO) as usize + 8;
                self.scene_note(index, on, presets)
            }
            _ => DeskEvent::Ignored,
        }
    }

    fn scene_note(&mut self, index: usize, on: bool, presets: &PresetBank) -> DeskEvent {
        if index >= SCENE_COUNT {
            return DeskEvent::Ignored;
        }
        if !on {
            self.buttons.preset[index] = false;
            return DeskEvent::Continuous;
        }
        if Cx::monotonic_now() < self.scene_cooldown_until {
            return DeskEvent::Ignored;
        }
        self.buttons.preset.fill(false);
        self.buttons.preset[index] = true;
        self.scene_cooldown_until = Cx::monotonic_now() + SCENE_UI_COOLDOWN_SECS;
        if self.write_preset {
            presets.save_slot(index, &self.state);
            self.last_scene = Some(index);
            return DeskEvent::SceneSave(index);
        }
        if presets.load_slot(index, &mut self.state) {
            self.last_scene = Some(index);
            DeskEvent::SceneLoad(index)
        } else {
            DeskEvent::SceneMissing(index)
        }
    }

    pub fn set_fader(&mut self, index: usize, value: f32) {
        if index < self.state.fade.len() {
            self.state.fade[index] = clamp01(value);
        }
    }

    pub fn set_top_knob(&mut self, index: usize, value: f32) {
        if index < self.state.dial_top.len() {
            self.state.dial_top[index] = clamp01(value);
        }
    }

    pub fn set_device_knob(&mut self, track: usize, index: usize, value: f32) {
        let value = clamp01(value);
        let bank = match track {
            0 => &mut self.state.dial_0,
            1 => &mut self.state.dial_1,
            2 => &mut self.state.dial_2,
            3 => &mut self.state.dial_3,
            4 => &mut self.state.dial_4,
            5 => &mut self.state.dial_5,
            6 => &mut self.state.dial_6,
            7 => &mut self.state.dial_7,
            _ => return,
        };
        if index < bank.len() {
            bank[index] = value;
        }
    }

    pub fn set_power(&mut self, on: bool) {
        self.buttons.power = on;
    }

    pub fn set_write(&mut self, on: bool) {
        self.write_preset = on;
        self.buttons.write_preset = on;
    }

    pub fn trigger_scene(&mut self, index: usize, presets: &PresetBank) -> DeskEvent {
        self.scene_note(index, true, presets)
    }
}

#[derive(Clone, Debug)]
pub struct PresetBank {
    pub dir: PathBuf,
}

impl PresetBank {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_dir() -> PathBuf {
        if let Ok(path) = std::env::var("VJ_DMX_PRESET_DIR") {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
        PathBuf::from(DEFAULT_PRESET_DIR)
    }

    pub fn current_path(&self) -> PathBuf {
        self.dir.join("current.ron")
    }

    pub fn slot_path(&self, slot: usize) -> PathBuf {
        self.dir.join(format!("preset_{slot:02}.ron"))
    }

    pub fn load_current(&self) -> Option<ControllerState> {
        load_state_file(&self.current_path())
    }

    pub fn save_current(&self, state: &ControllerState) {
        let _ = std::fs::create_dir_all(&self.dir);
        save_state_file(&self.current_path(), state);
    }

    pub fn load_slot(&self, slot: usize, state: &mut ControllerState) -> bool {
        let Some(original) = RECOVERED_PRESETS.get(slot) else {
            return false;
        };
        let path = self.slot_path(slot);
        let loaded = match std::fs::read_to_string(&path) {
            Ok(text) => parse_preset(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // A dangling symlink is still an explicit (broken) override.
                match path.symlink_metadata() {
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => parse_preset(original),
                    _ => None,
                }
            }
            Err(_) => None,
        };
        // An existing unreadable or invalid overlay must never select another scene.
        let Some(mut loaded) = loaded else {
            return false;
        };
        // Automate kept the smoke-timing bank (`dial_0`) across scene loads.
        loaded.dial_0 = state.dial_0;
        *state = loaded;
        true
    }

    pub fn save_slot(&self, slot: usize, state: &ControllerState) {
        if slot >= SCENE_COUNT {
            return;
        }
        let _ = std::fs::create_dir_all(&self.dir);
        save_state_file(&self.slot_path(slot), state);
    }
}

#[derive(DeRon)]
struct LegacyControllerState {
    fade: [f32; 9],
    dial_a: [f32; 8],
    dial_b: [f32; 8],
    dial_c: [f32; 8],
}

fn parse_preset(text: &str) -> Option<ControllerState> {
    if let Ok(state) = ControllerState::deserialize_ron(text) {
        return Some(state);
    }
    let legacy = LegacyControllerState::deserialize_ron(text).ok()?;
    // The January 14 hardware migration (683bbf606) merged both speed knobs.
    // They agree (both zero) in all three recovered legacy scenes. Reject an
    // ambiguous override rather than silently discarding one of its speeds.
    if legacy.dial_a[6] != legacy.dial_a[7] {
        return None;
    }
    let mut state = ControllerState {
        fade: legacy.fade,
        tempo: legacy.dial_a[6],
        ..Default::default()
    };
    state.fade.swap(1, 2); // Inner/outer movers.
    state.fade.swap(6, 7); // UV.
    state.dial_1[..6].copy_from_slice(&legacy.dial_a[..6]);
    state.dial_top[0] = legacy.dial_c[0];
    state.dial_top[1] = legacy.dial_c[2];
    state.dial_top[2] = legacy.dial_c[1];
    state.dial_top[3] = legacy.dial_c[3];
    state.dial_top[5] = legacy.dial_b[1];
    state.dial_5[..4].copy_from_slice(&[
        legacy.dial_b[0],
        legacy.dial_b[2],
        legacy.dial_b[3],
        legacy.dial_b[4],
    ]);
    state.dial_0[..3].copy_from_slice(&legacy.dial_b[5..8]);
    Some(state)
}

fn load_state_file(path: &Path) -> Option<ControllerState> {
    let text = std::fs::read_to_string(path).ok()?;
    ControllerState::deserialize_ron(&text).ok()
}

fn save_state_file(path: &Path, state: &ControllerState) {
    let _ = std::fs::write(path, state.serialize_ron().as_bytes());
}

#[derive(Clone, Debug)]
pub struct RoomSnapshot {
    pub state: ControllerState,
    pub buttons: ControllerButtons,
    pub last_scene: Option<usize>,
    pub packets: u64,
}

/// Automate-identical Art-Net writer: 44 Hz, universe 0, `apply_dmx_mapping`.
pub struct RoomShow {
    shared: Arc<Mutex<DeskState>>,
    packets: Arc<Mutex<u64>>,
    presets: PresetBank,
    stop: Arc<AtomicBool>,
    #[cfg(not(target_arch = "wasm32"))]
    thread: Option<JoinHandle<()>>,
}

impl RoomShow {
    #[cfg(target_arch = "wasm32")]
    pub fn start(_preset_dir: PathBuf, _target_addr: String) -> Result<Self, String> {
        Err("Art-Net room control is unavailable on web".to_string())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(preset_dir: PathBuf, target_addr: String) -> Result<Self, String> {
        let presets = PresetBank::new(preset_dir);
        let mut desk = DeskState::default();
        if let Some(state) = presets.load_current() {
            desk.state = state;
        }
        desk.buttons.power = true;

        let instance_lock = UdpSocket::bind(CONTROLLER_INSTANCE_LOCK_ADDR).map_err(|e| {
            format!("show control already active at {CONTROLLER_INSTANCE_LOCK_ADDR}: {e}")
        })?;
        let socket = UdpSocket::bind(ARTNET_BIND_ADDR)
            .map_err(|e| format!("Art-Net bind {ARTNET_BIND_ADDR} failed: {e}"))?;
        socket
            .set_broadcast(true)
            .map_err(|e| format!("Art-Net broadcast setup failed: {e}"))?;

        let shared = Arc::new(Mutex::new(desk));
        let packets = Arc::new(Mutex::new(0u64));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_shared = shared.clone();
        let worker_packets = packets.clone();
        let worker_stop = stop.clone();
        let worker_presets = presets.clone();
        let thread = std::thread::Builder::new()
            .name("makepad-artnet-room-desk".into())
            .spawn(move || {
                let _instance_lock = instance_lock;
                let mut universe = ArtNetPacket::default();
                let mut persist = 0u32;
                let mut clock = 0.0f64;
                let mut sent = 0u64;
                while !worker_stop.load(Ordering::Acquire) {
                    let started = Cx::monotonic_now();
                    let desk = worker_shared.lock().unwrap().clone();
                    universe.set_sequence((sent % 255 + 1) as u8);
                    {
                        let dmx = universe.dmx_mut();
                        dmx.fill(0);
                        apply_dmx_mapping(&desk.state, &desk.buttons, dmx, clock);
                    }
                    let _ = socket.send_to(universe.as_bytes(), target_addr.as_str());
                    sent += 1;
                    persist += 1;
                    clock += DMX_FRAME_DT;
                    *worker_packets.lock().unwrap() = sent;
                    if persist >= DMX_FRAME_HZ as u32 {
                        worker_presets.save_current(&desk.state);
                        persist = 0;
                    }
                    let spent = Cx::monotonic_now() - started;
                    if spent < DMX_FRAME_DT {
                        std::thread::sleep(Duration::from_secs_f64(DMX_FRAME_DT - spent));
                    }
                }
                // Terminal blackout must reach the node that received the live
                // frames: a unicast target never sees a broadcast-only frame
                // and would hold the last lit universe (smoke channel included)
                // after shutdown. Broadcast too, belt-and-braces.
                let mut universe = ArtNetPacket::default();
                universe.dmx_mut().fill(0);
                let _ = socket.send_to(universe.as_bytes(), target_addr.as_str());
                let _ = socket.send_to(universe.as_bytes(), ARTNET_BROADCAST_ADDR);
            })
            .map_err(|e| format!("room desk thread: {e}"))?;

        Ok(Self {
            shared,
            packets,
            presets,
            stop,
            thread: Some(thread),
        })
    }

    pub fn handle_midi(&self, data: [u8; 3]) -> DeskEvent {
        self.shared
            .lock()
            .unwrap()
            .handle_midi(data, &self.presets)
    }

    pub fn set_fader(&self, index: usize, value: f32) {
        self.shared.lock().unwrap().set_fader(index, value);
    }

    pub fn set_top_knob(&self, index: usize, value: f32) {
        self.shared.lock().unwrap().set_top_knob(index, value);
    }

    pub fn set_device_knob(&self, track: usize, index: usize, value: f32) {
        self.shared
            .lock()
            .unwrap()
            .set_device_knob(track, index, value);
    }

    pub fn set_power(&self, on: bool) {
        self.shared.lock().unwrap().set_power(on);
    }

    pub fn set_write(&self, on: bool) {
        self.shared.lock().unwrap().set_write(on);
    }

    pub fn trigger_scene(&self, index: usize) -> DeskEvent {
        self.shared
            .lock()
            .unwrap()
            .trigger_scene(index, &self.presets)
    }

    pub fn snapshot(&self) -> RoomSnapshot {
        let desk = self.shared.lock().unwrap();
        RoomSnapshot {
            state: desk.state,
            buttons: desk.buttons,
            last_scene: desk.last_scene,
            packets: *self.packets.lock().unwrap(),
        }
    }
}

impl Drop for RoomShow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestBank(PresetBank);

    impl TestBank {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../local/agent_state/dmx-preset-tests")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
            std::fs::create_dir(&dir).unwrap();
            Self(PresetBank::new(dir))
        }
    }

    impl Drop for TestBank {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0.dir).unwrap();
        }
    }

    fn assert_state_eq(actual: &ControllerState, expected: &ControllerState) {
        assert_eq!(actual.serialize_ron(), expected.serialize_ron());
    }

    #[test]
    fn all_sixteen_originals_load_with_source_values_and_preserve_smoke() {
        let bank = TestBank::new();
        // Literal dial_5[0] and dial_top[2] values from the originals (legacy
        // b0/c1 for P14–P16). Repeated P9–P12 are intentional, not deduplicated.
        let expected = [
            (0.4566929, 0.28346458),
            (0.015748031, 0.31496063),
            (0.48818898, 0.5590551),
            (0.15748031, 0.97637796),
            (0.015748031, 0.31496063),
            (0.9212598, 0.27559054),
            (0.03937008, 0.22834645),
            (0.28346458, 0.27559054),
            (0.511811, 0.28346458),
            (0.511811, 0.28346458),
            (0.511811, 0.28346458),
            (0.511811, 0.28346458),
            (0.511811, 0.9055118),
            (0.93700784, 0.61417323),
            (0.93700784, 0.61417323),
            (0.36220473, 0.8976378),
        ];
        assert_eq!(SCENE_COUNT, expected.len());
        let smoke = [0.11, 0.22, 0.33, 0.44, 0.55, 0.66, 0.77, 0.88];
        for (slot, (laser, color)) in expected.into_iter().enumerate() {
            let mut state = ControllerState { dial_0: smoke, ..Default::default() };
            assert!(bank.0.load_slot(slot, &mut state), "P{}", slot + 1);
            assert_eq!(state.dial_5[0], laser, "P{} laser", slot + 1);
            assert_eq!(state.dial_top[2], color, "P{} color", slot + 1);
            assert_eq!(state.tempo, if slot == 7 { 0.27999997 } else { 0.0 });
            assert_eq!(state.dial_0, smoke);
            if slot < 13 {
                let mut original = ControllerState::deserialize_ron(RECOVERED_PRESETS[slot]).unwrap();
                original.dial_0 = smoke;
                assert_state_eq(&state, &original);
            }
        }
        // Reading fallbacks does not copy them into the writable overlay.
        assert_eq!(std::fs::read_dir(&bank.0.dir).unwrap().count(), 0);
    }

    #[test]
    fn recovered_legacy_values_follow_the_january_hardware_migration() {
        let expected = [
            (13, [0.0, 0.0, 0.27559054, 0.023622047, 0.0, 0.7559055, 0.0, 0.0, 0.015748031],
                [0.38582677, 0.77165353, 0.61417323, 0.5748032, 0.0, 0.14173229, 0.0, 0.0],
                [0.93700784, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0], 0.26771653),
            (14, [0.0, 0.023622047, 0.0, 0.0, 0.0, 0.7559055, 0.26771653, 1.0, 0.015748031],
                [0.38582677, 0.1496063, 0.61417323, 0.5748032, 0.0, 0.070866145, 0.0, 0.0],
                [0.93700784, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0], 0.0),
            (15, [0.0, 0.23622048, 0.1496063, 0.023622047, 0.0, 0.7559055, 0.0, 0.0, 0.015748031],
                [0.38582677, 0.28346458, 0.8976378, 0.68503934, 0.0, 0.0, 0.0, 0.0],
                [0.36220473, 0.42519686, 0.27559054, 0.8425197, 0.0, 0.0, 0.0, 0.0], 0.26771653),
        ];
        for (slot, fade, dial_top, dial_5, smoke) in expected {
            let legacy = LegacyControllerState::deserialize_ron(RECOVERED_PRESETS[slot]).unwrap();
            assert_eq!(legacy.dial_a[6], 0.0);
            assert_eq!(legacy.dial_a[7], legacy.dial_a[6]);
            let converted = parse_preset(RECOVERED_PRESETS[slot]).unwrap();
            let expected = ControllerState {
                fade,
                dial_top,
                dial_5,
                dial_1: [0.7480315, 0.15748031, 0.0, 0.0, 0.43307087, 0.0, 0.0, 0.0],
                dial_0: [0.0, 0.0, smoke, 0.0, 0.0, 0.0, 0.0, 0.0],
                ..Default::default()
            };
            assert_state_eq(&converted, &expected);
        }
    }

    const LEGACY_TEST: &str = "(
        fade:(0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8),
        dial_a:(0.11, 0.12, 0.13, 0.14, 0.15, 0.16, 0.17, 0.17),
        dial_b:(0.21, 0.22, 0.23, 0.24, 0.25, 0.26, 0.27, 0.28),
        dial_c:(0.31, 0.32, 0.33, 0.34, 0.35, 0.36, 0.37, 0.38),
    )";

    #[test]
    fn legacy_mapping_retains_active_fields_even_when_originals_have_zeroes() {
        let expected = ControllerState {
            fade: [0.0, 0.2, 0.1, 0.3, 0.4, 0.5, 0.7, 0.6, 0.8],
            tempo: 0.17,
            dial_0: [0.26, 0.27, 0.28, 0.0, 0.0, 0.0, 0.0, 0.0],
            dial_1: [0.11, 0.12, 0.13, 0.14, 0.15, 0.16, 0.0, 0.0],
            dial_5: [0.21, 0.23, 0.24, 0.25, 0.0, 0.0, 0.0, 0.0],
            dial_top: [0.31, 0.33, 0.32, 0.34, 0.0, 0.22, 0.0, 0.0],
            ..Default::default()
        };
        assert_state_eq(&parse_preset(LEGACY_TEST).unwrap(), &expected);
        assert!(parse_preset(&LEGACY_TEST.replace("0.17, 0.17", "0.17, 0.18")).is_none());
    }

    #[test]
    fn all_scenes_trigger_through_the_shared_desk() {
        let bank = TestBank::new();
        let mut desk = DeskState::default();
        desk.state.dial_0 = [0.42; 8];
        for slot in 0..SCENE_COUNT {
            desk.scene_cooldown_until = 0.0;
            assert_eq!(desk.trigger_scene(slot, &bank.0), DeskEvent::SceneLoad(slot));
            assert_eq!(desk.last_scene, Some(slot));
            assert_eq!(desk.buttons.preset.len(), SCENE_COUNT);
            for (index, pressed) in desk.buttons.preset.iter().enumerate() {
                assert_eq!(*pressed, index == slot);
            }
            assert_eq!(desk.state.dial_0, [0.42; 8]);
        }
        assert_eq!(desk.trigger_scene(SCENE_COUNT, &bank.0), DeskEvent::Ignored);
    }

    #[test]
    fn original_thirteen_midi_scene_routes_keep_their_slots() {
        let bank = TestBank::new();
        let mut desk = DeskState::default();
        for slot in 0..13 {
            let (channel, note) = if slot < 8 { (slot as u8, 52) } else { (0, 82 + slot as u8 - 8) };
            desk.scene_cooldown_until = 0.0;
            assert_eq!(desk.handle_midi(note_on(channel, note), &bank.0), DeskEvent::SceneLoad(slot));
            assert_eq!(desk.last_scene, Some(slot));
            let mut expected = ControllerState::default();
            assert!(bank.0.load_slot(slot, &mut expected));
            assert_state_eq(&desk.state, &expected);
            assert_eq!(desk.handle_midi([0x80 | channel, note, 0], &bank.0), DeskEvent::Continuous);
            assert!(!desk.buttons.preset[slot]);
        }
        for data in [note_on(8, 52), note_on(9, 52), note_on(10, 52), note_on(0, 87), note_on(0, 88)] {
            desk.scene_cooldown_until = 0.0;
            assert_eq!(desk.handle_midi(data, &bank.0), DeskEvent::Ignored);
        }
    }

    #[test]
    fn explicit_overlay_wins_and_saves_only_to_that_overlay() {
        let bank = TestBank::new();
        let mut desk = DeskState::default();
        desk.state.fade = [0.37; 9];
        desk.state.tempo = 0.61;
        desk.state.dial_0 = [0.28; 8];
        desk.set_write(true);
        assert_eq!(desk.trigger_scene(15, &bank.0), DeskEvent::SceneSave(15));
        let mut loaded = ControllerState { dial_0: [0.82; 8], ..Default::default() };
        assert!(bank.0.load_slot(15, &mut loaded));
        let mut expected = desk.state;
        expected.dial_0 = loaded.dial_0;
        assert_state_eq(&loaded, &expected);
        assert_eq!(loaded.dial_0, [0.82; 8]);
        assert_eq!(std::fs::read_dir(&bank.0.dir).unwrap().count(), 1);
        assert!(bank.0.load_slot(14, &mut loaded));
        assert_eq!(loaded.dial_top[1], 0.1496063);

        std::fs::write(bank.0.slot_path(15), LEGACY_TEST).unwrap();
        assert!(bank.0.load_slot(15, &mut loaded));
        assert_eq!(loaded.tempo, 0.17);
        assert_eq!(loaded.dial_5[0], 0.21);
        assert_eq!(loaded.dial_0, [0.82; 8]);
    }

    #[test]
    fn invalid_or_unreadable_overrides_do_not_fall_back_or_change_state() {
        let bank = TestBank::new();
        let mut desk = DeskState::default();
        desk.state.fade = [0.37; 9];
        desk.last_scene = Some(2);
        let before = desk.state;
        for invalid in [String::new(), "(fade: wrong)".into(), LEGACY_TEST.replace("0.17, 0.17", "0.17, 0.18")] {
            std::fs::write(bank.0.slot_path(13), invalid).unwrap();
            desk.scene_cooldown_until = 0.0;
            assert_eq!(desk.trigger_scene(13, &bank.0), DeskEvent::SceneMissing(13));
            assert_state_eq(&desk.state, &before);
            assert_eq!(desk.last_scene, Some(2));
        }
        std::fs::remove_file(bank.0.slot_path(13)).unwrap();
        std::fs::create_dir(bank.0.slot_path(13)).unwrap();
        assert!(!bank.0.load_slot(13, &mut desk.state));
        assert_state_eq(&desk.state, &before);
        assert!(!bank.0.load_slot(SCENE_COUNT, &mut desk.state));
        #[cfg(unix)]
        {
            std::fs::remove_dir(bank.0.slot_path(13)).unwrap();
            std::os::unix::fs::symlink(bank.0.dir.join("missing.ron"), bank.0.slot_path(13)).unwrap();
            assert!(!bank.0.load_slot(13, &mut desk.state));
            assert_state_eq(&desk.state, &before);
        }
    }

    #[test]
    fn current_state_comes_only_from_the_writable_overlay() {
        let bank = TestBank::new();
        assert!(bank.0.load_current().is_none());
        let current = ControllerState { tempo: 0.71, dial_0: [0.83; 8], ..Default::default() };
        std::fs::write(bank.0.dir.join("dmx.ron"), current.serialize_ron()).unwrap();
        assert!(bank.0.load_current().is_none());
        bank.0.save_current(&current);
        assert_state_eq(&bank.0.load_current().unwrap(), &current);
        let mut scene = ControllerState::default();
        assert!(bank.0.load_slot(0, &mut scene));
        assert_eq!(scene.tempo, 0.0);
    }

    #[test]
    fn default_overlay_is_absolute_and_environment_override_is_honored() {
        let default = PathBuf::from(DEFAULT_PRESET_DIR);
        assert!(default.is_absolute());
        assert!(default.ends_with("local/vj/dmx/2025-04-30"));
        let expected = std::env::var("VJ_DMX_PRESET_DIR")
            .ok()
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or(default);
        assert_eq!(PresetBank::default_dir(), expected);
    }

    fn cc(channel: u8, param: u8, value: u8) -> [u8; 3] {
        [0xb0 | channel, param, value]
    }

    fn note_on(channel: u8, note: u8) -> [u8; 3] {
        [0x90 | channel, note, 127]
    }

    #[test]
    fn clip_grid_and_crossfader_are_left_to_vj() {
        let mut desk = DeskState::default();
        let presets = PresetBank::new("/tmp/vj-desk-none");
        desk.state.fade[0] = 0.25;
        assert_eq!(
            desk.handle_midi(note_on(0, 0), &presets),
            DeskEvent::Ignored
        );
        assert_eq!(
            desk.handle_midi(note_on(0, 39), &presets),
            DeskEvent::Ignored
        );
        assert_eq!(
            desk.handle_midi(cc(0, VJ_CROSSFADER_CC, 127), &presets),
            DeskEvent::Ignored
        );
        assert_eq!(desk.state.fade[0], 0.25);
        assert!(is_vj_reserved_midi(note_on(3, 12)));
        assert!(!is_vj_reserved_midi(cc(0, CC_FADER, 64)));
    }

    #[test]
    fn faders_and_device_knobs_match_automate() {
        let mut desk = DeskState::default();
        let presets = PresetBank::new("/tmp/vj-desk-none");
        assert_eq!(
            desk.handle_midi(cc(3, CC_FADER, 127), &presets),
            DeskEvent::Continuous
        );
        assert_eq!(desk.state.fade[3], 1.0);
        assert_eq!(
            desk.handle_midi(cc(1, 18, 64), &presets),
            DeskEvent::Continuous
        );
        assert!((desk.state.dial_1[2] - 64.0 / 127.0).abs() < 1e-6);
        assert_eq!(
            desk.handle_midi(cc(0, 50, 0), &presets),
            DeskEvent::Continuous
        );
        assert_eq!(desk.state.dial_top[2], 0.0);
    }

    #[test]
    fn power_and_write_notes_are_lighting_not_vj_surfaces() {
        let mut desk = DeskState::default();
        let presets = PresetBank::new("/tmp/vj-desk-none");
        assert_eq!(
            desk.handle_midi([0x80, NOTE_POWER, 0], &presets),
            DeskEvent::Power(false)
        );
        assert!(!desk.buttons.power);
        assert_eq!(
            desk.handle_midi(note_on(0, NOTE_WRITE), &presets),
            DeskEvent::Write(true)
        );
        assert!(desk.write_preset);
    }
}
