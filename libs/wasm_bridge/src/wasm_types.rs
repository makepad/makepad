use crate::from_wasm::*;
use crate::to_wasm::*;
use crate::{LiveId,live_id};
use makepad_derive_wasm_bridge::*;
pub struct WasmDataU8(Vec<u8>);

impl WasmDataU8 {
    pub fn new_and_release_ownership(capacity: usize) -> u32 {
        let mut v = Vec::<u8>::new();
        v.reserve_exact(capacity);
        let mut v = std::mem::ManuallyDrop::new(v);
        let ptr = v.as_mut_ptr();
        let cap = v.capacity();
        if cap != capacity {panic!()};
        ptr as u32
    }
    
    pub fn take_ownership(ptr:u32, len:u32, cap:u32)->Self{
        unsafe {
            Self(Vec::from_raw_parts(ptr as *mut u8, len as usize, cap as usize))
        }
    }
    
    pub fn from_vec_u8(v:Vec<u8>)->Self{
        Self(v)
    }
    
    pub fn into_vec_u8(self)->Vec<u8>{
        self.0
    }
}

impl ToWasm for WasmDataU8 {
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        
        let ptr = inp.read_u32();
        let len = inp.read_u32() as usize;
        unsafe {
            Self (Vec::from_raw_parts(ptr as *mut u8, len, len))
        }
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("this.push_data_u8({});", prop));
    }
    
    fn u32_size() -> usize {2}
}

impl FromWasm for WasmDataU8 {
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("{} = {{ptr:app.u32[this.u32_offset++],len:app.u32[this.u32_offset++],capacity:app.u32[this.u32_offset++]}};", prop));
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        let mut v = std::mem::ManuallyDrop::new(self.0);
        out.push_u32(v.as_mut_ptr() as u32);
        out.push_u32(v.len() as u32);
        out.push_u32(v.capacity() as u32);
    }
}
/*
impl FromWasm for WasmDataU8 {
    fn write_from_wasm(&self, out: &mut FromWasmMsg) {
        let swap = Vec::new();
        
        let mut v = std::mem::ManuallyDrop::new(v);
        let ptr = v.as_mut_ptr();
        let cap = v.capacity();
        
        out.push_u32()
        let ptr = inp.read_u32();
        let len = inp.read_u32() as usize;
        unsafe {
            Self (Vec::from_raw_parts(ptr as *mut u8, len, len))
        }
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("this.push_data_u8({});", prop));
    }
    
    fn u32_size() -> usize {2}
}*/

pub struct WasmPtrF32(u32);

impl WasmPtrF32 {
    pub fn new(ptr: *const f32) -> Self {
        Self (ptr as u32)
    }
}

impl FromWasm for WasmPtrF32 {
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("{} = app.u32[this.u32_offset++];", prop));
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_u32(self.0)
    }
}


#[derive(FromWasm)]
pub struct WasmDataF32 {
    pub ptr: WasmPtrF32,
    pub len: usize
}

impl WasmDataF32 {
    pub fn new(data:&[f32]) -> Self {
        if data.len() != 0{
            Self{ptr:WasmPtrF32::new(data.as_ptr()), len:data.len()}
        }
        else{
            Self{ptr:WasmPtrF32::new(0 as *const f32), len:0}
        }
    }
}

#[derive(FromWasm)]
pub struct WasmDataU32 {
    pub ptr: WasmPtrU32,
    pub len: usize
}

impl WasmDataU32 {
    pub fn new(data:&[u32]) -> Self {
        if data.len() != 0{
            Self{ptr:WasmPtrU32::new(data.as_ptr()), len:data.len()}
        }
        else{
            Self{ptr:WasmPtrU32::new(0 as *const u32), len:0}
        }
    }
}


pub struct WasmPtrU32(u32);

impl WasmPtrU32 {
    pub fn new(ptr: *const u32) -> Self {
        Self (ptr as u32)
    }
}

impl FromWasm for WasmPtrU32 {
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("{} = app.u32[this.u32_offset++];", prop));
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_u32(self.0)
    }
}





