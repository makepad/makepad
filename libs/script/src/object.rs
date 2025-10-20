use std::fmt;
use crate::value::*;
use crate::value_map::*;

#[derive(Default)]
pub struct ObjectTag(u64); 

#[derive(Copy,Clone,Eq,PartialEq, Ord, PartialOrd)]
pub struct ObjectType(u8);

impl fmt::Debug for ObjectType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for ObjectType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self{
            Self::AUTO=>write!(f, "AUTO"),
            Self::VEC2=>write!(f, "VEC2"),
            Self::MAP=>write!(f, "MAP"),
            Self::VEC1=>write!(f, "VEC1"),
            Self::U8=>write!(f, "U8"),
            Self::U16=>write!(f, "U16"),
            Self::U32=>write!(f, "U32"),
            Self::I8=>write!(f, "I8"),
            Self::I16=>write!(f, "I16"),
            Self::I32=>write!(f, "I32"),
            Self::F32=>write!(f, "F32"),
            Self::F64=>write!(f, "F64"),
            _=>write!(f, "?ObjectType"),
        }
    }
}

impl ObjectType{
    pub const AUTO: Self = Self(0);
    pub const VEC2: Self = Self(1);
    pub const MAP: Self = Self(2);
    pub const VEC1:Self = Self(3);
        
    pub fn uses_vec2(&self)->bool{
        *self <= Self::VEC2
    }
        
    pub fn is_auto(&self)->bool{
        *self == Self::AUTO
    }
        
    pub fn is_vec1(&self)->bool{
        *self == Self::VEC1
    }
            
    pub fn is_vec2(&self)->bool{
        *self == Self::VEC2
    }
        
    pub fn is_map(&self)->bool{
        *self == Self::MAP
    }
            
    pub fn has_paired_vec(&self)->bool{
        return self.0 <= 2
    }
                    
    pub fn is_gc(&self)->bool{
        return self.0 <= 3
    }
        
    pub fn is_typed(&self)->bool{
        return self.0 >= 4
    }
        
    pub const U8: Self = Self(4);
    pub const U16: Self = Self(5);
    pub const U32: Self = Self(6);
    pub const I8: Self = Self(7);
    pub const I16: Self = Self(8);
    pub const I32: Self = Self(9);
    pub const F32: Self = Self(10);
    pub const F64: Self = Self(11);
    // cant really use these
}

pub struct RustRef(u64);

#[derive(Debug,Clone,Copy)]
pub struct NativeId{
    pub index: u32
}

#[derive(Debug,Clone,Copy)]
pub enum ScriptFnPtr{
    Script(ScriptIp),
    Native(NativeId)
}

impl ObjectTag{
    // marked in the mark-sweep gc
    pub const MARK:u64 = 0x40;
    // object is not 'free'
    pub const ALLOCED:u64 = 0x80;
    // object is 'deep' aka writes to protochain
    pub const DEEP:u64 = 0x100;
    // used to mark objects dirty for Rust deserialisers
    pub const DIRTY:u64 = 0x200;
    // used to quick-free objects if not set
    pub const REFFED: u64 = 0x400;
    // object is skipped in gc passes
    pub const STATIC: u64 = 0x800;
    
    // marks object readonly
    pub const FROZEN: u64 = 0x1000;
    // for readonly allow writes if checked passes
    pub const VALIDATED: u64 = 0x2000;
    // for read only allow writes only if map item doesnt exist
    pub const MAP_ADD: u64 = 0x4000;
    // vec is frozen
    pub const VEC_FROZEN: u64 = 0x8000;
    
    pub const FREEZE_MASK: u64 = Self::FROZEN|Self::VALIDATED|Self::MAP_ADD|Self::VEC_FROZEN;

    pub const FLAG_MASK: u64 = 0xff40;
                
    pub const SCRIPT_FN: u64 = 0x10;
    pub const NATIVE_FN: u64 = 0x20;
    pub const RUST_REF:  u64 = 0x30;
    pub const REF_MASK:  u64 = 0x30;
        
