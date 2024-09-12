
use {
    crate::{
        makepad_draw::*,
        makepad_widgets::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    
    DrawWave = {{DrawWave}} {
        texture wave_texture: texture2d
        
        fn vu_fill(self)->vec4{
            return vec4(Pal::iq1((1.0-0.5*self.pos.y)),1.0)
        }
        
        fn pixel(self) -> vec4 {
            let wave = sample2d(self.wave_texture, vec2(self.pos.x, 0.0));
            let right = (wave.y + wave.z / 256.0 - 0.5) * 3.0;
            let left = (wave.w + wave.x / 256.0 - 0.5) * 3.0;
            //return mix(#f00,#0f0, left+0.5);
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            let step = 0.0;
            // lets draw a gradient vu meter
            let vu_ht = self.rect_size.y*(1.0-self.vu_left * self.gain);
            sdf.rect(0.,vu_ht,self.rect_size.x, self.rect_size.y-vu_ht);
            sdf.fill(self.vu_fill())
            for i in 0..4 {
                let wave = sample2d(self.wave_texture, vec2(self.pos.x, step));
                let right = (wave.y + wave.z / 256.0 - 0.5) * 3.0;
                let left = (wave.w + wave.x / 256.0 - 0.5) * 3.0;
                let audio = (left + right) - 0.01;
                let half = self.rect_size.y * 0.5;
                let scale = half * 2.0;
                sdf.hline(half + audio * scale, abs(audio) * scale);
                let color = Pal::iq1(0.35 + 2.0 * step) * 0.8;
                sdf.fill_premul(vec4(color, 0.0))
                step += 1.0 / 16.0;
            }
            return sdf.result
        }
    }
    
    DisplayAudio = {{DisplayAudio}} {
        width: Fill,
        height: Fill
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook, LiveRegister)]#[repr(C)]
struct DrawWave {
    #[deref] draw_super: DrawQuad,
    #[live] gain: f32,
    #[live] vu_left: f32,
    #[live] vu_right: f32
}

#[derive(Live, Widget)]
pub struct DisplayAudio {
    #[walk] walk: Walk,
    #[redraw] #[live] draw_wave: DrawWave,
    #[rust(Texture::new(cx))] wave_texture: Texture,
    #[rust] data_offset: [usize; 32],
    #[rust([0; 32])] active: [usize; 32],
    #[rust([(0.0,0.0);32])] vu:[(f32,f32); 32],
}


impl Widget for DisplayAudio {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope){
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep { 
        self.draw_wave.draw_vars.set_texture(0, &self.wave_texture);
        self.draw_wave.vu_left = self.vu[0].0.powf(1.0/3.0)*1.2;
        self.draw_wave.vu_right = self.vu[0].1.powf(1.0/3.0)*1.2;
        self.vu[0].0 *= 0.95;
        self.vu[0].1 *= 0.95;
        self.draw_wave.draw_walk(cx, walk);
        DrawStep::done()
    }
}

#[derive(Clone, DefaultNone)]
pub enum DisplayAudioAction {
    None
}
const WAVE_SIZE_X: usize = 1024;
const WAVE_SIZE_Y: usize = 16;

impl LiveHook for DisplayAudio {

    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.wave_texture = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
            data: {
                let mut data = Vec::new();
                data.resize(WAVE_SIZE_X * WAVE_SIZE_Y, 0);
                for j in 0..WAVE_SIZE_Y {
                    for i in 0..WAVE_SIZE_X {
                        let left_u16 = 32767;
                        let right_u16 = 32767;
                        data[j * WAVE_SIZE_X + i] = left_u16 << 16 | right_u16;
                    }
                }
                Some(data)
            },
            width: WAVE_SIZE_X,
            height: WAVE_SIZE_Y,
            updated: TextureUpdated::Full,
        });
    }
}

impl DisplayAudio {
    pub fn process_buffer(&mut self, cx: &mut Cx, chan: Option<usize>, voice: usize, audio: &AudioBuffer, gain:f32) {
        let mut wave_buf = self.wave_texture.take_vec_u32(cx);
        let frames = audio.frame_count();
        let wave_off = self.data_offset[voice];
        let voice_offset = voice * WAVE_SIZE_X;
        let mut is_active = false;
        for i in 0..frames {
            let left = audio.channel(chan.unwrap_or(0))[i];
            let right = audio.channel(chan.unwrap_or(audio.channel_count().min(1)))[i];
            if left.abs() > self.vu[voice].0 {self.vu[voice].0 = left.abs()};
            if right.abs() > self.vu[voice].1 {self.vu[voice].1 = right.abs()};
            let left_u16 = ((left + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            let right_u16 = ((right + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            if left.abs()>0.0000000000001 || right.abs()>0.0000000000001 {
                is_active = true;
            }
            let off = voice_offset + ((wave_off + i) % (WAVE_SIZE_X));
            wave_buf[off] = left_u16 << 16 | right_u16;
        }
        self.draw_wave.gain = gain;
        self.wave_texture.put_back_vec_u32(cx, wave_buf, None);
        self.data_offset[voice] = (self.data_offset[voice] + frames) % (WAVE_SIZE_X);
        if is_active {
            self.active[voice] = 6
        }
        if self.active[voice]>0 {
            self.draw_wave.redraw(cx);
            self.active[voice] -= 1;
        }
    }
}

impl DisplayAudioRef {
    pub fn process_buffer(&self, cx: &mut Cx, chan: Option<usize>, voice: usize, buffer: &AudioBuffer, gain:f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.process_buffer(cx, chan, voice, buffer, gain);
        }
    }
    
    pub fn voice_off(&self, _cx: &mut Cx, _voice: usize,) {
    }
}

impl DisplayAudioSet {
    pub fn process_buffer(&self, cx: &mut Cx, chan: Option<usize>, voice: usize, buffer: &AudioBuffer, gain:f32) {
        for item in self.iter(){
            item.process_buffer(cx, chan, voice, buffer, gain);
        }
    }
    
    pub fn voice_off(&self, cx: &mut Cx, voice: usize,) {
        for item in self.iter(){
            item.voice_off(cx, voice);
        }
    }
}
