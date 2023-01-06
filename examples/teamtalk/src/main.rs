/*
TeamTalk is a LAN (only) p2p audiochat supporting as many clients as you have bandwidth.
For 6 clients it should pull about 100 megabits. You can use it to have a super low latency
helicopter-headset experience, silent disco, and so on.
*/

pub use makepad_audio_graph;
pub use makepad_audio_graph::makepad_widgets;
pub use makepad_audio_graph::makepad_platform;
pub use makepad_micro_serde;
use {
    crate::{
        makepad_micro_serde::{SerBin, DeBin, DeBinErr},
        makepad_audio_graph::audio_stream::AudioStreamSender,
        makepad_widgets::*,
        makepad_platform::audio::AudioBuffer,
        makepad_draw::*,
    },
    std::collections::HashMap,
    std::thread,
    std::time,
    std::net::UdpSocket,
    std::time::{Duration,Instant},
};


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

#[derive(SerBin, DeBin)]
enum TeamTalkWire {
    Silence,
    Chunk(Vec<f32>),
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_audio_graph::live_design(cx);
    }
    
    pub fn start_audio_io(&mut self, cx: &mut Cx) {
        // the number of audio frames (samples) in a UDP packet
        const WIRE_FRAMES: usize = 100;
        
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
            let dummy = [0u8];
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
            loop { 
                let mut dummy = [0u8];
                let time_now = Instant::now();
                // nonblockingly (timeout=1ns) check our discovery socket for peers
                while let Ok((_, mut addr)) = read_discovery.recv_from(&mut dummy) {
                    addr.set_port(41532);
                    if let Some(time) = peer_addrs.get_mut(&addr){
                        *time = time_now;
                    }
                    else{
                        peer_addrs.insert(addr, time_now);
                    }
                }
                // flush peers we havent heard from more than 5 seconds ago
                peer_addrs.retain(|_,time| *time > time_now - Duration::from_secs(5) );
                
                // fill the mic stream recv side buffers
                mic_recv.recv_stream();
                
                // read as many WIRE_FRAMES size buffers from our micstream and send to all peers
                loop {
                    let mut output_buffer = AudioBuffer::new_with_size(WIRE_FRAMES, 2);
                    if mic_recv.read_buffer(0, &mut output_buffer, 1, 10) == 0 {
                        break;
                    }
                    let buf = output_buffer.channel(0);
                    
                    // do a quick volume check so we can send 1 byte packets if silent
                    let mut sum = 0.0;
                    for v in buf {
                        sum += v.abs();
                    }
                    let peak = sum / buf.len() as f32;
                    let data = output_buffer.into_data();
                    
                    let wire_packet = if peak>0.001 {
                        TeamTalkWire::Chunk(data)
                    }
                    else {
                        TeamTalkWire::Silence
                    };
                    // serialise the packet enum for sending over the wire
                    wire_data.clear();
                    wire_packet.ser_bin(&mut wire_data);
                    // send to all peers
                    for addr in peer_addrs.keys(){
                        write_audio.send_to(&wire_data, addr).unwrap();
                    }
                };
            }
        });
        
        // the network audio receiving thread
        std::thread::spawn(move || {
            let mut read_buf = [0u8; 2048];
            while let Ok((len, addr)) = read_audio.recv_from(&mut read_buf) {
                let read_buf = &read_buf[0..len];

                // deserialize the packet from the buffer
                let packet = TeamTalkWire::deserialize_bin(&read_buf).unwrap();
                
                // create an audiobuffer from the data
                let buffer = match packet {
                    TeamTalkWire::Chunk(data) => {
                        AudioBuffer::from_data(data, 2)
                    }
                    TeamTalkWire::Silence => {
                        AudioBuffer::new_with_size(WIRE_FRAMES, 2)
                    }
                };
                // use the last digit of our ipv4 address as our 'route' id
                // the audio stream supports multiple id'ed paths so you can mix at output
                let id = if let std::net::IpAddr::V4(v4) = addr.ip() {
                    v4.octets()[3] as u64
                }else {1};
                mix_send.write_buffer(id, buffer).unwrap();
            }
        });
        
        // the audio output thread
        cx.start_audio_output(move | _time, output_buffer | {
            output_buffer.zero();
            // fill our read buffers on the audiostream without blocking
            mix_recv.try_recv_stream();
            let mut chan = AudioBuffer::new_like(output_buffer);
            for i in 0..mix_recv.num_routes() {
                // every route is a 'peer's audios tream, pull them into a buffer
                // and then just add it (mix) in the output buffer
                if mix_recv.read_buffer(i, &mut chan, 0, 10) != 0 {
                    // lets mix it in
                    for i in 0..chan.data.len() {
                        output_buffer.data[i] += chan.data[i];
                    }
                }
            }
        });
        
        // the microphone input thread, just pushes the input data into an audiostream
        cx.start_audio_input(move | _time, input_buffer | {
            mic_send.write_buffer(0, input_buffer).unwrap();
            AudioBuffer::default()
        });
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // no UI as of yet
        if let Event::Draw(event) = event {
            return self.draw(&mut Cx2d::new(cx, event));
        }
        if let Event::Construct = event {
            println!("{:?}", cx.platform_type());
            self.start_audio_io(cx);
        }
        self.ui.handle_event(cx, event);
        self.window.handle_event(cx, event);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).not_redrawing() {
            return;
        }
        
        while self.ui.draw(cx).is_not_done() {};
        
        self.ui.redraw(cx);
        self.window.end(cx);
    }
}