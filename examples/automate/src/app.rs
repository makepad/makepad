use crate::makepad_micro_serde::*;
use makepad_widgets::*;
use std::net::UdpSocket;
use std::time::Duration;

app_main!(App);

const DMX_FRAME_HZ: f64 = 44.0;
const DMX_FRAME_DT: f64 = 1.0 / DMX_FRAME_HZ;
const ARTNET_BIND_ADDR: &str = "0.0.0.0:0";
const ARTNET_BROADCAST_ADDR: &str = "255.255.255.255:6454";
const PRESET_DIR: &str = "examples/automate/local/dmx";
const CURRENT_STATE_FILE: &str = "examples/automate/local/dmx/current.ron";

pub const DMXOUTPUT_HEADER: [u8; 18] = [
    b'A', b'r', b't', b'-', b'N', b'e', b't', b'\0',
    0,    // opcode hi
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

let Panel = RoundedView{
    width: Fill
    height: Fit
    flow: Down
    spacing: 8
    padding: Inset{top: 12. bottom: 12. left: 12. right: 12.}
    draw_bg.color: #x18212d
    draw_bg.border_radius: 10.
    draw_bg.border_size: 1.
    draw_bg.border_color: #x2f3a4b
}

let MonoLine = Label{
    width: Fill
    draw_text.color: #xccd6e6
    draw_text.text_style: theme.font_code{font_size: 11.}
}

let DimLine = Label{
    width: Fill
    draw_text.color: #x90a0bb
    draw_text.text_style.font_size: 10.
}

startup() do #(App::script_component(vm)){
    ui: Root{
        main_window := Window{
            window.title: "Automate - Home MIDI/DMX"
            window.inner_size: vec2(1520, 940)
            pass.clear_color: vec4(0.05, 0.07, 0.10, 1.0)
            body +: {
                app_root := SolidView{
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 12
                    padding: Inset{top: 16. bottom: 16. left: 16. right: 16.}
                    draw_bg.color: #x0d1219

                    header := Panel{
                        Label{
                            text: "Home Automation Controller"
                            draw_text.color: #xfff
                            draw_text.text_style: theme.font_bold{font_size: 20.}
                        }
                        DimLine{
                            text: "APC40 mapped state + Art-Net DMX output (AI/image generation removed)."
                        }
                        status_line := MonoLine{text: "Status: booting..."}
                    }

                    View{
                        width: Fill
                        height: Fill
                        flow: Right
                        spacing: 12

                        controller_panel := Panel{
                            width: Fill
                            height: Fill
                            Label{
                                text: "Mapped Controller State"
                                draw_text.color: #xfff
                                draw_text.text_style: theme.font_bold{font_size: 14.}
                            }
                            ScrollYView{
                                width: Fill
                                height: Fill
                                flow: Down
                                spacing: 6
                                midi_ports := MonoLine{text: "MIDI inputs: waiting..."}
                                last_event := MonoLine{text: "Last MIDI event: none"}
                                tempo_line := MonoLine{text: "TEMPO 0:000[......]"}
                                top_dials := MonoLine{text: "TOP ..."}
                                fader_line := MonoLine{text: "FAD ..."}
                                preset_line := MonoLine{text: "PRE ..."}
                                switch_line := MonoLine{text: "SWI ..."}
                                channel_0 := MonoLine{text: "CH0 ..."}
                                channel_1 := MonoLine{text: "CH1 ..."}
                                channel_2 := MonoLine{text: "CH2 ..."}
                                channel_3 := MonoLine{text: "CH3 ..."}
                                channel_4 := MonoLine{text: "CH4 ..."}
                                channel_5 := MonoLine{text: "CH5 ..."}
                                channel_6 := MonoLine{text: "CH6 ..."}
                                channel_7 := MonoLine{text: "CH7 ..."}
                            }
                        }

                        dmx_panel := Panel{
                            width: 470
                            height: Fill
                            Label{
                                text: "DMX Output"
                                draw_text.color: #xfff
                                draw_text.text_style: theme.font_bold{font_size: 14.}
                            }
                            dmx_info := MonoLine{text: "Art-Net target: 255.255.255.255:6454"}
                            scene_hint := MonoLine{text: "Dominant zone: n/a"}
                            DimLine{text: "Preview (first 32 channels in hex):"}
                            dmx_preview := MonoLine{text: "CH001:00 CH002:00 CH003:00 CH004:00"}
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
    fn bank(&self, index: usize) -> &[f32; 8] {
        match index {
            0 => &self.dial_0,
            1 => &self.dial_1,
            2 => &self.dial_2,
            3 => &self.dial_3,
            4 => &self.dial_4,
            5 => &self.dial_5,
            6 => &self.dial_6,
            _ => &self.dial_7,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ControllerButtons {
    preset: [bool; 13],
    write_preset: bool,
    power: bool,
}

impl ControllerButtons {
    fn active_preset(&self) -> Option<usize> {
        for index in 0..self.preset.len() {
            if self.preset[index] {
                return Some(index);
            }
        }
        None
    }

    fn rising_preset(&self, previous: &Self) -> Option<usize> {
        for index in 0..self.preset.len() {
            if !previous.preset[index] && self.preset[index] {
                return Some(index);
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
struct ControllerSnapshot {
    state: ControllerState,
    buttons: ControllerButtons,
    last_event: String,
    dmx_packets: u64,
    dmx_preview: [u8; 32],
}

impl Default for ControllerSnapshot {
    fn default() -> Self {
        Self {
            state: ControllerState::default(),
            buttons: ControllerButtons::default(),
            last_event: "Waiting for MIDI input...".to_string(),
            dmx_packets: 0,
            dmx_preview: [0; 32],
        }
    }
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
    map_wargb(state.dial_top[2], 1.0, dmx, &[spot1 + 16 - 1, spot2 + 16 - 1]);

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
    dmx_f32(if state.tempo < 0.1 { 0.0 } else { state.tempo }, dmx, &uv, 2);
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
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }

    fn meter(value: f32, width: usize) -> String {
        let mut out = String::with_capacity(width);
        let filled = (clamp01(value) * width as f32).round() as usize;
        for idx in 0..width {
            out.push(if idx < filled { '#' } else { '.' });
        }
        out
    }

    fn cell(index: usize, value: f32) -> String {
        let midi = (clamp01(value) * 127.0).round() as u8;
        format!("{index}:{midi:03}[{}]", Self::meter(value, 6))
    }

    fn format_bank(name: &str, values: &[f32; 8]) -> String {
        let mut out = format!("{name} ");
        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            out.push_str(&Self::cell(index, *value));
        }
        out
    }

    fn format_faders(values: &[f32; 9]) -> String {
        let mut out = String::from("FAD ");
        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            let midi = (clamp01(*value) * 127.0).round() as u8;
            out.push_str(&format!("{index}:{midi:03}[{}]", Self::meter(*value, 8)));
        }
        out
    }

    fn format_presets(buttons: &ControllerButtons) -> String {
        let mut out = String::from("PRE ");
        for (index, active) in buttons.preset.iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            out.push_str(&format!("{index:02}[{}]", if *active { "X" } else { "." }));
        }
        out
    }

    fn format_dmx_preview(values: &[u8; 32]) -> String {
        let mut out = String::new();
        for row in 0..4 {
            for col in 0..8 {
                let index = row * 8 + col;
                if col > 0 {
                    out.push(' ');
                }
                out.push_str(&format!("CH{:03}:{:02X}", index + 1, values[index]));
            }
            if row != 3 {
                out.push('\n');
            }
        }
        out
    }

    fn scene_hint(snapshot: &ControllerSnapshot) -> String {
        let mut dominant = 0usize;
        let mut level = 0.0f32;
        for (index, value) in snapshot.state.fade.iter().enumerate() {
            if *value > level {
                dominant = index;
                level = *value;
            }
        }
        format!(
            "Dominant zone: fader {} at {:.0}% | power:{} | write:{}",
            dominant,
            level * 100.0,
            if snapshot.buttons.power { "on" } else { "off" },
            if snapshot.buttons.write_preset { "on" } else { "off" }
        )
    }

    fn refresh_ui(&mut self, cx: &mut Cx) {
        self.ui
            .label(cx, ids!(status_line))
            .set_text(
                cx,
                &format!(
                    "{} | DMX packets: {}",
                    self.midi_ports_summary, self.snapshot.dmx_packets
                ),
            );
        self.ui
            .label(cx, ids!(midi_ports))
            .set_text(cx, &self.midi_ports_summary);
        self.ui
            .label(cx, ids!(last_event))
            .set_text(cx, &format!("Last MIDI event: {}", self.snapshot.last_event));
        self.ui.label(cx, ids!(tempo_line)).set_text(
            cx,
            &format!(
                "TEMPO {}",
                Self::cell(0, self.snapshot.state.tempo),
            ),
        );
        self.ui.label(cx, ids!(top_dials)).set_text(
            cx,
            &Self::format_bank("TOP", &self.snapshot.state.dial_top),
        );
        self.ui.label(cx, ids!(fader_line)).set_text(
            cx,
            &Self::format_faders(&self.snapshot.state.fade),
        );
        self.ui.label(cx, ids!(preset_line)).set_text(
            cx,
            &Self::format_presets(&self.snapshot.buttons),
        );
        self.ui.label(cx, ids!(switch_line)).set_text(
            cx,
            &format!(
                "SWI write:[{}] power:[{}]",
                if self.snapshot.buttons.write_preset {
                    "X"
                } else {
                    "."
                },
                if self.snapshot.buttons.power { "X" } else { "." }
            ),
        );
        self.ui.label(cx, ids!(channel_0)).set_text(
            cx,
            &Self::format_bank("CH0", self.snapshot.state.bank(0)),
        );
        self.ui.label(cx, ids!(channel_1)).set_text(
            cx,
            &Self::format_bank("CH1", self.snapshot.state.bank(1)),
        );
        self.ui.label(cx, ids!(channel_2)).set_text(
            cx,
            &Self::format_bank("CH2", self.snapshot.state.bank(2)),
        );
        self.ui.label(cx, ids!(channel_3)).set_text(
            cx,
            &Self::format_bank("CH3", self.snapshot.state.bank(3)),
        );
        self.ui.label(cx, ids!(channel_4)).set_text(
            cx,
            &Self::format_bank("CH4", self.snapshot.state.bank(4)),
        );
        self.ui.label(cx, ids!(channel_5)).set_text(
            cx,
            &Self::format_bank("CH5", self.snapshot.state.bank(5)),
        );
        self.ui.label(cx, ids!(channel_6)).set_text(
            cx,
            &Self::format_bank("CH6", self.snapshot.state.bank(6)),
        );
        self.ui.label(cx, ids!(channel_7)).set_text(
            cx,
            &Self::format_bank("CH7", self.snapshot.state.bank(7)),
        );

        self.ui
            .label(cx, ids!(dmx_info))
            .set_text(
                cx,
                &format!(
                    "Art-Net target: {} | bind: {}",
                    ARTNET_BROADCAST_ADDR, ARTNET_BIND_ADDR
                ),
            );
        self.ui
            .label(cx, ids!(scene_hint))
            .set_text(cx, &Self::scene_hint(&self.snapshot));
        self.ui.label(cx, ids!(dmx_preview)).set_text(
            cx,
            &Self::format_dmx_preview(&self.snapshot.dmx_preview),
        );
    }

    fn start_dmx_bridge(&mut self, cx: &mut Cx) {
        let _ = std::fs::create_dir_all(PRESET_DIR);

        let mut midi_input = cx.midi_input();
        let state_sender = self.state_updates.sender();

        std::thread::spawn(move || {
            let mut state = load_state_file(CURRENT_STATE_FILE).unwrap_or_default();
            let mut buttons = ControllerButtons::default();
            buttons.power = true;
            let mut previous_buttons = buttons;

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

            loop {
                while let Some((_port, data)) = midi_input.receive() {
                    match data.decode() {
                        MidiEvent::ControlChange(cc) => {
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
                            match note.note_number {
                                81 => buttons.write_preset = note.is_on,
                                89 => buttons.power = note.is_on,
                                52 => {
                                    let channel = note.channel as usize;
                                    if channel < 8 {
                                        buttons.preset[channel] = note.is_on;
                                    }
                                }
                                82..=86 => {
                                    let index = (note.note_number - 82) as usize + 8;
                                    if index < buttons.preset.len() {
                                        buttons.preset[index] = note.is_on;
                                    }
                                }
                                _ => {}
                            }

                            last_event = format!(
                                "NOTE ch:{} num:{} on:{} vel:{}",
                                note.channel, note.note_number, note.is_on, note.velocity
                            );
                        }
                        other => {
                            last_event = format!("MIDI {:?}", other);
                        }
                    }
                }

                if buttons.write_preset {
                    if let Some(slot) = buttons.rising_preset(&previous_buttons) {
                        save_state_file(&preset_file(slot), &state);
                        last_event = format!("Saved preset {:02}", slot);
                    } else if !previous_buttons.write_preset {
                        if let Some(slot) = buttons.active_preset() {
                            save_state_file(&preset_file(slot), &state);
                            last_event = format!("Saved preset {:02}", slot);
                        }
                    }
                } else if let Some(slot) = buttons.rising_preset(&previous_buttons) {
                    if let Some(mut loaded) = load_state_file(&preset_file(slot)) {
                        loaded.dial_0 = state.dial_0;
                        state = loaded;
                        last_event = format!("Loaded preset {:02}", slot);
                    } else {
                        last_event = format!("Preset {:02} not found", slot);
                    }
                }
                previous_buttons = buttons;

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
                    let mut dmx_preview = [0u8; 32];
                    dmx_preview.copy_from_slice(
                        &universe[DMXOUTPUT_HEADER.len()..DMXOUTPUT_HEADER.len() + 32],
                    );
                    let _ = state_sender.send(ControllerSnapshot {
                        state,
                        buttons,
                        last_event: last_event.clone(),
                        dmx_packets,
                        dmx_preview,
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
            format!("MIDI inputs: {}", names.join(", "))
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

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
