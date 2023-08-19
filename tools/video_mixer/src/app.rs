use std::sync::{Arc};

use {
    crate::{
        makepad_audio_widgets::display_audio::*,
        makepad_widgets::*,
        makepad_widgets::slides_view::*,
        makepad_platform::midi::*,
        makepad_platform::audio::*,
        makepad_platform::live_atomic::*,
        makepad_platform::audio_stream::*,
        makepad_platform::thread::*,
        makepad_platform::video::*,
    },
};


live_design!{
    import makepad_widgets::frame::*;
    import makepad_widgets::image::Image;
    import makepad_widgets::slides_view::Slide;
    import makepad_widgets::slides_view::SlidesView;
    import makepad_widgets::slides_view::SlideBody;
    import makepad_draw::shader::std::*;
    
    import makepad_widgets::desktop_window::DesktopWindow
    import makepad_widgets::multi_window::MultiWindow
    import makepad_audio_widgets::display_audio::DisplayAudio
    
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
            
            fn get_video_pixel(self, pos:vec2) -> vec4 {
                let pix = self.pos * self.image_size;
                
                // fetch pixel
                let data = sample2d(self.image, pos).xyzw;
                if self.is_rgb > 0.5 {
                    return vec4(data.xyz, 1.0);
                }
                if mod (pix.x, 2.0)>1.0 {
                    return yuv_to_rgb(data.x, data.y, data.w)
                }
                return yuv_to_rgb(data.z, data.y, data.w)
            }
            
            fn pixel(self) -> vec4 {
                return self.get_video_pixel(self.pos);
            }
        }
    }
    
    VideoFrameRound = <VideoFrame> {
        draw_bg: {
            uniform alpha: 1.0
            fn pixel(self) -> vec4 {
                let color = self.get_video_pixel(vec2(1.0-self.pos.x,self.pos.y));
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
    
    MainSlides = <Frame> {
        layout: {flow: Overlay, align: {x: 1.0, y: 1.0}, padding: 0}
        slides_view = <SlidesView> {
            slide_width: 1920.0
            goal_pos: 0.0
            video_input0 = <VideoFrame> {
                walk: {width: 1920, height: Fill}
            }
            
            <Slide> {
                title = {label: "Schedule"},
                <SlideBody> {label: "18:00 Welcome & food\n19:00 Intro by Erik and Jasper\n19:10 Are we web yet, by Marlon Baeten\n19:40 Break\n20:00 Volumetric rendering using Rust, by Rosalie de Winther\n20:40 Building a Code Editor from scratch in Makepad, by Eddy Bruël\n21:10 Drinks"}
            }
        }
    }
    
    MeetupOverlay = <Image> {
        source: dep("crate://self/rust-meetup-breda.png"),
        walk: {width: 1920, height: 1080}
    }
    
    App = {{App}} {
        mixer: {
            channel: [2.0, 2.0, 2.0, 2.0]
        }
        ui: <MultiWindow> {
            window1 = <DesktopWindow> {
                window: {inner_size: vec2(960, 540), dpi_override: 1.0},
                <Frame> {
                    layout: {flow: Overlay}
                    <MainSlides> {}
                    meetup_overlay = <MeetupOverlay> {}
                    <Frame> {
                        layout: {align: {y: 1.0}, spacing: 5, padding: 40}
                        chan1 = <DisplayChannel> {}
                        chan2 = <DisplayChannel> {}
                        chan3 = <DisplayChannel> {}
                        chan4 = <DisplayChannel> {}
                        video_input1 = <VideoFrameRound> {
                            walk: {width: 480, height: 270}
                        }
                    }
                }
            }
            
            window2 = <DesktopWindow> {
                window: {inner_size: vec2(960, 540), position: vec2(0, 540), dpi_override: 1.0},
                layout: {flow: Overlay}
                <MainSlides> {}
                meetup_overlay = <MeetupOverlay> {}
            }
            window3 = <DesktopWindow> {
                window: {inner_size: vec2(960, 540), position: vec2(0, 540), dpi_override: 1.0},
                <Frame> {
                    layout: {flow: Overlay}
                    <MainSlides> {
                    }
                    <Frame> {
                        video_input1 = <VideoFrameRound> {
                            walk: {width: 480, height: 270}
                        }
                        layout: {align: {x: 1.0, y: 1.0}, spacing: 5, padding: 40}
                    }
                    meetup_overlay = <MeetupOverlay> {}
                }
            }
        }
    }
}
app_main!(App);

#[derive(Live, LiveAtomic, LiveHook)]
pub struct AudioMixer {
    #[live] channel: [f32a; 4],
    #[live] gain: [f32a; 4]
}

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust([Texture::new(cx), Texture::new(cx)])] video_input: [Texture; 2],
    #[live] mixer: Arc<AudioMixer>,
    #[rust(cx.midi_input())] midi_input: MidiInput,
    #[rust] video_recv: ToUIReceiver<(usize, VideoBuffer)>,
    #[rust] audio_recv: ToUIReceiver<(usize, AudioBuffer)>,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_audio_widgets::live_design(cx);
    }
}

