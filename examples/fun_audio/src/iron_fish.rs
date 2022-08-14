
// Iron fish is MIT licensed, (C) Stijn Kuipers


#![allow(unused)]
use {
    std::sync::Arc,
    crate::{
        audio::*,
        makepad_platform::live_atomic::*,
        makepad_platform::thread::*,
        makepad_media::*,
        makepad_media::audio_graph::*,
        makepad_draw_2d::*
    },
};


#[derive(Live, LiveHook, LiveAtomic, Debug, LiveRead)]
pub enum OscType {
    DPWSawPulse,
    TrivialSaw,
    #[pick] BlampTri,
    Naive,
    Pure
}

#[derive(Live, LiveHook, PartialEq, LiveAtomic, Debug, LiveRead)]
pub enum FilterType {
    #[pick] Lowpass,
    Highpass,
    Bandpass
}
/*
struct LaddFilterCoefficients {
    k: f64,
    alpha: f64,
    alpha0: f64,
    sigma: f64,
    bass_comp: f64,
    g: f64,
    subfilter_beta: [f64; 4]
}

impl Default for LaddFilterCoefficients {
    fn default() -> Self {
        Self {
            k: 1.0,
            alpha: 0.0,
            alpha0: 1.0,
            sigma: 1.0,
            bass_comp: 1.0,
            g: 1.0,
            
            subfilter_beta: [0.0, 0.0, 0.0, 0.0]
        }
    }
}*/

#[derive(Live, LiveHook, LiveAtomic, Debug, LiveRead)]
pub struct OscSettings {
    osc_type: U32A<OscType>,
    #[live(-12)] transpose: i64a,
    #[live(0.0)] detune: f32a
}

#[derive(Live, LiveHook, LiveAtomic, Debug, LiveRead)]
pub struct EnvelopeSettings {
    #[live(0.0)] predelay: f32a,
    #[live(0.1)] a: f32a,
    #[live(0.0)] h: f32a,
    #[live(0.6)] d: f32a,
    #[live(0.3)] s: f32a,
    #[live(0.5)] r: f32a
}

#[derive(Live, LiveHook, LiveAtomic, Debug, LiveRead)]
pub struct FilterSettings {
    filter_type: U32A<FilterType>,
    envelope: EnvelopeSettings,
    #[live(300.0 / 44100.0)] cutoff: f32a,
    #[live(0.05)] resonance: f32a,
    #[live(0.1)] envelope_amount: f32a,
    #[live(0.0)] envelope_curvature: f32a
}

#[derive(Live, LiveHook, LiveAtomic, Debug, LiveRead)]
#[live_ignore]
pub struct IronFishSettings {
    osc1: OscSettings,
    osc2: OscSettings,
    filter1: FilterSettings,
    volume_envelope: EnvelopeSettings,
    #[live(44100.0)] sample_rate: f32a,
    #[live(0.5)] osc_balance: f32a,
}

#[derive(Copy, Clone)]
pub struct OscillatorState {
    phase: f32,
    delta_phase: f32,
    dpw_gain1: f32,
    dpw_gain2: f32,
    dpw_diff_b: [f32; 4],
    dpw_diff_b_write_index: i8, // diffB write index
    dpw_init_countdown: i8
}

impl OscillatorState {
    fn dpw(&mut self) -> f32 {
        
        let triv = 2.0 * self.phase - 1.0;
        
        let sqr = triv * triv;
        
        let poly = sqr * sqr - 2.0 * sqr;
        
        self.dpw_diff_b[self.dpw_diff_b_write_index as usize] = poly;
        self.dpw_diff_b_write_index = self.dpw_diff_b_write_index + 1;
        
        if self.dpw_diff_b_write_index == 4 {self.dpw_diff_b_write_index = 0};
        
        if self.dpw_init_countdown > 0{
            self.dpw_init_countdown = self.dpw_init_countdown - 1;
            
            return poly;
        }
        
        let mut tmp_a = [0.0, 0.0, 0.0, 0.0];
        let mut dbr = self.dpw_diff_b_write_index - 1;
        if dbr<0 {dbr = 3}
        
        for i in 0..4 {
            tmp_a[i] = self.dpw_diff_b[dbr as usize];
            dbr = dbr - 1;
            if dbr < 0 {
                dbr = 3;
            }
        }
        
        let gain = self.dpw_gain1;
        tmp_a[0] = gain * (tmp_a[0] - tmp_a[1]);
        tmp_a[1] = gain * (tmp_a[1] - tmp_a[2]);
        tmp_a[2] = gain * (tmp_a[2] - tmp_a[3]);
        
        tmp_a[0] = gain * (tmp_a[0] - tmp_a[1]);
        tmp_a[1] = gain * (tmp_a[1] - tmp_a[2]);
        
        tmp_a[0] = gain * (tmp_a[0] - tmp_a[1]);
        
        
        return tmp_a[0] * self.dpw_gain2; //* self.dpw_gain;
    }
    
