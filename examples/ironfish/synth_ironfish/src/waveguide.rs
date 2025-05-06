const MAX_WG_DELAY: usize = 100000;

#[derive(Clone)]
pub struct Waveguide {
    headposition: usize,
    buffer: Vec<f32>
}

impl Default for Waveguide {
    fn default() -> Self {
        Self {
            buffer:{let mut v = Vec::new(); v.resize(MAX_WG_DELAY+1,0.0);v},
            headposition: 0,
        }
    }
}

impl Waveguide {

    pub fn _clear(&mut self)
    {
        self.headposition = 0;
        for s in 0..MAX_WG_DELAY
        {
             self.buffer[s]=0.0;
        }
    }

    pub fn feed(&mut self, input: f32, feedback: f32, delaypos: f32) -> f32
    {
        let mut back:f32 = (self.headposition as f32)-delaypos;

        if back<0.0
        {
            back+= MAX_WG_DELAY as f32;
        }

        let index0 = back.floor() as usize;

        // compute interpolation right-floor
        let mut index_1=(index0 as i32)-1;
        let mut index1=index0+1;
        let mut index2=index0+2;

        // clip interp. buffer-bound
        if index_1 < 0
        {
            index_1 = (MAX_WG_DELAY as i32)-1;
        };

        if index1 >= MAX_WG_DELAY
        {
            index1 = 0
        };

        if index2 >= MAX_WG_DELAY
        {
            index2 = 0
        };

        let  y_1= self.buffer[index_1 as usize];
        let  y0 = self.buffer[index0];
        let  y1 = self.buffer[index1];
        let  y2 = self.buffer[index2];


        let  x=back-(index0 as f32);

        // calculate
        let  c0 = y0;
        let  c1 = 0.5*(y1-y_1);
        let  c2 = y_1 - 2.5*y0 + 2.0*y1 - 0.5*y2;
        let  c3 = 0.5*(y2-y_1) + 1.5*(y0-y1);

        let  output=((c3*x+c2)*x+c1)*x+c0;

        // add to delay buffer
        self.buffer[self.headposition]=input+output*feedback;

        // increment delay counter
        self.headposition = self.headposition + 1;

        // clip delay counter
        if self.headposition>=MAX_WG_DELAY
        {
                self.headposition = 0;
        }

        // return output
        return output;
    }
}