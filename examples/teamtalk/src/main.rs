pub use makepad_widgets;

/*
TeamTalk: LAN p2p voice chat on makepad-teamtalk — a super low latency
helicopter-headset experience. This example is the thin app shell: it opens
the default mic (with OS echo cancellation) and speakers, pushes the device
blocks into a VoiceLink, and prints live stats. All transport, jitter and
mixing logic lives in libs/teamtalk.

    cargo run -p makepad-example-teamtalk --release -- [flags]

Flags:
    --device=NAME     input device substring match (default: default mic)
    --loopback        capture "System Audio" instead of a microphone
    --vol=0..1        mic volume (cubic taper, default 1.0)
    --channel=N       team channel to talk on (0 = everyone, default 0)
    --listen=1,2      team channels to hear (default: all; 0 always plays)
    --port=N          UDP port (default 41531 — keep it: firewalls know it)
    --peer=IP:PORT    extra unicast peer (repeatable; e.g. across subnets)
    --broadcast-audio broadcast audio frames instead of unicasting
    --frame=N         samples per 48 kHz frame: 120|240|480|960 (default 240)
    --ogg             send 4-bit ADPCM in Ogg pages (~300 kbit/s) instead of raw
    --mute            start muted

The UDP protocol carries a codec id (raw i16 now, ogg later), a team channel
byte, a sender id, sequence numbers and timestamps; see libs/teamtalk.
*/

use makepad_teamtalk::{Delivery, VoiceConfig, VoiceLink};
use makepad_widgets::makepad_platform::audio::AudioInputOptions;
use makepad_widgets::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

struct Args {
    device: Option<String>,
    loopback: bool,
    vol: f32,
    channel: u8,
    listen: Option<Vec<u8>>,
    port: u16,
    peers: Vec<std::net::SocketAddr>,
    broadcast_audio: bool,
    frame: usize,
    mute: bool,
    ogg: bool,
}

impl Args {
    fn parse() -> Self {
        let mut args = Self {
            device: None,
            loopback: false,
            vol: 1.0,
            channel: 0,
            listen: None,
            port: makepad_teamtalk::DEFAULT_PORT,
            peers: Vec::new(),
            broadcast_audio: false,
            frame: 240,
            mute: false,
            ogg: false,
        };
        for arg in std::env::args().skip(1) {
            if let Some(v) = arg.strip_prefix("--device=") {
                args.device = Some(v.to_string());
            } else if arg == "--loopback" {
                args.loopback = true;
            } else if let Some(v) = arg.strip_prefix("--vol=") {
                if let Ok(v) = v.parse::<f32>() {
                    args.vol = v.clamp(0.0, 1.0);
                }
            } else if let Some(v) = arg.strip_prefix("--channel=") {
                args.channel = v.parse().unwrap_or(0);
            } else if let Some(v) = arg.strip_prefix("--listen=") {
                args.listen = Some(v.split(',').filter_map(|c| c.trim().parse().ok()).collect());
            } else if let Some(v) = arg.strip_prefix("--port=") {
                args.port = v.parse().unwrap_or(makepad_teamtalk::DEFAULT_PORT);
            } else if let Some(v) = arg.strip_prefix("--peer=") {
                match v.parse() {
                    Ok(a) => args.peers.push(a),
                    Err(_) => println!("bad --peer address: {v}"),
                }
            } else if arg == "--broadcast-audio" {
                args.broadcast_audio = true;
            } else if let Some(v) = arg.strip_prefix("--frame=") {
                args.frame = v.parse().unwrap_or(240);
            } else if arg == "--mute" {
                args.mute = true;
            } else if arg == "--ogg" {
                args.ogg = true;
            }
        }
        args
    }
}

/// Mic volume 0..1 with a cubic taper (0.5 feels like half loudness).
fn cubic_gain(v: f32) -> f32 {
    v * v * v
}

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    #(App::script_api(vm)){
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[new]
    window: WindowHandle,
    #[new]
    pass: DrawPass,
    #[new]
    main_draw_list: DrawList2d,
    #[rust]
    link: Option<Arc<VoiceLink>>,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.window.set_pass(cx, &self.pass);
        self.pass
            .set_window_clear_color(cx, vec4(0.2, 0.2, 0.3, 1.0));
        self.start_voice(cx);
    }

    fn handle_draw_2d(&mut self, cx: &mut Cx2d) {
        if !cx.will_redraw(&mut self.main_draw_list, Walk::default()) {
            return;
        }
        cx.begin_pass(&self.pass, None);
        self.main_draw_list.begin_always(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        // No UI — audio only; stats go to stdout.
        cx.end_pass_sized_turtle();
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass);
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        let args = Args::parse();
        for desc in &devices.descs {
            println!("{}", desc)
        }
        let inputs = if args.loopback {
            devices.match_inputs(&["System Audio"])
        } else if let Some(ref name) = args.device {
            let matched = devices.match_inputs(&[name.as_str()]);
            if matched.is_empty() {
                println!("No input matching '{name}', using the default mic");
                devices.default_input()
            } else {
                matched
            }
        } else {
            devices.default_input()
        };
        // Voice-processing capture: the OS removes what the speakers play
        // from the mic signal, so a speaker+mic setup does not echo.
        cx.use_audio_inputs_with_options(
            &inputs,
            AudioInputOptions {
                echo_cancellation: !args.loopback,
            },
        );
        cx.use_audio_outputs(&devices.default_output());
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let _ = self.match_event_with_draw_2d(cx, event);
    }
}

