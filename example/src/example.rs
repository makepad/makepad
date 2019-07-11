/// This is Makepad, a work-in-progress livecoding IDE for 2D Design.
// This application is nearly 100% Wasm running on webGL. NO HTML AT ALL.
// The vision is to build a livecoding / design hybrid program,
// here procedural design and code are fused in one environment.
// If you have missed 'learnable programming' please check this out:
// http://worrydream.com/LearnableProgramming/
// Makepad aims to fulfill (some) of these ideas using a completely
// from-scratch renderstack built on the GPU and Rust/wasm.
// It will be like an IDE meets a vector designtool, and had offspring.
// Direct manipulation of the vectors modifies the code, the code modifies the vectors.
// And the result can be lasercut, embroidered or drawn using a plotter.
// This means our first goal is 2D path CNC with booleans (hence the CAD),
// and not dropshadowy-gradients.

// Find the repo and more explanation at github.com/makepad/makepad.
// We are developing the UI kit and code-editor as MIT, but the full stack
// will be a cloud/native app product in a few months.

// However to get to this amazing mixed-mode code editing-designtool,
// we first have to build an actually nice code editor (what you are looking at)
// And a vector stack with booleans (in progress)
// Up next will be full multiplatform support and more visual UI.
// All of the code is written in Rust, and it compiles to native and Wasm
// Its built on a novel immediate-mode UI architecture
// The styling is done with shaders written in Rust, transpiled to metal/glsl

// for now enjoy how smooth a full GPU editor can scroll (try select scrolling the edge)
// Also the tree fold-animates and the docking panel system works.
// Multi cursor/grid cursor also works with ctrl+click / ctrl+shift+click
// press alt or escape for animated codefolding outline view!

/*
Vision of makepad is to build 'learnable programming' for Rust.
Easy to learn, hard to master. Few if any barriers to high perf.

Reinventing UI as simple as possible, but not simpler.

Quick makepad UI demo - tree, codefold, dock

Future - live editing of programs, sliders, colorpickers

Web - marketing tool not real target for makepad itself but possible for user programs.

Shaders as first class citizen 

UI system design
*/
use render::*;
use widget::*;
pub mod capture_winmf;
pub use crate::capture_winmf::*;
pub mod gamepad_winxinput;
pub use crate::gamepad_winxinput::*;
pub mod mandelbrot;
pub use crate::mandelbrot::*;
use serde::*;

#[derive(Clone, Serialize, Deserialize)]
struct AppState {
    loc: MandelbrotLoc,
    color_offset: f32,
    palette: f32,
    blend_mode: f32,
    palette_scale: f32,
    outer_scale: f32,
    alpha_offset: f32,
    lock_mode: bool,
    lock_rotate: f64,
    lock_zoom: f32,
    lock_cycle: f32,
}

const MAX_PRESETS: usize = 10;

impl Default for AppState {
    fn default() -> AppState {
        AppState {
            loc: MandelbrotLoc {
                zoom: 1.0,
                rotate: 0.0,
                center_x: -1.5,
                center_y: 0.0
            },
            color_offset: 0.0,
            palette: 1.0,
            palette_scale: 1.0,
            outer_scale: 1.0,
            blend_mode: 0.0,
            alpha_offset: 1.0,
            lock_mode: false,
            lock_rotate: 0.0,
            lock_zoom: 1.0,
            lock_cycle: 0.0,
        }
    }
}

struct App {
    window_template: DesktopWindow,
    windows: Vec<DesktopWindow>,
    frame: f32,
    
    capture: Capture,
    gamepad: Gamepad,
    
    mandel: Mandelbrot,
    loc_history: Vec<MandelbrotLoc>,
    mandel_blit: Blit,
    
    state: AppState,
    
    presets: Vec<AppState>,
    current_preset: usize,
    preset_file_read: FileRead,
    
