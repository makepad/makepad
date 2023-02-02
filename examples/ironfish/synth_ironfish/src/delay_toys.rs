
const MAX_DELAYTOY_BITS: usize = 16;
const MAX_DELAYTOY_DELAY: usize = 1<<MAX_DELAYTOY_BITS;
const DELAYTOY_BUFFERMASK: usize = MAX_DELAYTOY_DELAY-1;

#[derive(Clone)]
pub struct DelayToy {
    writeidx: usize,
    localidx: usize,
    accumulator: f32,
    _feedback1: f32,
    buffer: Vec<f32>
}

impl Default for DelayToy {
    fn default() -> Self {
        Self {
            buffer:{let mut v = Vec::new(); v.resize(MAX_DELAYTOY_DELAY,0.0);v},
            writeidx: 0,
            localidx: 0,
            accumulator: 0.0,
            _feedback1: 0.0
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

    pub fn start(&mut self) {
        self.localidx = self.writeidx;
    }

    pub fn end(&mut self){
        self.writeidx = (self.writeidx+ 1) & DELAYTOY_BUFFERMASK;
    }

    pub fn all_pass(&mut self, length: usize, coeff: f32 ){
        let j = (self.localidx + length) & DELAYTOY_BUFFERMASK;
        let d = self.buffer[j];
        self.accumulator -= d * coeff;
        self.buffer[j] = self.saturate(self.accumulator);
        self.accumulator =  (self.accumulator * coeff) + d;
        self.localidx = j;
    }

    pub fn linear_interpolate(&mut self, index: usize, offset: f32 ) -> f32
    {
        let  adjustedindex = (index + MAX_DELAYTOY_DELAY - (offset.floor() as usize))&DELAYTOY_BUFFERMASK;
        let frac = offset.fract();
        let ifrac = 1.0 - frac;
        return self.buffer[adjustedindex] * ifrac + self.buffer[(adjustedindex+1)&DELAYTOY_BUFFERMASK] * frac;
    }

    pub fn all_pass_wobble(&mut self, length: usize, coeff: f32, _lengthoffset: f32  ){
        let j = (self.localidx + length) & DELAYTOY_BUFFERMASK;
        let d = self.buffer[j];

        self.accumulator -= d * coeff;
        self.buffer[j] = self.saturate(self.accumulator);
        self.accumulator =  (self.accumulator * coeff) + d;
        self.localidx = j;
    }

/*
    #define DELAY(len) { \
		int j = (localidx + len) & WHITERABBIT_BUFFERMASK; \
		Buffer[localidx] = Saturate(acc); \
		acc = Buffer[j]; \
		localidx = j; \
	}
    #define DELAY_WOBBLE(len, wobpos) { \
		int j = (localidx + len) & WHITERABBIT_BUFFERMASK; \
		Buffer[localidx] = Saturate(acc); \
		acc = LINEARINTERPRV16(Buffer, j, wobpos); \
		localidx=j; \
	}
*/
  
    pub fn delay(&mut self, length: usize){
        let j = (self.localidx + length) & DELAYTOY_BUFFERMASK;
        self.buffer[self.localidx] = self.saturate(self.accumulator);
        self.accumulator = self.buffer[j];
        self.localidx = j;
    }


    pub fn saturate(&mut self, input: f32) -> f32{
        if input > 1.0 {return 1.0};
        if input < -1.0 {return -1.0};
        return input;
    }
/*
    pub fn griesingerReverb(&mut self, mut left: f32, mut right: f32, send: f32){       
        let mut leftOut: f32 = left;      
        let mut rightOut: f32 = right;
        self.start();
        self.accumulator = (left + right) * send;
        self.allPass(142, 0.5);
        self.allPass(379, 0.5);
        self.accumulator += (left + right) * send;
        self.allPass(107, 0.5);
        self.allPass(277, 0.5);
        let reinject = self.accumulator;
        self.accumulator += self.feedback1;
		
       // self.allPassWobble(672, 0.5, wobble1);
        self.allPass(1800, 0.5);
        self.delay(4453);


        self.end();

        right = rightOut;
        left = leftOut;
       
    }*/
}