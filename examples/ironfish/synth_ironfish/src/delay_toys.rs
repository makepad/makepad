#![allow(non_snake_case)]

const MAX_DELAYTOY_BITS: usize = 16;
const MAX_DELAYTOY_DELAY: usize = 1<<MAX_DELAYTOY_BITS;
const DELAYTOY_BUFFERMASK: usize = MAX_DELAYTOY_DELAY-1;

#[derive(Clone)]
pub struct DelayToy {
    writeidx: usize,
    localidx: usize,
    accumulator: f32,
    buffer: Vec<f32>
}

impl Default for DelayToy {
    fn default() -> Self {
        Self {
            buffer:{let mut v = Vec::new(); v.resize(MAX_DELAYTOY_DELAY,0.0);v},
            writeidx: 0,
            localidx: 0,
            accumulator: 0.0
        }
    }    
}

impl DelayToy {
    pub fn _clear(&mut self) {
        self.writeidx = 0;
        self.localidx = 0;
        for s in 0..MAX_DELAYTOY_DELAY {
             self.buffer[s]=0.0;
        }
    }

    pub fn Start(&mut self) {
        self.localidx = self.writeidx;
    }

    pub fn End(&mut self){
        self.writeidx = (self.writeidx+ 1) & DELAYTOY_BUFFERMASK;
    }

    pub fn AllPass(&mut self, length: usize ){
        let j = (self.localidx + length) & DELAYTOY_BUFFERMASK;
        let d = self.buffer[j];

        self.accumulator -= d * 0.5;
        self.buffer[j] = self.Saturate(self.accumulator);
        self.accumulator =  (self.accumulator * 0.5) + d;
        self.localidx = j;
    }

    pub fn LinearInterpolate(&mut self, index: usize, offset: f32 )
    {
        let  _adjustedindex = index + MAX_DELAYTOY_DELAY - (offset.floor() as usize);
    }

    pub fn AllPassWobble(&mut self, length: usize, _lengthoffset: f32  ){
        let j = (self.localidx + length) & DELAYTOY_BUFFERMASK;
        let d = self.buffer[j];

        self.accumulator -= d * 0.5;
        self.buffer[j] = self.Saturate(self.accumulator);
        self.accumulator =  (self.accumulator * 0.5) + d;
        self.localidx = j;
    }

    pub fn Saturate(&mut self, input: f32) -> f32
    {
        if input > 1.0 {return 1.0};
        if input < -1.0 {return -1.0};
        return input;
    }
}