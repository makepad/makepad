/*
TeamTalk is a LAN (only) p2p audiochat supporting as many clients as you have bandwidth.
For 6 clients it should pull about 25 megabits. You can use it to have a super low latency
helicopter-headset experience, silent disco, and so on.
*/

pub use makepad_audio_graph;
pub use makepad_audio_graph::makepad_widgets;
pub use makepad_audio_graph::makepad_platform;
pub use makepad_micro_serde;
use {
    crate::{
        makepad_micro_serde::{SerBin, DeBin, DeBinErr},
        makepad_audio_graph::audio_stream::{AudioStreamSender},
        makepad_widgets::*,
        makepad_platform::audio::*,
        makepad_draw::*,
    },
    std::collections::HashMap,
    std::thread,
    std::time,
    std::net::UdpSocket,
    std::time::{Duration, Instant},
};

// We dont have a UI yet

live_design!{
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    registry Widget::*;
    App = {{App}} {
        ui: {
            walk: {width: Fill, height: Fill},
            draw_bg: {
                shape: Rect
                fn pixel(self) -> vec4 {
                    return Pal::premul(#3)
                }
            }
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    window: DesktopWindow,
    ui: FrameRef,
}

// this is the protocol enum with 'micro-serde' binary serialise/deserialise macro on it.
#[derive(SerBin, DeBin)]
enum TeamTalkWire {
    Silence {order: u64, frame_count: u32},
    Audio {order: u64, channel_count: u32, data: Vec<i16>},
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_audio_graph::live_design(cx);
    }
    
    pub fn start_network_stack(&mut self, cx: &mut Cx) {
        // not a very good uid, but it'l do.
        let client_uid = LiveId::from_str(&format!("{:?}", std::time::SystemTime::now())).unwrap().0;
        // Audiostream is an mpsc channel that buffers at the recv side
        // and allows arbitrary chunksized reads. Little utility struct
        let (mic_send, mut mic_recv) = AudioStreamSender::create_pair();
        let (mix_send, mut mix_recv) = AudioStreamSender::create_pair();
        
        // the UDP broadcast socket for peer discovery
        let write_discovery = UdpSocket::bind("0.0.0.0:41531").unwrap();
        write_discovery.set_read_timeout(Some(Duration::new(0, 1))).unwrap();
        write_discovery.set_broadcast(true).unwrap();
        let read_discovery = write_discovery.try_clone().unwrap();
        
        // this thread does udp broadcast every second to announce our existence
        std::thread::spawn(move || {
            let dummy = client_uid.to_be_bytes();
            loop {
                write_discovery.send_to(&dummy, "255.255.255.255:41531").unwrap();
                thread::sleep(time::Duration::from_secs(1));
            }
        });
        
        // the audio read/write UDP socket (peer 2 peer)
        let write_audio = UdpSocket::bind("0.0.0.0:41532").unwrap();
        write_audio.set_read_timeout(Some(Duration::new(5, 0))).unwrap();
        let read_audio = write_audio.try_clone().unwrap();
        
        // our microphone broadcast network thread
        std::thread::spawn(move || {
            let mut wire_data = Vec::new();
            let mut peer_addrs = HashMap::new();
            let mut order = 0u64;
            let mut output_buffer = AudioBuffer::new_with_size(640, 1);
            loop {
                let mut other_uid = [0u8; 8];
                let time_now = Instant::now();
                // nonblockingly (timeout=1ns) check our discovery socket for peers
                while let Ok((_, mut addr)) = read_discovery.recv_from(&mut other_uid) {
                    //if client_uid == u64::from_be_bytes(other_uid) {
                    //    continue;
                    //}
                    addr.set_port(41532);
                    if let Some(time) = peer_addrs.get_mut(&addr) {
                        *time = time_now;
                    }
                    else {
                        peer_addrs.insert(addr, time_now);
                    }
                }
                // flush peers we havent heard from more than 5 seconds ago
                peer_addrs.retain( | _, time | *time > time_now - Duration::from_secs(5));
                
                // fill the mic stream recv side buffers, and block if nothing
                mic_recv.recv_stream(1, 3);
                loop {
                    if mic_recv.read_buffer(0, &mut output_buffer, 0, 0) == 0 {
                        break;
                    }
                    
                    let buf = output_buffer.channel(0);
                    // do a quick volume check so we can send 1 byte packets if silent
                    let mut sum = 0.0;
                    for v in buf {
                        sum += v.abs();
                    }
                    let peak = sum / buf.len() as f32;
                    order += 1;
                    let wire_packet = if peak>0.005 {
                        TeamTalkWire::Audio {order, channel_count: 1, data: output_buffer.to_i16()}
                    }
                    else {
                        TeamTalkWire::Silence {order, frame_count: output_buffer.frame_count() as u32}
                    };
                    // serialise the packet enum for sending over the wire
                    wire_data.clear();
                    wire_packet.ser_bin(&mut wire_data);
                    // send to all peers
                    for addr in peer_addrs.keys() {
                        write_audio.send_to(&wire_data, addr).unwrap();
                    }
                };
            }
        });
        
        // the network audio receiving thread
        std::thread::spawn(move || {
            let mut read_buf = [0u8; 4096];
            let mut last_orders: Vec<(u64, u64)> = Vec::new();
            
            while let Ok((len, addr)) = read_audio.recv_from(&mut read_buf) {
                let route_id = if let std::net::IpAddr::V4(v4) = addr.ip() {
                    v4.octets()[3] as u64
                }else {1};
                
                let read_buf = &read_buf[0..len];
                
                let packet = TeamTalkWire::deserialize_bin(&read_buf).unwrap();
                
                // create an audiobuffer from the data
                let (order, buffer) = match packet {
                    TeamTalkWire::Audio {order, channel_count, data} => {
                        (order, AudioBuffer::from_i16(&data, channel_count as usize))
                    }
                    TeamTalkWire::Silence {order, frame_count} => {
                        (order, AudioBuffer::new_with_size(frame_count as usize, 1))
                    }
                };
                
                if let Some((_, order_store)) = last_orders.iter_mut().find( | v | v.0 == route_id) {
                    let last_order = *order_store;
                    *order_store = order;
                    if last_order != order - 1 {
                        println!("Lost packet detected!")
                    }
                }
                else {
                    last_orders.push((route_id, order));
                }
                
                mix_send.write_buffer(route_id, buffer).unwrap();
            }
        });
        
        cx.audio_input(move | _id, _device, _time, mut input_buffer | {
            input_buffer.make_single_channel();
            mic_send.write_buffer(0, input_buffer).unwrap();
            AudioBuffer::default()
        });
        
        cx.audio_output(move | _id, _device, _time, output_buffer | {
            //println!("buffer {:?}",_time);
            output_buffer.zero();
            // fill our read buffers on the audiostream without blocking
            mix_recv.try_recv_stream(1, 8);
            let mut chan = AudioBuffer::new_like(output_buffer);
            for i in 0..mix_recv.num_routes() {
                if mix_recv.read_buffer(i, &mut chan, 0, 1) != 0 {
                    for i in 0..chan.data.len() {
                        output_buffer.data[i] += chan.data[i];
                    }
                }
            }
        });
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // no UI as of yet
        match event {
            Event::Draw(event) => {
                return self.draw(&mut Cx2d::new(cx, event));
            }
            Event::Construct => {
                self.start_network_stack(cx);
            }
            Event::MidiPorts(ports) => {
                cx.use_midi_inputs(&ports.all_inputs());
            }
            Event::AudioDevices(devices) => {
                cx.use_audio_inputs(&devices.default_input());
                cx.use_audio_outputs(&devices.default_output());
            }
            _ => ()
        }
        
        self.ui.handle_event(cx, event);
        self.window.handle_event(cx, event);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        while self.ui.draw(cx).is_not_done() {};
        
        //self.ui.redraw(cx);
        self.window.end(cx);
    }
}