    pub const TYPE_MASK: u64 = 0x0f;
            
    const PROTO_FWD:u64 = Self::ALLOCED|Self::DEEP|Self::TYPE_MASK|Self::VALIDATED|Self::MAP_ADD|Self::VEC_FROZEN;

    pub fn freeze(&mut self){
        self.0 &= !(Self::FREEZE_MASK);
        self.0  |= Self::FROZEN
    }
    
    pub fn freeze_api(&mut self){
        self.0 &= !(Self::FREEZE_MASK);
        self.0 |= Self::FROZEN|Self::VALIDATED|Self::VEC_FROZEN
    }

    pub fn freeze_module(&mut self){
        self.0 &= !(Self::FREEZE_MASK);
        self.0  |= Self::MAP_ADD|Self::VEC_FROZEN
    }
    
    pub fn freeze_component(&mut self){
        self.0 &= !(Self::FREEZE_MASK);
        self.0 |= Self::FROZEN|Self::VALIDATED
    }
    
    pub fn has_freeze(&self)->bool{
        self.0 & Self::FREEZE_MASK != 0
    }
    
    pub fn is_frozen(&self)->bool{
        self.0 & Self::FROZEN != 0
    }
            
    pub fn is_validated(&self)->bool{
        self.0 & Self::VALIDATED != 0
    }
    
    pub fn is_map_add(&self)->bool{
        self.0 & Self::MAP_ADD != 0
    }
    
    pub fn is_vec_frozen(&self)->bool{
        self.0 & (Self::VEC_FROZEN|Self::FROZEN) != 0
    }
  
    pub fn set_flags(&mut self, flags:u64){
        self.0 |= flags
    }
        
    pub fn set_fn(&mut self, ptr:ScriptFnPtr){
        self.0 &= !(Self::REF_MASK);
        match ptr{
            ScriptFnPtr::Script(ip)=>{
                self.0 |= ((ip.index as u64)<<32) | ((ip.body as u64)<<16) | Self::SCRIPT_FN
            }
            ScriptFnPtr::Native(ni)=>{
                self.0 |= Self::NATIVE_FN | ((ni.index as u64)<<32)
            }
        }
        
    }
    
    pub fn set_rust_ref(&mut self, value: RustRef){
        self.0 &= !(Self::REF_MASK);
        self.0 |= ((value.0 as u64)<<16) | Self::RUST_REF
    }
    
    pub fn as_rust_ref(&mut self)->Option<RustRef>{
        if self.0 & Self::REF_MASK == Self::SCRIPT_FN{
            Some(RustRef(self.0>>16))
        }
        else{
            None
        }
    }
    
    pub fn as_fn(&self)->Option<ScriptFnPtr>{
        if self.0 & Self::REF_MASK == Self::SCRIPT_FN{
            Some(ScriptFnPtr::Script(ScriptIp{body:((self.0>>16)&0xffff) as u16, index:(self.0 >> 32) as u32}))
        }
        else if self.0 & Self::REF_MASK == Self::NATIVE_FN{
            Some(ScriptFnPtr::Native(NativeId{index:(self.0 >> 32) as u32}))
        }
        else{
            None
        }
    }
        
    pub fn is_script_fn(&self)->bool{
        self.0 & Self::REF_MASK == Self::SCRIPT_FN
    }
        
    pub fn is_native_fn(&self)->bool{
        self.0 & Self::REF_MASK == Self::NATIVE_FN
    }
    
    pub fn is_fn(&self)->bool{
        self.is_script_fn() || self.is_native_fn()
    }
    
    pub fn proto_fwd(&self)->u64{
        self.0 & Self::PROTO_FWD
    }
        
    pub fn set_proto_fwd(&mut self, fwd:u64){
        self.0 |= fwd
    }
        
