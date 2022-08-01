
use {
    crate::{
        makepad_platform::*,
        makepad_math::complex::fft_f32_recursive_pow2_forward,
        makepad_component::*,
        makepad_platform::audio::*,
        makepad_component::imgui::*
    }
};

live_register!{
    import makepad_platform::shader::std::*;
    import makepad_component::theme::*;
    
    DrawFFT: {{DrawFFT}} {
        texture wave_texture: texture2d
        fn pixel(self) -> vec4 {
            let wave = sample2d(self.wave_texture, vec2(self.pos.x, 1.0 / 2.0));
            let left = wave.y + wave.x / 256.0 - 0.5;
            //let right = (wave.w * 256 + wave.z - 127);
            // lets draw a line in the center
            let fac = abs(left)*2.;
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.clear(#0000);
            //return mix(#f00,#0f0,left);;
            sdf.box(
                0.,
                self.rect_size.y * 0.5 - self.rect_size.y * fac,
                self.rect_size.x,
                2.0*fac * self.rect_size.y,
                2.0
            );
            sdf.fill(#fff);
            return sdf.result
        }
    }
    
    DisplayAudio: {{DisplayAudio}} {
        walk: {
            width: Size::Fit,
            height: Size::Fit
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawFFT {
    draw_super: DrawQuad,
}

#[derive(Live, FrameComponent)]
#[live_register(frame_component!(DisplayAudio))]
pub struct DisplayAudio {
    view: View,
    walk: Walk,
    fft: DrawFFT,
    wave_texture: Texture,
    #[rust] data_offset: usize
}

#[derive(Clone, FrameAction)]
pub enum DisplayAudioAction {
    None
}
const BUFFER_SIZE_X: usize = 1024;
const BUFFER_SIZE_Y: usize = 2;
impl LiveHook for DisplayAudio {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.wave_texture.set_desc(cx, TextureDesc {
            format: TextureFormat::ImageBGRA,
            width: Some(BUFFER_SIZE_X),
            height: Some(BUFFER_SIZE_Y),
            multisample: None
        });
    }
}

impl DisplayAudio {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        // alright lets draw em fuckers
        if self.view.begin(cx, walk, Layout::default()).not_redrawing() {
            return
        };
        self.fft.draw_vars.set_texture(0, &self.wave_texture);
        self.fft.draw_walk(cx, Walk::fill());
        self.view.end(cx);
    }
    
    pub fn process_buffer(&mut self, cx: &mut Cx, audio: &AudioBuffer) {
        // alright we have a texture. lets write the audio somewhere
        let mut buf = Vec::new();
        self.wave_texture.swap_image_u32(cx, &mut buf);
        buf.resize(BUFFER_SIZE_X * BUFFER_SIZE_Y, 0);
        let frames = audio.frame_count();
        let (left, right) = audio.stereo();
        
        for i in 0..frames {
            let off = (i + self.data_offset) & (BUFFER_SIZE_X - 1);
            // letspack left and right
            let left = ((left[i] + 0.5)*65535.0).max(0.0).min(65535.0) as u32;
            //let left = ((left[i] + 127.0) * 256.0).max(0.0).min(65535.0) as u32;
            //let right = ((right[i] + 127.0) * 256.0).max(0.0).min(65535.0) as u32;
            buf[off] = left;
            //buf[off] = (left << 16) | right;
        }
        
        self.data_offset = (self.data_offset + frames) & (BUFFER_SIZE_X - 1);
        self.wave_texture.swap_image_u32(cx, &mut buf);
        self.view.redraw(cx);
    }
    
    pub fn handle_event(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, DisplayAudioAction),
    ) {
    }
}

// ImGUI convenience API for Piano

pub struct DisplayAudioImGUI(ImGUIRef);

impl DisplayAudioImGUI {
    pub fn process_buffer(&self, cx: &mut Cx, buffer: &AudioBuffer) {
        if let Some(mut inner) = self.inner() {
            inner.process_buffer(cx, buffer);
        }
    }
    
    pub fn inner(&self) -> Option<std::cell::RefMut<'_, DisplayAudio >> {
        self.0.inner()
    }
}

pub trait DisplayAudioImGUIExt {
    fn display_audio(&mut self, path: &[LiveId]) -> DisplayAudioImGUI;
}

impl<'a> DisplayAudioImGUIExt for ImGUIRun<'a> {
    fn display_audio(&mut self, path: &[LiveId]) -> DisplayAudioImGUI {
        let mut frame = self.imgui.frame();
        DisplayAudioImGUI(self.safe_ref::<DisplayAudio>(frame.component_by_path(path)))
    }
}

