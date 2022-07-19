
// Iron fish is MIT licensed, (C) Stijn Kuipers


#![allow(unused)]
use {
    crate::{
        audio::*,
        makepad_platform::*
    },
};


pub enum OscType {
    DPWSawPulse,
    TrivialSaw,
    BlampTri,
    Naive,
    Pure
}

#[derive(PartialEq)]
pub enum FilterType {
    Lowpass,
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

pub struct OscSettings {
    pub osc_type: OscType,
    pub transpose: i8,
    pub detune: f32
}

impl Default for OscSettings {
    fn default() -> Self {
        Self {
            osc_type: OscType::BlampTri,
            transpose: -12,
            detune: 0.0
        }
    }
}

pub struct EnvelopeSettings {
    pub predelay: f32,
    pub a: f32,
    pub h: f32,
    pub d: f32,
    pub s: f32,
    pub r: f32
}

impl Default for EnvelopeSettings {
    fn default() -> Self {
        Self {
            predelay: 0.0,
            a: 0.1,
            h: 0.0,
            d: 0.6,
            s: 0.3,
            r: 0.5
        }
    }
}

pub struct FilterSettings {
    pub cutoff: f32,
    pub resonance: f32,
    pub filter_type: FilterType,
    pub envelope: EnvelopeSettings,
    pub envelope_amount: f32,
    pub envelope_curvature: f32
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self {
            cutoff: 300.0 / 44100.0,
            resonance: 0.05,
            envelope: EnvelopeSettings::default(),
            envelope_amount: 0.1,
            envelope_curvature: 0.0,
            filter_type: FilterType::Lowpass
        }
    }
}
pub struct IronFishSettings {
    pub osc1: OscSettings,
    pub osc2: OscSettings,
    pub osc_balance: f32,
    pub filter1: FilterSettings,
    pub volume_envelope: EnvelopeSettings,
    pub sample_rate: f32
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
        
        match settings.osc_type {
            OscType::Pure => return (self.phase * 6.283).sin(),
            OscType::DPWSawPulse => return self.dpw(),
            OscType::TrivialSaw => return self.trivialsaw(),
            OscType::BlampTri => return self.blamptriangle(),
            OscType::Naive => return 0.0,
        }
    }
    
    fn set_note(&mut self, note: u8, samplerate: f32, settings: &OscSettings){
        let freq = 440.0 * f32::powf(2.0, ((note as f32) - 69.0 + settings.transpose as f32 + settings.detune) / 12.0);
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
                    if settings.h != 0.0 {
                        self.phase = EnvelopePhase::Hold;
                        self.delta_value = 0.0;
                        self.current_value = 1.0;
                        self.target_value = 1.0;
                        self.state_time = (settings.h * samplerate) as i32;
                        
                    }
                    else {
                        self.phase = EnvelopePhase::Decay;
                        self.delta_value = -(1.0 - settings.s) / (settings.d * samplerate);
                        self.current_value = 1.0;
                        self.target_value = settings.s;
                        self.state_time = (settings.d * samplerate) as i32;
                    }
                }
                
                EnvelopePhase::Hold => {
                    
                    self.phase = EnvelopePhase::Decay;
                    self.delta_value = -(1.0 - settings.s) / (settings.d * samplerate);
                    self.current_value = 1.0;
                    self.target_value = settings.s;
                    self.state_time = (settings.d * samplerate) as i32;
                }
                
                EnvelopePhase::Decay => {
                    self.phase = EnvelopePhase::Sustain;
                    self.delta_value = 0.0;
                    self.current_value = settings.s;
                    self.target_value = settings.s;
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
                    self.delta_value = (1.0 - self.current_value) / (settings.a * samplerate);
                    self.state_time = (settings.a * samplerate) as i32;
                    self.target_value = 1.0;
                }
                
                _ => {}
            }
        }
        return self.current_value;
    }
    
    fn trigger_on(&mut self, velocity: f32, settings: &EnvelopeSettings, samplerate: f32) {
        if settings.predelay != 0.0 {
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
            self.state_time = (samplerate * settings.predelay) as i32;
            return;
        };
        
        self.phase = EnvelopePhase::Attack;
        self.delta_value = (1.0 - self.current_value) / (settings.a * samplerate);
        self.state_time = (settings.a * samplerate) as i32;
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
                self.delta_value = -self.current_value / (settings.r * samplerate);
                self.state_time = (settings.r * samplerate) as i32;
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
        self.fc = (settings.cutoff + envelope * settings.envelope_amount).clamp(0.0, 1.0);
        self.damp = settings.resonance;
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
        self.volume_envelope.trigger_off(velocity, &settings.volume_envelope, settings.sample_rate);
        self.filter1.filter_envelope.trigger_off(velocity, &settings.filter1.envelope, settings.sample_rate);
    }
    
    pub fn note_on(&mut self, b1: u8, b2: u8, settings: &IronFishSettings) {
        
        let velocity = (b2 as f32) / 127.0;
        self.osc1.set_note(b1, settings.sample_rate, &settings.osc1);
        self.osc2.set_note(b1, settings.sample_rate, &settings.osc2);
        self.volume_envelope.trigger_on(velocity, &settings.volume_envelope, settings.sample_rate);
        self.filter1.filter_envelope.trigger_on(velocity, &settings.filter1.envelope, settings.sample_rate);
        self.current_note = b1 as i16;
    }
    
    pub fn one(&mut self, settings: &IronFishSettings) -> f32 {
        let osc1 = self.osc1.get(&settings.osc1, settings.sample_rate);
        let osc2 = self.osc2.get(&settings.osc2, settings.sample_rate);
        let volume_envelope = self.volume_envelope.get(&settings.volume_envelope, settings.sample_rate);
        let filter_envelope = self.filter1.filter_envelope.get(&settings.filter1.envelope, settings.sample_rate);
        
        self.filter1.set_cutoff(&settings.filter1, filter_envelope, settings.sample_rate);
        let oscinput = osc1 * settings.osc_balance + osc2 * (1.0 - settings.osc_balance);
        let filter = self.filter1.get(oscinput);
        
        let output = volume_envelope * filter;
        
        return output * 0.006; //* 1000.0;
    }
    
    pub fn fill_buffer(&mut self, buffer: &mut AudioBuffer, settings: &IronFishSettings) {
        
        // let pf = profile_start();
        let frame_count = buffer.frame_count();
        let (left, right) = buffer.stereo_mut();
        for i in 0..frame_count {
            let output = self.one(&settings) * 8.0;
            left[i] += output as f32;
            right[i] += output as f32;
        }
        
        // profile_end(pf);
    }
    
}
pub struct IronFishState {
    pub settings: Box<IronFishSettings>,
    pub voices: [IronFishVoice; 16]
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
    