impl FromWasm for String {
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("{} = this.read_str();", prop));
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_str(&self);
    }
}

impl FromWasm for &str {
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("{} = this.read_str();", prop));
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_str(&self);
    }
}

impl ToWasm for String {
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        inp.read_string()
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("this.push_str({});", prop));
    }
    
    fn u32_size() -> usize {1}
}



impl FromWasm for bool {
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("{} = app.u32[this.u32_offset++]!==0?true:false;", prop));
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_u32(if self {1} else {0})
    }
}

impl ToWasm for bool {
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        inp.read_u32() != 0
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("app.u32[this.u32_offset++] = {};", prop));
    }
    fn u32_size() -> usize {1}
}


impl FromWasm for usize {
    
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("{} = app.u32[this.u32_offset++];", prop));
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_u32(self as u32)
    }
}

impl ToWasm for usize {
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        inp.read_u32() as usize
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("app.u32[this.u32_offset++] = {};", prop));
    }
    fn u32_size() -> usize {1}
}



impl FromWasm for u32 {
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("{} = app.u32[this.u32_offset++];", prop));
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_u32(self)
    }
}

impl ToWasm for u32 {
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        inp.read_u32()
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("app.u32[this.u32_offset++] = {};", prop));
    }
    fn u32_size() -> usize {1}
}




impl FromWasm for f32 {
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("{} = app.f32[this.u32_offset++];", prop));
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_f32(self)
    }
}

impl ToWasm for f32 {
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        inp.read_f32()
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, &format!("app.f32[this.u32_offset++] = {};", prop));
    }
    fn u32_size() -> usize {1}
}




impl FromWasm for f64 {
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, "this.u32_offset += this.u32_offset&1;");
        out.push_ln(slot, &format!("{} = app.f64[this.u32_offset>>1];", prop));
        out.push_ln(slot, "this.u32_offset += 2;");
    }
    
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_f64(self)
    }
}

impl ToWasm for f64 {
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        inp.read_f64()
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, "this.u32_offset += this.u32_offset&1;");
        out.push_ln(slot, &format!("app.f64[this.u32_offset>>1] = {};", prop));
        out.push_ln(slot, "this.u32_offset += 2;");
    }
    
    fn u32_size() -> usize {3}
}





impl<T, const N: usize> FromWasm for [T; N] where T: FromWasm {
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        for item in self {
            item.from_wasm_inner(out);
        }
    }
    
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, is_recur: bool, prop: &str, temp: usize) {
        out.push_ln(slot, &format!("if({0} === undefined) {0} = [];", prop));
        out.push_ln(slot, &format!("let t{} = {};", temp, prop));
        out.push_ln(slot, &format!("for(let i{0} = 0; i{0} < {1}; i{0}++){{", temp, N));
        let new_temp = out.alloc_temp();
        T::from_wasm_js_body(out, slot, is_recur, &format!("t{0}[i{0}]", temp), new_temp);
        out.push_ln(slot, "}");
    }
}

impl<T, const N: usize> ToWasm for [T; N] where T: ToWasm {
    fn u32_size() -> usize {T::u32_size() * N}
    
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        unsafe {
            let mut to = std::mem::MaybeUninit::<[T; N]>::uninit();
            let top: *mut T = std::mem::transmute(&mut to);
            for i in 0..N {
                top.add(i).write(ToWasm::read_to_wasm(inp));
            }
            to.assume_init()
        }
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, is_recur: bool, prop: &str, temp: usize) {
        out.push_ln(slot, &format!("let t{} = {}", temp, prop));
        out.push_ln(slot, &format!("for(let i{0} = 0; i{0} < {1}; i{0}++){{", temp, N));
        let new_temp = out.alloc_temp();
        T::to_wasm_js_body(out, slot, is_recur, &format!("t{0}[i{0}]", temp), new_temp);
        out.push_ln(slot, "}");
    }
}

