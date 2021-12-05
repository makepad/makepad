 
pub enum GpuPerformance {
    Tier1, // quest 1
    Tier2, // quest 2
    Tier3, // intel, androids
    Tier4, // nvidia/ati/apple
    Tier5, // need a way to detect a 3090 :)
}

pub struct GpuInfo {
    pub min_uniform_vectors: u32,
    pub performance: GpuPerformance,
    pub vendor: String,
    pub renderer: String
} 

impl Default for GpuInfo{
    fn default()->Self{
        Self{// default to a nice gpu
            min_uniform_vectors:1024,
            performance: GpuPerformance::Tier4,
            vendor: "unknown".to_string(),
            renderer: "unknown".to_string()
        }
    }
}

impl GpuInfo{

    pub fn init_from_info(&mut self, min_uniform_vectors:u32, vendor:String, renderer:String){
        self.vendor = vendor;
        self.vendor.make_ascii_lowercase();
        self.renderer = renderer;
        self.renderer.make_ascii_lowercase();

        self.min_uniform_vectors = min_uniform_vectors;

        // default tier 3
        self.performance = GpuPerformance::Tier3;
        
        // extremely useless performance separation. need to make this better
        if self.vendor.contains("qualcomm") && self.renderer.contains("540"){ // its a quest 1
            self.performance = GpuPerformance::Tier1;
        }
        if self.vendor.contains("qualcomm") && self.renderer.contains("610"){ // its a quest 2
            self.performance = GpuPerformance::Tier2;
        }
        if self.vendor.contains("intel"){
            self.performance = GpuPerformance::Tier3;
        }
        if self.vendor.contains("ati"){
            self.performance = GpuPerformance::Tier4;
        }
        if self.vendor.contains("nvidia"){
            self.performance = GpuPerformance::Tier4;
        }
    }

    pub fn is_low_on_uniform_vectors(&self)->bool{
        self.min_uniform_vectors < 512
    }
    
}

