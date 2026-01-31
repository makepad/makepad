use crate::makepad_live_id::*;
use crate::value::*;
use crate::object::*;
use crate::string::*;
use crate::trap::*;
use crate::traits::*;
use crate::array::*;
use crate::gc::*;
use crate::handle::*;
use crate::pod::*;

use std::rc::Rc;
use std::cell::RefCell;
use std::fmt::Write;
use std::collections::HashMap;

#[derive(Default)]
pub struct ScriptHeap{
    pub modules: ScriptObject,
    pub(crate) mark_vec: Vec<ScriptGcMark>,
    
    pub(crate) root_objects: Rc<RefCell<HashMap<ScriptObject, usize>>>,
    pub(crate) root_arrays: Rc<RefCell<HashMap<ScriptArray, usize>>>,
    pub(crate) root_handles: Rc<RefCell<HashMap<ScriptHandle, usize>>>,
    
    pub(crate) type_defaults: HashMap<ScriptTypeIndex, ScriptObject>,
    
    pub(crate) objects: Vec<ScriptObjectData>,
    pub(crate) objects_free: Vec<ScriptObject>,
    
    pub(crate) string_intern: HashMap<ScriptRcString, ScriptString>,
    pub(crate) strings_reuse: Vec<String>,
    pub(crate) strings: Vec<Option<ScriptStringData>>,
    pub(crate) strings_free: Vec<ScriptString>,
    
    pub(crate) arrays: Vec<ScriptArrayData>,
    pub(crate) arrays_free: Vec<ScriptArray>,
    
    pub(crate) pod_types: Vec<ScriptPodTypeData>,
    pub(crate) pod_types_free: Vec<ScriptPodType>,
    pub(crate) pods: Vec<ScriptPodData>,
    pub(crate) pods_free: Vec<ScriptPod>,
    
    pub(crate) type_check: Vec<ScriptTypeCheck>,
    pub(crate) type_index: HashMap<ScriptTypeId, ScriptTypeIndex>,
    
    pub(crate) handles: Vec<Option<ScriptHandleData>>,
    pub(crate) handles_free: Vec<ScriptHandle>
}

impl ScriptHeap{
    
    pub fn empty()->Self{
        let mut v = Self{
            root_objects: Default::default(),
            modules: ScriptObject::ZERO,
            objects: vec![Default::default()],
            arrays: vec![Default::default()],
            pods: vec![Default::default()],
            handles: vec![None],
            ..Default::default()
        };
        // object zero
        v.objects[0].tag.set_alloced();
        v.objects[0].tag.set_static();
        v.objects[0].tag.freeze();
        v.arrays[0].tag.set_alloced();
        v.arrays[0].tag.freeze();
                
        v.modules = v.new_with_proto(id!(mod).into()); 
        v.root_objects.borrow_mut().insert(v.modules, 1);
        
        v
    }
    
    
    pub fn  registered_type(&self, id:ScriptTypeId)->Option<&ScriptTypeCheck>{
        if let Some(index) = self.type_index.get(&id){
            Some(&self.type_check[index.0 as usize])
        }
        else{
            None
        }
    }
        
    pub fn register_type(&mut self, type_id:Option<ScriptTypeId>, ty_check:ScriptTypeCheck)-> ScriptTypeIndex{
        let index = ScriptTypeIndex(self.type_check.len() as _);
        if let Some(type_id) = type_id{
            self.type_index.insert(type_id, index);
        }
        self.type_check.push(ty_check);
        index
    }
    
    pub fn type_matches_id(&self, ptr:ScriptObject, type_id:ScriptTypeId)->bool{
        let obj = &self.objects[ptr.index as usize];
        if let Some(ti) = obj.tag.as_type_index(){
            if let Some(object) = &self.type_check[ti.0 as usize].object{
                return object.type_id == type_id
            }
        }
        false
    }
    
    /// Returns the TypeId for an object if it has a registered type.
    pub fn object_type_id(&self, ptr: ScriptObject) -> Option<ScriptTypeId> {
        let obj = &self.objects[ptr.index as usize];
        if let Some(ti) = obj.tag.as_type_index() {
            if let Some(object) = &self.type_check[ti.0 as usize].object {
                return Some(object.type_id)
            }
        }
        None
    }
    
    pub fn new_module(&mut self, id:LiveId)->ScriptObject{
        let md = self.new_with_proto(id.into());
        self.set_value_def(self.modules, id.into(), md.into());
        md
    }
    
