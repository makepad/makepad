use {
    crate::{
        makepad_micro_serde::{SerBin, DeBin, DeBinErr},
        makepad_draw::makepad_image_formats,
        makepad_widgets::*,
        makepad_platform::thread::*,
        makepad_platform::video::*,
    },
    std::io::Write,
    std::sync::{Arc, Mutex},
    std::thread,
    std::net::{UdpSocket,TcpListener, TcpStream, Shutdown},
    std::time::{self, Duration},
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
    
    App = {{App}} {
        window: {ui: {inner_view = {
            show_bg:true 
            draw_bg:{color:#00f}
            video_input1 = <VideoFrame> {
            }
        }}}
        
    }
}
app_main!(App);


#[derive(Live, LiveHook)]
#[live_design_with{
    crate::makepad_audio_widgets::live_design(cx);
}]      
pub struct App {
    window: DesktopWindow,
    video_input1: Texture,
    #[rust] send_video_buffer: Arc<Mutex<Option<VideoBuffer>>>,
    #[rust] video_recv: ToUIReceiver<VideoBuffer>,
}

#[derive(SerBin, DeBin)]
enum VideoSenderWire {
    Image {buffer: Vec<u8>},
}

pub fn write_bytes_to_tcp_stream_no_error(tcp_stream: &mut TcpStream, bytes: &[u8])->bool {
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &bytes[(bytes_total - bytes_left)..bytes_total];
        if let Ok(bytes_written) = tcp_stream.write(buf) {
            if bytes_written == 0 {
                return true
            }
            bytes_left -= bytes_written;
        }
        else {
            return true
        }
    }
    false
}

impl AppMain for App{

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Signal => {
                if let Ok(mut vfb) = self.video_recv.try_recv_flush() {
                    self.video_input1.set_desc(cx, TextureDesc {
                        format: TextureFormat::ImageBGRA,
                        width: Some(vfb.format.width),
                        height: Some(vfb.format.height)
                    });
                    let image_size = [vfb.format.width as f32, vfb.format.height as f32];
                    let mut is_rgb = [0.0];
                    if let Some(buf) = vfb.as_vec_u32() {
                        self.video_input1.swap_image_u32(cx, buf);
                    }
                    else if let Some(buf) = vfb.as_vec_u8() { // lets decode a jpeg for the fun of it
                        
                        match makepad_image_formats::jpeg::decode(buf) {
                            Ok(mut data)=>{
                                is_rgb = [1.0];
                                self.video_input1.swap_image_u32(cx,&mut data.data);
                            }
                            Err(e)=>{ 
                                log!("JPEG DECODE ERROR {}", e);
                            }
                        }
                        *self.send_video_buffer.lock().unwrap() = Some(vfb);
                    }
                    
                    for v in [
                        self.window.ui.get_frame(id!(video_input1)),
                    ] {
                        v.set_texture(0, &self.video_input1);
                        v.set_uniform(cx, id!(image_size), &image_size);
                        v.set_uniform(cx, id!(is_rgb), &is_rgb);
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
            Event::VideoInputs(devices) => {
                //log!("Got devices! {:?}", devices);
                cx.use_video_input(&devices.find_format(0, 640, 480, VideoPixelFormat::MJPEG));
            }
            _ => ()
        }
        
        self.window.handle_event(cx, event);
    }    
}

impl App {
  
    pub fn start_inputs(&mut self, cx: &mut Cx) {
        let video_sender = self.video_recv.sender();
        cx.video_input(0, move | img | {
            let _ = video_sender.send(img.to_buffer());
            
        })
    }
    
    pub fn start_network_stack(&mut self, _cx: &mut Cx) {
        let client_uid = LiveId::from_str(&format!("{:?}", std::time::SystemTime::now())).unwrap().0;

        let write_discovery = UdpSocket::bind("0.0.0.0:42531").unwrap();
        write_discovery.set_read_timeout(Some(Duration::new(0, 1))).unwrap();
        write_discovery.set_broadcast(true).unwrap(); 
        
        std::thread::spawn(move || {
            let dummy = client_uid.to_be_bytes();
            loop {   
                let _ = write_discovery.send_to(&dummy, "255.255.255.255:42531");
                thread::sleep(time::Duration::from_secs(1));
            }
        });
        
        let listener = if let Ok(listener) = TcpListener::bind("0.0.0.0:42532") {
            listener
        } else {
            log!("Cannot bind tcp port");
            return 
        };
        
        let send_video_buffer = self.send_video_buffer.clone();
        std::thread::spawn(move || {
            for tcp_stream in listener.incoming() {
                let mut tcp_stream = if let Ok(tcp_stream) = tcp_stream {
                    tcp_stream
                }
                else {
                    println!("Incoming stream failure");
                    continue
                };
                let send_video_buffer = send_video_buffer.clone();
                std::thread::spawn(move || {
                    loop{
                        // lock it, take buffer out
                        if let Some(buffer) = send_video_buffer.lock().unwrap().take(){
                            // lets send over the wire
                            let buffer = buffer.into_vec_u8().unwrap();
                            let len = (buffer.len() as u32).to_be_bytes();
                            if write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &len){
                                break;
                            };
                            if write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &buffer){
                                break;
                            };
                        }
                        thread::sleep(time::Duration::from_millis(10));
                    }
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                });
            } 
        });
    }
    
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).is_redrawing() {
            self.window.end(cx);
        }
    }
}