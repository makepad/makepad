use crate::from_wasm::*;
use crate::to_wasm::*;


pub struct ToWasmDataU8(Vec<u8>);

impl ToWasmDataU8{
    pub fn new_into_wasm_ptr(capacity: usize)->u32{
        let mut v = Vec::<u8>::new();
        v.reserve_exact(capacity);
        let mut v = std::mem::ManuallyDrop::new(v);
        let ptr = v.as_mut_ptr();
        let cap = v.capacity();
        if cap != capacity{panic!()};
        ptr as u32
    }
}


impl ToWasm for ToWasmDataU8 {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        let ptr = inp.read_u32();
        let len = inp.read_u32() as usize;
        unsafe {
            let ptr = ptr as *mut u8;
            Self(Vec::from_raw_parts(ptr, len, len))
        }
    }

    fn to_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str("           this.push_data_u8(");
        out.push_str(prop);
        out.push_str(");\n");
    }
    
    fn u32_size() -> usize {2}
}


pub struct WasmPtrF32(u32);

impl WasmPtrF32{
    pub fn new(ptr: *const f32)->Self{
        Self(ptr as u32)
    }
}

impl FromWasm for WasmPtrF32 {
    fn from_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str(prop);
        out.push_str(" = app.u32[this.u32_offset++];\n");
    }

    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        out.push_u32(self.0)
    }
}




pub struct WasmPtrU32(u32);

impl WasmPtrU32{
    pub fn new(ptr: *const u32)->Self{
        Self(ptr as u32)
    }
}

impl FromWasm for WasmPtrU32 {
    fn from_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str(prop);
        out.push_str(" = app.u32[this.u32_offset++];\n");
    }

    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        out.push_u32(self.0)
    }
}





impl FromWasm for String {
    fn from_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str(prop);
        out.push_str(" = this.read_str();\n");
    }
    
    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        out.push_str(self);
    }
}

impl ToWasm for String {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_string()
    }

    fn to_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str("this.push_str(");
        out.push_str(prop);
        out.push_str(");\n");
    }
    
    fn u32_size() -> usize {1}
}



impl FromWasm for bool {
    fn from_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str(prop);
        out.push_str(" = app.u32[this.u32_offset++];\n");
    }

    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        out.push_u32(if *self{1} else {0})
    }
}

impl ToWasm for bool {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_u32() != 0
    }

    fn to_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str("            app.u32[this.u32_offset++] = ");
        out.push_str(prop);
        out.push_str(";\n");
    }
    fn u32_size() -> usize {1}
}


impl FromWasm for usize {
    fn from_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str(prop);
        out.push_str(" = app.u32[this.u32_offset++];\n");
    }

    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        out.push_u32(*self as u32)
    }
}

impl ToWasm for usize {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_u32() as usize
    }

    fn to_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str("            app.u32[this.u32_offset++] = ");
        out.push_str(prop);
        out.push_str(";\n");
    }
    fn u32_size() -> usize {1}
}



impl FromWasm for u32 {
    fn from_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str(prop);
        out.push_str(" = app.u32[this.u32_offset++];\n");
    }

    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        out.push_u32(*self)
    }
}

impl ToWasm for u32 {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_u32()
    }

    fn to_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str("            app.u32[this.u32_offset++] = ");
        out.push_str(prop);
        out.push_str(";\n");
    }
    fn u32_size() -> usize {1}
}




impl FromWasm for f32 {
    fn from_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str(prop);
        out.push_str(" = app.f32[this.u32_offset++];\n");
    }

    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        out.push_f32(*self)
    }
}

impl ToWasm for f32 {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_f32()
    }
    
    fn to_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str("            app.f32[this.u32_offset++] = ");
        out.push_str(prop);
        out.push_str(";\n");
    }
    fn u32_size() -> usize {1}
}




impl FromWasm for f64 {
    fn from_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str("            this.u32_offset += this.u32_offset&1;\n");
        out.push_str(prop);
        out.push_str("            = app.f64[this.u32_offset>>1];\n");
        out.push_str("            this.u32_offset += 2;\n");
    }
    
    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        out.push_f64(*self)
    }
}

impl ToWasm for f64 {
    fn to_wasm(inp: &mut ToWasmMsg) -> Self {
        inp.read_f64()
    }