    pub fn module(&mut self, id:LiveId)->ScriptObject{
        self.value(self.modules, id.into(), NoTrap).into()
    }
      
     
     
    
    // Accessors
    
    
    pub fn has_proto(&mut self, ptr:ScriptObject, rhs:ScriptValue)->bool{
        let mut ptr = ptr;
        loop{
            let object = &mut self.objects[ptr.index as usize];
            if object.proto == rhs{
                return true
            }            
            if let Some(object) = object.proto.as_object(){
                ptr = object
            }
            else{
                return false
            }
        }
    }
     
    pub fn proto(&self, ptr:ScriptObject)->ScriptValue{
        self.objects[ptr.index as usize].proto
    }
    
    pub fn root_proto(&self, ptr:ScriptObject)->ScriptValue{
        let mut ptr = ptr;
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(next_ptr) = object.proto.as_object(){
                ptr = next_ptr
            }
            else{
                return object.proto
            } 
        }
    }
    
    pub fn object_data(&self, ptr:ScriptObject)->&ScriptObjectData{
        &self.objects[ptr.index as usize]
    }
    
    pub fn type_check(&self, index: ScriptTypeIndex)->&ScriptTypeCheck{
        &self.type_check[index.0 as usize]
    }
    
    pub fn set_type_default(&mut self, obj: ScriptObject)->bool{
        let object = &self.objects[obj.index as usize];
        if let Some(ty_index) = object.tag.as_type_index(){
            // Add to type_defaults mapping (GC will scan this table)
            self.type_defaults.insert(ty_index, obj);
            true
        }
        else{
            false
        }
    }
    
    pub fn type_default(&self, ty_index: ScriptTypeIndex)->Option<ScriptObject>{
        self.type_defaults.get(&ty_index).copied()
    }
    
    pub fn type_default_for_id(&self, type_id: ScriptTypeId)->Option<ScriptObject>{
        if let Some(ty_index) = self.type_index.get(&type_id){
            self.type_defaults.get(ty_index).copied()
        }
        else{
            None
        }
    }
    
    /// Look up a field's ScriptTypeId from the type-check structure of an object.
    /// This is used when the field value isn't on the prototype but the type is registered.
    pub fn field_type_from_type_check(&self, obj: ScriptObject, field_id: LiveId) -> Option<ScriptTypeId> {
        let object = &self.objects[obj.index as usize];
        if let Some(ty_index) = object.tag.as_type_index() {
            let type_check = &self.type_check[ty_index.0 as usize];
            if let Some(prop) = type_check.props.props.get(&field_id) {
                return Some(prop.ty);
            }
        }
        // Also check the prototype chain
        if let Some(proto_obj) = object.proto.as_object() {
            return self.field_type_from_type_check(proto_obj, field_id);
        }
        None
    }
                
        
    #[inline]
    pub fn cast_to_f64(&self, v:ScriptValue, ip:ScriptIp)->f64{
        if let Some(v) = v.as_f64(){
            return v
        }
        if let Some(v) = v.as_u40(){
            return v as _
        }
        if let Some(v) = v.as_string(){
            let str = self.string(v);
            if let Ok(v) = str.parse::<f64>(){
                return v
            }
            else{
                return 0.0
            }
        }
        if let Some(v) = v.as_bool(){
            return if v{1.0}else{0.0}
        }
        if let Some(v) = v.as_f32(){
            return v as f64
        }
        if let Some(v) = v.as_f16(){
            return v as f64
        }
        if let Some(v) = v.as_u32(){
            return v as f64
        }
        if let Some(v) = v.as_i32(){
            return v as f64
        }
        if let Some(v) = v.as_color(){
            return v as f64
        }
        if v.is_nil(){
            return 0.0
        }
        ScriptValue::from_f64_traced_nan(f64::NAN, ip).as_f64().unwrap()
    }
    
    pub fn cast_to_bool(&self, v:ScriptValue)->bool{
        if let Some(b) = v.as_bool(){
            return b
        }
        if v.is_nil(){
            return false
        }
        if let Some(v) = v.as_f64(){
            return v != 0.0
        }
        if let Some(v) = v.as_u40(){
            return v != 0
        }
        if let Some(v) = v.as_f32(){
            return v != 0.0
        }
        if let Some(v) = v.as_f16(){
            return v != 0.0
        }
        if let Some(v) = v.as_u32(){
            return v != 0
        }
        if let Some(v) = v.as_i32(){
            return v != 0
        }
        if let Some(_v) = v.as_object(){
            return true
        }
        if v.inline_string_not_empty(){
            return true
        }
        if let Some(v) = v.as_string(){
            return self.string(v).len() != 0
        }
        if let Some(_v) = v.as_id(){
            return true
        }
        if let Some(_v) = v.as_color(){
            return true
        }
        if v.is_opcode(){
            return true
        }
        false
    }
    
    
    // Debug and utility
    
    
    
    pub fn deep_eq(&self, a:ScriptValue, b:ScriptValue)->bool{
        if a == b{
            return true
        }
        if let Some(a) = a.as_number(){
            if let Some(b) = b.as_number(){
                return a == b
            }
            return false
        }
        if a.is_object(){
            let mut aw = a;
            let mut bw = b;
            loop{
                if let Some(pa) = aw.as_object(){
                    if let Some(pb) = bw.as_object(){
                        let oa = &self.objects[pa.index as usize];
                        let ob = &self.objects[pb.index as usize];
                        if oa.vec.len() != ob.vec.len(){
                            return false
                        }
                        for (a,b) in oa.vec.iter().zip(ob.vec.iter()){
                            if !self.deep_eq(a.key, b.key) || !self.deep_eq(a.value,b.value){
                                return false
                            }
                        }
                        if oa.map_len() != ob.map_len(){
                            return false
                        }
                        if let Some(ret) = oa.map_iter_ret(|k,v1|{
                            if let Some(v2) = ob.map_get(&k){
                                if !self.deep_eq(v1, v2){
                                    return Some(false)
                                }
                                return None
                            }
                            // lets do the string keys shenanigans to make json ok
                            else if k.is_id() && ob.tag.is_string_keys(){
                                let id = k.as_id().unwrap();
                                if let Some(v2) = id.as_string(|s|{
                                    if let Some(s) = s{
                                        if let Some(idx) = self.check_intern_string(s){
                                            ob.map_get(&idx)
                                        }
                                        else{
                                            None
                                        }
                                    }
                                    else{
                                        None
                                    }
                                }){
                                    if !self.deep_eq(v1, v2){
                                        return Some(false)
                                    }
                                    return None
                                }
                            }
                            else if k.is_string_like() && !ob.tag.is_string_keys(){
                                let id = if let Some(s) = k.as_string(){
                                    if let Some(s) = &self.strings[s.index as usize]{LiveId::from_str(&s.string.0)}else{LiveId(0)}
                                }
                                else {
                                    k.as_inline_string(|s| LiveId::from_str(s)).unwrap()
                                };
                                if let Some(v2) = ob.map_get(&id.into()){
                                    if !self.deep_eq(v1, v2){
                                        return Some(false)
                                    }
                                    return None
                                }
                            }
                            Some(false)
                        }){
                            return ret
                        }
                        aw = oa.proto;
                        bw = ob.proto;
                        if aw == bw{
                            return true
                        }
                    }
                    else{
                        return false
                    }
                }
                else{
                    return false
                }
            }
        }
        else if let Some(arr1) = a.as_array(){
            if let Some(arr2) = b.as_array(){
                match &self.arrays[arr1.index as usize].storage{
                    ScriptArrayStorage::ScriptValue(arr1)=>match &self.arrays[arr2.index as usize].storage{
                        ScriptArrayStorage::ScriptValue(arr2)=>{
                            if arr1.len() != arr2.len(){
                                return false
                            }
                            for (a,b) in arr1.iter().zip(arr2.iter()){
                                if !self.deep_eq(*a, *b){
                                    return false
                                }
                            }
                            return true
                        }
                        _=>{return false}
                    }
                    ScriptArrayStorage::F32(arr1)=>match &self.arrays[arr2.index as usize].storage{
                        ScriptArrayStorage::F32(arr2)=>{return arr1 == arr2},
                        _=>{return false}
                    }
                    ScriptArrayStorage::U32(arr1)=>match &self.arrays[arr2.index as usize].storage{
                        ScriptArrayStorage::U32(arr2)=>{return arr1 == arr2},
                        _=>{return false}
                    }
                    ScriptArrayStorage::U16(arr1)=>match &self.arrays[arr2.index as usize].storage{
                        ScriptArrayStorage::U16(arr2)=>{return arr1 == arr2},
                        _=>{return false}
                    }
                    ScriptArrayStorage::U8(arr1)=>match &self.arrays[arr2.index as usize].storage{
                        ScriptArrayStorage::U8(arr2)=>{return arr1 == arr2},
                        _=>{return false}
                    }
                }
            }
            return false
        }
        false
    }
    
    pub fn println(&self, value:ScriptValue){
        let mut out = String::new();
        let mut recur = Vec::new();
        self.to_debug_string(value, &mut recur, &mut out, true, 0);
        println!("{out}");
    }
    
    pub fn to_debug_string(&self, value:ScriptValue, recur:&mut Vec<ScriptValue>, out:&mut String, formatted: bool, depth: usize){
        fn write_indent(out: &mut String, depth: usize) {
            for _ in 0..depth {
                out.push_str("- - ");
            }
        }
        
        fn write_separator(out: &mut String, formatted: bool, depth: usize, first: bool) {
            if !first {
                if formatted {
                    out.push_str(",\n");
                    write_indent(out, depth);
                } else {
                    out.push_str(", ");
                }
            }
        }
        
        if let Some(obj) = value.as_object(){
            if self.is_fn(obj){
                write!(out, "<fn {}>", obj.index()).ok();
                return
            }
            if recur.iter().any(|v| *v == value){
                write!(out, "<recur>").ok();
                return
            }
            recur.push(value);
            
            let object = &self.objects[obj.index as usize];
            if object.tag.is_script_fn(){
                write!(out, "Fn").ok();
            }
            else if object.tag.is_native_fn(){
               write!(out, "Native").ok();
            }
            let mut ptr = obj;
            // scan up the chain to set the proto value
            write!(out, "<{}>{{", obj.index()).ok();
            
            // Check if object has any content (for formatted output)
            let has_content = {
                let obj_data = &self.objects[obj.index as usize];
                obj_data.map_len() > 0 || !obj_data.vec.is_empty() || obj_data.tag.as_type_index().is_some()
            };
            
            if formatted && has_content {
                out.push('\n');
                write_indent(out, depth + 1);
            }
            
            let mut first = true;
            
            // if we have a type index, output type checked base properties first
            if let Some(ty_index) = object.tag.as_type_index(){
                write!(out, "<type ").ok();
                let type_check = &self.type_check[ty_index.0 as usize];
                for (prop_id, _prop_ty) in type_check.props.iter_ordered(){
                    if !first{write!(out, ", ").ok();}
                    write!(out, "{}", prop_id).ok();
                    first = false;
                }
                write!(out, ">").ok();
                if formatted {
                    out.push('\n');
                    write_indent(out, depth + 1);
                }
                first = true;
            }
            
            loop{
                let object = &self.objects[ptr.index as usize];
                
                object.map_iter_ordered(|key,value|{
                    write_separator(out, formatted, depth + 1, first);
                    if key != NIL{
                        self.to_debug_string(key, recur, out, formatted, depth + 1);
                        write!(out, ": ").ok();
                    }
                    self.to_debug_string(value, recur, out, formatted, depth + 1);
                    first = false;
                });
                for kv in object.vec.iter(){
                    write_separator(out, formatted, depth + 1, first);
                    if kv.key != NIL{
                        write!(out, "{}: ", kv.key).ok();
                    }
                    self.to_debug_string(kv.value, recur, out, formatted, depth + 1);
                    first = false;
                }
                if let Some(next_ptr) = object.proto.as_object(){
                    if formatted {
                        if !first { out.push_str(",\n"); write_indent(out, depth + 1); }
                        write!(out, "^<{}>", next_ptr.index()).ok();
                    } else {
                        if !first{write!(out, ",").ok();}
                        write!(out, "^<{}>", next_ptr.index()).ok();
                    }
                    ptr = next_ptr
                }
                else{
                    if formatted && has_content {
                        out.push('\n');
                        write_indent(out, depth);
                    }
                    write!(out, "/{}", object.proto).ok();
                    break;
                }
            }
            write!(out, "}}").ok();
            recur.pop();
        }
        else if let Some(arr) = value.as_array(){
            if recur.iter().any(|v| *v == value){
                write!(out, "<recur>").ok();
                return
            }
            recur.push(value);
            let array = &self.arrays[arr.index as usize];
            let len = array.storage.len();
            write!(out, "[").ok();
            
            if formatted && len > 0 {
                out.push('\n');
                write_indent(out, depth + 1);
            }
            
            for i in 0..len{
                if i != 0 {
                    if formatted {
                        out.push_str(",\n");
                        write_indent(out, depth + 1);
                    } else {
                        out.push_str(", ");
                    }
                }
                self.to_debug_string(array.storage.index(i).unwrap(), recur, out, formatted, depth + 1);
            }
            
            if formatted && len > 0 {
                out.push('\n');
                write_indent(out, depth);
            }
            
            write!(out, "]").ok();
            recur.pop();
        }
        else if let Some(s) = value.as_string(){
            let s = if let Some(s) = &self.strings[s.index as usize]{&s.string.0}else{""};
            write!(out, "\"").ok();
            write!(out, "{}", s).ok();
            write!(out, "\"").ok();
        }
        else if value.as_inline_string(|s|{
            write!(out, "\"").ok();
            write!(out, "{}", s).ok();
            write!(out, "\"").ok();
        }).is_some(){}
        else if let Some(pod) = value.as_pod(){
            let pod = &self.pods[pod.index as usize];
            let pod_type = &self.pod_types[pod.ty.index as usize];
            self.pod_debug(out, pod_type, 0, &pod.data);
        }else{
            write!(out, "{}", value).ok();
        }
    }
    
    pub fn to_json(&mut self, value:ScriptValue)->ScriptValue{
        self.new_string_with(|heap, s|{
            heap.to_json_inner(value, s);
        })
    }
    
    pub fn to_json_inner(&self, value:ScriptValue, out:&mut String){
        fn escape_str(inp:&str, out:&mut String){
            for c in inp.chars(){
                match c{
                    '\x08'=>out.push_str("\\b"),
                    '\x0c'=>out.push_str("\\f"),
                    '\n'=>out.push_str("\\n"),
                    '\r'=>out.push_str("\\r"),
                    '"'=>out.push_str("\\\""),
                    '\\'=>out.push_str("\\"),
                    c=>{
                        out.push(c);
                    }
                }
            }
        }
        if let Some(obj) = value.as_object(){
            let mut ptr = obj;
            // scan up the chain to set the proto value
            out.push('{');
            let mut first = true;
            loop{
                let object = &self.objects[ptr.index as usize];
                object.map_iter(|key,value|{
                    if !first{out.push(',')}
                    self.to_json_inner(key, out);
                    out.push(':');
                    self.to_json_inner(value, out);
                    first = false;
                });
                for kv in object.vec.iter(){
                    if !first{out.push(',')}
                    first = false;
                    self.to_json_inner(kv.key, out);
                    out.push(':');
                    self.to_json_inner(kv.value, out);
                }
                if let Some(next_ptr) = object.proto.as_object(){
                    ptr = next_ptr
                }
                else{
                    break;
                }
            }
            out.push('}');
        }
        else if let Some(arr) = value.as_array(){
            let array = &self.arrays[arr.index as usize];
            let len = array.storage.len();
            let mut first = true;
            out.push('[');
            for i in 0..len{
                if let Some(value) =array.storage.index(i){
                    if !first{out.push(',')}
                    first = false;
                    self.to_json_inner(value, out);
                }
            }
            out.push(']');
        }
        else if let Some(id) = value.as_id(){
            out.push('"');
            id.as_string(|s|{
                if let Some(s) = s {
                    escape_str(s, out);
                }
            });
            out.push('"');
            // alright. sself is json eh. so.
        }
        else if let Some(s) = value.as_string(){
            let s = if let Some(s) = &self.strings[s.index as usize]{&s.string.0}else{""};
            out.push('"');
            escape_str(s, out);
            out.push('"');
        }
        else if value.as_inline_string(|s|{
            out.push('"');
            escape_str(s, out);
            out.push('"');
        }).is_some(){}
        else if let Some(v) = value.as_bool(){
            if v{out.push_str("true")}
            else{out.push_str("false")}
        }
        else if let Some(v) = value.as_number(){
            write!(out, "{}", v).ok();
        }
        else if let Some(v) = value.as_handle(){
            write!(out, "Handle{:?}", v).ok();
        }
        else {
            out.push_str("null");
        }
    }
        
    // memory  usage
    pub fn objects_len(&self)->usize{
        self.objects.len()
    }
    
    /// Checks if a value has an apply transform without calling it.
    /// Used by type_check to be permissive when a transform exists.
    pub fn has_apply_transform(&self, value: ScriptValue) -> bool {
        if let Some(obj) = value.as_object() {
            return self.objects[obj.index as usize].tag.as_apply_transform().is_some();
        }
        if let Some(arr) = value.as_array() {
            return self.arrays[arr.index as usize].tag.as_apply_transform().is_some();
        }
        false
    }
}
