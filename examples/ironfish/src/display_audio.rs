
use {
    crate::{
        makepad_draw_2d::*,
        makepad_math::complex::*,
        makepad_widgets::*,
        makepad_media::*,
    }
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawFFT = {{DrawFFT}} {
        texture wave_texture: texture2d
        texture fft_texture: texture2d
        fn pixel(self) -> vec4 {
            let wave = sample2d(self.wave_texture, vec2(self.pos.x, 0.5));
            
            let fft = sample2d(
                self.fft_texture,
                vec2(mod (1.0 - self.pos.y * 0.5, 0.5), fract(self.pos.x + self.shift_fft))
                //vec2(self.pos.y, fract(self.pos.x + self.shift_fft))
            );
            
            let right = (wave.y + wave.z / 256.0 - 0.5) * 3.0;
            let left = (wave.w + wave.x / 256.0 - 0.5) * 3.0;
            let right_fft = fft.y + fft.z / 256.0;
            let left_fft = fft.w + fft.x / 256.0;
            
            let sdf = Sdf2d::viewport(self.pos * self.rect_size * vec2(1.0, 0.5));
            let color = Pal::iq1(self.layer + 0.25) * 0.5;
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
            
            if self.pos.y>0.5 {
                result += left_fft * color;
            }
            else {
                result += right_fft * color;
            }
            
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
struct DrawFFT {
    draw_super: DrawQuad,
    shift_fft: f32,
    layer: f32
}

#[derive(Live, Widget)]
#[live_design_fn(widget_factory!(DisplayAudio))]
pub struct DisplayAudio {
    //view: View,
    walk: Walk,
    fft: DrawFFT,
    #[rust] area: Area,
    #[rust] layers: Vec<DisplayAudioLayer>
}

pub struct DisplayAudioLayer {
    active: bool,
    wave_texture: Texture,
    fft_texture: Texture,
    fft_slot: usize,
    fft_buffer: [Vec<ComplexF32>; 2],
    fft_empty_count: usize,
    fft_scratch: Vec<ComplexF32>,
    data_offset: usize
}

impl DisplayAudioLayer {
    pub fn new(cx: &mut Cx) -> Self {
        let wave_texture = Texture::new(cx);
        let fft_texture = Texture::new(cx);
        wave_texture.set_desc(cx, TextureDesc {
            format: TextureFormat::ImageBGRA,
            width: Some(WAVE_SIZE_X),
            height: Some(WAVE_SIZE_Y),
            multisample: None
        });
        fft_texture.set_desc(cx, TextureDesc {
            format: TextureFormat::ImageBGRA,
            width: Some(FFT_SIZE_X),
            height: Some(FFT_SIZE_Y),
            multisample: None
        });
        Self {
            fft_empty_count: FFT_SIZE_X * FFT_SIZE_Y + 1,
            active: false,
            wave_texture,
            fft_texture,
            fft_slot: 0,
            fft_buffer: Default::default(),
            fft_scratch: Default::default(),
            data_offset: 0
        }
    }
    
    
    pub fn process_buffer(&mut self, cx: &mut Cx, active: bool, audio: &AudioBuffer) -> bool {
        if self.fft_empty_count >= FFT_SIZE_T * FFT_SIZE_Y && !active {
            return false
        }
        // alright we have a texture. lets write the audio somewhere
        //return;
        let mut buf = Vec::new();
        self.wave_texture.swap_image_u32(cx, &mut buf);
        buf.resize(WAVE_SIZE_X * WAVE_SIZE_Y, 0);
        
        if !self.active {
            let left_u16 = ((0.0 + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            let right_u16 = ((0.0 + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            for i in 0..buf.len() {buf[i] = left_u16 << 16 | right_u16}
            // clear the texture
            self.data_offset = 0;
            // clear the fft
            let mut buf = Vec::new();
            self.fft_texture.swap_image_u32(cx, &mut buf);
            for i in 0..buf.len() {buf[i] = 0}
            self.fft_texture.swap_image_u32(cx, &mut buf);
            self.fft_slot = 0;
        }
        self.active = true;
        
        let frames = audio.frame_count();
        
        self.fft_buffer[0].resize(FFT_SIZE_T, cf32(0.0, 0.0));
        self.fft_buffer[1].resize(FFT_SIZE_T, cf32(0.0, 0.0));
        self.fft_scratch.resize(FFT_SIZE_T, cf32(0.0, 0.0));
        
        let (left, right) = audio.stereo();
        
        let wave_off = self.data_offset;
        let fft_off = (self.data_offset) & (FFT_SIZE_T - 1);
        
        for i in 0..frames {
            let left_u16 = ((left[i] + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            let right_u16 = ((right[i] + 0.5) * 65536.0).max(0.0).min(65535.0) as u32;
            buf[(wave_off + i) & (WAVE_SIZE_X - 1)] = left_u16 << 16 | right_u16;
            let fft_now = (fft_off + i) & (FFT_SIZE_T - 1);
            self.fft_buffer[0][fft_now] = cf32(left[i], 0.0);
            self.fft_buffer[1][fft_now] = cf32(right[i], 0.0);
            if left[i] != 0.0 || right[i] != 0.0 {
                self.fft_empty_count = 0;
            }
            else {
                self.fft_empty_count += 1;
            }
            // if the fft buffer is full, emit a new fftline
            if fft_now == FFT_SIZE_T - 1 {
                let mut buf = Vec::new();
                self.fft_texture.swap_image_u32(cx, &mut buf);
                buf.resize(FFT_SIZE_X * FFT_SIZE_Y, 0);
                if self.fft_empty_count < FFT_SIZE_T {
                    fft_f32_recursive_pow2_forward(&mut self.fft_buffer[0], &mut self.fft_scratch);
                    fft_f32_recursive_pow2_forward(&mut self.fft_buffer[1], &mut self.fft_scratch);
                }
                
                // lets write fft_buffer[0] to the texture
                for i in 0..FFT_SIZE_X {
                    let left = self.fft_buffer[0][i].magnitude();
                    let right = self.fft_buffer[1][i].magnitude();
                    let left_u16 = (left * 10000.0).max(0.0).min(65535.0) as u32;
                    let right_u16 = (right * 10000.0).max(0.0).min(65535.0) as u32;
                    buf[self.fft_slot * FFT_SIZE_X + i] = left_u16 << 16 | right_u16;
                }
                self.fft_slot = (self.fft_slot + 1) & (FFT_SIZE_Y - 1);
                self.fft_texture.swap_image_u32(cx, &mut buf);
            }
        }
        // every time we wrap around we should feed it to the FFT
        self.wave_texture.swap_image_u32(cx, &mut buf);
        self.data_offset = (self.data_offset + frames) & (WAVE_SIZE_X - 1);
        self.fft_empty_count < FFT_SIZE_T * FFT_SIZE_Y
    }
}


#[derive(Clone, WidgetAction)]
pub enum DisplayAudioAction {
    None
}
const WAVE_SIZE_X: usize = 1024;
const WAVE_SIZE_Y: usize = 1;
const FFT_SIZE_T: usize = 512;
const FFT_SIZE_X: usize = FFT_SIZE_T >> 1;
const FFT_SIZE_Y: usize = 512;

impl LiveHook for DisplayAudio {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        for _ in 0..16 {
            self.layers.push(DisplayAudioLayer::new(cx))
        }
    }
}

impl DisplayAudio {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        // alright lets draw em fuckers
        //if self.view.begin(cx, walk, Layout::default()).not_redrawing() {
        //    return
        //};
        // ok so we walk and get a rect
        
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        
        for (index, layer) in self.layers.iter().enumerate() {
            if !layer.active || layer.fft_empty_count >= FFT_SIZE_T * FFT_SIZE_Y {
                continue
            }
            self.fft.layer = index as f32 / self.layers.len() as f32;
            self.fft.shift_fft = layer.fft_slot as f32 / FFT_SIZE_Y as f32;
            self.fft.draw_vars.set_texture(0, &layer.wave_texture);
            self.fft.draw_vars.set_texture(1, &layer.fft_texture);
            self.fft.draw_abs(cx, rect);
        }
        //self.view.end(cx);
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
            if inner.layers[voice].process_buffer(cx, active, buffer) {
                inner.area.redraw(cx);
            }
        }
    }
    
    pub fn voice_off(&self, _cx: &mut Cx, _voice: usize,) {
    }
}