    fn to_wasm_js_body(out: &mut String, prop: &str) {
        out.push_str("            this.u32_offset += this.u32_offset&1;\n");
        out.push_str("            app.f64[this.u32_offset>>1] = ");
        out.push_str(prop);
        out.push_str(";\n");
        out.push_str("            this.u32_offset += 2;\n");
    }
    fn u32_size() -> usize {3}
}





impl<T, const N:usize> FromWasm for [T;N] where T:FromWasm{
    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        for item in self{
            item.from_wasm_inner(out);
        }
    }
    
    fn from_wasm_js_body(out: &mut String, prop: &str){
        out.push_str(prop);
        out.push_str(&format!("if({0} === undefined) {0} = [];\n", prop));
        out.push_str(&format!("
            for(let i = 0; i < {0}; i++){{
        ", N));
        T::from_wasm_js_body(out, &format!("{}[i]", prop));
        out.push_str("}");
    }
}

impl<T, const N:usize> ToWasm for [T;N] where T:ToWasm{
    fn u32_size() -> usize{T::u32_size() * N}
    
    fn to_wasm(inp: &mut ToWasmMsg) -> Self{
        unsafe{
            let mut to = std::mem::MaybeUninit::<[T; N]>::uninit();
            let top: *mut T = std::mem::transmute(&mut to);
            for i in 0..N{
                top.add(i).write(ToWasm::to_wasm(inp));
            }
            to.assume_init()
        }
    }
    
    fn to_wasm_js_body(out: &mut String, prop: &str){
        out.push_str(&format!("
                for(let i = 0; i < {0}; i++){{
        ", N));
        T::to_wasm_js_body(out, &format!("{}[i]", prop));
        out.push_str("}");
    }
}

impl<T> FromWasm for Vec<T> where T:FromWasm{
    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        out.push_u32(self.len() as u32);
        for item in self{
            item.from_wasm_inner(out);
        }
    }
    
    fn from_wasm_js_body(out: &mut String, prop: &str){
        out.push_str(prop);
        out.push_str(" = [];\n");
        out.push_str(&format!("
            {0}.length = app.u32[this.u32_offset++];
            for(let i = 0; i < {0}.length; i++){{
        ", prop));
        T::from_wasm_js_body(out, &format!("{}[i]", prop));
        out.push_str("}");
    }
}

impl<T> ToWasm for Vec<T> where T:ToWasm{
    fn u32_size() -> usize{1}
    
    fn to_wasm(inp: &mut ToWasmMsg) -> Self{
        let len = inp.read_u32();
        let mut ret = Vec::new();
        for _ in 0..len{
            ret.push(ToWasm::to_wasm(inp));
        }
        ret
    }
    
    fn to_wasm_js_body(out: &mut String, prop: &str){
        let item_size = T::u32_size();
        out.push_str(&format!("
            if(Array.isArray({0})){{
                app.u32[this.u32_offset ++] = {0}.length
                this.reserve_u32({1} * {0}.length)
                for(let i = 0; i < {0}.length; i++){{
        ", prop, item_size));
        T::to_wasm_js_body(out, &format!("{}[i]", prop));
        out.push_str("}} else {");
        out.push_str("   app.u32[this.u32_offset ++] = 0");
        out.push_str("}");

    }
}

impl<T> FromWasm for Option<T> where T:FromWasm{
    fn from_wasm_inner(&self, out: &mut FromWasmMsg) {
        if let Some(val) = self{
            out.push_u32(1);
            val.from_wasm_inner(out);
        }
        else{
            out.push_u32(0);
        }
    }
    
    fn from_wasm_js_body(out: &mut String, prop: &str){
        out.push_str("if(app.u32[this.u32_offset++] != 0){");
        T::from_wasm_js_body(out, prop);
        out.push_str("} else { ");
        out.push_str(&format!("
            {0} = undefined
        }}", prop));
    }
}

impl<T> ToWasm for Option<T> where T:ToWasm{
    fn u32_size() -> usize{1 + T::u32_size()}
    
    fn to_wasm(inp: &mut ToWasmMsg) -> Self{
        if inp.read_u32() == 0{
            None
        }
        else{
            Some(ToWasm::to_wasm(inp))
        }
    }
    
    fn to_wasm_js_body(out: &mut String, prop: &str){
        out.push_str(&format!("
            if({0} === undefined){{
                app.u32[this.u32_offset ++] = 0
            }}
            else {{
                app.u32[this.u32_offset ++] = 1
        ", prop));
        T::to_wasm_js_body(out, prop);
        out.push_str("}");
    }
}