impl App {
    fn start_voice(&mut self, cx: &mut Cx) {
        let args = Args::parse();
        let mut link = match VoiceLink::bind(VoiceConfig {
            port: args.port,
            static_peers: args.peers.clone(),
            delivery: if args.broadcast_audio {
                Delivery::Broadcast
            } else {
                Delivery::Unicast
            },
            frame_samples: args.frame,
            codec: if args.ogg {
                makepad_teamtalk::Codec::Ogg
            } else {
                makepad_teamtalk::Codec::RawI16
            },
            channel: args.channel,
            ..VoiceConfig::default()
        }) {
            Ok(link) => link,
            Err(e) => {
                println!("voice: bind failed: {e}");
                return;
            }
        };
        link.set_input_gain(cubic_gain(args.vol));
        link.set_muted(args.mute);
        match &args.listen {
            Some(channels) => link.set_listen_channels(channels),
            None => link.set_listen_all(),
        }
        println!(
            "voice: {} talk-channel {} frame {} samples ({:.1} ms) {}",
            link.local_addr(),
            args.channel,
            args.frame,
            args.frame as f64 * 1000.0 / makepad_teamtalk::INTERNAL_RATE,
            if args.broadcast_audio {
                "broadcast"
            } else {
                "unicast"
            }
        );

        let mut capture = link.take_capture().unwrap();
        let mut playback = link.take_playback().unwrap();

        // Device geometry, published by the callbacks for the stats line:
        // (rate << 20 | channels << 16 | block_frames).
        let in_geom = Arc::new(AtomicU64::new(0));
        let out_geom = Arc::new(AtomicU64::new(0));
        let pack = |rate: f64, chans: usize, frames: usize| {
            ((rate as u64) << 20) | ((chans as u64) << 16) | frames as u64
        };

        let geom = in_geom.clone();
        cx.audio_input(0, move |info, input| {
            geom.store(
                pack(info.sample_rate, input.channel_count(), input.frame_count()),
                Ordering::Relaxed,
            );
            capture.push_planar(
                info.sample_rate,
                input.frame_count(),
                input.channel_count(),
                &input.data,
            );
        });

        let geom = out_geom.clone();
        cx.audio_output(0, move |info, output| {
            geom.store(
                pack(info.sample_rate, output.channel_count(), output.frame_count()),
                Ordering::Relaxed,
            );
            output.zero();
            playback.mix_into_planar(
                info.sample_rate,
                output.frame_count(),
                output.channel_count(),
                &mut output.data,
            );
        });

        let link = Arc::new(link);
        let stats_link = link.clone();
        std::thread::Builder::new()
            .name("teamtalk-stats".into())
            .spawn(move || {
                let unpack = |v: u64| (v >> 20, (v >> 16) & 0xF, v & 0xFFFF);
                let mut last = stats_link.stats();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    let s = stats_link.stats();
                    let (irate, ich, iblk) = unpack(in_geom.load(Ordering::Relaxed));
                    let (orate, och, oblk) = unpack(out_geom.load(Ordering::Relaxed));
                    println!(
                        "voice: in {irate}Hz {ich}ch block {iblk} | out {orate}Hz {och}ch block {oblk} | tx {}/s ({} B/s) rx {}/s | peers {} | late {} err {}",
                        (s.packets_sent - last.packets_sent) / 2,
                        (s.bytes_sent - last.bytes_sent) / 2,
                        (s.packets_recv - last.packets_recv) / 2,
                        s.active_peers,
                        s.filtered,
                        s.send_errors,
                    );
                    for p in stats_link.peers() {
                        println!(
                            "voice:   peer {:08x} {} ch{} {} buf {:.1}ms target {} late {} dup {}",
                            p.sender as u32,
                            p.addr.map(|a| a.to_string()).unwrap_or_default(),
                            p.channel,
                            if p.talking { "TALKING" } else { "quiet" },
                            p.buffered_ms,
                            p.target_frames,
                            p.frames_late,
                            p.frames_duplicate,
                        );
                    }
                    last = s;
                }
            })
            .unwrap();
        self.link = Some(link);
    }
}