impl<T> FromWasm for Vec<T> where T: FromWasm {
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        out.push_u32(self.len() as u32);
        for item in self {
            item.from_wasm_inner(out);
        }
    }
    
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, temp: usize) {
        out.push_ln(slot, &format!("let t{} = {} = [];", temp, prop));
        out.push_ln(slot, &format!("t{}.length = app.u32[this.u32_offset++];", temp));
        out.push_ln(slot, &format!("for(let i{0} = 0; i{0} < t{0}.length; i{0}++){{", temp));
        let new_temp = out.alloc_temp();
        T::from_wasm_js_body(out, slot, true, &format!("t{0}[i{0}]", temp), new_temp);
        out.push_ln(slot, "}");
    }
}

impl<T> ToWasm for Vec<T> where T: ToWasm {
    fn u32_size() -> usize {1}
    
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        let len = inp.read_u32();
        let mut ret = Vec::new();
        for _ in 0..len {
            ret.push(ToWasm::read_to_wasm(inp));
        }
        ret
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, temp: usize) {
        let item_size = T::u32_size();
        
        out.push_ln(slot, &format!("let t{} = {};", temp, prop));
        out.push_ln(slot, &format!("if(Array.isArray(t{})){{", temp));
        out.push_ln(slot, &format!("app.u32[this.u32_offset ++] = t{}.length;", temp));
        out.push_ln(slot, &format!("this.reserve_u32({} * t{}.length);", item_size, temp));
        out.push_ln(slot, &format!("for(let i{0} = 0; i{0} < t{0}.length; i{0}++){{", temp));
        let new_temp = out.alloc_temp();
        T::to_wasm_js_body(out, slot, true, &format!("t{0}[i{0}]", temp), new_temp);
        out.push_ln(slot, "}} else {");
        out.push_ln(slot, "   app.u32[this.u32_offset ++] = 0;");
        out.push_ln(slot, "}");
        
    }
}

impl<T> FromWasm for Box<T> where T: FromWasm {
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        (*self).from_wasm_inner(out);
    }
    
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, _temp: usize) {
        let new_temp = out.alloc_temp();
        T::from_wasm_js_body(out, slot, true, prop, new_temp);
    }
}

impl<T> ToWasm for Box<T> where T: ToWasm {
    fn u32_size() -> usize {0}
    
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        Self::new(ToWasm::read_to_wasm(inp))

    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, _is_recur: bool, prop: &str, temp: usize) {
        let item_size = T::u32_size();
        out.push_ln(slot, &format!("this.reserve_u32({});", item_size));
        T::to_wasm_js_body(out, slot, true, prop, temp);
    }
}

impl<T> FromWasm for Option<T> where T: FromWasm {
    fn from_wasm_inner(self, out: &mut FromWasmMsg) {
        if let Some(val) = self {
            out.push_u32(1);
            val.from_wasm_inner(out);
        }
        else {
            out.push_u32(0);
        }
    }
    
    fn from_wasm_js_body(out: &mut WasmJSOutput, slot: usize, is_recur: bool, prop: &str, _temp: usize) {
        out.push_ln(slot, "if(app.u32[this.u32_offset++] !== 0){");
        let new_temp = out.alloc_temp();
        T::from_wasm_js_body(out, slot, is_recur, prop, new_temp);
        out.push_ln(slot, "} else {");
        out.push_ln(slot, &format!("{} = undefined;", prop));
        out.push_ln(slot, "}");
    }
}

impl<T> ToWasm for Option<T> where T: ToWasm {
    fn u32_size() -> usize {1 + T::u32_size()}
    
    fn read_to_wasm(inp: &mut ToWasmMsgRef) -> Self {
        if inp.read_u32() == 0 {
            None
        }
        else {
            Some(ToWasm::read_to_wasm(inp))
        }
    }
    
    fn to_wasm_js_body(out: &mut WasmJSOutput, slot: usize, is_recur: bool, prop: &str, temp: usize) {
        out.push_ln(slot, &format!("if({0} === undefined){{", prop));
        out.push_ln(slot, "app.u32[this.u32_offset ++] = 0;");
        out.push_ln(slot, "} else {");
        out.push_ln(slot, "app.u32[this.u32_offset ++] = 1;");
        T::to_wasm_js_body(out, slot, is_recur, prop, temp);
        out.push_ln(slot, "}");
    }
}