    pub fn set_type_unchecked(&mut self, ty:ObjectType){
        self.0 &= !Self::TYPE_MASK;
        self.0 |= (ty.0 as u64) & Self::TYPE_MASK;
    }
        
    pub fn get_type(&self)->ObjectType{
        return ObjectType( (self.0 & Self::TYPE_MASK) as u8 )
    }
        
    pub fn set_deep(&mut self){
        self.0 |= Self::DEEP
    }
    
    pub fn set_reffed(&mut self){
        self.0 |= Self::REFFED
    }
        
    pub fn is_reffed(&self)->bool{
        self.0 & Self::REFFED != 0
    }
        
    pub fn clear_deep(&mut self){
        self.0 &= !Self::DEEP
    }
        
    pub fn is_deep(&self)->bool{
        self.0 & Self::DEEP != 0
    }
            
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
}

impl fmt::Debug for ObjectTag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for ObjectTag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ObjectType(").ok();
        write!(f, "{}|",self.get_type()).ok();
        if self.is_marked(){write!(f,"MARK|").ok();}
        if self.is_alloced(){write!(f,"ALLOCED|").ok();}
        if self.is_deep(){write!(f,"DEEP|").ok();}
        if self.is_script_fn(){write!(f,"SCRIPT_FN({:?})|", self.as_fn().unwrap()).ok();}
        if self.is_native_fn(){write!(f,"NATIVE_FN({:?})|", self.as_fn().unwrap()).ok();}
        if self.is_reffed(){write!(f,"REFFED|").ok();}
        if self.is_frozen(){write!(f,"FROZEN|").ok();}
        if self.is_vec_frozen(){write!(f,"VEC_FROZEN|").ok();}
        if self.is_validated(){write!(f,"VALIDATED|").ok();}
        if self.is_map_add(){write!(f,"MAP_ADD|").ok();}
        if self.is_script_fn(){write!(f,"SCRIPT_FN|").ok();}
        if self.is_native_fn(){write!(f,"NATIVE_FN|").ok();}
                                
        write!(f, ")")
    }
}

#[derive(Default, Debug)]
pub struct Object{
    pub tag: ObjectTag,
    pub proto: Value,
    pub map: ValueMap<Value, Value>,
    pub vec: Vec<Value>,
}

impl Object{
    pub fn merge_map_from_other(&mut self, other:&Object){
        self.map.extend(other.map.iter());
    }
     
    pub fn push_vec_from_other(&mut self, other:&Object){
        // alright lets go and push the vec from other
        let ty_self = self.tag.get_type();
        let ty_other = other.tag.get_type();
        if ty_self.has_paired_vec() && ty_other.has_paired_vec(){
            self.vec.extend_from_slice(&other.vec);
            return
        }
        if ty_self.is_vec1() && ty_other.has_paired_vec(){
            for chunk in other.vec.chunks(2){
                self.vec.push(chunk[1])
            }
            return
        }
                
        if ty_self.has_paired_vec() && ty_other.is_vec1(){
            for value in &other.vec{
                self.vec.extend_from_slice(&[NIL, *value]);
            }
            return
        }
        println!("implement push_vec_from_other {} {}", ty_self, ty_other);
    }
    
    pub fn set_type(&mut self, ty_new:ObjectType){
        let ty_now = self.tag.get_type();
        // block flipping from raw data mode to gc'ed mode
        if !ty_now.is_gc() && ty_new.is_gc(){
            self.vec.clear();
        }
        if !ty_now.has_paired_vec() && ty_new.has_paired_vec(){
            if self.vec.len() & 1 != 0{
                self.vec.push(NIL)
            }
        }
        self.tag.set_type_unchecked(ty_new)
    }
    //const DONT_RECYCLE_WHEN: usize = 1000;
    pub fn with_proto(proto:Value)->Self{
        Self{
            proto,
            ..Default::default()
        }
    }
    
    pub fn clear(&mut self){
        self.proto = NIL;
        self.tag.clear();
        self.map.clear();
        self.vec.clear();
    }
}