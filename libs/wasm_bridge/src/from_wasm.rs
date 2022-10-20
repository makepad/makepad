use makepad_live_id::*;
use crate::to_wasm::*;

pub trait FromWasm {
    fn type_name()->&'static str{panic!()}
    fn live_id()->LiveId{panic!()}

    fn write_from_wasm(self, out: &mut FromWasmMsg) where Self:Sized {
        out.push_u64(Self::live_id().0);
        let block_len_offset = out.data_len();
        out.push_u32(0);
        self.from_wasm_inner(out);
        out.or_even_u32(block_len_offset, (out.data_len() - block_len_offset + 1) as u32);
        // align it
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg);
    
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot:usize, is_recur: bool, prop:&str, temp:usize);
    
    fn from_wasm_js_reuse(wrapper: &mut String) {
        Self::from_wasm_js_inner(wrapper, true);
    }
    
    fn to_string()->String {
        let mut wrapper = String::new();
        Self::from_wasm_js_inner(&mut wrapper, false);
        wrapper
    }
    
    fn from_wasm_js_inner(wrapper:&mut String, reuse_arg:bool){
        let id = Self::live_id();

        wrapper.push_str(&format!("{}(){{\n", id.0 & 0xffff_ffff));
        wrapper.push_str("let app = this.app;\n");
        if reuse_arg{
            wrapper.push_str("let args = app.from_wasm_args;\n");
        }
        else{
            wrapper.push_str("let args = {};\n");
        }
        
        let mut out = WasmJSOutput{temp_alloc:0, fns:vec![WasmJSOutputFn{name:String::new(), body:String::new(), temp:0}]};
        let new_nest = out.alloc_temp();
        
        Self::from_wasm_js_body(&mut out, 0, false, &format!("args.{}", Self::type_name()), new_nest);
        
        for p in out.fns.iter().rev(){
            if p.name == ""{
                wrapper.push_str(&p.body);
            }
            else{
                wrapper.push_str(&format!("let {} = (t{})=>{{\n{}}}\n", p.name, p.temp, p.body))
            }
        }
        
        wrapper.push_str(&format!("app.{0}(args.{0});\n", Self::type_name()));
        wrapper.push_str("}\n");
    }
}

pub struct FromWasmMsg {
    pub(crate) data: Vec<u64>,
    pub(crate) odd: bool
}

impl FromWasmMsg {
    pub fn new() -> Self {
        Self {
            data: vec![0],
            odd: false
        }
    }
    
    pub fn reserve_u64(mut self, capacity_u64: usize) -> Self {
        self.data.reserve(capacity_u64);
        self
    }

    pub fn data_len(& self)->usize{ self.data.len() }
    
    pub fn or_even_u32(&mut self, index:usize, data:u32){
        self.data[index] |= data as u64;
    }
    
    pub fn push_u32(&mut self, data: u32) {
        if self.odd {
            let len = self.data.len();
            self.data[len - 1] |= (data as u64) << 32;
            self.odd = false
        }
        else {
            self.data.push(data as u64);
            self.odd = true;
        }
    }
    
    pub fn push_f32(&mut self, data: f32) {
        self.push_u32(data.to_bits())
    }

    pub fn push_u64(&mut self, data: u64) {
        self.odd = false;
        self.data.push(data);
    }
    
    pub fn push_f64(&mut self, data: f64) {
        self.odd = false;
        self.data.push(data.to_bits());
    }
    
    pub fn push_str(&mut self, val: &str) {
        let chars = val.chars().count();
        self.push_u32(chars as u32);
        for c in val.chars() {
            self.push_u32(c as u32);
        }
    }
    
    pub fn release_ownership(self) -> u32 {
        unsafe {
            let mut v = std::mem::ManuallyDrop::new(self.data);
            let ptr = v.as_mut_ptr();
            let len = v.len();
            let cap = v.capacity();
            
            ptr.offset(0).write((len as u64) << 32 | cap as u64);

            ptr as u32
        }
    }

    pub fn from_wasm(&mut self, from_wasm:impl FromWasm){
        from_wasm.write_from_wasm(self);
    }

}
