use std::fmt;
use crate::makepad_value::value::*;
use std::collections::BTreeMap;

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

impl ObjectTag{
    pub const MARK:u64 = 0x100;
    pub const ALLOCED:u64 = 0x200;
    pub const DEEP:u64 = 0x400;
    pub const SCRIPT_FN: u64 = 0x800;
    pub const NATIVE_FN: u64 = 0x1000;
    pub const REFFED: u64 = 0x2000;
    pub const HAS_METHODS: u64 = 0x4000;
        
    pub const TYPE_MASK: u64 = 0xff;
        
    const PROTO_FWD:u64 = Self::ALLOCED|Self::DEEP|Self::TYPE_MASK|Self::HAS_METHODS;
        
    pub fn set_flags(&mut self, flags:u64){
        self.0 |= flags
    }
        
    pub fn set_script_fn(&mut self, val: u32){
        self.0 |= ((val as u64)<<32) | Self::SCRIPT_FN
    }
        
    pub fn get_fn(&self)->u32{
        (self.0 >> 32) as u32
    }
        
    pub fn set_native_fn(&mut self, val: u32){
        self.0 |= Self::NATIVE_FN | ((val as u64)<<32)
    }
            
    pub fn is_native_fn(&self)->bool{
        self.0 & Self::NATIVE_FN != 0
    }
    
    pub fn is_fn(&self)->bool{
        self.0 & (Self::SCRIPT_FN|Self::NATIVE_FN) != 0
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
        
    pub fn is_script_fn(&self)->bool{
        self.0 & Self::SCRIPT_FN != 0
    }
            
    pub fn set_deep(&mut self){
        self.0 |= Self::DEEP
    }
    
    pub fn set_has_methods(&mut self){
        self.0 |= Self::HAS_METHODS;
    }
    
    pub fn has_methods(&self)->bool{
        self.0 & Self::HAS_METHODS != 0
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
        if self.is_script_fn(){write!(f,"SCRIPT_FN({})|", self.get_fn()).ok();}
        if self.is_native_fn(){write!(f,"NATIVE_FN({})|", self.get_fn()).ok();}
        if self.is_reffed(){write!(f,"REFFED").ok();}
        write!(f, ")")
    }
}

#[derive(Default, Debug)]
pub struct Object{
    pub tag: ObjectTag,
    pub proto: Value,
    pub map: BTreeMap<Value, Value>,
    pub vec: Vec<Value>,
}

impl Object{
    pub fn merge_map_from_other(&mut self, other:&Object){
        self.map.extend(&other.map);
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
                self.vec.extend_from_slice(&[Value::NIL, *value]);
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
                self.vec.push(Value::NIL)
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
        self.proto = Value::NIL;
        self.tag.clear();
        self.map.clear();
        self.vec.clear();
    }
}