    fn trivialsaw(self) -> f32{
        return self.phase * 2.0 - 1.0
    }
    
    fn blamp(&mut self, t: f32, dt: f32) -> f32 {
        let mut y = 0.0;
        if 0.0 <= t && t<2.0 * dt {
            let x = t / dt;
            let u = 2.0 - x;
            let u2 = u * u;
            let u4 = u2 * u2;
            y -= u4;
            if t<dt {
                let v = 1.0 - x;
                let v2 = v * v;
                let v5 = v * v2 * v2;
                y += 4.0 * v5;
            }
        }
        return y * dt / 15.0
        
    }
    
    fn blamptriangle(&mut self) -> f32 {
        let mut tri = 2.0 * (2.0 * self.phase - 1.0).abs() - 1.0;
        
        tri += self.blamp(self.phase, self.delta_phase);
        tri += self.blamp(1.0 - self.phase, self.delta_phase);
        let mut t2 = self.phase + 0.5;
        t2 -= t2.floor();
        tri -= self.blamp(t2, self.delta_phase);
        tri -= self.blamp(1.0 - t2, self.delta_phase);
        return tri;
    }
    
    fn get(&mut self, settings: &OscSettings, _samplerate: f32) -> f32 {
        self.phase += self.delta_phase;
        while self.phase > 1.0{
            self.phase -= 1.0
        }
        // return self.dpw();
        
        match settings.osc_type.get() {
            OscType::Pure => (self.phase * 6.283).sin(),
            OscType::DPWSawPulse => self.dpw(),
            OscType::TrivialSaw => self.trivialsaw(),
            OscType::BlampTri => self.blamptriangle(),
            OscType::Naive => 0.0,
        }
    }
    
    fn set_note(&mut self, note: u8, samplerate: f32, settings: &OscSettings){
        let freq = 440.0 * f32::powf(2.0, ((note as f32) - 69.0 + settings.transpose.get() as f32 + settings.detune.get()) / 12.0);
        self.delta_phase = (6.283 * freq) / samplerate;
        //let w = freq * 6.283 / samplerate;
        let sampletime = 1.0 / samplerate;
        let prep = samplerate / freq; // / samplerate;
        //self.dpw_gain1 = (prep*prep*prep);
        //self.dpw_gain2 = 1.0/192.0 ;// (1.0 / 24.0 * (3.1415 / (2.0 * (3.1415 * prep).sin())).powf(3.0)).powf(1.0/3.0);
        self.dpw_gain1 = (1.0 / 24.0 * (3.1415 / (2.0 * (3.1415 / prep).sin())).powf(3.0)).powf(1.0 / 3.0);
        
        // println!("gain: {} {} ", self.dpw_gain, prep);
        //  gain = std::pow(1.f / factorial(dpwOrder) * std::pow(M_PI / (2.f*sin(M_PI*pitch * APP->engine->getSampleTime())),  dpwOrder-1.f), 1.0 / (dpwOrder-1.f));
    }
}

impl Default for OscillatorState {
    fn default() -> Self {
        Self {
            phase: 0.0,
            delta_phase: 0.0001,
            dpw_gain1: 1.0,
            dpw_gain2: 1.0,
            dpw_init_countdown: 4,
            dpw_diff_b: [3.0, 3.0, 3.0, 3.0],
            dpw_diff_b_write_index: 0
        }
    }
}

impl Default for FilterState {
    fn default() -> Self {
        Self {
            filter_envelope: EnvelopeState::default(),
            hp: 0.0,
            bp: 0.0,
            lp: 0.0,
            gamma: 0.0,
            fc: 0.0,
            phi: 0.0,
            damp: 0.0
        }
    }
}

#[derive(PartialEq, Copy, Clone)]
enum EnvelopePhase {
    Idle,
    Predelay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    FastRelease
}

#[derive(Copy, Clone)]
struct EnvelopeState {
    phase: EnvelopePhase,
    delta_value: f32,
    current_value: f32,
    target_value: f32,
    state_time: i32
}

