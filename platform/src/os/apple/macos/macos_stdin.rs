use {
    std::{
        io,
        io::prelude::*,
        io::BufReader,
        path::Path,
        fs,
        process::Command,
    },
    crate::{
        makepad_live_id::*,
        makepad_math::*,
        makepad_micro_serde::*,
        makepad_live_compiler::LiveFileChange,
        event::Event,
        window::CxWindowPool,
        event::WindowGeom,
        texture::{Texture, TextureFormat},
        thread::SignalToUI,
        os::{
            url_session::{make_http_request},
            apple_sys::*,
            metal_xpc::{
                xpc_service_proxy,
                //xpc_service_proxy_poll_run_loop,
                fetch_xpc_service_texture,
            },
            metal::{MetalCx, DrawPassMode},
            cx_stdin::{HostToStdin, PresentableDraw, StdinToHost, Swapchain, PollTimer},
        },
        pass::{CxPassParent, PassClearColor, CxPassColorTexture},
        cx_api::CxOsOp,
        cx::Cx,
    }
};

impl Cx {
    
    pub (crate) fn stdin_send_draw_complete(presentable_draw: PresentableDraw) {
        let _ = io::stdout().write_all(StdinToHost::DrawCompleteAndFlip(presentable_draw).to_json().as_bytes());
    }
    
