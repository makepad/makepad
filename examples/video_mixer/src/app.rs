pub use makepad_audio_widgets;
pub use makepad_audio_widgets::makepad_widgets;
pub use makepad_widgets::makepad_platform;
use std::sync::{Arc};

use {
    crate::{
        makepad_audio_widgets::display_audio::*,
        makepad_draw::makepad_image_formats,
        makepad_widgets::*,
        makepad_platform::midi::*,
        makepad_platform::audio::*,
        makepad_platform::live_atomic::*,
        makepad_platform::audio_stream::*,
        makepad_platform::thread::*,
        makepad_platform::video::*,
        makepad_draw::*,
    },
    std::thread,
    std::io::{Read},
    std::net::{UdpSocket,TcpStream},
    std::time::{Duration},
};


live_design!{
    import makepad_widgets::frame::*;
    import makepad_draw::shader::std::*;
    //import crate::video_view::VideoView;
    registry Widget::*;
    
    VideoFrame = <Frame> {
        show_bg: true,
        walk: {width: Fill, height: Fill},
        draw_bg: {
            texture image: texture2d
            uniform image_size: vec2
            uniform is_rgb: 0.0
            fn yuv_to_rgb(y: float, u: float, v: float) -> vec4 {
                return vec4(
                    y + 1.14075 * (v - 0.5),
                    y - 0.3455 * (u - 0.5) - 0.7169 * (v - 0.5),
                    y + 1.7790 * (u - 0.5),
                    1.0
                )
            }
            
            fn get_video_pixel(self) -> vec4 {
                let pix = self.pos * self.image_size;
                
                // fetch pixel
                let data = sample2d(self.image, self.pos).xyzw;
                if self.is_rgb > 0.5{
                    return vec4(data.xyz, 1.0);
                }
                if mod (pix.x, 2.0)>1.0 {
                    return yuv_to_rgb(data.x, data.y, data.w)
                }
                return yuv_to_rgb(data.z, data.y, data.w)
            }
            
            fn pixel(self) -> vec4 {
                return self.get_video_pixel();
            }
        }
    }
    
    DisplayChannel = <Box> {
        draw_bg: {color: #3337, radius: 10.0}
        walk: {width: Fill, height: 300}
        disp = <DisplayAudio> {
        }
    }
    AudioMixer = {{AudioMixer}} {}
    App = {{App}} {
        mixer: {
            channel: [2.0, 2.0, 2.0, 2.0]
        }
        window: {ui: {inner_view = {
            video_input1 = <VideoFrame> {
                network_video = <VideoFrame> {
                    walk:{width:200,height:200}
                }
                layout: {align: {y: 1.0}, spacing: 5, padding: 10}
                chan1 = <DisplayChannel> {}
                chan2 = <DisplayChannel> {}
                chan3 = <DisplayChannel> {}
                chan4 = <DisplayChannel> {}
            }
        }}}
        window1: {
            window: {inner_size: vec2(400, 300)},
            ui: {inner_view = {
                video_input1 = <VideoFrame> {
                }
            }}
        }
        window2: {
            window: {inner_size: vec2(400, 300)},
            ui: {inner_view = {
                video_input1 = <VideoFrame> {
                }
            }}
        }
    }
}
app_main!(App);

#[derive(Live, LiveAtomic, LiveHook)]
pub struct AudioMixer {
    channel: [f32a; 4]
}

#[derive(Live, LiveHook)]
pub struct App {
    window: DesktopWindow,
    window1: DesktopWindow,
    window2: DesktopWindow,
    video_input1: Texture,
    video_network: Texture,
    mixer: Arc<AudioMixer>,
    #[rust(cx.midi_input())] midi_input: MidiInput,
    #[rust] network_recv: ToUIReceiver<makepad_image_formats::ImageBuffer>,
    #[rust] video_recv: ToUIReceiver<VideoBuffer>,
    #[rust] audio_recv: ToUIReceiver<(usize, AudioBuffer)>,
}

pub fn read_exact_bytes_from_tcp_stream(tcp_stream: &mut TcpStream, bytes: &mut [u8]) -> bool{
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &mut bytes[(bytes_total - bytes_left)..bytes_total];
        if let Ok(bytes_read) = tcp_stream.read(buf){
            if bytes_read == 0 {
                return true;
            }
            bytes_left -= bytes_read;
        }
    }
    false
}

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_audio_widgets::live_design(cx);
    }
    
    pub fn start_network_stack(&mut self, _cx: &mut Cx) {
        
        let read_discovery = UdpSocket::bind("0.0.0.0:42531").unwrap();
        read_discovery.set_read_timeout(Some(Duration::new(0, 1))).unwrap();
        read_discovery.set_broadcast(true).unwrap();
        let sender = self.network_recv.sender();
        
        std::thread::spawn(move || {
            let peer_addr;
            loop{
                let mut data  = [0u8;32];
                if let Ok((_, mut addr)) = read_discovery.recv_from(&mut data) {
                    addr.set_port(42532);
                    peer_addr = Some(addr);
                    break;
                } 
                thread::sleep(Duration::from_millis(10));
            }
            // alright if we have a peer addr lets connect to it
            loop{
                if let Ok(mut tcp_stream) = TcpStream::connect(peer_addr.unwrap()){
                    //let mut frame_count = 0;
                    loop{
                      //  frame_count += 1;
                        let mut len = [0u8;4];
                        if read_exact_bytes_from_tcp_stream(&mut tcp_stream, &mut len){break;}
                        let len:u32 = u32::from_be_bytes(len);
                        if len == 0{
                            break;
                        }
                        let mut buffer = Vec::new();
                        buffer.resize(len as usize, 0);
                        if read_exact_bytes_from_tcp_stream(&mut tcp_stream, &mut buffer){break;}
                        
                        // decode jpeg
                        match makepad_image_formats::jpeg::decode(&buffer) {
                            Ok(data)=>{
                                //let _ = std::fs::File::create(&format!("dump{frame_count}.jpg")).unwrap().write(&buffer);
                                let _= sender.send(data);
                            }
                            Err(e)=>{ 
                                log!("JPEG DECODE ERROR {}", e);
                            }
                        }
                    }
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
    }
    
    pub fn start_inputs(&mut self, cx: &mut Cx) {
        let (send, mut recv) = AudioStreamSender::create_pair(1, 1);
        let send1 = send.clone();
        let audio_send1 = self.audio_recv.sender();
        cx.audio_input(0, move | _info, input | {
            let _ = audio_send1.send((0, input.clone()));
            let _ = send1.send(0, input.clone());
        });
        
        let send2 = send.clone();
        let audio_send2 = self.audio_recv.sender();
        cx.audio_input(1, move | _info, input | {
            let _ = audio_send2.send((1, input.clone()));
            let _ = send2.send(1, input.clone());
        });
        let mixer = self.mixer.clone();
        cx.audio_output(0, move | _info, output | {
            recv.try_recv_stream();
            let mut input1 = AudioBuffer::new_like(output);
            let mut input2 = AudioBuffer::new_like(output);
            recv.read_buffer(0, &mut input1);
            recv.read_buffer(1, &mut input2);
            // now lets mix our inputs we combine every input
            for j in 0..output.frame_count() {
                let audio =
                input1.channel(0)[j] * mixer.channel[0].get()
                    + input1.channel(1)[j] * mixer.channel[1].get()
                    + input2.channel(0)[j] * mixer.channel[2].get()
                    + input2.channel(1)[j] * mixer.channel[3].get();
                output.channel_mut(0)[j] = audio * 5.0;
                output.channel_mut(1)[j] = audio * 5.0;
            }
        });
        let video_sender = self.video_recv.sender();
        cx.video_input(0, move | img | {
            let _ = video_sender.send(img.to_buffer());
        })
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Signal => {
                while let Ok(mut nw_image) = self.network_recv.try_recv(){
                    self.video_network.set_desc(cx, TextureDesc {
                        format: TextureFormat::ImageBGRA,
                        width: Some(nw_image.width),
                        height: Some(nw_image.height)
                    });
                    self.video_network.swap_image_u32(cx, &mut nw_image.data);
                    let image_size = [nw_image.width as f32, nw_image.height as f32];
                    
                    for v in [
                        self.window.ui.get_frame(id!(network_video)),
                    ] {
                        v.set_texture(0, &self.video_network);
                        v.set_uniform(cx, id!(image_size), &image_size);
                        v.set_uniform(cx, id!(is_rgb), &[1.0]);
                        v.redraw(cx);
                    }
                }
                while let Some((_, data)) = self.midi_input.receive() {
                    match data.decode() {
                        MidiEvent::ControlChange(cc) => {
                            if cc.param == 2 {self.mixer.channel[0].set(cc.value as f32 / 63.0)};
                            if cc.param == 3 {self.mixer.channel[1].set(cc.value as f32 / 63.0)};
                            if cc.param == 4 {self.mixer.channel[2].set(cc.value as f32 / 63.0)};
                            if cc.param == 5 {self.mixer.channel[3].set(cc.value as f32 / 63.0)};
                        }
                        _ => ()
                    }
                    println!("{:?}", data.decode());
                }
                // lets receive the audio buffers
                while let Ok((input, audio)) = self.audio_recv.try_recv() {
                    if input == 0 {
                        self.window.ui.get_display_audio(id!(chan1.disp)).process_buffer(cx, Some(0), 0, &audio);
                        self.window.ui.get_display_audio(id!(chan2.disp)).process_buffer(cx, Some(1), 0, &audio);
                    }
                    if input == 1 {
                        self.window.ui.get_display_audio(id!(chan3.disp)).process_buffer(cx, Some(0), 0, &audio);
                        self.window.ui.get_display_audio(id!(chan4.disp)).process_buffer(cx, Some(1), 0, &audio);
                    }
                }
                if let Ok(mut vfb) = self.video_recv.try_recv_flush() {
                    self.video_input1.set_desc(cx, TextureDesc {
                        format: TextureFormat::ImageBGRA,
                        width: Some(vfb.format.width / 2),
                        height: Some(vfb.format.height)
                    });
                    if let Some(buf) = vfb.as_vec_u32() {
                        self.video_input1.swap_image_u32(cx, buf);
                    }
                    let image_size = [vfb.format.width as f32, vfb.format.height as f32];
                    
                    for v in [
                        self.window.ui.get_frame(id!(video_input1)),
                        self.window1.ui.get_frame(id!(video_input1)),
                        self.window2.ui.get_frame(id!(video_input1))
                    ] {
                        v.set_texture(0, &self.video_input1);
                        v.set_uniform(cx, id!(image_size), &image_size);
                        v.set_uniform(cx, id!(is_rgb), &[0.0]);
                        v.redraw(cx);
                    }
                }
            }
            Event::Draw(event) => {
                return self.draw(&mut Cx2d::new(cx, event));
            }
            Event::Construct => {
                self.start_inputs(cx);
                self.start_network_stack(cx);
            }
            Event::MidiPorts(ports) => {
                cx.use_midi_inputs(&ports.all_inputs());
            }
            Event::AudioDevices(devices) => {
                // ok which audio devices
                //println!("{}", devices);
                let inputs = devices.match_inputs(&[
                    "Wireless GO II RX",
                    "USB Capture HDMI 4K+ Mic"
                ]);
                cx.use_audio_inputs(&inputs);
                let output = devices.match_outputs(&[
                    "G432 Gaming Headset",
                ]);
                cx.use_audio_outputs(&output);
            }
            Event::VideoInputs(devices) => {
                //println!("Got devices! {:?}", devices);
                cx.use_video_input(&devices.find_highest_at_res(0, 3840, 2160));
            }
            _ => ()
        }
        
        self.window.handle_event(cx, event);
        self.window1.handle_event(cx, event);
        self.window2.handle_event(cx, event);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).is_redrawing() {
            self.window.end(cx);
        }
        if self.window1.begin(cx).is_redrawing() {
            self.window1.end(cx);
        }
        if self.window2.begin(cx).is_redrawing() {
            self.window2.end(cx);
        }
    }
}