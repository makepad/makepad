use crate::makepad_micro_serde::*;
use makepad_widgets::*;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

app_main!(App);

const DMX_FRAME_HZ: f64 = 44.0;
const DMX_FRAME_DT: f64 = 1.0 / DMX_FRAME_HZ;
const ARTNET_BIND_ADDR: &str = "0.0.0.0:0";
const ARTNET_BROADCAST_ADDR: &str = "255.255.255.255:6454";
const PRESET_DIR: &str = "examples/automate/local/dmx";
const CURRENT_STATE_FILE: &str = "examples/automate/local/dmx/current.ron";
const DEBUG_SCENE_EVENTS: bool = true;
const CONTROLLER_INSTANCE_LOCK_ADDR: &str = "127.0.0.1:64640";

pub const DMXOUTPUT_HEADER: [u8; 18] = [
    b'A', b'r', b't', b'-', b'N', b'e', b't', b'\0', 0,    // opcode hi
    0x50, // opcode lo = output
    0,    // proto hi
    0xe,  // proto lo = 14
    0,    // sequence
    0,    // physical
    0,    // sub uni
    0,    // net
    2,    // buffer hi size (512)
    0,    // buffer lo
];

script_mod! {
use mod.prelude.widgets.*

let DeckFrame = RoundedView{
    width: Fill
    height: Fill
    flow: Down
    spacing: 12
    padding: Inset{top: 14. bottom: 14. left: 14. right: 14.}
    draw_bg.color: #x0b141f
    draw_bg.border_radius: 14.
    draw_bg.border_size: 1.
    draw_bg.border_color: #x2a3645
}

let SectionFrame = RoundedView{
    width: Fill
    height: Fit
    flow: Down
    spacing: 8
    padding: Inset{top: 10. bottom: 10. left: 10. right: 10.}
    draw_bg.color: #x111c2a
    draw_bg.border_radius: 10.
    draw_bg.border_size: 1.
    draw_bg.border_color: #x304359
}

let SectionTitle = Label{
    width: Fit
    draw_text.color: #xd2deef
    draw_text.text_style: theme.font_bold{font_size: 11.}
}

let ScenePad = Button{
    width: 88
    height: 36
    draw_text +: {
        color: #xbfd2ec
        color_hover: #xeff5ff
        color_down: #xffffff
        color_focus: #xffffff
    }
    draw_bg +: {
        border_radius: uniform(7.)
        color: uniform(#x162638)
        color_hover: uniform(#x1d3249)
        color_down: uniform(#x304f73)
        color_focus: uniform(#x365d87)
        border_color: uniform(#x2f4966)
        border_color_hover: uniform(#x48698f)
        border_color_down: uniform(#x66a0e2)
        border_color_focus: uniform(#x78bcff)
        border_color_2: uniform(#x223447)
        border_color_2_hover: uniform(#x37516c)
        border_color_2_down: uniform(#x6fa5e2)
        border_color_2_focus: uniform(#x86c7ff)
    }
}

let TransportToggle = Toggle{
    width: 108
    height: 34
    draw_text +: {
        color: #xbfd0e4
        color_hover: #xdbe7f7
        color_down: #xffffff
        color_active: #xd6ffe2
    }
    draw_bg +: {
        color: uniform(#x131f2f)
        color_hover: uniform(#x1a2a3f)
        color_down: uniform(#x102134)
        color_active: uniform(#x1f3f33)
        color_focus: uniform(#x2a4664)
        border_color: uniform(#x34465a)
        border_color_hover: uniform(#x4d6683)
        border_color_down: uniform(#x3b4f68)
        border_color_active: uniform(#x4d8f6d)
        border_color_focus: uniform(#x68a7e8)
        mark_color: uniform(#x8b99ab)
        mark_color_hover: uniform(#xb5c6de)
        mark_color_down: uniform(#xd4e0ee)
        mark_color_active: uniform(#x84f0b0)
        mark_color_active_hover: uniform(#xa6ffca)
    }
}

let EncoderKnob = Rotary{
    width: 72
    height: 72
    min: 0.
    max: 127.
    precision: 0
}

let APCFader = Slider{
    axis: Vertical
    width: 54
    height: 220
    min: 0.
    max: 127.
    precision: 0
    step: 1.
    text: ""

    draw_bg +: {
        body_color: uniform(#x08121d)
        track_color: uniform(#x111d2b)
        fill_color: uniform(#x79b8f2)
        cap_color: uniform(#xe1e7f0)
        cap_shadow: uniform(#x8d98a7)

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(3., 2., self.rect_size.x - 6., self.rect_size.y - 4., 6.)
            sdf.fill(self.body_color)

            let top = 12.
            let bottom_pad = 12.
            let h = self.rect_size.y - top - bottom_pad
            let track_w = self.rect_size.x * 0.30
            let track_x = (self.rect_size.x - track_w) * 0.5

            sdf.box(track_x, top, track_w, h, 3.)
            sdf.fill(self.track_color)

            let fill_h = h * self.slide_pos
            let fill_h_inner = max(1., fill_h - 3.)
            sdf.box(
                track_x + 1.5
                top + (h - fill_h) + 1.5
                track_w - 3.
                fill_h_inner
                2.
            )
            sdf.fill(self.fill_color)

            let cap_h = 16.
            let cap_y = top + (h - fill_h) - cap_h * 0.5
            sdf.box(7., cap_y + 1.5, self.rect_size.x - 14., cap_h, 4.)
            sdf.fill(self.cap_shadow)
            sdf.box(6., cap_y, self.rect_size.x - 12., cap_h, 4.)
            sdf.fill(self.cap_color)

            return sdf.result
        }
    }
}

startup() do #(App::script_component(vm)){
    ui: Root{
        main_window := Window{
            window.title: "Automate - Control Surface"
            window.inner_size: vec2(1520, 940)
            pass.clear_color: vec4(0.03, 0.05, 0.08, 1.0)
            body +: {
                    app_root := SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        padding: Inset{top: 14. bottom: 14. left: 14. right: 14.}
                        draw_bg.color: #x070d14

                        deck := DeckFrame{
                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 12

                                SectionFrame{
                                    width: Fill
                                    SectionTitle{text: "ENCODERS"}
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Right
                                        spacing: 7
                                        top_knob_0 := EncoderKnob{text: "T0"}
                                        top_knob_1 := EncoderKnob{text: "T1"}
                                        top_knob_2 := EncoderKnob{text: "T2"}
                                        top_knob_3 := EncoderKnob{text: "T3"}
                                        top_knob_4 := EncoderKnob{text: "T4"}
                                        top_knob_5 := EncoderKnob{text: "T5"}
                                        top_knob_6 := EncoderKnob{text: "T6"}
                                        top_knob_7 := EncoderKnob{text: "T7"}
                                    }
                                }

                                SectionFrame{
                                    width: 360
                                    SectionTitle{text: "TRANSPORT"}
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 6
                                        View{
                                            width: Fit
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            trn_btn_0 := TransportToggle{text: "REW"}
                                            trn_btn_1 := TransportToggle{text: "FF"}
                                            trn_btn_2 := TransportToggle{text: "STOP"}
                                        }
                                        View{
                                            width: Fit
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            trn_btn_3 := TransportToggle{text: "PLAY"}
                                            trn_btn_4 := TransportToggle{text: "REC"}
                                            trn_btn_5 := TransportToggle{text: "SHIFT"}
                                        }
                                        View{
                                            width: Fit
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            trn_btn_6 := TransportToggle{text: "TAP"}
                                            trn_btn_7 := TransportToggle{text: "WRITE"}
                                            trn_btn_8 := TransportToggle{text: "POWER"}
                                        }
                                    }
                                }
                            }

                            SectionFrame{
                                width: Fill
                                SectionTitle{text: "SCENES"}
                                View{
                                    width: Fit
                                    height: Fit
                                    flow: Right
                                    spacing: 6
                                    scene_btn_0 := ScenePad{text: "P00"}
                                    scene_btn_1 := ScenePad{text: "P01"}
                                    scene_btn_2 := ScenePad{text: "P02"}
                                    scene_btn_3 := ScenePad{text: "P03"}
                                    scene_btn_4 := ScenePad{text: "P04"}
                                    scene_btn_5 := ScenePad{text: "P05"}
                                    scene_btn_6 := ScenePad{text: "P06"}
                                }
                                View{
                                    width: Fit
                                    height: Fit
                                    flow: Right
                                    spacing: 6
                                    scene_btn_7 := ScenePad{text: "P07"}
                                    scene_btn_8 := ScenePad{text: "P08"}
                                    scene_btn_9 := ScenePad{text: "P09"}
                                    scene_btn_10 := ScenePad{text: "P10"}
                                    scene_btn_11 := ScenePad{text: "P11"}
                                    scene_btn_12 := ScenePad{text: "P12"}
                                }
                            }

                            SectionFrame{
                                width: Fill
                                SectionTitle{text: "FADERS"}
                                View{
                                    width: Fit
                                    height: Fit
                                    flow: Right
                                    spacing: 8
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 4
                                        align: Align{x: 0.5}
                                        Label{text: "F0" draw_text.color: #x95a8c0 draw_text.text_style.font_size: 11.}
                                        fader_0 := APCFader{}
                                    }
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 4
                                        align: Align{x: 0.5}
                                        Label{text: "F1" draw_text.color: #x95a8c0 draw_text.text_style.font_size: 11.}
                                        fader_1 := APCFader{}
                                    }
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 4
                                        align: Align{x: 0.5}
                                        Label{text: "F2" draw_text.color: #x95a8c0 draw_text.text_style.font_size: 11.}
                                        fader_2 := APCFader{}
                                    }
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 4
                                        align: Align{x: 0.5}
                                        Label{text: "F3" draw_text.color: #x95a8c0 draw_text.text_style.font_size: 11.}
                                        fader_3 := APCFader{}
                                    }
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 4
                                        align: Align{x: 0.5}
                                        Label{text: "F4" draw_text.color: #x95a8c0 draw_text.text_style.font_size: 11.}
                                        fader_4 := APCFader{}
                                    }
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 4
                                        align: Align{x: 0.5}
                                        Label{text: "F5" draw_text.color: #x95a8c0 draw_text.text_style.font_size: 11.}
                                        fader_5 := APCFader{}
                                    }
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 4
                                        align: Align{x: 0.5}
                                        Label{text: "F6" draw_text.color: #x95a8c0 draw_text.text_style.font_size: 11.}
                                        fader_6 := APCFader{}
                                    }
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 4
                                        align: Align{x: 0.5}
                                        Label{text: "F7" draw_text.color: #x95a8c0 draw_text.text_style.font_size: 11.}
                                        fader_7 := APCFader{}
                                    }
                                    View{
                                        width: Fit
                                        height: Fit
                                        flow: Down
                                        spacing: 4
                                        align: Align{x: 0.5}
                                        Label{text: "M" draw_text.color: #x95a8c0 draw_text.text_style.font_size: 11.}
                                        fader_8 := APCFader{}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, SerRon, DeRon)]
struct ControllerState {
    fade: [f32; 9],
    tempo: f32,
    dial_0: [f32; 8],
    dial_1: [f32; 8],
    dial_2: [f32; 8],
    dial_3: [f32; 8],
    dial_4: [f32; 8],
    dial_5: [f32; 8],
    dial_6: [f32; 8],
    dial_7: [f32; 8],
    dial_top: [f32; 8],
}

impl ControllerState {
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ControllerButtons {
    preset: [bool; 13],
    write_preset: bool,
    power: bool,
}

impl ControllerButtons {
}

#[derive(Debug, Clone)]
struct MidiMirrorState {
    cc: [[u8; 128]; 16],
    note_velocity: [[u8; 128]; 16],
}

impl Default for MidiMirrorState {
    fn default() -> Self {
        Self {
            cc: [[0; 128]; 16],
            note_velocity: [[0; 128]; 16],
        }
    }
}

impl MidiMirrorState {
    fn set_cc(&mut self, channel: usize, param: usize, value: u8) {
        if channel < self.cc.len() && param < self.cc[channel].len() {
            self.cc[channel][param] = value;
        }
    }

    fn set_note(&mut self, channel: usize, note: usize, is_on: bool, velocity: u8) {
        if channel < self.note_velocity.len() && note < self.note_velocity[channel].len() {
            self.note_velocity[channel][note] = if is_on { velocity.max(1) } else { 0 };
        }
    }

    fn note_is_on(&self, channel: usize, note: usize) -> bool {
        if channel < self.note_velocity.len() && note < self.note_velocity[channel].len() {
            self.note_velocity[channel][note] > 0
        } else {
            false
        }
    }
}

#[derive(Debug, Clone)]
struct ControllerSnapshot {
    state: ControllerState,
    mirror: MidiMirrorState,
    last_event: String,
    dmx_packets: u64,
}

impl Default for ControllerSnapshot {
    fn default() -> Self {
        Self {
            state: ControllerState::default(),
            mirror: MidiMirrorState::default(),
            last_event: "Waiting for MIDI input...".to_string(),
            dmx_packets: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum UiControlMessage {
    SetTopKnob { index: usize, value: f32 },
    SetFader { index: usize, value: f32 },
    TriggerScene { index: usize },
    SetTransport { index: usize, on: bool },
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn hsv_to_rgb(mut hue: f32, sat: f32, val: f32) -> (f32, f32, f32) {
    hue = clamp01(hue) * 6.0;
    let sector = hue.floor() as i32;
    let fract = hue - sector as f32;
    let p = val * (1.0 - sat);
    let q = val * (1.0 - sat * fract);
    let t = val * (1.0 - sat * (1.0 - fract));
    match sector {
        0 => (val, t, p),
        1 => (q, val, p),
        2 => (p, val, t),
        3 => (p, q, val),
        4 => (t, p, val),
        _ => (val, p, q),
    }
}

fn map_wargb(value: f32, fade: f32, out: &mut [u8], bases: &[usize]) {
    let fade = clamp01(fade);
    let (r, g, b) = hsv_to_rgb(value, 1.0, fade);
    for base in bases {
        if *base == 0 || *base + 1 >= out.len() {
            continue;
        }
        out[*base - 1] = (r * 255.0) as u8;
        out[*base] = (g * 255.0) as u8;
        out[*base + 1] = (b * 255.0) as u8;
    }
}

fn dmx_u8(value: u8, out: &mut [u8], bases: &[usize], channel: usize) {
    for base in bases {
        let index = base.saturating_sub(1) + channel.saturating_sub(1);
        if index < out.len() {
            out[index] = value;
        }
    }
}

fn dmx_f32(value: f32, out: &mut [u8], bases: &[usize], channel: usize) {
    let value = (clamp01(value) * 255.0) as u8;
    dmx_u8(value, out, bases, channel);
}

fn apply_dmx_mapping(
    state: &ControllerState,
    buttons: &ControllerButtons,
    dmx: &mut [u8],
    clock: f64,
) {
    if !buttons.power {
        return;
    }

    map_wargb(state.dial_top[3], 1.0, dmx, &[110 + 2 - 1]);
    let rgb_laser_addr = 110;
    match (state.fade[3] * 3.0) as usize {
        0 => {
            dmx_u8(0, dmx, &[rgb_laser_addr], 1);
        }
        1 => {
            dmx_u8(255, dmx, &[rgb_laser_addr], 1);
            dmx_f32(0.75, dmx, &[rgb_laser_addr], 6);
            dmx_u8(32, dmx, &[rgb_laser_addr], 7);
        }
        2 => {
            dmx_u8(255, dmx, &[rgb_laser_addr], 1);
            dmx_f32(1.0, dmx, &[rgb_laser_addr], 6);
            dmx_u8(32, dmx, &[rgb_laser_addr], 7);
        }
        _ => {}
    }

    map_wargb(state.dial_top[3], 1.0, dmx, &[rgb_laser_addr + 2 - 1]);
    match (state.fade[3] * 4.0) as usize {
        0 => dmx_u8(0, dmx, &[rgb_laser_addr], 1),
        1 => {
            dmx_u8(255, dmx, &[rgb_laser_addr], 1);
            dmx_f32(1.0, dmx, &[rgb_laser_addr], 6);
            dmx_u8(32, dmx, &[rgb_laser_addr], 7);
        }
        2 => {
            dmx_u8(255, dmx, &[rgb_laser_addr], 1);
            dmx_f32(0.75, dmx, &[rgb_laser_addr], 6);
            dmx_u8(32, dmx, &[rgb_laser_addr], 7);
        }
        3 => dmx_u8(0, dmx, &[rgb_laser_addr], 1),
        _ => {}
    }

    let rgb_strobe = 120;
    map_wargb(state.dial_top[3], state.fade[3], dmx, &[rgb_strobe + 3 - 1]);
    dmx_f32(state.fade[3], dmx, &[rgb_strobe], 1);
    dmx_f32(state.tempo, dmx, &[rgb_strobe], 10);
    dmx_f32(state.dial_3[0], dmx, &[rgb_strobe], 13);
    dmx_f32(state.dial_3[1], dmx, &[rgb_strobe], 14);
    dmx_f32(state.dial_3[2], dmx, &[rgb_strobe], 15);
    dmx_f32(state.dial_3[3], dmx, &[rgb_strobe], 16);
    dmx_f32(state.dial_3[4], dmx, &[rgb_strobe], 17);

    dmx_f32(state.fade[4], dmx, &[rgb_strobe], 6);
    dmx_f32(state.tempo, dmx, &[rgb_strobe], 8);
    dmx_f32(state.dial_4[0], dmx, &[rgb_strobe], 11);
    dmx_f32(state.dial_4[1], dmx, &[rgb_strobe], 12);

    let spot1 = 200;
    let spot2 = 250;
    dmx_f32(state.fade[1], dmx, &[spot1, spot2], 6);
    dmx_f32(state.dial_1[0], dmx, &[spot1], 1);
    dmx_f32(state.dial_1[0], dmx, &[spot2], 1);
    dmx_f32(state.dial_1[1], dmx, &[spot1, spot2], 3);
    dmx_f32(state.dial_top[1], dmx, &[spot1, spot2], 8);
    dmx_f32(state.dial_1[4], dmx, &[spot1, spot2], 12);
    dmx_f32(state.dial_1[3], dmx, &[spot1, spot2], 13);
    dmx_f32(state.dial_1[2], dmx, &[spot1, spot2], 10);

    dmx_f32(state.fade[2], dmx, &[spot1, spot2], 14);
    map_wargb(
        state.dial_top[2],
        1.0,
        dmx,
        &[spot1 + 16 - 1, spot2 + 16 - 1],
    );

    let smoke = 300;
    let slot_len = 101.0f64;
    let needed = slot_len * state.dial_0[0] as f64;
    let t = clock.rem_euclid(slot_len);
    dmx_f32(if t < needed { 1.0 } else { 0.0 }, dmx, &[smoke], 1);

    let smoke2 = 310;
    dmx_f32(state.dial_0[2], dmx, &[smoke2], 1);
    dmx_f32(state.dial_0[1], dmx, &[smoke2], 2);

    let lasers = [400, 420, 440, 460, 480];
    dmx_f32(state.fade[5], dmx, &lasers, 1);
    dmx_f32(state.dial_5[0], dmx, &lasers, 2);
    dmx_f32(state.dial_top[5], dmx, &lasers, 11);
    dmx_f32(state.dial_5[1], dmx, &lasers, 12);
    dmx_f32(0.5, dmx, &lasers, 3);
    dmx_f32(0.3, dmx, &lasers, 4);
    dmx_f32(state.dial_5[2], dmx, &lasers, 5);
    dmx_f32(state.dial_5[3], dmx, &lasers, 6);
    dmx_f32(0.5, dmx, &lasers, 7);
    dmx_f32(0.5, dmx, &lasers, 8);
    dmx_f32(0.5, dmx, &lasers, 10);
    dmx_f32(0.5, dmx, &lasers, 9);

    let uv = [500, 502, 504];
    dmx_f32(state.fade[6], dmx, &uv, 1);
    dmx_f32(
        if state.tempo < 0.1 { 0.0 } else { state.tempo },
        dmx,
        &uv,
        2,
    );
    dmx_f32(if state.fade[7] > 0.5 { 1.0 } else { 0.0 }, dmx, &uv, 3);
}

fn preset_file(slot: usize) -> String {
    format!("{PRESET_DIR}/preset_{slot:02}.ron")
}

fn load_state_file(path: &str) -> Option<ControllerState> {
    let text = std::fs::read_to_string(path).ok()?;
    ControllerState::deserialize_ron(&text).ok()
}

fn save_state_file(path: &str, state: &ControllerState) {
    let _ = std::fs::write(path, state.serialize_ron().as_bytes());
}

fn load_preset_slot(slot: usize, state: &mut ControllerState) -> bool {
    if let Some(mut loaded) = load_state_file(&preset_file(slot)) {
        loaded.dial_0 = state.dial_0;
        *state = loaded;
        true
    } else {
        false
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state_updates: ToUIReceiver<ControllerSnapshot>,
    #[rust(ControllerSnapshot::default())]
    snapshot: ControllerSnapshot,
    #[rust("MIDI inputs: waiting for device scan".to_string())]
    midi_ports_summary: String,
    #[rust(FromUISender::default())]
    ui_controls: FromUISender<UiControlMessage>,
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }

    fn set_slider_widget_value(&self, cx: &mut Cx, id: LiveId, value: u8) {
        if let Some(mut slider) = self.ui.widget(cx, &[id]).borrow_mut::<Slider>() {
            slider.set_value(cx, value as f64);
        }
    }

    fn set_toggle_widget_active(&self, cx: &mut Cx, id: LiveId, active: bool) {
        if let Some(mut toggle) = self.ui.widget(cx, &[id]).borrow_mut::<CheckBox>() {
            if toggle.active(cx) != active {
                toggle.set_active(cx, active);
            }
        }
    }

    fn slider_action_value(&self, cx: &mut Cx, actions: &Actions, id: LiveId) -> Option<f64> {
        let widget = self.ui.widget(cx, &[id]);
        if let Some(action) = actions.find_widget_action(widget.widget_uid()) {
            match action.cast() {
                SliderAction::TextSlide(v) | SliderAction::Slide(v) | SliderAction::EndSlide(v) => {
                    Some(v)
                }
                _ => None,
            }
        } else {
            None
        }
    }

    fn toggle_action_value(&self, cx: &mut Cx, actions: &Actions, id: LiveId) -> Option<bool> {
        let widget = self.ui.widget(cx, &[id]);
        if let Some(action) = actions.find_widget_action(widget.widget_uid()) {
            match action.cast() {
                CheckBoxAction::Change(v) => Some(v),
                _ => None,
            }
        } else {
            None
        }
    }

    fn button_clicked(&self, cx: &mut Cx, actions: &Actions, id: LiveId) -> bool {
        let widget = self.ui.widget(cx, &[id]);
        if let Some(action) = actions.find_widget_action(widget.widget_uid()) {
            matches!(action.cast(), ButtonAction::Clicked(_))
        } else {
            false
        }
    }

    fn slider_ui_to_norm(value: f64) -> f32 {
        let v = value as f32;
        if v <= 1.0 {
            clamp01(v)
        } else {
            clamp01(v / 127.0)
        }
    }

    fn refresh_ui(&mut self, cx: &mut Cx) {
        for index in 0..8 {
            let knob = LiveId::from_str(&format!("top_knob_{index}"));
            let value = (clamp01(self.snapshot.state.dial_top[index]) * 127.0).round() as u8;
            self.set_slider_widget_value(cx, knob, value);
        }
        for index in 0..9 {
            let fader = LiveId::from_str(&format!("fader_{index}"));
            let value = (clamp01(self.snapshot.state.fade[index]) * 127.0).round() as u8;
            self.set_slider_widget_value(cx, fader, value);
        }

        let transport_notes = [91usize, 92, 93, 94, 95, 98, 99, 81, 89];
        for (index, note) in transport_notes.iter().enumerate() {
            let id = LiveId::from_str(&format!("trn_btn_{index}"));
            self.set_toggle_widget_active(cx, id, self.snapshot.mirror.note_is_on(0, *note));
        }
    }

    fn start_dmx_bridge(&mut self, cx: &mut Cx) {
        let _ = std::fs::create_dir_all(PRESET_DIR);

        let mut midi_input = cx.midi_input();
        let state_sender = self.state_updates.sender();
        let ui_receiver = self.ui_controls.receiver();

        std::thread::spawn(move || {
            let _instance_lock = match UdpSocket::bind(CONTROLLER_INSTANCE_LOCK_ADDR) {
                Ok(socket) => socket,
                Err(err) => {
                    println!(
                        "scene_change source=runtime action=instance_lock_busy addr={} err={}",
                        CONTROLLER_INSTANCE_LOCK_ADDR, err
                    );
                    return;
                }
            };

            let mut state = load_state_file(CURRENT_STATE_FILE).unwrap_or_default();
            let mut buttons = ControllerButtons::default();
            buttons.power = true;
            let mut mirror = MidiMirrorState::default();

            let mut universe = [0u8; DMXOUTPUT_HEADER.len() + 512];
            universe[0..DMXOUTPUT_HEADER.len()].copy_from_slice(&DMXOUTPUT_HEADER);

            let socket = match UdpSocket::bind(ARTNET_BIND_ADDR) {
                Ok(socket) => {
                    let _ = socket.set_broadcast(true);
                    Some(socket)
                }
                Err(_) => None,
            };

            let mut last_event = String::from("Waiting for MIDI input...");
            let mut dmx_packets: u64 = 0;
            let mut persist_counter: u32 = 0;
            let mut clock = 0.0f64;
            let mut ui_scene_cooldown_until = Instant::now();
            let transport_notes = [91usize, 92, 93, 94, 95, 98, 99, 81, 89];

            loop {
                while let Ok(msg) = ui_receiver.try_recv() {
                    match msg {
                        UiControlMessage::SetTopKnob { index, value } => {
                            if index < state.dial_top.len() {
                                state.dial_top[index] = clamp01(value);
                                let midi = (clamp01(value) * 127.0).round() as u8;
                                mirror.set_cc(0, 48 + index, midi);
                                last_event = format!("UI top knob {} = {}", index, midi);
                            }
                        }
                        UiControlMessage::SetFader { index, value } => {
                            if index < state.fade.len() {
                                state.fade[index] = clamp01(value);
                                let midi = (clamp01(value) * 127.0).round() as u8;
                                mirror.set_cc(index, 7, midi);
                                last_event = format!("UI fader {} = {}", index, midi);
                            }
                        }
                        UiControlMessage::TriggerScene { index } => {
                            if index < 13 {
                                if DEBUG_SCENE_EVENTS {
                                    println!(
                                        "scene_change source=ui action=trigger slot={index:02}"
                                    );
                                }
                                if load_preset_slot(index, &mut state) {
                                    last_event = format!("UI loaded preset {:02}", index);
                                    if DEBUG_SCENE_EVENTS {
                                        println!("scene_change source=ui action=load slot={index:02}");
                                    }
                                } else {
                                    last_event = format!("UI preset {:02} not found", index);
                                    if DEBUG_SCENE_EVENTS {
                                        println!("scene_change source=ui action=missing slot={index:02}");
                                    }
                                }
                                ui_scene_cooldown_until = Instant::now() + Duration::from_millis(350);
                                buttons.preset.fill(false);
                                buttons.preset[index] = true;
                                for channel in 0..8 {
                                    mirror.set_note(channel, 52, false, 0);
                                }
                                for note in 82..=86 {
                                    mirror.set_note(0, note, false, 0);
                                }
                                if index < 8 {
                                    mirror.set_note(index, 52, true, 127);
                                } else {
                                    mirror.set_note(0, 82 + (index - 8), true, 127);
                                }
                            }
                        }
                        UiControlMessage::SetTransport { index, on } => {
                            if index < transport_notes.len() {
                                let note = transport_notes[index];
                                mirror.set_note(0, note, on, if on { 127 } else { 0 });
                                if note == 81 {
                                    buttons.write_preset = on;
                                }
                                if note == 89 {
                                    buttons.power = on;
                                }
                                last_event = format!("UI transport {} = {}", note, on);
                            }
                        }
                    }
                }

                while let Some((_port, data)) = midi_input.receive() {
                    match data.decode() {
                        MidiEvent::ControlChange(cc) => {
                            mirror.set_cc(cc.channel as usize, cc.param as usize, cc.value);
                            if cc.param == 13 {
                                if cc.value == 1 {
                                    state.tempo = (state.tempo + 0.02).min(1.0);
                                } else {
                                    state.tempo = (state.tempo - 0.02).max(0.0);
                                }
                            }

                            let value = cc.value as f32 / 127.0;
                            if cc.param == 7 {
                                let channel = cc.channel as usize;
                                if channel < state.fade.len() {
                                    state.fade[channel] = value;
                                }
                            }

                            if (16..=23).contains(&cc.param) {
                                let index = (cc.param - 16) as usize;
                                match cc.channel {
                                    0 => state.dial_0[index] = value,
                                    1 => state.dial_1[index] = value,
                                    2 => state.dial_2[index] = value,
                                    3 => state.dial_3[index] = value,
                                    4 => state.dial_4[index] = value,
                                    5 => state.dial_5[index] = value,
                                    6 => state.dial_6[index] = value,
                                    7 => state.dial_7[index] = value,
                                    _ => {}
                                }
                            }

                            if cc.channel == 0 && (48..=55).contains(&cc.param) {
                                state.dial_top[(cc.param - 48) as usize] = value;
                            }

                            last_event = format!(
                                "CC ch:{} param:{} value:{}",
                                cc.channel, cc.param, cc.value
                            );
                        }
                        MidiEvent::Note(note) => {
                            mirror.set_note(
                                note.channel as usize,
                                note.note_number as usize,
                                note.is_on,
                                note.velocity,
                            );
                            match note.note_number {
                                81 => buttons.write_preset = note.is_on,
                                89 => buttons.power = note.is_on,
                                52 => {
                                    let channel = note.channel as usize;
                                    if channel < 8 {
                                        if DEBUG_SCENE_EVENTS {
                                            println!(
                                                "scene_change source=midi action=note slot={channel:02} note=52 ch={} on={} vel={}",
                                                note.channel,
                                                note.is_on,
                                                note.velocity
                                            );
                                        }
                                        if note.is_on {
                                            if Instant::now() < ui_scene_cooldown_until {
                                                if DEBUG_SCENE_EVENTS {
                                                    println!(
                                                        "scene_change source=midi action=ignored slot={channel:02} note=52 ch={} reason=ui_cooldown",
                                                        note.channel
                                                    );
                                                }
                                            } else {
                                                buttons.preset.fill(false);
                                                buttons.preset[channel] = true;
                                                if buttons.write_preset {
                                                    save_state_file(&preset_file(channel), &state);
                                                    last_event = format!("Saved preset {:02}", channel);
                                                    if DEBUG_SCENE_EVENTS {
                                                        println!(
                                                            "scene_change source=midi action=save slot={channel:02} note=52 ch={}",
                                                            note.channel
                                                        );
                                                    }
                                                } else if load_preset_slot(channel, &mut state) {
                                                    last_event = format!("Loaded preset {:02}", channel);
                                                    if DEBUG_SCENE_EVENTS {
                                                        println!(
                                                            "scene_change source=midi action=load slot={channel:02} note=52 ch={}",
                                                            note.channel
                                                        );
                                                    }
                                                } else {
                                                    last_event =
                                                        format!("Preset {:02} not found", channel);
                                                    if DEBUG_SCENE_EVENTS {
                                                        println!(
                                                            "scene_change source=midi action=missing slot={channel:02} note=52 ch={}",
                                                            note.channel
                                                        );
                                                    }
                                                }
                                            }
                                        } else {
                                            buttons.preset[channel] = false;
                                        }
                                    }
                                }
                                82..=86 => {
                                    let index = (note.note_number - 82) as usize + 8;
                                    if index < buttons.preset.len() {
                                        if DEBUG_SCENE_EVENTS {
                                            println!(
                                                "scene_change source=midi action=note slot={index:02} note={} ch={} on={} vel={}",
                                                note.note_number,
                                                note.channel,
                                                note.is_on,
                                                note.velocity
                                            );
                                        }
                                        if note.is_on {
                                            if Instant::now() < ui_scene_cooldown_until {
                                                if DEBUG_SCENE_EVENTS {
                                                    println!(
                                                        "scene_change source=midi action=ignored slot={index:02} note={} ch={} reason=ui_cooldown",
                                                        note.note_number,
                                                        note.channel
                                                    );
                                                }
                                            } else {
                                                buttons.preset.fill(false);
                                                buttons.preset[index] = true;
                                                if buttons.write_preset {
                                                    save_state_file(&preset_file(index), &state);
                                                    last_event = format!("Saved preset {:02}", index);
                                                    if DEBUG_SCENE_EVENTS {
                                                        println!(
                                                            "scene_change source=midi action=save slot={index:02} note={} ch={}",
                                                            note.note_number,
                                                            note.channel
                                                        );
                                                    }
                                                } else if load_preset_slot(index, &mut state) {
                                                    last_event = format!("Loaded preset {:02}", index);
                                                    if DEBUG_SCENE_EVENTS {
                                                        println!(
                                                            "scene_change source=midi action=load slot={index:02} note={} ch={}",
                                                            note.note_number,
                                                            note.channel
                                                        );
                                                    }
                                                } else {
                                                    last_event =
                                                        format!("Preset {:02} not found", index);
                                                    if DEBUG_SCENE_EVENTS {
                                                        println!(
                                                            "scene_change source=midi action=missing slot={index:02} note={} ch={}",
                                                            note.note_number,
                                                            note.channel
                                                        );
                                                    }
                                                }
                                            }
                                        } else {
                                            buttons.preset[index] = false;
                                        }
                                    }
                                }
                                _ => {}
                            }
                            if !matches!(note.note_number, 52 | 82..=86) {
                                last_event = format!(
                                    "NOTE ch:{} num:{} on:{} vel:{}",
                                    note.channel, note.note_number, note.is_on, note.velocity
                                );
                            }
                        }
                        other => {
                            last_event = format!("MIDI {:?}", other);
                        }
                    }
                }

                universe[12] = (dmx_packets % 255) as u8;
                {
                    let dmx = &mut universe[DMXOUTPUT_HEADER.len()..];
                    dmx.fill(0);
                    apply_dmx_mapping(&state, &buttons, dmx, clock);
                }
                if let Some(socket) = socket.as_ref() {
                    let _ = socket.send_to(&universe, ARTNET_BROADCAST_ADDR);
                }

                dmx_packets += 1;
                persist_counter += 1;
                clock += DMX_FRAME_DT;

                if persist_counter >= DMX_FRAME_HZ as u32 {
                    save_state_file(CURRENT_STATE_FILE, &state);
                    persist_counter = 0;
                }

                if dmx_packets % 3 == 0 {
                    let _ = state_sender.send(ControllerSnapshot {
                        state,
                        mirror: mirror.clone(),
                        last_event: last_event.clone(),
                        dmx_packets,
                    });
                }

                std::thread::sleep(Duration::from_secs_f64(DMX_FRAME_DT));
            }
        });
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.start_dmx_bridge(cx);
        self.refresh_ui(cx);
    }

    fn handle_midi_ports(&mut self, cx: &mut Cx, ports: &MidiPortsEvent) {
        cx.use_midi_inputs(&ports.all_inputs());

        let mut names = Vec::new();
        for desc in &ports.descs {
            if desc.port_type.is_input() {
                names.push(desc.name.clone());
            }
        }

        self.midi_ports_summary = if names.is_empty() {
            "MIDI inputs: none detected".to_string()
        } else {
            let shown: Vec<String> = names.iter().take(3).cloned().collect();
            let extra = names.len().saturating_sub(shown.len());
            if extra > 0 {
                format!(
                    "MIDI inputs ({}): {} (+{} more)",
                    names.len(),
                    shown.join(", "),
                    extra
                )
            } else {
                format!("MIDI inputs ({}): {}", names.len(), shown.join(", "))
            }
        };
        self.refresh_ui(cx);
    }

    fn handle_signal(&mut self, cx: &mut Cx) {
        let mut changed = false;
        while let Ok(snapshot) = self.state_updates.try_recv() {
            self.snapshot = snapshot;
            changed = true;
        }
        if changed {
            self.refresh_ui(cx);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for index in 0..8 {
            let id = LiveId::from_str(&format!("top_knob_{index}"));
            if let Some(value) = self.slider_action_value(cx, actions, id) {
                let _ = self.ui_controls.send(UiControlMessage::SetTopKnob {
                    index,
                    value: Self::slider_ui_to_norm(value),
                });
            }
        }
        for index in 0..9 {
            let id = LiveId::from_str(&format!("fader_{index}"));
            if let Some(value) = self.slider_action_value(cx, actions, id) {
                let _ = self.ui_controls.send(UiControlMessage::SetFader {
                    index,
                    value: Self::slider_ui_to_norm(value),
                });
            }
        }
        for index in 0..13 {
            let id = LiveId::from_str(&format!("scene_btn_{index}"));
            if self.button_clicked(cx, actions, id) {
                if DEBUG_SCENE_EVENTS {
                    println!("scene_change source=ui action=click slot={index:02}");
                }
                let _ = self
                    .ui_controls
                    .send(UiControlMessage::TriggerScene { index });
            }
        }
        for index in 0..9 {
            let id = LiveId::from_str(&format!("trn_btn_{index}"));
            if let Some(on) = self.toggle_action_value(cx, actions, id) {
                let _ = self
                    .ui_controls
                    .send(UiControlMessage::SetTransport { index, on });
            }
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