    blit: Blit,
    cam_blit: Blit,
    cam_pass: Pass,
    cam_view: View<ScrollBar>, // we have a root view otherwise is_overlay subviews can't attach topmost
    cam_buffer: Texture,
    
}

main_app!(App, "Example");

impl Style for App {
    fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        let mut presets = Vec::new();
        for _ in 0..MAX_PRESETS {presets.push(AppState::default())}
        let window_template = DesktopWindow {inner_over_chrome: true, ..DesktopWindow::style(cx)};
        Self {
            windows: vec![window_template.clone()],
            window_template: window_template,
            /*view: View {
                is_overlay: true,
                scroll_h: Some(ScrollBar::style(cx)),
                scroll_v: Some(ScrollBar::style(cx)),
                ..Style::style(cx)
            },*/
            frame: 0.,
            current_preset:0,
            capture: Capture::default(),
            gamepad: Gamepad::default(),
            
            mandel: Mandelbrot::style(cx),
            loc_history: Vec::new(),
            mandel_blit: Blit {
                shader: cx.add_shader(App::def_mandel_blit_shader(), "App.mandelblit"),
                ..Blit::style(cx)
            },
            presets: presets,
            preset_file_read: FileRead::default(),
            state: AppState::default(),
            cam_blit: Blit {
                shader: cx.add_shader(App::def_cam_blit_shader(), "App.camblit"),
                ..Blit::style(cx)
            },
            blit: Blit::style(cx),
            cam_pass: Pass::default(),
            cam_view: View::style(cx),
            cam_buffer: Texture::default(),
        }
    }
}

