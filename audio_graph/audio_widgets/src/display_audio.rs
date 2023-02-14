
use {
    crate::{
        makepad_draw::*,
        makepad_widgets::*,
        makepad_platform::audio::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawWave = {{DrawWave}} {
        texture wave_texture: texture2d
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            let step = 0.0;
            for i in 0..4{
                let wave = sample2d(self.wave_texture, vec2(self.pos.x, step));
                let right = (wave.y + wave.z / 256.0 - 0.5) * 3.0;
                let left = (wave.w + wave.x / 256.0 - 0.5) * 3.0;
                let audio = (left + right)-0.01;
                let half = self.rect_size.y*0.5;
                let scale = half * 2.0;
                sdf.hline(half + audio * scale, abs(audio) * scale);
                let color = Pal::iq1(0.35+2.0*step)*0.8;
                sdf.fill_premul(vec4(color,0.0))
                step += 1.0/16.0;
            }
            return sdf.result
        }
    }
    
    DisplayAudio = {{DisplayAudio}} {
        walk: {
            width: Fill,
            height: Fill
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawWave {
    draw_super: DrawQuad,
}

#[derive(Live, Widget)]
#[live_design_fn(widget_factory!(DisplayAudio))]
pub struct DisplayAudio {
    walk: Walk,
    draw_wave: DrawWave,
    wave_texture: Texture,
    #[rust] data_offset: [usize; 32],
    #[rust([true;32])] active: [bool; 32],
}

#[derive(Clone, WidgetAction)]
pub enum DisplayAudioAction {
    None
}
const WAVE_SIZE_X: usize = 1024;
const WAVE_SIZE_Y: usize = 16;

impl LiveHook for DisplayAudio {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.wave_texture.set_desc(cx, TextureDesc {
            format: TextureFormat::ImageBGRA,
            width: Some(WAVE_SIZE_X),
            height: Some(WAVE_SIZE_Y),
            multisample: None
        });
        let mut wave_buf = Vec::new();
        self.wave_texture.swap_image_u32(cx, &mut wave_buf);
        wave_buf.resize(WAVE_SIZE_X * WAVE_SIZE_Y, 0);
        for j in 0..WAVE_SIZE_Y{
            for i in 0..WAVE_SIZE_X{
                let left_u16 = 32767;
                let right_u16 = 32767;
                wave_buf[j*WAVE_SIZE_X+i] = left_u16 << 16 | right_u16;
            }
        }
        self.wave_texture.swap_image_u32(cx, &mut wave_buf);
    }
}

impl DisplayAudio {
    pub fn process_buffer(&mut self, cx: &mut Cx, voice: usize, audio: &AudioBuffer) {
        let mut wave_buf = Vec::new();
        self.wave_texture.swap_image_u32(cx, &mut wave_buf);
        let frames = audio.frame_count();
        let (left, right) = audio.stereo();
        let wave_off = self.data_offset[voice];
        let voice_offset = voice * WAVE_SIZE_X;
        let mut is_active = false;
        for i in 0..frames {
            let left_u16 = ((left[i] + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            let right_u16 = ((right[i] + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            if left[i].abs()>0.000000000001 || right[i].abs()>0.000000000001{
                is_active = true;
            }
            wave_buf[voice_offset + ((wave_off + i) & (WAVE_SIZE_X - 1))] = left_u16 << 16 | right_u16;
        }
        self.wave_texture.swap_image_u32(cx, &mut wave_buf);
        self.data_offset[voice] = (self.data_offset[voice] + frames) & (WAVE_SIZE_X - 1);
        if self.active[voice] || is_active{
            self.draw_wave.redraw(cx);
        }
        self.active[voice] = is_active;
    }
}

impl DisplayAudio {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_wave.draw_vars.set_texture(0, &self.wave_texture);
        self.draw_wave.draw_walk(cx, walk);
    }
    
    pub fn area(&self)->Area{
        self.draw_wave.area()
    }
    
    
    pub fn handle_event_fn(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, DisplayAudioAction),
    ) {
    }
}

// ImGUI convenience API for Piano
#[derive(Clone, PartialEq, WidgetRef)]
pub struct DisplayAudioRef(WidgetRef);

impl DisplayAudioRef {
    pub fn process_buffer(&self, cx: &mut Cx, _active: bool, voice: usize, buffer: &AudioBuffer) {
        if let Some(mut inner) = self.inner_mut() {
            inner.process_buffer(cx, voice, buffer);
        }
    }
    
    pub fn voice_off(&self, _cx: &mut Cx, _voice: usize,) {
    }
}
