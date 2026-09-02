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
pub const SCENE_COUNT: usize = 13;

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
        PathBuf::from("examples/automate/local/dmx")
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
        let Some(mut loaded) = load_state_file(&self.slot_path(slot)) else {
            return false;
        };
        // Automate kept the smoke-timing bank (`dial_0`) across scene loads.
        loaded.dial_0 = state.dial_0;
        *state = loaded;
        true
    }

    pub fn save_slot(&self, slot: usize, state: &ControllerState) {
        let _ = std::fs::create_dir_all(&self.dir);
        save_state_file(&self.slot_path(slot), state);
    }
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