impl App {
    fn def_mandel_blit_shader() -> ShaderGen {
        Blit::def_blit_shader().compose(shader_ast!({
            let texcam: texture2d<Texture>;
            let time: float<Uniform>;
            
            let scale_delta: float<Uniform>;
            let offset_delta: vec2<Uniform>;
            let rotate_sin: float<Uniform>;
            let rotate_cos: float<Uniform>;
            let color_offset: float<Uniform>;
            let palette: float<Uniform>;
            let palette_scale: float<Uniform>;
            let outer_scale: float<Uniform>;
            let blend_mode: float<Uniform>;
            
            fn kaleido(uv: vec2, angle: float, base: float, spin: float) -> vec2 {
                let a = atan(uv.y, uv.x);
                if a<0. {a = PI + a}
                let d = length(uv);
                a = abs(fmod (a + spin, angle * 2.0) - angle);
                return vec2(abs(sin(a + base)) * d, abs(cos(a + base)) * d);
            }
            
            fn pixel() -> vec4 {
                let geom_cen = (geom.xy - vec2(0.5, 0.5)) * vec2(5.33, 3.0);
                
                if scale_delta > 100. {
                    return vec4(0., 0., 0., 1.);
                }
                let geom_rot = vec2(geom_cen.x * rotate_cos - geom_cen.y * rotate_sin, geom_cen.y * rotate_cos + geom_cen.x * rotate_sin) / vec2(5.33, 3.0);
                let comp_texpos1 = geom_rot * scale_delta + vec2(0.5, 0.5) + offset_delta;
                
                let fr1 = sample2d(texturez, comp_texpos1).rg; //kaleido(geom.xy-vec2(0.5,0.5), 3.14/8., time, 2.*time)).rg;
                
                let cam = sample2d(texcam, geom.xy).xyz;
                //let fr2 = sample2d(texturez, comp_texpos2).rg;
                //let comp_texpos2 = (vec2(1.0 - geom.x, geom.y) - vec2(0.5, 0.5)) * scale_delta + vec2(0.5, 0.5) + offset_delta;
                //kaleido(geom.xy-vec2(0.5,0.5), 3.14/8., time, 2.*time));
                let center_blend = 0.0;
                
                let dx = sin(2. * atan(dfdx(fr1.y), dfdy(fr1.y)) + time * 10.) * 0.5 + 0.5;
                let df = dfdx(fr1.x)+dfdy(fr1.x);
                let index = abs(color_offset + fr1.x * 8.0 - outer_scale * 0.05 * log(fr1.y)) * palette_scale;
                let fract = vec3(0., 0., 0.);
                
                if fr1.x > 1.0 {
                    center_blend = 1.0;
                    fract.xyz = vec3(0., 0., 0,);
                }
                else if palette == 0.0 {fract = df_iq_pal0(index)}
                else if palette == 1.0 {fract = df_iq_pal1(index)}
                else if palette == 2.0 {fract = df_iq_pal2(index)}
                else if palette == 3.0 {fract = df_iq_pal3(index)}
                else if palette == 4.0 {fract = df_iq_pal4(index)}
                else if palette == 5.0 {fract = df_iq_pal5(index)}
                else if palette == 6.0 {fract = df_iq_pal6(index)}
                else if palette == 7.0 {fract = df_iq_pal7(index)}
                
                if blend_mode == 0.0 {return vec4(fract.xyz, 1.0)}
                else if blend_mode == 1.0 {return vec4(cam.xyz, 1.0);}
                else if blend_mode == 2.0 {return vec4(mix(fract.xyz, cam.xyz, center_blend), 1.0);}
                else if blend_mode == 3.0 {return vec4(fract.xyz * cam.xyz, 1.0);}
                else if blend_mode == 4.0 {return vec4(mix(fract.xyz, cam.xyz, smoothstep(0.5, 1.0, max(max(cam.x, cam.y), cam.z))), 1.0);}
                else if blend_mode == 5.0 {return vec4(mix(fract.xyz, cam.xyz, smoothstep(0.5, 1.0, max(max(fract.x, fract.y), fract.z))), 1.0);}
                else if blend_mode == 6.0 {return vec4(mix(fract.xyz, df_iq_pal5(cam.y), smoothstep(0.5, 1.0, max(max(fract.x, fract.y), fract.z))), 1.0);}
                else if blend_mode == 7.0 {return vec4(mix(
                    fract.xyz,
                    df_iq_pal5(sample2d(texcam, kaleido(geom.xy - vec2(0.5, 0.5), 3.14 / 8., 0., 0.)).y),
                    smoothstep(0.5, 1.0, max(max(fract.x, fract.y), fract.z))
                ), 1.0);}
                return vec4(cam.xyz, 1.0);
            }
        }))
    }
    
