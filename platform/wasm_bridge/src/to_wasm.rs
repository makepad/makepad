use makepad_live_id::*;
use crate::from_wasm::*;

pub struct WasmJSOutputFn{
    pub name: String,
    pub body: String,
    pub temp: usize
}

pub struct WasmJSOutput{
    pub temp_alloc: usize,
    pub fns: Vec<WasmJSOutputFn>,
}

impl WasmJSOutput{
    pub fn alloc_temp(&mut self)->usize{
       self.temp_alloc += 1;
       self.temp_alloc 
    }
    
    pub fn check_slot(&mut self, slot:usize, is_recur:bool, prop:&str, temp:usize, name:&str)->Option<usize>{
        // ok so if we recur
        if is_recur{ // call body
            self.push_ln(slot, &format!("{}({});", name, prop));
            // check if we already have the fn
            if self.fns.iter().find(|p| p.name == name).is_some(){
                return None
            }
            self.fns.push(WasmJSOutputFn{name: name.to_string(), body:String::new(), temp});
            return Some(self.fns.len() - 1)
        }
        else{
            self.push_ln(slot, &format!("let t{} = {};", temp, prop));
            return Some(slot)
        }
    }
    
    pub fn push_ln(&mut self, slot:usize, s:&str){
        self.fns[slot].body.push_str(s);
        self.fns[slot].body.push_str("\n");
    }
}

pub trait ToWasm {
    fn u32_size() -> usize;
    
    fn type_name()->&'static str{panic!()}
    fn live_id()->LiveId{panic!()}

    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self;
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot:usize, is_recur: bool, prop:&str, temp:usize);
    
    fn to_string()->String{
        let mut wrapper = String::new();
        let id = Self::live_id();
        wrapper.push_str(&format!("{}(t0){{\n", Self::type_name()));
        wrapper.push_str("let app = this.app;\n");
        wrapper.push_str(&format!("this.reserve_u32({});\n", 4 + Self::u32_size()));
        wrapper.push_str(&format!("app.u32[this.u32_offset ++] = {};\n", id.0 & 0xffff_ffff));
        wrapper.push_str(&format!("app.u32[this.u32_offset ++] = {};\n", (id.0 >> 32)));
        wrapper.push_str("let block_len_offset = this.u32_offset ++;\n\n");
        
        let mut out = WasmJSOutput{temp_alloc:0, fns:vec![WasmJSOutputFn{name:String::new(), body:String::new(), temp:0}]};
        
        let new_temp = out.alloc_temp();
        Self::to_wasm_js_body(&mut out, 0, false, "t0", new_temp);

        for p in out.fns.iter().rev(){
            if p.name == ""{
                wrapper.push_str(&p.body);
            }
            else{
                wrapper.push_str(&format!("let {} = (t{})=>{{\n{}}}\n", p.name, p.temp, p.body))
            }
        }
        
        wrapper.push_str("if( (this.u32_offset & 1) != 0){ app.u32[this.u32_offset ++] = 0;}\n");
        wrapper.push_str("let new_len = (this.u32_offset - this.u32_ptr) >> 1;\n");
        wrapper.push_str("app.u32[block_len_offset] = new_len - app.u32[this.u32_ptr + 1];\n");
        wrapper.push_str("app.u32[this.u32_ptr + 1] = new_len;\n");
        wrapper.push_str("}\n");
        wrapper
    }
}

#[derive(Clone, Default, Debug)]
pub struct ToWasmMsg {
    data: Vec<u64>,
}

pub struct ToWasmBlockSkip{
    len:usize,
    base:usize
}

#[derive(Clone, Default, Debug)]
pub struct ToWasmMsgRef<'a> {
    data: &'a[u64],
    pub u32_offset: usize
}

impl ToWasmMsg {
    
    pub fn take_ownership(val: u32) -> Self {
        unsafe {
            let ptr = val as *mut u64;
            let head = ptr.offset(0).read();
            let len = (head >> 32) as usize;
            let cap = (head & 0xffff_ffff) as usize;
            
            Self {
                data: Vec::from_raw_parts(ptr, len, cap),
                //u32_offset: 2,
            }
        }
    }
    
    pub fn into_from_wasm(self) -> FromWasmMsg {
        FromWasmMsg {
            data: self.data,
            odd: false
        }
    }
    
    pub fn as_ref(&self) ->ToWasmMsgRef{
        ToWasmMsgRef{
            data: &self.data,
            u32_offset: 2
        }
    }
    
    pub fn as_ref_at(&self, offset:usize) ->ToWasmMsgRef{
        ToWasmMsgRef{
            data: &self.data,
            u32_offset: offset
        }
    }
}

impl<'a> ToWasmMsgRef<'a> {

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
    
    pub fn read_block_skip(&mut self)->ToWasmBlockSkip{
        ToWasmBlockSkip{
            base: self.u32_offset >> 1,
            len: self.read_u32() as usize, 
        }
    }
    
    pub fn block_skip(&mut self, block_skip:ToWasmBlockSkip){
        self.u32_offset = (block_skip.base + block_skip.len - 1)<<1
    }
    
    pub fn read_f32(&mut self) -> f32 {
        f32::from_bits(self.read_u32())
    }
    
    pub fn read_u64(&mut self) -> u64 {
        self.u32_offset += self.u32_offset & 1;
        let ret = self.data[self.u32_offset >> 1];
        self.u32_offset += 2;
        ret
    }
    
    pub fn read_f64(&mut self) -> f64 {
        f64::from_bits(self.read_u64())
    }
    
    pub fn read_string(&mut self) -> String {
        let chars = self.read_u32();
        let mut out = String::new();
        for _ in 0..chars {
            out.push(char::from_u32(self.read_u32()).unwrap_or('?'));
        }
        out
    }
    
    pub fn was_last_block(&mut self)->bool{
        self.u32_offset += self.u32_offset & 1;
        self.u32_offset>>1 >= self.data.len()
    }
}