    pub fn fill_buffer(&mut self, buffer: &mut AudioBuffer) {
        
        buffer.zero();
        for i in 0..self.voices.len() {
            if self.voices[i].active() > -1 {
                self.voices[i].fill_buffer(buffer, &self.settings);
            }
        }
    }
}


impl Default for IronFishState {
    fn default() -> Self {
        Self {
            settings: Box::new(IronFishSettings::default()),
            voices: [IronFishVoice::default(); 16]
        }
    }
}


impl Default for IronFishSettings {
    fn default() -> Self {
        Self {
            filter1: FilterSettings::default(),
            osc1: OscSettings::default(),
            osc2: OscSettings::default(),
            osc_balance: 0.5,
            volume_envelope: EnvelopeSettings::default(),
            sample_rate: 44100.0
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
    }
}

//enum ToUI {}
enum FromUI {}

#[derive(Live, LiveHook)]
#[live_register(audio_component_factory!(IronFish))]
struct IronFish {
    #[rust] from_ui: FromUISender<FromUI>,
}

#[derive(Default)]
struct Node {
    iron_fish_state: IronFishState,
}

impl AudioGraphNode for Node{
    fn handle_midi_1_data(&mut self, data:Midi1Data){
        match data.decode(){
            Midi1Event::Note(note)  =>{
                if note.is_on{
                    self.iron_fish_state.note_on(note.note_number, note.velocity);
                }
                else{
                    self.iron_fish_state.note_off(note.note_number, note.velocity);
                }
            }
            _=>()
        }
    }
    
    fn render_to_audio_buffer(&mut self, _time: AudioTime, outputs: &mut [&mut AudioBuffer], _inputs: &[&AudioBuffer]){
        self.iron_fish_state.fill_buffer(outputs[0])
    }
}

impl AudioComponent for IronFish {
    fn get_graph_node(&mut self) -> Box<dyn AudioGraphNode + Send>{
        self.from_ui.new_channel();
        Box::new(Node::default())
    }
    
    fn handle_event_with_fn(&mut self, _cx: &mut Cx, _event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)){
    }
}
/*
pub fn ironfish_print()
{
    return;
    
    let mut fish = IronFishState::default();
    let mut env = EnvelopeState::default();
    
    let envset = EnvelopeSettings::default();
    println!("on");
    env.trigger_on(1.0, &envset, 44100.0);
    fish.note_on(0x90, 127);
    for i in 0..100000 {
        let envres = env.get(&envset, 44100.0);
        let one = fish.one();
        if i % 2000 == 0 {
            println!("env: {} {} {}", i, envres, one);
        }
    }
    println!("off");
    env.trigger_off(1.0, &envset, 44100.0);
    for i in 0..100000 {
        let envres = env.get(&envset, 44100.0);
        let one = fish.one();
        if i % 2000 == 0 {
            println!("env: {} {} {}", i + 10000, envres, one);
        }
    }
}
fn main()
{
    
}*/

