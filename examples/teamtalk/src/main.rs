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
    std::sync::mpsc,
    std::thread,
    std::time,
    std::net::UdpSocket,
    std::time::Duration
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
    
    pub fn start_audio_output(&mut self, cx: &mut Cx) {
        const WIRE_CHUNK: usize = 100;
        
        let (mic_send, mut mic_recv) = AudioStreamSender::create_pair();
        let (mix_send, mut mix_recv) = AudioStreamSender::create_pair();
        
        let write_announce = UdpSocket::bind("0.0.0.0:41531").unwrap();
        write_announce.set_read_timeout(Some(Duration::new(0, 1))).unwrap();
        write_announce.set_broadcast(true).unwrap();
        let read_announce = write_announce.try_clone().unwrap();
        
        std::thread::spawn(move || {
            let announce_data = [0u8];
            loop {
                write_announce.send_to(&announce_data, "255.255.255.255:41531").unwrap();
                thread::sleep(time::Duration::from_secs(1));
            }
        });
        
        let write_audio = UdpSocket::bind("0.0.0.0:41532").unwrap();
        write_audio.set_read_timeout(Some(Duration::new(5, 0))).unwrap();
        let read_audio = write_audio.try_clone().unwrap();
        
        std::thread::spawn(move || {
            let mut wire_data = Vec::new();
            let mut addrs = Vec::new();
            loop {
                let mut announce_data = [0u8];
                while let Ok((_, mut addr)) = read_announce.recv_from(&mut announce_data) {
                    addr.set_port(41532);
                    if !addrs.contains(&addr){
                        addrs.push(addr);
                    }
                }
                mic_recv.recv_stream();
                // broadcast the micstream to our addr list
                loop {
                    let mut output_buffer = AudioBuffer::new_with_size(WIRE_CHUNK, 2);
                    if mic_recv.read_buffer(0, &mut output_buffer, 1, 10) == 0 {
                        break;
                    }
                    let buf = output_buffer.channel(0);
                    let mut sum = 0.0;
                    for v in buf {
                        sum += v.abs();
                    }
                    let peak = sum / buf.len() as f32;
                    let data = output_buffer.into_data();
                    let packet = if peak>0.00001 {
                        TeamTalkWire::Chunk(data)
                    }
                    else {
                        TeamTalkWire::Silence
                    };
                    wire_data.clear();
                    packet.ser_bin(&mut wire_data);
                    for addr in &addrs{
                        write_audio.send_to(&wire_data, addr).unwrap();
                    }
                };
            }
        });
        
        std::thread::spawn(move || {
            let mut read_buf = [0u8; 2048];
            while let Ok((len, addr)) = read_audio.recv_from(&mut read_buf) {
                let read_buf = &read_buf[0..len];
                let packet = TeamTalkWire::deserialize_bin(&read_buf).unwrap();
                // lets get an audio buffer
                let buffer = match packet {
                    TeamTalkWire::Chunk(data) => {
                        AudioBuffer::from_data(data, 2)
                    }
                    TeamTalkWire::Silence => {
                        AudioBuffer::new_with_size(WIRE_CHUNK, 2)
                    }
                };
                let id = if let std::net::IpAddr::V4(v4) = addr.ip() {
                    v4.octets()[3] as u64
                }else {1};
                mix_send.write_buffer(id, buffer).unwrap();
            }
        });
        
        cx.start_audio_output(move | _time, output_buffer | {
            output_buffer.zero();
            mix_recv.try_recv_stream();
            let mut chan = AudioBuffer::new_like(output_buffer);
            for i in 0..mix_recv.num_routes() {
                if mix_recv.read_buffer(i, &mut chan, 0, 10) != 0 {
                    // lets mix it in
                    for i in 0..chan.data.len() {
                        output_buffer.data[i] += chan.data[i];
                    }
                }
            }
        });
        
        cx.start_audio_input(move | _time, input_buffer | {
            mic_send.write_buffer(0, input_buffer).unwrap();
            AudioBuffer::default()
        });
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.draw(&mut Cx2d::new(cx, event));
        }
        if let Event::Construct = event {
            println!("{:?}", cx.platform_type());
            self.start_audio_output(cx);
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