    fn def_cam_blit_shader() -> ShaderGen {
        Blit::def_blit_shader().compose(shader_ast!({
            let alpha_offset: float<Uniform>;
            
            fn sample_camera_depth(dpix: vec2) -> vec4 {
                let cam1 = sample2d(texturez, geom.xy + dpix * vec2(0., 0.));
                let cam2 = sample2d(texturez, geom.xy + dpix * vec2(1., 0.));
                let cam3 = sample2d(texturez, geom.xy + dpix * vec2(0., 1.));
                let cam4 = sample2d(texturez, geom.xy + dpix * vec2(1., 1.));
                
                let cam5 = sample2d(texturez, geom.xy + dpix * vec2(2., 0.));
                let cam6 = sample2d(texturez, geom.xy + dpix * vec2(3., 0.));
                let cam7 = sample2d(texturez, geom.xy + dpix * vec2(2., 1.));
                let cam8 = sample2d(texturez, geom.xy + dpix * vec2(3., 1.));
                
                let cam9 = sample2d(texturez, geom.xy + dpix * vec2(0., 2.));
                let cam10 = sample2d(texturez, geom.xy + dpix * vec2(1., 3.));
                let cam11 = sample2d(texturez, geom.xy + dpix * vec2(0., 2.));
                let cam12 = sample2d(texturez, geom.xy + dpix * vec2(1., 3.));
                
                let cam13 = sample2d(texturez, geom.xy + dpix * vec2(2., 2.));
                let cam14 = sample2d(texturez, geom.xy + dpix * vec2(3., 2.));
                let cam15 = sample2d(texturez, geom.xy + dpix * vec2(2., 3.));
                let cam16 = sample2d(texturez, geom.xy + dpix * vec2(3., 3.));
                
                let cama = cam1.y + cam2.y + cam3.y + cam4.y;
                let camb = cam5.y + cam6.y + cam7.y + cam8.y;
                let camc = cam9.y + cam10.y + cam11.y + cam12.y;
                let camd = cam13.y + cam14.y + cam15.y + cam16.y;
                
                let delta = smoothstep(0.35, 0.5, abs(cama - camb) + abs(cama - camc) + abs(cama - camd) + abs(camb - camd) + abs(camc - camd));
                return vec4(cam1.xyz, delta);
            }
            
            fn pixel() -> vec4 {
                //return color("pink");
                let cam = sample_camera_depth(vec2(1.0 / 3840.0, 1.0 / 2160.0));
                let alpha = min(cam.w + alpha_offset, 1.0);
                return vec4(cam.xyz * alpha, alpha);
            }
        }))
    }
    
    
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        
        match event {
            Event::Construct => {
                self.capture.init(cx, 0, CaptureFormat::NV12, 3840, 2160, 30.0);
                self.mandel.init(cx);
                self.gamepad.init(0, 0.1);
                self.preset_file_read = cx.file_read("mandelbrot_presets.json");
            },
            Event::FileRead(fr) => if let Some(utf8_data) = self.preset_file_read.resolve_utf8(fr) {
                if let Ok(utf8_data) = utf8_data {
                    if let Ok(presets) = serde_json::from_str(&utf8_data) {
                        self.presets = presets;
                    }
                }
            },
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::F1 => {
                    if self.windows[0].window.is_fullscreen(cx) {self.windows[0].window.restore_window(cx);}
                    else {
                        self.windows[0].window.maximize_window(cx);
                    }
                },
                KeyCode::F2 => {
                    while self.windows.len() <= 1 {self.windows.push(self.window_template.clone());}
                    if self.windows[1].window.is_fullscreen(cx) {self.windows[1].window.restore_window(cx);}
                    else {
                        self.windows[1].window.set_position(cx, Vec2 {x: 1920. + 100., y: 0.});
                        self.windows[1].window.maximize_window(cx);
                    }
                },
                KeyCode::F3 => {
                    while self.windows.len() <= 2 {self.windows.push(self.window_template.clone());} 
                    if self.windows[2].window.is_fullscreen(cx) {self.windows[2].window.restore_window(cx);}
                    else {
                        self.windows[2].window.set_position(cx, Vec2 {x: 1920. * 2. + 100., y: 0.});
                        self.windows[2].window.maximize_window(cx);
                    }
                },
                KeyCode::F4 => { // oh noes. my really crappy shader code is hitting a limit. only 3 4k60 windows on a 1080ti!
                     while self.windows.len() <= 3 {self.windows.push(self.window_template.clone());}
                    if self.windows[3].window.is_fullscreen(cx) {self.windows[3].window.restore_window(cx);}
                    else {
                        self.windows[3].window.maximize_window(cx);
                    }
                },
                _ => ()
            }
            _ => ()
        }
        
        for i in 0..self.windows.len() {
            self.windows[i].handle_desktop_window(cx, event);
        }
        
        if let CaptureEvent::NewFrame = self.capture.handle_signal(cx, event) {
        }
        
        if self.mandel.handle_signal(cx, event) {
        }
    }
    
    fn map_input(val:f32, dead_zone:f32, exp:f32)->f32{
         if val > dead_zone {
           return ((val- dead_zone) / (1.-dead_zone)).powf(exp); //self.gamepad.right_trigger;
        }
        else if val < -dead_zone {
           return - ((-val- dead_zone) / (1.-dead_zone)).powf(exp); //self.gamepad.right_trigger;
        }
        0.0
    }
    
    fn do_gamepad_interaction(&mut self, cx: &mut Cx) {
        self.gamepad.poll();
        
        // preset mgmt
        let mut preset_handled = false;
        if self.gamepad.buttons & GamepadButtonBack>0 || self.gamepad.buttons & GamepadButtonStart>0 {
            preset_handled = true;
            
            // add 
            if self.gamepad.buttons & GamepadButtonBack>0 && self.gamepad.buttons & GamepadButtonStart>0 && 
                self.gamepad.buttons_down_edge & GamepadButtonA > 0{
                self.current_preset = self.presets.len(); 
                self.presets.push(self.state.clone());
                let json = serde_json::to_string(&self.presets).unwrap();
                cx.file_write("mandelbrot_presets.json", json.as_bytes());
            }
            // remove
            else if self.gamepad.buttons & GamepadButtonBack>0 && self.gamepad.buttons & GamepadButtonStart>0 && 
                self.gamepad.buttons_down_edge & GamepadButtonB > 0{
                self.presets.remove(self.current_preset);
                if self.current_preset>0{
                    self.current_preset -= 1;
                }
                if self.presets.len() == 0{
                    self.presets.push(self.state.clone());
                }
                self.state = self.presets[self.current_preset].clone();
                let json = serde_json::to_string(&self.presets).unwrap();
                cx.file_write("mandelbrot_presets.json", json.as_bytes());
            }
            // next
            else if self.gamepad.buttons_down_edge & GamepadButtonB > 0{
                self.current_preset += 1;
                if self.current_preset >= self.presets.len(){
                    self.current_preset = 0;
                }
                self.state = self.presets[self.current_preset].clone();
            }
            //prev
            else if self.gamepad.buttons_down_edge & GamepadButtonA > 0{
                if self.current_preset >0{
                    self.current_preset -= 1;
                }
                else{
                    self.current_preset = self.presets.len() - 1;
                }
                self.state = self.presets[self.current_preset].clone();
            }
        }
        
        // update the mandelbrot location
        let state = &mut self.state;
        
        let last_lock_mode = state.lock_mode;
        if !preset_handled {
            if self.gamepad.buttons_down_edge & GamepadButtonRightShoulder > 0 {
                state.lock_mode = true;
            }
            if self.gamepad.buttons_down_edge & GamepadButtonLeftShoulder > 0 {
                state.lock_mode = false;
            }
            if self.gamepad.buttons_down_edge & GamepadButtonLeftThumb > 0 {
                state.loc.zoom = 1.0;
            }
        }
        if state.lock_mode == false {
            state.lock_rotate = 0.03 * Self::map_input(self.gamepad.left_trigger - self.gamepad.right_trigger, 0.0, 3.0) as f64;
            state.lock_zoom = 0.;
            state.lock_zoom = 0.06 * Self::map_input(self.gamepad.left_thumb.y, 0.4, 2.0);
            state.lock_cycle = 0.03 * Self::map_input(self.gamepad.left_thumb.x, 0.4, 3.0);
        }
        let mut zoom = state.lock_zoom;
        let mut rotate = state.lock_rotate;
        let mut cycle = state.lock_cycle;
        if state.lock_mode == true {
            rotate += 0.03 * Self::map_input(self.gamepad.left_trigger - self.gamepad.right_trigger, 0.0, 3.0) as f64;
            zoom += 0.06 *  Self::map_input(self.gamepad.left_thumb.y, 0.4, 2.0);
            cycle += 0.03 * Self::map_input(self.gamepad.left_thumb.x, 0.4, 3.0);
        }
        if last_lock_mode == true && self.gamepad.buttons_down_edge & GamepadButtonRightShoulder > 0 {
            state.lock_zoom = zoom;
            state.lock_rotate = rotate;
            state.lock_cycle = cycle;
        }
        state.loc.rotate += rotate;
        state.color_offset += cycle;
        //state.color_offset -= 0.02 * self.gamepad.left_trigger;
        let dx = 0.1 * Self::map_input(self.gamepad.right_thumb.x, 0., 2.0) as f64 * state.loc.zoom;
        let dy = -0.1 * Self::map_input(self.gamepad.right_thumb.y, 0., 2.0) as f64 * state.loc.zoom;
        
        let dx_rot = dx * state.loc.rotate.cos() - dy * state.loc.rotate.sin();
        let dy_rot = dy * state.loc.rotate.cos() + dx * state.loc.rotate.sin();
        
        state.loc.center_x += dx_rot;
        state.loc.center_y += dy_rot;
        state.loc.zoom = state.loc.zoom * (1.0 - zoom as f64);
        
        if state.loc.zoom < 0.00000000000002 {
            state.loc.zoom = 0.00000000000002;
            if state.lock_mode && state.lock_zoom != 0.0 {
                state.loc.zoom = 1.0;
            }
        }
        else if state.loc.zoom > 1.0 {
            state.loc.zoom = 1.0;
        }
        
        let mut pred_loc = state.loc.clone();
        for _ in 0..10 {
            pred_loc.center_x += dx_rot;
            pred_loc.center_y += dy_rot;
            if zoom<0.0 {
                pred_loc.zoom = pred_loc.zoom * (1.0 - zoom as f64);
           };
        }
        
        // we only send a new loc when we move more than 10% or zoom more than 25%
        if self.loc_history.len()>0{
            let last_loc = &self.loc_history[self.loc_history.len()-1];
            if 
            (last_loc.rotate-pred_loc.rotate).abs()>0.05 
            || (last_loc.center_x != pred_loc.center_x || last_loc.center_y != pred_loc.center_y) &&  dx == 0.0 && dy ==0.0
            || (last_loc.center_x-pred_loc.center_x).abs()>0.1*pred_loc.zoom 
            || (last_loc.center_y-pred_loc.center_y).abs()>0.1*pred_loc.zoom
            || last_loc.zoom > pred_loc.zoom && last_loc.zoom > pred_loc.zoom*1.1
            || last_loc.zoom < pred_loc.zoom && last_loc.zoom < pred_loc.zoom*0.9{
                self.mandel.send_new_loc(self.loc_history.len(), pred_loc.clone(), zoom.abs()>0.01);
                self.loc_history.push(pred_loc.clone());
            }
            
        }
        else{
            self.mandel.send_new_loc(self.loc_history.len(), pred_loc.clone(), true);
            self.loc_history.push(pred_loc.clone());
        }
        
        if !preset_handled {
            if self.gamepad.buttons_down_edge & GamepadButtonA > 0 {
                if state.palette > 0.0 {state.palette -= 1.0;}
                else {state.palette = 0.0;}
            }
            if self.gamepad.buttons_down_edge & GamepadButtonB > 0 {
                if state.palette < 7.0 {state.palette += 1.0;}
                else {state.palette = 7.0;}
            }
            if self.gamepad.buttons_down_edge & GamepadButtonDpadUp > 0 {
                if state.palette_scale > 0.5 {state.palette_scale -= 0.50;}
                else {state.palette_scale = 0.5;}
            }
            if self.gamepad.buttons_down_edge & GamepadButtonDpadDown > 0 {
                if state.palette_scale < 10.0 {state.palette_scale += 0.5;}
                else {state.palette_scale = 10.0;}
            }
            if self.gamepad.buttons_down_edge & GamepadButtonDpadLeft > 0 {
                if state.outer_scale > 0.02 {state.outer_scale -= 0.5;}
                else {state.outer_scale = 0.02;}
            }
            if self.gamepad.buttons_down_edge & GamepadButtonDpadRight > 0 {
                if state.outer_scale < 5.0 {state.outer_scale += 0.5;}
                else {state.outer_scale = 5.0;}
            }
            // camera control
            if self.gamepad.buttons_down_edge & GamepadButtonRightThumb > 0 {
                state.alpha_offset = 1.0 - state.alpha_offset;
            }
            if self.gamepad.buttons_down_edge & GamepadButtonX > 0 {
                if state.blend_mode > 0.0 {state.blend_mode -= 1.0;}
                else {state.blend_mode = 0.0;}
            }
            if self.gamepad.buttons_down_edge & GamepadButtonY > 0 {
                if state.blend_mode < 7.0 {state.blend_mode += 1.0;}
                else {state.blend_mode = 7.0;}
            }
        }
    }
    
    
    fn draw_window(&mut self, window_id: usize, cx: &mut Cx) {
        
        if let Err(_) = self.windows[window_id].begin_desktop_window(cx) {
            return
        }
        let state = &self.state;
        // do our cam buffer pass
        if self.capture.initialized {
            self.cam_pass.begin_pass(cx);
            self.cam_pass.add_color_texture(cx, &mut self.cam_buffer, None);
            let _ = self.cam_view.begin_view(cx, Layout::default());
            let inst = self.cam_blit.draw_blit_walk(cx, &self.capture.texture, Bounds::Fill, Bounds::Fill, Margin::zero());
            inst.push_uniform_float(cx, state.alpha_offset);
            self.cam_view.redraw_view_area(cx);
            self.cam_view.end_view(cx);
            self.cam_pass.end_pass(cx);
        }
        
        
        // draw our mandelbrot
        if window_id == 3 {
            self.blit.draw_blit_walk(cx, &self.cam_buffer, Bounds::Fill, Bounds::Fill, Margin::zero());
        }
        else {
            let inst = self.mandel_blit.draw_blit_walk(cx, &self.mandel.texture, Bounds::Fill, Bounds::Fill, Margin::zero());
            if let Some(index) = cx.get_mapped_texture_user_data(&self.mandel.texture) {
                inst.push_uniform_texture_2d(cx, &self.cam_buffer);
                inst.push_uniform_float(cx, self.frame);
                inst.push_uniform_float(cx, (state.loc.zoom / self.loc_history[index].zoom) as f32);
                
                let dx = self.loc_history[index].center_x - state.loc.center_x;
                let dy = self.loc_history[index].center_y - state.loc.center_y;
                let rev_rotate = -state.loc.rotate + (-self.loc_history[index].rotate + state.loc.rotate);
                let dx_rot = dx * rev_rotate.cos() - dy * rev_rotate.sin();
                let dy_rot = dy * rev_rotate.cos() + dx * rev_rotate.sin();
                let screen_dx = -(dx_rot / self.loc_history[index].zoom) / 5.33;
                let screen_dy = -(dy_rot / self.loc_history[index].zoom) / 3.0;
                inst.push_uniform_vec2f(cx, screen_dx as f32, screen_dy as f32);
                
                inst.push_uniform_float(cx, (-self.loc_history[index].rotate + state.loc.rotate).sin() as f32);
                inst.push_uniform_float(cx, (-self.loc_history[index].rotate + state.loc.rotate).cos() as f32);
                inst.push_uniform_float(cx, state.color_offset);
                inst.push_uniform_float(cx, state.palette as f32);
                inst.push_uniform_float(cx, state.palette_scale as f32);
                inst.push_uniform_float(cx, state.outer_scale as f32);
                inst.push_uniform_float(cx, state.blend_mode as f32);
                cx.reset_turtle_walk();
            }
        }
        
        self.frame += 0.001;
        
        self.windows[window_id].inner_view.redraw_view_area(cx);
        
        self.windows[window_id].end_desktop_window(cx);
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        self.do_gamepad_interaction(cx);
        
        for i in 0..self.windows.len() {
            self.draw_window(i, cx);
        }
        
    }
}