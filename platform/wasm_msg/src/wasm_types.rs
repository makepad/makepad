use makepad_live_id::*;

#[export_name = "new_wasm_msg_with_u64_capacity"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn new_wasm_msg_with_u64_capacity(capacity_u64: u32) -> u32 {
    FromWasmMsg::new().reserve_u64(capacity_u64 as usize).into_wasm_ptr()
}

#[export_name = "wasm_msg_reserve_u64"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_msg_reserve_u64(ptr: u32, capacity_u64: u32) -> u32 {
    ToWasmMsg::from_wasm_ptr(ptr).into_from_wasm().reserve_u64(capacity_u64 as usize).into_wasm_ptr()
}

#[export_name = "wasm_msg_free"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_msg_free(ptr: u32) {
    ToWasmMsg::from_wasm_ptr(ptr);
}

extern "C" {
    pub fn _console_log(chars: u32, len: u32);
}

pub fn console_log(val: &str) {
    unsafe {
        let chars = val.chars().collect::<Vec<char >> ();
        _console_log(chars.as_ptr() as u32, chars.len() as u32);
    }
}


pub trait FromWasm {
    fn from_wasm(&self, out: &mut FromWasmMsg);
}

pub trait ToWasm {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self;
    fn codegen_js_body(out:&mut String, prop:&str);
    fn u32_size()->usize;
    
    fn codegen_js_method(out:&mut String, name:&str){
        let id = LiveId::from_str(name).unwrap();
        out.push_str(&format!("
            {}(obj){{
                let app = this.app;
                this.reserve_u32({});
                app.u32[this.u32_offset ++] = {};
                app.u32[this.u32_offset ++] = {};
                let block_len_offset = this.u32_offset ++;
        \n", name, 3 + Self::u32_size(), id.0&0xffff_ffff, (id.0>>32)));
        
        Self::codegen_js_body(out, "obj");
        
        out.push_str("
                if( this.u32_offset & 1 != 0){ app.u32[this.u32_offset ++] = 0;}
                let new_len = (this.u32_offset - this.u32_ptr) >> 1;
                app.u32[block_len_offset] = new_len - app.u32[this.u32_ptr + 1];
                app.u32[this.u32_ptr + 1] = new_len;
            }
        ");
    }
}

impl FromWasm for String {
    fn from_wasm(&self, out: &mut FromWasmMsg) {
        out.push_str(self);
    }
}

impl ToWasm for String {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_string()
    }
    
    fn codegen_js_body(out:&mut String, prop:&str){
        out.push_str("this.push_str(");
        out.push_str(prop);
        out.push_str(");\n");
    }
    
    fn u32_size()->usize{1}
}

impl FromWasm for u32 {
    fn from_wasm(&self, out: &mut FromWasmMsg) {
        out.push_u32(*self)
    }
}

impl FromWasm for f32 {
    fn from_wasm(&self, out: &mut FromWasmMsg) {
        out.push_f32(*self)
    }
}

impl FromWasm for f64 {
    fn from_wasm(&self, out: &mut FromWasmMsg) {
        out.push_f64(*self)
    }
}

impl ToWasm for u32 {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_u32()
    }
    
    fn codegen_js_body(out:&mut String, prop:&str){
        out.push_str("            app.u32[this.u32_offset++] = ");
        out.push_str(prop);
        out.push_str(";\n");
    }
    fn u32_size()->usize{1}
}

impl ToWasm for f32 {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_f32()
    }

    fn codegen_js_body(out:&mut String, prop:&str){
        out.push_str("            app.f32[this.u32_offset++] = ");
        out.push_str(prop);
        out.push_str(";\n");
    }
    fn u32_size()->usize{1}
}

impl ToWasm for f64 {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_f64()
    }

    fn codegen_js_body(out:&mut String, prop:&str){
        out.push_str("            this.u32_offset += this.u32_offset&1;\n");
        out.push_str("            app.f64[this.u32_offset>>1] = ");
        out.push_str(prop);
        out.push_str(";\n");
        out.push_str("            this.u32_offset += 2;\n");
    }
    fn u32_size()->usize{3}
}

pub struct ToWasmMsg {
    data: Vec<u64>,
    pub u32_offset: usize
}

pub struct FromWasmMsg {
    data: Vec<u64>,
    odd: bool
}

impl ToWasmMsg {
    
    pub fn from_wasm_ptr(val: u32) -> Self {
        unsafe {
            let ptr = val as *mut u64;
            let head = ptr.offset(0).read();
            let len =  (head>>32) as usize;
            let cap =  (head&0xffff_ffff) as usize;
            Self {
                data: Vec::from_raw_parts(ptr, len, cap),
                u32_offset: 2,
            }
        }
    }
    
    pub fn into_from_wasm(self) -> FromWasmMsg {
        FromWasmMsg {
            data: self.data,
            odd: false
        }
    }
    
    pub fn read_u32(&mut self) -> u32 {
        let ret = if self.u32_offset & 1 != 0 {
            (self.data[self.u32_offset >> 1] >> 32) as u32
        }
        else {
            (self.data[self.u32_offset >> 1] & 0xffff_ffff) as u32
        };
        self.u32_offset += 1;
        ret
    }
    
    pub fn read_f32(&mut self) -> f32 {
        f32::from_bits(self.read_u32())
    }

    pub fn read_u64(&mut self) -> u64 {
        self.u32_offset += self.u32_offset&1;
        let ret = self.data[self.u32_offset >> 1];
        self.u32_offset += 2;
        ret
    }    
    
    pub fn read_f64(&mut self) -> f64 {
        f64::from_bits(self.read_u64())
    }
    
    pub fn read_string(&mut self)->String{
        let chars = self.read_u32();
        let mut out = String::new();
        for _ in 0..chars {
            out.push(char::from_u32(self.read_u32()).unwrap_or('?'));
        }
        out
    }
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
    
    pub fn push_f64(&mut self, data: f64) {
        self.odd = false;
        self.data.push(data.to_bits());
    }
    
    pub fn push_str(&mut self, val:&str){
        let chars = val.chars().count();
        self.push_u32(chars as u32);
        for c in val.chars() {
            self.push_u32(c as u32);
        }
    }
    
    pub fn into_wasm_ptr(self) -> u32 {
        unsafe {
            let mut v = std::mem::ManuallyDrop::new(self.data);
            let ptr = v.as_mut_ptr();
            let len = v.len();
            let cap = v.capacity();

            ptr.offset(0).write((len as u64) << 32 | cap as u64);
            ptr as u32
        }
    }
}