impl App {
    
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
            let _ = video_sender.send((0, img.to_buffer()));
        });
        let video_sender = self.video_recv.sender();
        cx.video_input(1, move | img | {
            let _ = video_sender.send((1, img.to_buffer()));
        });
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Signal => {
                while let Some((_, data)) = self.midi_input.receive() {
                    match data.decode() {
                        MidiEvent::ControlChange(cc) => {
                            if cc.param == 2 {self.mixer.channel[0].set(cc.value as f32 / 63.0)};
                            if cc.param == 3 {self.mixer.channel[1].set(cc.value as f32 / 63.0)};
                            if cc.param == 4 {self.mixer.channel[2].set(cc.value as f32 / 63.0)};
                            if cc.param == 5 {self.mixer.channel[3].set(cc.value as f32 / 63.0)};
                            if cc.param == 13 {self.mixer.gain[0].set(cc.value as f32 / 63.0)};
                            if cc.param == 14 {self.mixer.gain[1].set(cc.value as f32 / 63.0)};
                            if cc.param == 15 {self.mixer.gain[2].set(cc.value as f32 / 63.0)};
                            if cc.param == 16 {self.mixer.gain[3].set(cc.value as f32 / 63.0)};
                            if cc.param == 19 {
                                let val = cc.value as f32 / 127.0;
                                let v = self.ui.get_frame_set(ids!(video_input1));
                                v.set_uniform(cx, id!(alpha), &[val]);
                            };
                            if cc.param == 20 {
                                let val = cc.value as f32 / 127.0;
                                let v = self.ui.get_frame_set(ids!(meetup_overlay));
                                v.set_uniform(cx, id!(image_alpha), &[val]);
                            };
                            if cc.param == 62 && cc.value == 127 {
                                let v = self.ui.get_slides_view_set(ids!(slides_view));
                                v.prev_slide(cx);
                            }
                            if cc.param == 81 && cc.value == 127 {
                                let v = self.ui.get_slides_view_set(ids!(slides_view));
                                v.next_slide(cx);
                            }
                        }
                        _ => ()
                    }
                }
                // lets receive the audio buffers
                while let Ok((input, audio)) = self.audio_recv.try_recv() {
                    if input == 0 {
                        self.ui.get_display_audio(id!(chan1.disp)).process_buffer(cx, Some(0), 0, &audio, self.mixer.gain[0].get() * self.mixer.channel[0].get());
                        self.ui.get_display_audio(id!(chan2.disp)).process_buffer(cx, Some(1), 0, &audio, self.mixer.gain[1].get() * self.mixer.channel[1].get());
                    }
                    if input == 1 {
                        self.ui.get_display_audio(id!(chan3.disp)).process_buffer(cx, Some(0), 0, &audio, self.mixer.gain[2].get() * self.mixer.channel[2].get());
                        self.ui.get_display_audio(id!(chan4.disp)).process_buffer(cx, Some(1), 0, &audio, self.mixer.gain[3].get() * self.mixer.channel[3].get());
                    }
                }
                while let Ok((id, mut vfb)) = self.video_recv.try_recv() {
                    self.video_input[id].set_desc(cx, TextureDesc {
                        format: TextureFormat::ImageBGRA,
                        width: Some(vfb.format.width / 2),
                        height: Some(vfb.format.height)
                    });
                    if let Some(buf) = vfb.as_vec_u32() {
                        self.video_input[id].swap_image_u32(cx, buf);
                    }
                    let image_size = [vfb.format.width as f32, vfb.format.height as f32];
                    for v in self.ui.get_frame_set(if id == 0 {ids!(video_input0)}else {ids!(video_input1)}).iter() {
                        v.set_texture(0, &self.video_input[id]);
                        v.set_uniform(cx, id!(image_size), &image_size);
                        v.set_uniform(cx, id!(is_rgb), &[0.0]);
                        v.redraw(cx);
                    }
                }
            }
            Event::Draw(event) => {
                return self.ui.draw_widget_all(&mut Cx2d::new(cx, event));
            }
            Event::Construct => {
                self.start_inputs(cx);
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
                    "NINJA V",
                ]);
                //log!("{:?}", devices);
                cx.use_audio_outputs(&output);
            }
            Event::VideoInputs(devices) => {
                log!("{:?}", devices);
                //cx.use_video_input(&devices.find_highest_at_res(devices.find_device("USB Capture HDMI 4K+"), 1920, 1080));
                //let input_a = devices.find_highest_at_res(devices.find_device("USB Capture HDMI 4K+"), 1920, 1080, 90.0);
                //let input_b = devices.find_highest_at_res(devices.find_device("Game Capture HD60 X"), 1920, 1080, 90.0);
                let input_a = devices.find_highest_at_res(devices.find_device("USB Capture HDMI 4K+"), 1920, 1080, 90.0);
                let input_b = devices.find_highest_at_res(devices.find_device("Game Capture HD60 X"), 1920, 1080, 90.0);
                
                let mut devs = Vec::new();
                devs.extend(input_a);
                devs.extend(input_b);
                cx.use_video_input(&devs);
            }
            _ => ()
        }
        self.ui.handle_widget_event(cx, event);
    }
    
}