impl Default for EnvelopeState {
    fn default() -> Self {
        Self {
            phase: EnvelopePhase::Idle,
            delta_value: 0.0,
            current_value: 0.0,
            target_value: 0.0,
            state_time: 0
        }
    }
}

impl EnvelopeState {
    
    fn get(&mut self, settings: &EnvelopeSettings, samplerate: f32) -> f32 {
        
        self.current_value = self.current_value + self.delta_value;
        self.state_time = self.state_time - 1;
        //println!("st {}", self.state_time);
        if self.state_time < -1 {self.state_time = -1;}
        if self.state_time == 0 {
            match self.phase {
                
                EnvelopePhase::Attack => {
                    if settings.h.get() != 0.0 {
                        self.phase = EnvelopePhase::Hold;
                        self.delta_value = 0.0;
                        self.current_value = 1.0;
                        self.target_value = 1.0;
                        self.state_time = (settings.h.get() * samplerate) as i32;
                        
                    }
                    else {
                        self.phase = EnvelopePhase::Decay;
                        self.delta_value = -(1.0 - settings.s.get()) / (settings.d.get() * samplerate);
                        self.current_value = 1.0;
                        self.target_value = settings.s.get();
                        self.state_time = (settings.d.get() * samplerate) as i32;
                    }
                }
                
                EnvelopePhase::Hold => {
                    
                    self.phase = EnvelopePhase::Decay;
                    self.delta_value = -(1.0 - settings.s.get()) / (settings.d.get() * samplerate);
                    self.current_value = 1.0;
                    self.target_value = settings.s.get();
                    self.state_time = (settings.d.get() * samplerate) as i32;
                }
                
                EnvelopePhase::Decay => {
                    self.phase = EnvelopePhase::Sustain;
                    self.delta_value = 0.0;
                    self.current_value = settings.s.get();
                    self.target_value = settings.s.get();
                    self.state_time = -1;
                }
                
                EnvelopePhase::FastRelease |
                EnvelopePhase::Release => {
                    self.phase = EnvelopePhase::Idle;
                    self.delta_value = 0.0;
                    self.current_value = 0.0;
                    self.target_value = 0.0;
                    self.state_time = -1;
                }
                EnvelopePhase::Predelay => {
                    self.phase = EnvelopePhase::Attack;
                    self.delta_value = (1.0 - self.current_value) / (settings.a.get() * samplerate);
                    self.state_time = (settings.a.get() * samplerate) as i32;
                    self.target_value = 1.0;
                }
                
                _ => {}
            }
        }
        return self.current_value;
    }
    
    fn trigger_on(&mut self, velocity: f32, settings: &EnvelopeSettings, samplerate: f32) {
        if settings.predelay.get() != 0.0 {
            if self.phase != EnvelopePhase::Idle {
                self.phase = EnvelopePhase::FastRelease;
                self.target_value = 0.0;
                self.delta_value = -self.current_value / 100.0;
                self.state_time = 100;
                return;
            }
            self.delta_value = 0.0;
            self.current_value = 0.0;
            self.target_value = 0.0;
            self.phase = EnvelopePhase::Predelay;
            self.state_time = (samplerate * settings.predelay.get()) as i32;
            return;
        };
        
        self.phase = EnvelopePhase::Attack;
        self.delta_value = (1.0 - self.current_value) / (settings.a.get() * samplerate);
        self.state_time = (settings.a.get() * samplerate) as i32;
        self.target_value = 1.0;
        //  println!("attacking with {} {} {} {} {}",self.state_time,  settings.a, samplerate, settings.a * samplerate, self.delta_value);
    }
    
    fn trigger_off(&mut self, velocity: f32, settings: &EnvelopeSettings, samplerate: f32) {
        
        match self.phase {
            EnvelopePhase::Attack |
            EnvelopePhase::Decay |
            EnvelopePhase::Sustain => {
                self.phase = EnvelopePhase::Release;
                self.target_value = 0.0;
                self.delta_value = -self.current_value / (settings.r.get() * samplerate);
                self.state_time = (settings.r.get() * samplerate) as i32;
            }
            _ => {}
        }
    }
}

#[derive(Copy, Clone)]
struct FilterState {
    filter_envelope: EnvelopeState,
    
    hp: f32,
    bp: f32,
    lp: f32,
    phi: f32,
    gamma: f32,
    fc: f32,
    damp: f32
}

