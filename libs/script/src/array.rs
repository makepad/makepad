use crate::value::*;
use crate::heap::*;
use crate::native::*;
use crate::makepad_live_id::*;
use crate::methods::*;
use crate::object::*;
use crate::*;

#[derive(Default)]
pub struct ScriptArrayTag(u64); 

impl ScriptArrayTag{
    pub const MARK:u64 = 0x1<<40;
    pub const ALLOCED:u64 = 0x2<<40;
    pub const DIRTY: u64 = 0x40<<40;
    pub const FROZEN: u64 = 0x100<<40;
    
    pub fn is_alloced(&self)->bool{
        return self.0 & Self::ALLOCED != 0
    }
    
    pub fn set_alloced(&mut self){
        self.0 |= Self::ALLOCED
    }
    
    pub fn clear(&mut self){
        self.0 = 0;
    }
    
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
            
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
            
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
    
    pub fn freeze(&mut self){
        self.0  |= Self::FROZEN
    }
    
    pub fn is_frozen(&self)->bool{
        self.0 & Self::FROZEN != 0
    }
    
    pub fn set_dirty(&mut self){
        self.0  |= Self::DIRTY
    }
            
    pub fn check_and_clear_dirty(&mut self)->bool{
        if self.0 & Self::DIRTY !=  0{
            self.0 &= !Self::DIRTY;
            true
        }
        else{
            false
        }
    }
}

#[derive(PartialEq)]
pub enum ScriptArrayStorage{
    ScriptValue(Vec<ScriptValue>),
    F32(Vec<f32>),
    U32(Vec<u32>),
    U16(Vec<u16>),
    U8(Vec<u8>),
}