    pub (crate) fn stdin_handle_repaint(
        &mut self,
        metal_cx: &mut MetalCx,
        swapchain: &Swapchain<Option<Texture>>,
        time: f32,
    ) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for &pass_id in &passes_todo {
            self.passes[pass_id].set_time(time as f32);
            match self.passes[pass_id].parent.clone() {
                CxPassParent::Window(_) => {
                    let [current_image] = &swapchain.presentable_images;
                    if let Some(texture) = &current_image.image {
                        let window = &mut self.windows[CxWindowPool::id_zero()];
                        let pass = &mut self.passes[window.main_pass_id.unwrap()];
                        pass.color_textures = vec![CxPassColorTexture {
                            clear_color: PassClearColor::ClearWith(pass.clear_color),
                            texture: texture.clone(),
                        }];

                        let dpi_factor = self.passes[pass_id].dpi_factor.unwrap();
                        let pass_rect = self.get_pass_rect(pass_id, dpi_factor).unwrap();
                        let future_presentable_draw = PresentableDraw {
                            target_id: current_image.id,
                            width: (pass_rect.size.x * dpi_factor) as u32,
                            height: (pass_rect.size.y * dpi_factor) as u32,
                        };

                        // render to swapchain
                        self.draw_pass(pass_id, metal_cx, DrawPassMode::StdinMain(future_presentable_draw));

                        // and then wait for GPU, which calls stdin_send_draw_complete when its done
                    }
                }
                CxPassParent::Pass(_) => {
                    self.draw_pass(pass_id, metal_cx, DrawPassMode::Texture);
                },
                CxPassParent::None => {
                    self.draw_pass(pass_id, metal_cx, DrawPassMode::Texture);
                }
            }
        }
    }
    
    pub fn stdin_event_loop(&mut self, metal_cx: &mut MetalCx) {
        let service_proxy = xpc_service_proxy();

        let (json_msg_tx, json_msg_rx) = std::sync::mpsc::channel();
        {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(std::io::stdin().lock());
                let mut line = String::new();
                loop {
                    line.clear();
                    if let Ok(0) | Err(_) = reader.read_line(&mut line) {
                        break;
                    }

                    // alright lets put the line in a json parser
                    match HostToStdin::deserialize_json(&line) {
                        Ok(msg) => {
                            if json_msg_tx.send(msg).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            // we should output a log string
                            crate::error!("Cant parse stdin-JSON {} {:?}", line, err)
                        }
                    }
                }
            });
        }

        let _ = io::stdout().write_all(StdinToHost::ReadyToStart.to_json().as_bytes());

        let mut swapchain = None;

        self.call_event_handler(&Event::Startup);
        let (tx_fb, rx_fb) = std::sync::mpsc::channel::<RcObjcId> ();

        while let Ok(msg) =  json_msg_rx.recv(){
            match msg {
                HostToStdin::ReloadFile {file, contents} => {
                    // alright lets reload this file in our DSL system
                    let _ = self.live_file_change_sender.send(vec![LiveFileChange{
                        file_name: file,
                        content: contents
                    }]);
                }
                HostToStdin::KeyDown(e) => {
                    self.call_event_handler(&Event::KeyDown(e));
                }
                HostToStdin::KeyUp(e) => {
                    self.call_event_handler(&Event::KeyUp(e));
                }
                HostToStdin::TextInput(e) => {
                    self.call_event_handler(&Event::TextInput(e));
                }
                HostToStdin::MouseDown(e) => {
                    self.fingers.process_tap_count(
                        dvec2(e.x, e.y),
                        e.time
                    );
                    self.fingers.mouse_down(e.button);
                    
                    self.call_event_handler(&Event::MouseDown(e.into()));
                }
                HostToStdin::MouseMove(e) => {
                    self.call_event_handler(&Event::MouseMove(e.into()));
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                    self.fingers.switch_captures();
                }
                HostToStdin::MouseUp(e) => {
                    let button = e.button;
                    self.call_event_handler(&Event::MouseUp(e.into()));
                    self.fingers.mouse_up(button);
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                }
                HostToStdin::Scroll(e) => {
                    self.call_event_handler(&Event::Scroll(e.into()))
                }
                HostToStdin::WindowGeomChange { dpi_factor, inner_width, inner_height } => {
                    self.windows[CxWindowPool::id_zero()].window_geom = WindowGeom {
                        dpi_factor,
                        inner_size: dvec2(inner_width, inner_height),
                        ..Default::default()
                    };
                    self.redraw_all();
                }
                HostToStdin::Swapchain(new_swapchain) => {
                    swapchain = Some(new_swapchain.images_map(|_| None));

                    self.redraw_all();
                    self.stdin_handle_platform_ops(metal_cx);
                }
                HostToStdin::Tick {frame: _, buffer_id: _, time:_} => if let Some(swapchain) = &mut swapchain {
                    let [presentable_image] = &swapchain.presentable_images;
                    // lets fetch the framebuffers
                    if presentable_image.image.is_none() {
                        let tx_fb = tx_fb.clone();
                        fetch_xpc_service_texture(
                            service_proxy.as_id(),
                            presentable_image.id,
                            move |objcid| {let _ = tx_fb.send(objcid); },
                        ); 
                        // this is still pretty bad at 100ms if the service is still starting up
                        // we should 
                        if let Ok(fb) = rx_fb.recv_timeout(std::time::Duration::from_millis(100)) {
                            let format = TextureFormat::SharedBGRAu8 {
                                id: presentable_image.id,
                                width: swapchain.alloc_width as usize,
                                height: swapchain.alloc_height as usize,
                            };
                            let texture = Texture::new_with_format(self, format);
                            if self.textures[texture.texture_id()].update_from_shared_handle(
                                metal_cx,
                                fb.as_id(),
                            ) {
                                let [presentable_image] = &mut swapchain.presentable_images;
                                presentable_image.image = Some(texture);
                            }
                        }
                    }

                    // check signals
                    if SignalToUI::check_and_clear_ui_signal() {
                        self.handle_media_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                    for event in self.os.stdin_timers.get_dispatch() {
                        self.call_event_handler(&event);
                    }                    
                    if self.handle_live_edit() {
                        self.call_event_handler(&Event::LiveEdit);
                        self.redraw_all();
                    }
                    self.handle_networking_events();
                    self.stdin_handle_platform_ops(metal_cx);
                    // alright a tick.
                    // we should now run all the stuff.
                    if self.new_next_frames.len() != 0 {
                        self.call_next_frame_event(self.os.stdin_timers.time_now());
                    }
                    
                    if self.need_redrawing() {
                        self.call_draw_event();
                        self.mtl_compile_shaders(metal_cx);
                    }

                    let [presentable_image] = &swapchain.presentable_images;
                   // log!("TICKIN");
                    if presentable_image.image.is_some() {
                        self.stdin_handle_repaint(metal_cx, swapchain, self.os.stdin_timers.time_now() as f32);
                    }
                }
            }
        }
        // we should poll our runloop
        
        //xpc_service_proxy_poll_run_loop();
    }
    
    pub(crate)fn start_xpc_service(&mut self){
        
        pub fn mkdir(path: &Path) -> Result<(), String> {
            match fs::create_dir_all(path) { 
                Err(e) => {
                    Err(format!("mkdir {:?} failed {:?}", path, e))
                },
                Ok(()) => Ok(())
            }
        }
        
        pub fn shell(cwd: &Path, cmd: &str, args: &[&str]) -> Result<(), String> {
            let mut cmd_build = Command::new(cmd);
            
            cmd_build.args(args)
                .current_dir(cwd);
            
            let mut child = cmd_build.spawn().map_err( | e | format!("Error starting {} in dir {:?} - {:?}", cmd, cwd, e)) ?;
            
            let r = child.wait().map_err( | e | format!("Process {} in dir {:?} returned error {:?} ", cmd, cwd, e)) ?;
            if !r.success() {
                return Err(format!("Process {} in dir {:?} returned error exit code ", cmd, cwd));
            }
            Ok(())
        }
        
        pub fn write_text(path: &Path, data:&str) -> Result<(), String> {
            mkdir(path.parent().unwrap()) ?;
            match fs::File::create(path) { 
                Err(e) => {
                    Err(format!("file create {:?} failed {:?}", path, e))
                },
                Ok(mut f) =>{
                    f.write_all(data.as_bytes())
                        .map_err( | _e | format!("Cant write file {:?}", path))
                }
            }
        }
        
        pub fn get_exe_path()->String{
            let buf = [0u8;1024];
            let mut len = 1024u32;
            unsafe{_NSGetExecutablePath(buf.as_ptr() as *mut _, &mut len)};
            let end = buf.iter().position(|v| *v == 0).unwrap();
            std::str::from_utf8(&buf[0..end]).unwrap().to_string()
        }
        
        let exe_path = get_exe_path();
        
        let plist_body = format!(r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0">
            <dict>
                <key>Label</key>
                <string>dev.makepad.metalxpc</string>
                <key>Program</key>
                <string>{exe_path}</string>
                <key>ProgramArguments</key>
                <array>
                    <string>{exe_path}</string>
                    <string>--metal-xpc</string>
                </array>
                <key>MachServices</key>
                <dict>
                    <key>dev.makepad.metalxpc</key>
                    <true/>
                </dict>
            </dict>
            </plist>
            "#,
        );
        // lets write our service
        let home = std::env::var("HOME").unwrap();
        let plist_path = format!("{}/Library/LaunchAgents/dev.makepad.xpc.plist", home);
        let cwd = std::env::current_dir().unwrap();
        
        if let Ok(old) = fs::read_to_string(Path::new(&plist_path)){
            if old == plist_body{
                return
            }
            if std::env::args().find( | v | v == "--stdin-loop").is_some() {
                return
            }
        }
        shell(&cwd, "launchctl",&["unload",&plist_path]).unwrap();
        write_text(Path::new(&plist_path), &plist_body).unwrap();
        shell(&cwd, "launchctl",&["load",&plist_path]).unwrap();
    }
    
    
    fn stdin_handle_platform_ops(&mut self, _metal_cx: &MetalCx) {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(_window_id) => {
                    let window = &mut self.windows[CxWindowPool::id_zero()];
                    window.is_created = true;
                    // lets set up our render pass target
                    
                },
                CxOsOp::SetCursor(cursor) => {
                    let _ = io::stdout().write_all(StdinToHost::SetCursor(cursor).to_json().as_bytes());
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    self.os.stdin_timers.timers.insert(timer_id, PollTimer::new(interval, repeats));
                },
                CxOsOp::StopTimer(timer_id) => {
                    self.os.stdin_timers.timers.remove(&timer_id);
                },
                CxOsOp::HttpRequest {request_id, request} => {
                    make_http_request(request_id, request, self.os.network_response.sender.clone());
                },
                _ => ()
                /*
                CxOsOp::CloseWindow(_window_id) => {},
                CxOsOp::MinimizeWindow(_window_id) => {},
                CxOsOp::MaximizeWindow(_window_id) => {},
                CxOsOp::RestoreWindow(_window_id) => {},
                CxOsOp::FullscreenWindow(_window_id) => {},
                CxOsOp::NormalizeWindow(_window_id) => {}
                CxOsOp::SetTopmost(_window_id, _is_topmost) => {}
                CxOsOp::XrStartPresenting(_) => {},
                CxOsOp::XrStopPresenting(_) => {},
                CxOsOp::ShowTextIME(_area, _pos) => {},
                CxOsOp::HideTextIME => {},
                CxOsOp::SetCursor(_cursor) => {},
                CxOsOp::StartTimer {timer_id, interval, repeats} => {},
                CxOsOp::StopTimer(timer_id) => {},
                CxOsOp::StartDragging(dragged_item) => {}
                CxOsOp::UpdateMenu(menu) => {}*/
            }
        }
    }
    
}