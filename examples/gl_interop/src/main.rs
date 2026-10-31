//! GL interop: the app renders with raw GL into textures makepad displays. With the GLES HAL
//! backend (MAKEPAD_HAL_BACKEND=gl, or a Linux machine without Vulkan) `Cx::with_gl` runs on a
//! GL context of the HAL's share group, current on the app's thread:
//! - the left image is a makepad-owned render texture (`create_gl_render_texture`) the app
//!   clears every frame: four quadrants and a moving bar, through its own framebuffer;
//! - the right image is a texture the app made and owns (`adopt_gl_texture_2d`): a checkerboard.
//! The HAL samples them after the fence `with_gl` ends with. On the other backends GL is not
//! available and the status line says so.
//!
//! Rows: makepad samples a texture's row 0 as its top; GL calls row 0 the bottom of a framebuffer,
//! so the app's scissor rects count rows from the image's top here.
pub use makepad_widgets;

use makepad_widgets::makepad_platform::os::gl_sys as gl;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(600, 340)
                body +: {
                    flow: Down
                    padding: 16
                    spacing: 12
                    status := Label{ text: "starting" }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 16
                        rendered := Image{ width: 256 height: 256 }
                        adopted := Image{ width: 256 height: 256 }
                    }
                }
            }
        }
    }
}

const SIZE: usize = 256;

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    gl: Option<GlScene>,
    #[rust]
    next_frame: NextFrame,
}

/// The app's GL objects: the framebuffer it clears the render texture through.
struct GlScene {
    fbo: u32,
    _rendered: Texture,
    _adopted: Texture,
    start: f64,
}

impl App {
    fn start_gl(&mut self, cx: &mut Cx) {
        let (rendered, name) = cx.create_gl_render_texture(SIZE, SIZE);
        if name == 0 {
            self.ui.label(cx, ids!(status)).set_text(cx, "GL interop is not available on this GPU backend");
            return;
        }
        let made = cx.with_gl(|g| unsafe {
            let mut fbo = 0;
            (g.glGenFramebuffers)(1, &mut fbo);
            (g.glBindFramebuffer)(gl::FRAMEBUFFER, fbo);
            (g.glFramebufferTexture2D)(gl::FRAMEBUFFER, gl::COLOR_ATTACHMENT0, gl::TEXTURE_2D, name, 0);
            (g.glBindFramebuffer)(gl::FRAMEBUFFER, 0);
            // a checkerboard of 32-texel squares the app owns
            let pixels: Vec<u8> = (0..SIZE * SIZE).flat_map(|i| {
                let on = ((i % SIZE) / 32 + (i / SIZE) / 32) % 2 == 0;
                if on { [240, 240, 240, 255] } else { [40, 90, 200, 255] }
            }).collect();
            let mut own = 0;
            (g.glGenTextures)(1, &mut own);
            (g.glBindTexture)(gl::TEXTURE_2D, own);
            (g.glTexImage2D)(gl::TEXTURE_2D, 0, gl::RGBA as i32, SIZE as i32, SIZE as i32, 0, gl::RGBA, gl::UNSIGNED_BYTE, pixels.as_ptr() as _);
            (g.glBindTexture)(gl::TEXTURE_2D, 0);
            (fbo, own)
        });
        let Some((fbo, own)) = made else { return };
        let adopted = Texture::new(cx);
        if let Err(e) = adopted.adopt_gl_texture_2d(cx, own, SIZE, SIZE) {
            self.ui.label(cx, ids!(status)).set_text(cx, &e);
            return;
        }
        self.ui.image(cx, ids!(rendered)).set_texture(cx, Some(rendered.clone()));
        self.ui.image(cx, ids!(adopted)).set_texture(cx, Some(adopted.clone()));
        self.ui.label(cx, ids!(status)).set_text(cx, "GL interop: left rendered by the app each frame, right adopted from the app");
        self.gl = Some(GlScene { fbo, _rendered: rendered, _adopted: adopted, start: -1.0 });
        self.next_frame = cx.new_next_frame();
    }

    fn render(&mut self, cx: &mut Cx, time: f64) {
        let Some(scene) = &mut self.gl else { return };
        if scene.start < 0.0 { scene.start = time; }
        let t = (time - scene.start) as f32;
        let fbo = scene.fbo;
        cx.with_gl(|g| unsafe {
            (g.glBindFramebuffer)(gl::FRAMEBUFFER, fbo);
            (g.glViewport)(0, 0, SIZE as i32, SIZE as i32);
            (g.glEnable)(gl::SCISSOR_TEST);
            let half = (SIZE / 2) as i32;
            for (x, y, c) in [(0, 0, [1.0, 0.0, 0.0]), (half, 0, [0.0, 0.8, 0.0]), (0, half, [0.0, 0.0, 1.0]), (half, half, [1.0, 1.0, 0.0])] {
                (g.glScissor)(x, y, half, half);
                (g.glClearColor)(c[0], c[1], c[2], 1.0);
                (g.glClear)(gl::COLOR_BUFFER_BIT);
            }
            let bar = ((t * 0.5).fract() * (SIZE - 16) as f32) as i32;
            (g.glScissor)(bar, 0, 16, SIZE as i32);
            (g.glClearColor)(1.0, 1.0, 1.0, 1.0);
            (g.glClear)(gl::COLOR_BUFFER_BIT);
            (g.glDisable)(gl::SCISSOR_TEST);
            (g.glBindFramebuffer)(gl::FRAMEBUFFER, 0);
        });
        self.ui.redraw(cx);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.start_gl(cx);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                self.render(cx, ne.time);
                self.next_frame = cx.new_next_frame();
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
