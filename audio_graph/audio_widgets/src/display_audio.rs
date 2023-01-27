
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
            let wave = sample2d(self.wave_texture, vec2(self.pos.x, 0.05));
            
            let right = (wave.y + wave.z / 256.0 - 0.5) * 3.0;
            let left = (wave.w + wave.x / 256.0 - 0.5) * 3.0;
            let sdf = Sdf2d::viewport(self.pos * self.rect_size * vec2(1.0, 0.5));
            let color = Pal::iq1(0.25) * 0.5;
            
            if left < 0.0 {
                sdf.rect(0., self.rect_size.y * 0.25, self.rect_size.x, -left * self.rect_size.y + 0.5);
            }
            else {
                sdf.rect(0., self.rect_size.y * 0.25 - self.rect_size.y * left, self.rect_size.x, left * self.rect_size.y + 0.5);
            }
            sdf.fill(vec4(color, 1.0));
            
            if right < 0.0 {
                sdf.rect(0., self.rect_size.y * 0.75, self.rect_size.x, -right * self.rect_size.y + 0.5);
            }
            else {
                sdf.rect(0., self.rect_size.y * 0.75 - self.rect_size.y * right, self.rect_size.x, right * self.rect_size.y + 0.5);
            }
            sdf.fill(vec4(color, 1.0));
            
            let result = sdf.result.xyz;
            return vec4(result, 0.0)
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
    #[rust] data_offset: [usize;32],
    #[rust] area: Area,
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
    }
}

impl DisplayAudio {
    pub fn process_buffer(&mut self, cx: &mut Cx, voice: usize, audio: &AudioBuffer) {
        
        let mut wave_buf = Vec::new();
        self.wave_texture.swap_image_u32(cx, &mut wave_buf);
        wave_buf.resize(WAVE_SIZE_X * WAVE_SIZE_Y, 0);
        
        let frames = audio.frame_count();
        
        let (left, right) = audio.stereo();
        let wave_off = self.data_offset[voice];
        let voice_offset = voice * WAVE_SIZE_X;
        for i in 0..frames {
            let left_u16 = ((left[i] + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            let right_u16 = ((right[i] + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            wave_buf[voice_offset + ((wave_off + i) & (WAVE_SIZE_X - 1))] = left_u16 << 16 | right_u16;
        }
        // every time we wrap around we should feed it to the FFT
        self.wave_texture.swap_image_u32(cx, &mut wave_buf);
        self.data_offset[voice] = (self.data_offset[voice] + frames) & (WAVE_SIZE_X - 1);
    }
}

impl DisplayAudio {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.draw_wave.draw_vars.set_texture(0, &self.wave_texture);
        self.draw_wave.draw_abs(cx, rect);
    }
    
    pub fn area(&mut self) -> Area {
        self.area
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
    pub fn process_buffer(&self, cx: &mut Cx, active: bool, voice: usize, buffer: &AudioBuffer) {
        if let Some(mut inner) = self.inner_mut() {
            inner.process_buffer(cx, voice, buffer);
            if active{
                inner.area.redraw(cx);
            }
        }
    }
    
    pub fn voice_off(&self, _cx: &mut Cx, _voice: usize,) {
    }
}
