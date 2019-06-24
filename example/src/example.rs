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

use render::*;
use widget::*;
pub mod capture_winmf;
pub use crate::capture_winmf::*;
pub mod gamepad_winxinput;
pub use crate::gamepad_winxinput::*;
pub mod mandelbrot;
pub use crate::mandelbrot::*;

struct App {
    window: DesktopWindow,
    view: View<ScrollBar>,
    frame: f32,

    capture: Capture,
    gamepad: Gamepad,

    mandel: Mandelbrot,
    current_loc: MandelbrotLoc,
    loc_history: Vec<MandelbrotLoc>,
    mandel_blit: Blit,

    color_offset: f32,
    palette: f32,
    palette_scale: f32,
    outer_scale: f32,
    alpha_offset: f32,

    cam_blit: Blit,
    cam_pass: Pass,
    cam_view: View<ScrollBar>, // we have a root view otherwise is_overlay subviews can't attach topmost
    cam_buffer: Texture,
    
}

main_app!(App, "Example");

impl Style for App {
    fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        Self {
            window: DesktopWindow::style(cx),
            view: View {
                is_overlay: true,
                scroll_h: Some(ScrollBar::style(cx)),
                scroll_v: Some(ScrollBar::style(cx)),
                ..Style::style(cx)
            },
            frame: 0.,

            capture: Capture::default(),
            gamepad: Gamepad::default(),

            mandel: Mandelbrot::style(cx),
            current_loc: MandelbrotLoc {
                zoom: 1.0,
                center_x: -1.5,
                center_y: 0.0
            },
            loc_history: Vec::new(),
            mandel_blit: Blit {
                shader: cx.add_shader(App::def_mandel_blit_shader(), "App.mandelblit"),
                ..Blit::style(cx)
            },

            color_offset: 0.0,
            palette: 1.0,
            palette_scale: 1.0,
            outer_scale: 1.0,
            alpha_offset: 0.0,
            cam_blit: Blit {
                shader: cx.add_shader(App::def_cam_blit_shader(), "App.camblit"),
                ..Blit::style(cx)
            },
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
            let color_offset: float<Uniform>;
            let palette: float<Uniform>;
            let palette_scale: float<Uniform>;
            let outer_scale: float<Uniform>;
            
            fn kaleido(uv: vec2, angle: float, base: float, spin: float) -> vec2 {
                let a = atan(uv.y, uv.x);
                if a<0. {a = PI + a}
                let d = length(uv);
                a = abs(fmod (a + spin, angle * 2.0) - angle);
                return vec2(abs(sin(a + base)) * d, abs(cos(a + base)) * d);
            }
            
            fn pixel() -> vec4 {
                let comp_texpos1 = (geom.xy - vec2(0.5, 0.5)) * scale_delta + vec2(0.5, 0.5) + offset_delta;
                let fr1 = sample2d(texturez, comp_texpos1).rg; //kaleido(geom.xy-vec2(0.5,0.5), 3.14/8., time, 2.*time)).rg;
                
                //let fr2 = sample2d(texturez, comp_texpos2).rg;
                //let comp_texpos2 = (vec2(1.0 - geom.x, geom.y) - vec2(0.5, 0.5)) * scale_delta + vec2(0.5, 0.5) + offset_delta;
                let cam = sample2d(texcam, geom.xy);
                //kaleido(geom.xy-vec2(0.5,0.5), 3.14/8., time, 2.*time));
                if fr1.x > 1.0 {
                    return vec4(cam.xyz, 1.0);
                }
                let dx = sin(2. * atan(dfdx(fr1.y), dfdy(fr1.y)) + time * 10.) * 0.5 + 0.5;
                let index = (color_offset + fr1.x * 8.0 - outer_scale * 0.05 * log(fr1.y)) * palette_scale;
                let fract = mix(df_iq_pal0(index), mix(
                    df_iq_pal1(index),
                    mix(df_iq_pal2(index), mix(df_iq_pal3(index), mix(
                        df_iq_pal4(index),
                        mix(df_iq_pal5(index), mix(
                            df_iq_pal6(index),
                            df_iq_pal7(index),
                            clamp(palette - 6.0, 0., 1.)
                        ), clamp(palette - 5.0, 0., 1.)),
                        clamp(palette - 4.0, 0., 1.)
                    ), clamp(palette - 3.0, 0., 1.)), clamp(palette - 2.0, 0., 1.)),
                    clamp(palette - 1.0, 0., 1.)
                ), clamp(palette, 0., 1.));
                
                return vec4(fract.xyz, 1.0);
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
                let alpha = min(cam.w + alpha_offset,1.0);
                return vec4(cam.xyz*alpha, alpha); 
            }
        }))
    }
    
    
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        
        if let Event::Construct = event {
            self.capture.init(cx, 0, CaptureFormat::NV12, 3840, 2160, 30.0);
            self.mandel.init(cx);
            self.gamepad.init(0, 0.1);
        }
        
        self.window.handle_desktop_window(cx, event);
        
        self.view.handle_scroll_bars(cx, event);
        
        if let CaptureEvent::NewFrame = self.capture.handle_signal(cx, event) {
            self.view.redraw_view_area(cx);
        }
        
        if self.mandel.handle_signal(cx, event) {
            self.view.redraw_view_area(cx);
        }
    }
    
    fn do_gamepad_interaction(&mut self, _cx: &mut Cx) {
        self.gamepad.poll();
        // update the mandelbrot location
        
        self.current_loc.center_x += 0.05 * self.gamepad.right_thumb.x as f64 * self.current_loc.zoom;
        self.current_loc.center_y -= 0.05 * self.gamepad.right_thumb.y as f64 * self.current_loc.zoom;
        self.current_loc.zoom = self.current_loc.zoom * (1.0 - 0.03 * self.gamepad.left_thumb.y as f64);
        let mut pred_loc = self.current_loc.clone();
        for _ in 0..10 {
            pred_loc.center_x += 0.05 * self.gamepad.right_thumb.x as f64 * pred_loc.zoom;
            pred_loc.center_y -= 0.05 * self.gamepad.right_thumb.y as f64 * pred_loc.zoom;
            if self.gamepad.left_thumb.y<0.0 {
                for _ in 0..4 {
                    pred_loc.zoom = pred_loc.zoom * (1.0 - 0.03 * self.gamepad.left_thumb.y as f64);
                }
            };
        }
        
        self.mandel.send_new_loc(self.loc_history.len(), pred_loc.clone());
        self.loc_history.push(pred_loc.clone());
        if self.gamepad.buttons & GamepadButtonLeftShoulder > 0 {
            if self.palette > 0.0 {self.palette -= 0.02;}
            else {self.palette = 0.0;}
        }
        if self.gamepad.buttons & GamepadButtonRightShoulder > 0 {
            if self.palette < 7.0 {self.palette += 0.02;}
            else {self.palette = 7.0;}
        }
        if self.gamepad.buttons_down_edge & GamepadButtonDpadUp > 0 {
            if self.palette_scale > 0.5 {self.palette_scale -= 0.50;}
            else {self.palette_scale = 0.5;}
        }
        if self.gamepad.buttons_down_edge & GamepadButtonDpadDown > 0 {
            if self.palette_scale < 10.0 {self.palette_scale += 0.5;}
            else {self.palette_scale = 10.0;}
        }
        if self.gamepad.buttons_down_edge & GamepadButtonDpadLeft > 0 {
            if self.outer_scale > 0.02 {self.outer_scale -= 0.5;}
            else {self.outer_scale = 0.02;}
        }
        if self.gamepad.buttons_down_edge & GamepadButtonDpadRight > 0 {
            if self.outer_scale < 5.0 {self.outer_scale += 0.5;}
            else {self.outer_scale = 5.0;}
        }
        if self.gamepad.buttons_down_edge & GamepadButtonY > 0 {
            self.alpha_offset = 1.0 - self.alpha_offset;
        }
        self.color_offset += 0.02 * self.gamepad.right_trigger;
        self.color_offset -= 0.02 * self.gamepad.left_trigger;
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        self.do_gamepad_interaction(cx);
        
        let _ = self.window.begin_desktop_window(cx);
        
        // do our cam buffer pass
        if self.capture.initialized{
            self.cam_pass.begin_pass(cx);
            self.cam_pass.add_color_texture(cx, &mut self.cam_buffer, None);
            let _ = self.cam_view.begin_view(cx, Layout::default());
            let inst = self.cam_blit.draw_blit_walk(cx, &self.capture.texture, Bounds::Fill, Bounds::Fill, Margin::zero());
            inst.push_uniform_float(cx, self.alpha_offset);
            self.cam_view.redraw_view_area(cx);
            self.cam_view.end_view(cx);
            self.cam_pass.end_pass(cx); 
        }

        // render our camera to a cumulation buffer / rtt animationloop
        let _ = self.view.begin_view(cx, Layout {
            abs_origin: Some(Vec2::zero()),
            padding: Padding {l: 0., t: 0., r: 0., b: 0.},
            ..Default::default()
        });
        
        // draw our mandelbrot
        let inst = self.mandel_blit.draw_blit_walk(cx, &self.mandel.texture, Bounds::Fill, Bounds::Fill, Margin::zero());
        if let Some(index) = cx.get_mapped_texture_user_data(&self.mandel.texture) {
            inst.push_uniform_texture_2d(cx, &self.cam_buffer);
            inst.push_uniform_float(cx, self.frame);
            inst.push_uniform_float(cx, (self.current_loc.zoom / self.loc_history[index].zoom) as f32);
            let screen_dx = ((self.loc_history[index].center_x - self.current_loc.center_x) / self.loc_history[index].zoom) / 4.0;
            let screen_dy = ((self.loc_history[index].center_y - self.current_loc.center_y) / self.loc_history[index].zoom) / 3.0;
            inst.push_uniform_vec2f(cx, -screen_dx as f32, -screen_dy as f32);
            inst.push_uniform_float(cx, self.color_offset);
            inst.push_uniform_float(cx, self.palette as f32);
            inst.push_uniform_float(cx, self.palette_scale as f32);
            inst.push_uniform_float(cx, self.outer_scale as f32);
            cx.reset_turtle_walk();
        }
        
        self.frame += 0.001;
        
        self.view.redraw_view_area(cx);
        self.view.end_view(cx);
        
        self.window.end_desktop_window(cx);
    }
}