impl FilterState {
    fn get(&mut self, input: f32) -> f32 {
        self.bp = self.phi * self.hp + self.bp;
        self.lp = self.phi * self.bp + self.lp;
        self.hp = input - self.lp - self.gamma * self.bp;
        return self.lp;
    }
    
    fn set_cutoff(&mut self, settings: &FilterSettings, envelope: f32, sample_rate: f32) {
        self.fc = (settings.cutoff.get() + envelope * settings.envelope_amount.get()).clamp(0.0, 1.0);
        self.damp = settings.resonance.get();
        let preclamp = 2.0 * ((3.1415 * self.fc).sin());
        self.phi = (preclamp).clamp(0.0, 1.0);
        self.gamma = (2.0 * self.damp).clamp(0.0, 1.0);
    }
}

struct GriesingerReverb {
    lp_: f32,
    diffusion_: f32,
    dphase1: f32,
    dphase2: f32,
    writepos: i32,
    buffer: [f32; 96000]
}
/*
impl GriesingerReverb {
    pub fn init(&mut self, sample_rate: f32) {
        self.dphase1 = 0.5 / sample_rate;
        self.dphase2 = 0.3 / sample_rate;
        self.lp_ = 0.7;
        self.diffusion_ = 0.625;
    }
    
    pub fn fill_buffer(&mut self, buffer: &mut AudioBuffer) {
        
        for i in 0..buffer.frame_count() {
            let output = 0.0;
            buffer.left[i] += output as f32;
            buffer.right[i] += output as f32;
        }
    }
}
*/
#[derive(Copy, Clone)]
pub struct IronFishVoice {
    osc1: OscillatorState,
    osc2: OscillatorState,
    filter1: FilterState,
    volume_envelope: EnvelopeState,
    current_note: i16
}

impl IronFishVoice {
    pub fn active(&mut self) -> i16 {
        if self.volume_envelope.phase == EnvelopePhase::Idle {
            return -1;
        }
        
        return self.current_note;
    }
    
    pub fn note_off(&mut self, b1: u8, b2: u8, settings: &IronFishSettings) {
        let velocity = (b2 as f32) / 127.0;
        self.volume_envelope.trigger_off(velocity, &settings.volume_envelope, settings.sample_rate.get());
        self.filter1.filter_envelope.trigger_off(velocity, &settings.filter1.envelope, settings.sample_rate.get());
    }
    
    pub fn note_on(&mut self, b1: u8, b2: u8, settings: &IronFishSettings) {
        
        let velocity = (b2 as f32) / 127.0;
        self.osc1.set_note(b1, settings.sample_rate.get(), &settings.osc1);
        self.osc2.set_note(b1, settings.sample_rate.get(), &settings.osc2);
        self.volume_envelope.trigger_on(velocity, &settings.volume_envelope, settings.sample_rate.get());
        self.filter1.filter_envelope.trigger_on(velocity, &settings.filter1.envelope, settings.sample_rate.get());
        self.current_note = b1 as i16;
    }
    
    pub fn one(&mut self, settings: &IronFishSettings) -> f32 {
        let osc1 = self.osc1.get(&settings.osc1, settings.sample_rate.get());
        let osc2 = self.osc2.get(&settings.osc2, settings.sample_rate.get());
        let volume_envelope = self.volume_envelope.get(&settings.volume_envelope, settings.sample_rate.get());
        let filter_envelope = self.filter1.filter_envelope.get(&settings.filter1.envelope, settings.sample_rate.get());
        
        self.filter1.set_cutoff(&settings.filter1, filter_envelope, settings.sample_rate.get());
        let oscinput = osc1 * settings.osc_balance.get() + osc2 * (1.0 - settings.osc_balance.get());
        let filter = self.filter1.get(oscinput);
        
        let output = volume_envelope * filter;
        
        return output * 0.006; //* 1000.0;
    }
    