impl ScriptArrayStorage{
    pub fn clear(&mut self){
        match self{
            Self::ScriptValue(v)=>v.clear(),
            Self::F32(v)=>v.clear(),
            Self::U32(v)=>v.clear(),
            Self::U16(v)=>v.clear(),
            Self::U8(v)=>v.clear(),
        }
    }
    pub fn len(&self)->usize{
        match self{
            Self::ScriptValue(v)=>v.len(),
            Self::F32(v)=>v.len(),
            Self::U32(v)=>v.len(),
            Self::U16(v)=>v.len(),
            Self::U8(v)=>v.len(),
        }
    }
    pub fn index(&self, index:usize)->Option<ScriptValue>{
        match self{
            Self::ScriptValue(v)=>if let Some(v) = v.get(index){(*v).into()} else {None},
            Self::F32(v)=>if let Some(v) = v.get(index){Some((*v).into())} else {None},
            Self::U32(v)=>if let Some(v) = v.get(index){Some((*v).into())} else {None},
            Self::U16(v)=>if let Some(v) = v.get(index){Some((*v).into())} else {None},
            Self::U8(v)=>if let Some(v) = v.get(index){Some((*v).into())} else {None},
        }
    }
    pub fn set_index(&mut self, index:usize, value:ScriptValue){
        match self{
            Self::ScriptValue(v)=>{if index>=v.len(){v.resize(index+1, NIL);}v[index] = value;},
            Self::F32(v)=>{if index>=v.len(){v.resize(index+1, 0.0);}v[index] = value.as_f64().unwrap_or(0.0) as f32;},
            Self::U32(v)=>{if index>=v.len(){v.resize(index+1, 0);}v[index] = value.as_f64().unwrap_or(0.0) as u32;},
            Self::U16(v)=>{if index>=v.len(){v.resize(index+1, 0);}v[index] = value.as_f64().unwrap_or(0.0) as u16;},
            Self::U8(v)=>{if index>=v.len(){v.resize(index+1, 0);}v[index] = value.as_f64().unwrap_or(0.0) as u8;},
        }
    }
    pub fn push(&mut self, value:ScriptValue){
        match self{
            Self::ScriptValue(v)=>v.push(value),
            Self::F32(v)=>v.push(value.as_f64().unwrap_or(0.0) as f32),
            Self::U32(v)=>v.push(value.as_f64().unwrap_or(0.0) as u32),
            Self::U16(v)=>v.push(value.as_f64().unwrap_or(0.0) as u16),
            Self::U8(v)=>v.push(value.as_f64().unwrap_or(0.0) as u8),
        }
    }
    pub fn push_vec(&mut self, vec:&[ScriptVecValue]){
        match self{
            Self::ScriptValue(v)=>for a in vec{v.push(a.value)},
            Self::F32(v)=>for a in vec{v.push(a.value.as_f64().unwrap_or(0.0) as f32)},
            Self::U32(v)=>for a in vec{v.push(a.value.as_f64().unwrap_or(0.0) as u32)},
            Self::U16(v)=>for a in vec{v.push(a.value.as_f64().unwrap_or(0.0) as u16)},
            Self::U8(v)=>for a in vec{v.push(a.value.as_f64().unwrap_or(0.0) as u8)},
        }
    }
    pub fn pop(&mut self)->Option<ScriptValue>{
        match self{
            Self::ScriptValue(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
            Self::F32(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
            Self::U32(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
            Self::U16(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
            Self::U8(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
        }
    }
    pub fn remove(&mut self, index:usize)->ScriptValue{
        match self{
            Self::ScriptValue(v)=>v.remove(index),
            Self::F32(v)=>v.remove(index).into(),
            Self::U32(v)=>v.remove(index).into(),
            Self::U16(v)=>v.remove(index).into(),
            Self::U8(v)=>v.remove(index).into(),
        }
    }
    pub fn to_string(&self, heap:&ScriptHeap, s:&mut String){
        match self{
            Self::U8(bytes)=>{
                let v = String::from_utf8_lossy(bytes);
                s.push_str(v.as_ref());
            }
            Self::ScriptValue(vec)=>{
                for v in vec{
                    heap.cast_to_string(*v, s);
                }
            },
            Self::F32(v)=>{
                for v in v {
                    if let Some(c) = std::char::from_u32(*v as _){
                        s.push(c)
                    }
                }
            },
            Self::U32(v)=>{
                for v in v {
                    if let Some(c) = std::char::from_u32(*v){
                        s.push(c)
                    }
                }
            },
            Self::U16(v)=>{
                for v in v {
                    if let Some(c) = std::char::from_u32(*v as _){
                        s.push(c)
                    }
                }
            }
        }
    }
}

pub struct ScriptArrayData{
    pub tag: ScriptArrayTag,
    pub storage: ScriptArrayStorage
}

impl Default for ScriptArrayData{
    fn default()->Self{
        Self{
            tag: ScriptArrayTag::default(),
            storage: ScriptArrayStorage::ScriptValue(vec![])
        }
    }
}

impl ScriptArrayData{
        
    pub fn add_type_methods(tm: &mut ScriptTypeMethods, h: &mut ScriptHeap, native:&mut ScriptNative){
        tm.add(h, native, &[], ScriptValueType::REDUX_ARRAY, id!(string), |vm, args|{
            if let Some(arr) = script_value!(vm, args.this).as_array(){
                return vm.heap.new_string_with(|heap, s|{
                    heap.array_ref(arr).to_string(heap, s);
                }).into();
            }
            vm.thread.trap.err_unexpected()
        });
        
        tm.add(h, native, &[], ScriptValueType::REDUX_STRING, id!(read_json), |vm, args|{
            if let Some(arr) = script_value!(vm, args.this).as_array(){
                let mut s = String::new();
                std::mem::swap(&mut s, &mut vm.thread.json_parser.temp_string);
                s.clear();
                let array_ref = vm.heap.array_ref(arr);
                array_ref.to_string(vm.heap, &mut s);
                let r = vm.thread.json_parser.read_json(&s, vm.heap);
                std::mem::swap(&mut s, &mut vm.thread.json_parser.temp_string);
                return r
            }
            vm.thread.trap.err_unexpected()
        });
                
        tm.add(h, native, &[], ScriptValueType::REDUX_ARRAY, id!(push), |vm, args|{
            if let Some(this) = script_value!(vm, args.this).as_array(){
                vm.heap.array_push_vec(this, args, &vm.thread.trap);
                return NIL
            }
            vm.thread.trap.err_unexpected()
        });
                        
        tm.add(h, native, &[], ScriptValueType::REDUX_ARRAY, id!(pop), |vm, args|{
            if let Some(this) = script_value!(vm, args.this).as_array(){
                return vm.heap.array_pop(this, &mut vm.thread.trap)
            }
            vm.thread.trap.err_unexpected()
        });
                        
        tm.add(h, native, &[], ScriptValueType::REDUX_ARRAY, id!(len), |vm, args|{
            if let Some(this) = script_value!(vm, args.this).as_array(){
                return vm.heap.array_len(this).into()
            }
            vm.thread.trap.err_unexpected()
        });
                
        tm.add(h, native, &[], ScriptValueType::REDUX_ARRAY, id!(freeze), |vm, args|{
            if let Some(this) = script_value!(vm, args.this).as_array(){
                vm.heap.freeze_array(this);
                return this.into()
            }
            vm.thread.trap.err_unexpected()
        });
                        
        tm.add(h, native, script_args!(cb=NIL), ScriptValueType::REDUX_ARRAY, id!(retain), |vm, args|{
            if let Some(this) = script_value!(vm, args.this).as_array(){
                let fnptr = script_value!(vm, args.cb);
                let mut i = 0;
                while i < vm.heap.array_len(this){
                    let value = script_array_index!(vm, this[i]);
                    let ret = vm.call(fnptr, &[value]);
                    if ret.is_err(){
                        return ret;
                    }
                    if !vm.heap.cast_to_bool(ret){
                        vm.heap.array_remove(this, i, &mut vm.thread.trap);
                    }
                    else{
                        i += 1
                    }
                }
                return NIL
            }
            vm.thread.trap.err_not_impl()
        });
        
        
    }
    
    pub fn clear(&mut self){
        self.storage.clear();
        self.tag.clear()
    }
    
    pub fn is_value_array(&self)->bool{
        if let ScriptArrayStorage::ScriptValue(_) = &self.storage{
            true
        }
        else{
            false
        }
    }
}