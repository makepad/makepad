use std::sync::{Arc};

use {
    crate::{
        makepad_audio_widgets::display_audio::*,
        makepad_draw::makepad_image_formats,
        makepad_widgets::*,
        makepad_widgets::slides_view::*,
        makepad_platform::midi::*,
        makepad_platform::audio::*,
        makepad_platform::live_atomic::*,
        makepad_platform::audio_stream::*,
        makepad_platform::thread::*,
        makepad_platform::video::*,
    },
    std::thread,
    std::io::{Read},
    std::net::{UdpSocket, TcpStream, Shutdown},
    std::time::{Duration},
};


live_design!{
    import makepad_widgets::frame::*;
    import makepad_widgets::slides_view::*;
    import makepad_draw::shader::std::*;
    
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
                if self.is_rgb > 0.5 {
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
    
    VideoFrameRound = <VideoFrame> {
        draw_bg: {
            uniform alpha: 1.0
            fn pixel(self) -> vec4 {
                let color = self.get_video_pixel();
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box(
                    1,
                    1,
                    self.rect_size.x - 2,
                    self.rect_size.y - 2,
                    10
                )
                sdf.fill_keep(vec4(color.xyz, self.alpha))
                return sdf.result
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
    Slide = <Slide> {
        walk: {width: 1920.0, height: Fill}
    }
    
    MainSlides = <Frame> {
        layout: {flow: Overlay, align: {x: 1.0, y: 1.0}, padding: 0}
        slides_view = <SlidesView> {
            slide_width: 1920.0
            goal_pos: 15.0
            frame: {
                video_input1 = <VideoFrame> {
                    walk: {width: 1920, height: Fill}
                }
                <Image> {
                    image: d"crate://self/rust_meetup_slide.png",
                    walk: {width: 1920, height: 1080}
                }
                <Slide> {title = {text: "Intro"}, <SlideBody> {text: "Rik Arends\nBuilding Makepad\nRust livecoding IDE"}}
                <Slide> {title = {text: "Recording Talks"}, <SlideBody> {text: "Not as easy as you think"}}
                <Slide> {title = {text: "What do you need"}, <SlideBody> {text: "- Multiple microphones\n- Audio Mixing ability\n- Video mixing\n- Speaker camera\n- Event slide"}}
                <Image> {
                    image: d"crate://self/camera_gear.png",
                    walk: {width: 1920, height: 1080}
                }
                <Slide> {title = {text: "So much gear"}, <SlideBody> {text: "- What if we use software instead"}}
                <Slide> {title = {text: "We can replace"}, <SlideBody> {text: "- Expensive camera: cheap android\n- Audio mixer: laptop\n- Video mixer: laptop\n- Audio Mixer: Midi BLE controller"}}
                <Slide> {title = {text: "The software"}, <SlideBody> {text: "- Android app video send\n- Macos Audio+Video+Midi+Rendering"}}
                <Slide> {title = {text: "Remote Camera:"}, <SlideBody> {text: "- Android: Endless buildsystem wrangling\n- Gradle, Java, Android Studio, NDK, Linker, APK"}}
                <Slide> {title = {text: "Simple Rust on Android"}, <SlideBody> {text: "- Strip out Gradle/Android Studio\n- Repackage all buildtooling\n#cargo makepad android toolchain-install\n#cargo makepad android run makepad-example-video-sender"}}
                <Slide> {title = {text: "Remote camera: Solved"}, <SlideBody> {text: ""}network_video2 = <VideoFrameRound> {
                    walk: {width: 640, height: 480}
                }}
                
                <Slide> {title = {text: "Video mixer"}, <SlideBody> {text: "- Mixing inputs\n- Connect to network camera\n- Audio mixing"}}
                <Slide> {title = {text: "Makepad APIs"}, <SlideBody> {text: "- Windowing\n- Graphics\n- UI Components\n- Audio in/out\n- Midi in/out\n- Video In"}}
                <Slide> {title = {text: "Nice deadline"}, <SlideBody> {text: "- 22 platform apis in 10 weeks"}}
                <Slide> {title = {text: "Audio Inputs"}, <SlideBody> {}
                    <Box> {
                        walk:{height:Fit}
                        draw_bg:{color:#5}
                        chan5 = <DisplayChannel> {}
                        chan6 = <DisplayChannel> {}
                        chan7 = <DisplayChannel> {}
                        chan8 = <DisplayChannel> {}
                    }
                }
                <Slide> {title = {text: "DSL for styling"}, <SlideBody> {text: "- Almost not crap to do"}}
                <Image> {
                    image: d"crate://self/dsl_view.png",
                    walk: {width: 1920, height: 1080}
                }
                warp_image = <Image> {
                    draw_bg: {
                        uniform warp: 0.0
                        fn get_color(self) -> vec4 {
                            let wp = mix(
                                self.pos,
                                abs(sin(self.pos * 8.0)),
                                self.warp
                            );
                            return sample2d(self.image, wp).xyzw;
                        }
                    }
                    image: d"crate://self/rust_meetup_slide.png",
                    walk: {width: 1920, height: 1080}
                    layout: {align: {y: 0.0}, spacing: 5, padding: 10}
                    
                }
                <Slide> {title = {text: "Thank you"}, <SlideBody> {text: "- github.com/makepad/makepad\n- twitter: @rikarends"}}
            }
        }
    }
    App = {{App}} {
        mixer: {
            channel: [2.0, 2.0, 2.0, 2.0]
        }
        window1: {
            window: {inner_size: vec2(1920, 1080), position: vec2(1500, 1000)},
            ui: {
                inner_view = {
                    <MainSlides> {
                        <Frame> {
                            network_video = <VideoFrameRound> {
                                walk: {width: 320, height: 240}
                            }
                            layout: {align: {y: 1.0}, spacing: 5, padding: 10}
                            chan1 = <DisplayChannel> {}
                            chan2 = <DisplayChannel> {}
                            chan3 = <DisplayChannel> {}
                            chan4 = <DisplayChannel> {}
                        }
                    }
                }
            }
        }
        window2: {
            window: {inner_size: vec2(400, 300)},
            ui: {inner_view = {
                <MainSlides> {
                    
                    network_video = <VideoFrameRound> {
                        draw_bg: {alpha: 0.9},
                        walk: {width: 320, height: 240, margin: {bottom: 50, right: 50}}
                    }
                }
            }}
        }
        window3: {
            window: {inner_size: vec2(400, 300)},
            ui: {inner_view = {
                <MainSlides> {}
            }}
        }
    }
}
app_main!(App);

#[derive(Live, LiveAtomic, LiveHook)]
pub struct AudioMixer {
    channel: [f32a; 4],
    gain: [f32a; 4]
}

#[derive(Live, LiveHook)]
#[live_design_with {
    crate::makepad_audio_widgets::live_design(cx);
}]

pub struct App {
    window1: DesktopWindow,
    window2: DesktopWindow,
    window3: DesktopWindow,
    video_input1: Texture,
    video_network: Texture,
    mixer: Arc<AudioMixer>,
    #[rust] restart_network: Signal,
    #[rust(cx.midi_input())] midi_input: MidiInput,
    #[rust] network_recv: ToUIReceiver<makepad_image_formats::ImageBuffer>,
    #[rust] video_recv: ToUIReceiver<VideoBuffer>,
    #[rust] audio_recv: ToUIReceiver<(usize, AudioBuffer)>,
}

pub fn read_exact_bytes_from_tcp_stream(tcp_stream: &mut TcpStream, bytes: &mut [u8]) -> bool {
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &mut bytes[(bytes_total - bytes_left)..bytes_total];
        if let Ok(bytes_read) = tcp_stream.read(buf) {
            if bytes_read == 0 {
                return true;
            }
            bytes_left -= bytes_read;
        }
        else {
            return true;
        }
    }
    false
}

impl App {
    pub fn start_network_stack(&mut self, _cx: &mut Cx) {
        let read_discovery = UdpSocket::bind("0.0.0.0:42531").unwrap();
        read_discovery.set_read_timeout(Some(Duration::new(0, 10))).unwrap();
        read_discovery.set_broadcast(true).unwrap();
        let sender = self.network_recv.sender();
        let restart_network = self.restart_network.clone();
        std::thread::spawn(move || {
            loop {
                let mut peer_addr = Some("192.168.1.152:42532".parse().unwrap());
                let mut data = [0u8; 32];
                while let Ok((_, mut addr)) = read_discovery.recv_from(&mut data) {
                    addr.set_port(42532);
                    peer_addr = Some(addr);
                }
                // alright if we have a peer addr lets connect to it
                if peer_addr.is_none() {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                
                if let Ok(mut tcp_stream) = TcpStream::connect_timeout(&peer_addr.unwrap(), Duration::new(5, 0)) {
                    tcp_stream.set_read_timeout(Some(Duration::new(2, 0))).unwrap();
                    log!("Connected to phone {}", peer_addr.unwrap());
                    loop {
                        if restart_network.check_and_clear() {
                            break;
                        }
                        let mut len = [0u8; 4];
                        if read_exact_bytes_from_tcp_stream(&mut tcp_stream, &mut len) {break;}
                        let len: u32 = u32::from_be_bytes(len);
                        if len == 0 {
                            log!("Read failed restarting connection");
                            break;
                        }
                        if len < 5000 || len > 100000 {
                            log!("Length invalid {} restarting connection", len);
                            break;
                        }
                        let mut buffer = Vec::new();
                        buffer.resize(len as usize, 0);
                        if read_exact_bytes_from_tcp_stream(&mut tcp_stream, &mut buffer) {break;}
                        
                        match makepad_image_formats::jpeg::decode(&buffer) {
                            Ok(data) => {
                                let _ = sender.send(data);
                            }
                            Err(e) => {
                                log!("JPEG DECODE ERROR {}", e);
                            }
                        }
                    }
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                }
                thread::sleep(Duration::from_millis(300));
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
                input1.channel(0)[j] * mixer.channel[0].get() * mixer.gain[0].get()
                    + input1.channel(1)[j] * mixer.channel[1].get() * mixer.gain[1].get()
                    + input2.channel(0)[j] * mixer.channel[2].get() * mixer.gain[2].get()
                    + input2.channel(1)[j] * mixer.channel[3].get() * mixer.gain[3].get();
                output.channel_mut(0)[j] = audio * 5.0;
                output.channel_mut(1)[j] = audio * 5.0;
            }
        });
        let video_sender = self.video_recv.sender();
        cx.video_input(0, move | img | {
            let _ = video_sender.send(img.to_buffer());
        })
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window1.begin(cx).is_redrawing() {
            self.window1.end(cx);
        }
        if self.window2.begin(cx).is_redrawing() {
            self.window2.end(cx);
        }
        if self.window3.begin(cx).is_redrawing() {
            self.window3.end(cx);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Signal => {
                while let Ok(mut nw_image) = self.network_recv.try_recv() {
                    self.video_network.set_desc(cx, TextureDesc {
                        format: TextureFormat::ImageBGRA,
                        width: Some(nw_image.width),
                        height: Some(nw_image.height)
                    });
                    self.video_network.swap_image_u32(cx, &mut nw_image.data);
                    let image_size = [nw_image.width as f32, nw_image.height as f32];
                    
                    for v in [
                        self.window1.ui.get_frame(id!(network_video)),
                        self.window2.ui.get_frame(id!(network_video)),
                        self.window3.ui.get_frame(id!(network_video)),
                        self.window1.ui.get_frame(id!(network_video2)),
                        self.window2.ui.get_frame(id!(network_video2)),
                        self.window3.ui.get_frame(id!(network_video2)),
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
                            log!("{} {}", cc.param, cc.value);
                            if cc.param == 2 {self.mixer.channel[0].set(cc.value as f32 / 63.0)};
                            if cc.param == 3 {self.mixer.channel[1].set(cc.value as f32 / 63.0)};
                            if cc.param == 4 {self.mixer.channel[2].set(cc.value as f32 / 63.0)};
                            if cc.param == 5 {self.mixer.channel[3].set(cc.value as f32 / 63.0)};
                            if cc.param == 13 {self.mixer.gain[0].set(cc.value as f32 / 63.0)};
                            if cc.param == 14 {self.mixer.gain[1].set(cc.value as f32 / 63.0)};
                            if cc.param == 15 {self.mixer.gain[2].set(cc.value as f32 / 63.0)};
                            if cc.param == 16 {self.mixer.gain[3].set(cc.value as f32 / 63.0)};
                            if cc.param == 20 {
                                let val = cc.value as f32 / 127.0;
                                log!("{}", val);
                                for v in [
                                    self.window1.ui.get_frame(id!(warp_image)),
                                    self.window2.ui.get_frame(id!(warp_image)),
                                    self.window3.ui.get_frame(id!(warp_image)),
                                ] {
                                    v.set_uniform(cx, id!(warp), &[val]);
                                }
                            };
                            if cc.param == 28 && cc.value == 127 {
                                self.restart_network.set();
                            }
                            if cc.param == 62 && cc.value == 127 {
                                for v in [
                                    self.window1.ui.get_slides_view(id!(slides_view)),
                                    self.window2.ui.get_slides_view(id!(slides_view)),
                                    self.window3.ui.get_slides_view(id!(slides_view)),
                                ] {
                                    v.prev_slide(cx);
                                }
                            }
                            if cc.param == 81 && cc.value == 127 {
                                for v in [
                                    self.window1.ui.get_slides_view(id!(slides_view)),
                                    self.window2.ui.get_slides_view(id!(slides_view)),
                                    self.window3.ui.get_slides_view(id!(slides_view)),
                                ] {
                                    v.next_slide(cx);
                                }
                            }
                        }
                        _ => ()
                    }
                }
                // lets receive the audio buffers
                while let Ok((input, audio)) = self.audio_recv.try_recv() {
                    if input == 0 {
                        self.window1.ui.get_display_audio(id!(chan1.disp)).process_buffer(cx, Some(0), 0, &audio);
                        self.window1.ui.get_display_audio(id!(chan2.disp)).process_buffer(cx, Some(1), 0, &audio);
                        self.window1.ui.get_display_audio(id!(chan5.disp)).process_buffer(cx, Some(0), 0, &audio);
                        self.window1.ui.get_display_audio(id!(chan6.disp)).process_buffer(cx, Some(1), 0, &audio);
                    }
                    if input == 1 {
                        self.window1.ui.get_display_audio(id!(chan3.disp)).process_buffer(cx, Some(0), 0, &audio);
                        self.window1.ui.get_display_audio(id!(chan4.disp)).process_buffer(cx, Some(1), 0, &audio);
                        self.window1.ui.get_display_audio(id!(chan7.disp)).process_buffer(cx, Some(0), 0, &audio);
                        self.window1.ui.get_display_audio(id!(chan8.disp)).process_buffer(cx, Some(1), 0, &audio);
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
                        self.window1.ui.get_frame(id!(video_input1)),
                        self.window2.ui.get_frame(id!(video_input1)),
                        self.window3.ui.get_frame(id!(video_input1))
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
                let inputs = devices.match_inputs(&[
                    "Wireless GO II RX",
                ]);
                cx.use_audio_inputs(&inputs);
                let output = devices.match_outputs(&[
                    "External Headphones",
                ]);
                cx.use_audio_outputs(&output);
            }
            Event::VideoInputs(devices) => {
                cx.use_video_input(&devices.find_highest_at_res(devices.find_device("USB Capture HDMI 4k+"), 1920, 1080));
            }
            _ => ()
        }
        
        self.window1.handle_event(cx, event);
        self.window2.handle_event(cx, event);
        self.window3.handle_event(cx, event);
    }
    
}