    pub fn fill_buffer(&mut self, mix_buffer: &mut AudioBuffer, display_buffer: Option<&mut AudioBuffer>, settings: &IronFishSettings) {
        
        let frame_count = mix_buffer.frame_count();
        let (left, right) = mix_buffer.stereo_mut();
        
        if let Some(display_buffer) = display_buffer{
            let (left_disp, right_disp) = display_buffer.stereo_mut();
            for i in 0..frame_count {
                let output = self.one(&settings) * 8.0;
                left_disp[i] = output as f32;
                right_disp[i] = output as f32;
                left[i] += output as f32;
                right[i] += output as f32;
            }
        }
        else{
            for i in 0..frame_count {
                let output = self.one(&settings) * 8.0;
                left[i] += output as f32;
                right[i] += output as f32;
            }
        }
        // profile_end(pf);
    }
    
}
pub struct IronFishState {
    from_ui: FromUIReceiver<FromUI>,
    to_ui: ToUISender<ToUI>,
    display_buffers: Vec<AudioBuffer>,
    settings: Arc<IronFishSettings>,
    voices: [IronFishVoice; 16]
}

impl IronFishState {
    
    pub fn note_off(&mut self, b1: u8, b2: u8) {
        
        for i in 0..self.voices.len() {
            if self.voices[i].active() == b1 as i16 {
                self.voices[i].note_off(b1, b2, &self.settings)
            }
        }
    }
    
    pub fn note_on(&mut self, b1: u8, b2: u8) {
        for i in 0..self.voices.len() {
            if self.voices[i].active() == -1 {
                self.voices[i].note_on(b1, b2, &self.settings);
                return;
            }
        }
    }
    
    pub fn one(&mut self) -> f32 {
        let mut output: f32 = 0.0;
        for i in 0..self.voices.len() {
            output += self.voices[i].one(&self.settings);
        }
        return output; //* 1000.0;
    }
    
    pub fn fill_buffer(&mut self, buffer: &mut AudioBuffer, display:&mut DisplayAudioGraph) {
        
        buffer.zero();
        for i in 0..self.voices.len() {
            if self.voices[i].active() > -1 {
                let mut display_buffer = display.pop_buffer_resize(buffer.frame_count(), buffer.channel_count());
                self.voices[i].fill_buffer(buffer, display_buffer.as_mut(), &self.settings);
                if let Some(dp) = display_buffer{
                    display.send_buffer(true, i, dp);
                }
            }
            else{
                display.send_voice_off(i);
                let mut display_buffer = display.pop_buffer_resize(buffer.frame_count(), buffer.channel_count());
                if let Some(mut dp) = display_buffer{
                    dp.zero();
                    display.send_buffer(false, i,dp);
                }
            }
        }
    }
}

impl Default for IronFishVoice {
    fn default() -> Self {
        Self {
            osc1: OscillatorState::default(),
            osc2: OscillatorState::default(),
            filter1: FilterState::default(),
            volume_envelope: EnvelopeState::default(),
            current_note: -1
        }
    }
}

live_register!{
    IronFish: {{IronFish}} {
        settings:{}
    }
}

enum ToUI {
    DisplayAudio{
        voice:usize,
        buffer:AudioBuffer
    },
}

enum FromUI {
    DisplayAudio(AudioBuffer),
}

#[derive(Live, LiveHook)]
#[live_register(audio_component!(IronFish))]
pub struct IronFish {
    pub settings: Arc<IronFishSettings>,
    #[rust] to_ui: ToUIReceiver<ToUI>,
    #[rust] from_ui: FromUISender<FromUI>,
}
 
impl AudioGraphNode for IronFishState{
    fn handle_midi_1_data(&mut self, data:Midi1Data){
        match data.decode(){
            Midi1Event::Note(note)  =>{
                if note.is_on{
                    self.note_on(note.note_number, note.velocity);
                }
                else{
                    self.note_off(note.note_number, note.velocity);
                }
            }
            _=>()
        }
    }
    
    fn render_to_audio_buffer(&mut self, _time: AudioTime, outputs: &mut [&mut AudioBuffer], _inputs: &[&AudioBuffer], display:&mut DisplayAudioGraph){
        self.fill_buffer(outputs[0], display)
    }
}

impl AudioComponent for IronFish {
    fn get_graph_node(&mut self, cx:&mut Cx) -> Box<dyn AudioGraphNode + Send>{
        self.from_ui.new_channel();
        let mut buffers = Vec::new();
        for i in 0..12*16{ 
            buffers.push(AudioBuffer::default());
        }
        Box::new(IronFishState{
            display_buffers: buffers,
            settings: self.settings.clone(),
            voices:Default::default(),
            to_ui: self.to_ui.sender(),
            from_ui: self.from_ui.receiver()
        })
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)){
    }
    // we dont have inputs
    fn audio_query(&mut self, _query: &AudioQuery, _callback: &mut Option<AudioQueryCb>) -> AudioResult{
        AudioResult::not_found()
    }
}


