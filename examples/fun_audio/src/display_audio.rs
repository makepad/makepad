
use {
    crate::{
        makepad_platform::*,
        makepad_math::complex::*,
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
        texture fft_texture: texture2d
        fn pixel(self) -> vec4 {
            let wave = sample2d(self.wave_texture, vec2(self.pos.x, 0.5));
            
            
            
            let fft = sample2d(
                self.fft_texture,
                vec2(mod(0.5 - self.pos.y * 0.5,0.25), fract(self.pos.x + self.shift_fft))
            );
            
            let right = abs(wave.y + wave.x / 256.0 - 0.5) * 2.0;
            let left = abs(wave.w + wave.z / 256.0 - 0.5) * 2.0;
            
            let right_fft = fft.y + fft.x / 256.0;
            let left_fft = fft.w + fft.z / 256.0;
            
            //return
            
            //let right = (wave.w * 256 + wave.z - 127);
            // lets draw a line in the center
            
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            if self.pos.y>0.5{
                sdf.clear(mix(#fff0, #ffff, left_fft ))
            }
            else{
                sdf.clear(mix(#fff0, #ffff, right_fft ))
            }
            //sdf.clear( vec4(Pal::iq1(min(left_fft,0.99)),1.0));
            //return mix(#f00,#0f0,left);;
            sdf.box( 
                0.,
                self.rect_size.y * 0.25 - self.rect_size.y * left,
                self.rect_size.x,
                2.0 * left * self.rect_size.y,
                2.0
            );
            sdf.fill(#fffa);
            
            sdf.box(
                0.,
                self.rect_size.y * 0.75 - self.rect_size.y * right,
                self.rect_size.x,
                2.0 * right * self.rect_size.y,
                2.0
            );
            sdf.fill(#fffa);
            
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
    shift_fft: f32
}

#[derive(Live, FrameComponent)]
#[live_register(frame_component!(DisplayAudio))]
pub struct DisplayAudio {
    view: View,
    walk: Walk,
    fft: DrawFFT,
    wave_texture: Texture,
    fft_texture: Texture,
    #[rust] fft_slot: usize,
    #[rust] fft_buffer: [Vec<ComplexF32>; 2],
    #[rust] fft_scratch: Vec<ComplexF32>,
    #[rust] data_offset: usize
}

#[derive(Clone, FrameAction)]
pub enum DisplayAudioAction {
    None
}
const WAVE_SIZE_X: usize = 1024;
const WAVE_SIZE_Y: usize = 1;
const FFT_SIZE_X: usize = 512;
const FFT_SIZE_Y: usize = 512;

impl LiveHook for DisplayAudio {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.wave_texture.set_desc(cx, TextureDesc {
            format: TextureFormat::ImageBGRA,
            width: Some(WAVE_SIZE_X),
            height: Some(WAVE_SIZE_Y),
            multisample: None
        });
        self.fft_texture.set_desc(cx, TextureDesc {
            format: TextureFormat::ImageBGRA,
            width: Some(FFT_SIZE_X),
            height: Some(FFT_SIZE_Y),
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
        self.fft.shift_fft = self.fft_slot as f32 / FFT_SIZE_Y as f32;
        self.fft.draw_vars.set_texture(0, &self.wave_texture);
        self.fft.draw_vars.set_texture(1, &self.fft_texture);
        self.fft.draw_walk(cx, Walk::fill());
        self.view.end(cx);
    }
    
    pub fn process_buffer(&mut self, cx: &mut Cx, audio: &AudioBuffer) {
        // alright we have a texture. lets write the audio somewhere
        let mut buf = Vec::new();
        self.wave_texture.swap_image_u32(cx, &mut buf);
        buf.resize(WAVE_SIZE_X * WAVE_SIZE_Y, 0);
        
        let frames = audio.frame_count();
        
        self.fft_buffer[0].resize(512, cf32(0.0, 0.0));
        self.fft_buffer[1].resize(512, cf32(0.0, 0.0));
        self.fft_scratch.resize(512, cf32(0.0, 0.0));
        
        let (left, right) = audio.stereo();
        
        let wave_off = (self.data_offset) & (WAVE_SIZE_X - 1);
        let fft_off = (self.data_offset) & (FFT_SIZE_X - 1);
        for i in 0..frames {
            let left_u16 = ((left[i] + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            let right_u16 = ((right[i] + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            self.fft_buffer[0][fft_off + i] = cf32(left[i], 0.0);
            self.fft_buffer[1][fft_off + i] = cf32(right[i], 0.0);
            buf[wave_off + i] = left_u16 << 16 | right_u16;
        }
        // every time we wrap around we should feed it to the FFT
        self.wave_texture.swap_image_u32(cx, &mut buf);
        self.view.redraw(cx);
        
        if fft_off + frames >= FFT_SIZE_X {
            let mut buf = Vec::new();
            self.fft_texture.swap_image_u32(cx, &mut buf);
            buf.resize(FFT_SIZE_X * FFT_SIZE_Y, 0);
            fft_f32_recursive_pow2_forward(&mut self.fft_buffer[0], &mut self.fft_scratch);
            fft_f32_recursive_pow2_forward(&mut self.fft_buffer[1], &mut self.fft_scratch);
            // lets write fft_buffer[0] to the texture
            for i in 0..FFT_SIZE_X {
                let left = self.fft_buffer[0][i].magnitude();
                let right = self.fft_buffer[0][i].magnitude();
                let left_u16 = (left * 10000.0).max(0.0).min(65535.0) as u32;
                let right_u16 = (right * 10000.0).max(0.0).min(65535.0) as u32;
                buf[self.fft_slot * FFT_SIZE_X + i] = left_u16 << 16 | right_u16;
            }
            self.fft_slot = (self.fft_slot + 1) & (FFT_SIZE_Y - 1);
            self.fft_texture.swap_image_u32(cx, &mut buf);
        }
        self.data_offset += frames;
        if self.data_offset >= WAVE_SIZE_X * FFT_SIZE_X {
            self.data_offset = 0;
        }
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

