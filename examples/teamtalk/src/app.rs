/*
TeamTalk is a LAN (wired only) p2p audiochat supporting as many clients as you have bandwidth.
For 6 clients it should pull about 25 megabits. You can use it to have a super low latency
helicopter-headset experience, silent disco, and so on.
This example shows using networking and audio IO
*/

use makepad_draw2::*;
use makepad_draw2::makepad_platform::{
    audio_stream::AudioStreamSender,
    makepad_micro_serde::*,
};
use std::net::UdpSocket;
use std::time::Duration;

app_main!(App);

script_run!{
    use mod.std.*;
    #(App::script_api(vm)){
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_draw2::script_run(vm);
        App::script_run(vm, script_run)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[script] window: WindowHandle,
    #[script] pass: Pass,
    #[script] main_draw_list: DrawList2d,
}

// this is the protocol enum with 'micro-serde' binary serialise/deserialise macro on it.
#[derive(SerBin, DeBin, Debug)]
enum TeamTalkWire {
    Silence { client_uid: u64, frame_count: u32 },
    Audio { client_uid: u64, channel_count: u32, data: Vec<i16> },
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.window.set_pass(cx, &self.pass);
        self.pass.set_window_clear_color(cx, vec4(0.2, 0.2, 0.3, 1.0));
        self.start_network_stack(cx);
    }

    fn handle_draw_2d(&mut self, cx: &mut Cx2d) {
        if !cx.will_redraw(&mut self.main_draw_list, Walk::default()) {
            return
        }

        cx.begin_pass(&self.pass, None);
        self.main_draw_list.begin_always(cx);

        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());

        // No UI - just audio streaming

        cx.end_pass_sized_turtle();
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass);
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        for desc in &devices.descs {
            println!("{}", desc)
        }
        cx.use_audio_inputs(&devices.default_input());
        cx.use_audio_outputs(&devices.default_output());
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let _ = self.match_event_with_draw_2d(cx, event);
    }
}

impl App {
    pub fn start_network_stack(&mut self, cx: &mut Cx) {
        // not a very good uid, but it'll do.
        let my_client_uid = LiveId::from_str(&format!("{:?}", std::time::SystemTime::now())).0;
        
        // AudioStream is an mpsc channel that buffers at the recv side
        // and allows arbitrary chunksized reads. Little utility struct.
        // platform2's create_pair takes (min_buf, max_buf) at creation time
        let (mic_send, mut mic_recv) = AudioStreamSender::create_pair(1, 255);
        let (mix_send, mut mix_recv) = AudioStreamSender::create_pair(1, 4);

        // the UDP broadcast socket
        let write_audio = UdpSocket::bind("0.0.0.0:41531").unwrap();
        write_audio.set_read_timeout(Some(Duration::new(5, 0))).unwrap();
        write_audio.set_broadcast(true).unwrap();

        let read_audio = write_audio.try_clone().unwrap();
        
        // our microphone broadcast network thread
        std::thread::spawn(move || {
            let mut wire_data = Vec::new();
            let mut output_buffer = AudioBuffer::new_with_size(640, 1);
            loop {
                // fill the mic stream recv side buffers, and block if nothing
                mic_recv.recv_stream();
                loop {
                    // platform2's read_buffer doesn't take min_buf/max_buf (set at creation)
                    if mic_recv.read_buffer(0, &mut output_buffer) == 0 {
                        break;
                    }
                    let buf = output_buffer.channel(0);
                    // do a quick volume check so we can send 1 byte packets if silent
                    let mut sum = 0.0;
                    for v in buf {
                        sum += v.abs();
                    }
                    let peak = sum / buf.len() as f32;
                    
                    let min_volume = 0.0001f32; // threshold for silence detection
                    let wire_packet = if peak > min_volume {
                        TeamTalkWire::Audio {
                            client_uid: my_client_uid,
                            channel_count: 1,
                            data: output_buffer.to_i16()
                        }
                    } else {
                        TeamTalkWire::Silence {
                            client_uid: my_client_uid,
                            frame_count: output_buffer.frame_count() as u32
                        }
                    };
                    // serialise the packet enum for sending over the wire
                    wire_data.clear();
                    wire_packet.ser_bin(&mut wire_data);
                    // send to all peers
                    let _ = write_audio.send_to(&wire_data, "10.0.0.255:41531");
                };
            }
        });
        
        // the network audio receiving thread
        std::thread::spawn(move || {
            let mut read_buf = [0u8; 4096];

            while let Ok((len, _addr)) = read_audio.recv_from(&mut read_buf) {
                let read_buf = &read_buf[0..len];

                let packet = match TeamTalkWire::deserialize_bin(read_buf) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // create an audiobuffer from the data
                let (client_uid, buffer) = match packet {
                    TeamTalkWire::Audio { client_uid, channel_count, data } => {
                        (client_uid, AudioBuffer::from_i16(&data, channel_count as usize))
                    }
                    TeamTalkWire::Silence { client_uid, frame_count } => {
                        (client_uid, AudioBuffer::new_with_size(frame_count as usize, 1))
                    }
                };

                if client_uid != my_client_uid {
                    // platform2 uses send() instead of write_buffer()
                    let _ = mix_send.send(client_uid, buffer);
                }
            }
        });

        cx.audio_input(0, move |_info, input_buffer| {
            let mut input_buffer = input_buffer.clone();
            input_buffer.make_single_channel();
            // platform2 uses send() instead of write_buffer()
            let _ = mic_send.send(0, input_buffer);
        });

        let volume = 7.0f32; // output volume multiplier
        cx.audio_output(0, move |_info, output_buffer| {
            output_buffer.zero();
            // fill our read buffers on the audiostream without blocking
            mix_recv.try_recv_stream();
            let mut chan = AudioBuffer::new_like(output_buffer);
            for i in 0..mix_recv.num_routes() {
                // platform2's read_buffer doesn't take min_buf/max_buf
                if mix_recv.read_buffer(i, &mut chan) != 0 {
                    for j in 0..chan.data.len() {
                        output_buffer.data[j] += chan.data[j] * volume;
                    }
                }
            }
        });
    }
}
