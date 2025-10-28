use crate::makepad_live_id::*;
use crate::value::*;
use crate::object::*;
use crate::string::*;
use crate::trap::*;
use crate::traits::*;
use crate::array::*;
use crate::gc::*;

use std::fmt::Write;
use std::collections::HashMap;

#[derive(Default)]
pub struct ScriptHeap{
    pub modules: ScriptObject,
    pub(crate) mark_vec: Vec<ScriptGcMark>,
    pub(crate) roots: Vec<ScriptObject>,
    
    pub(crate) objects: Vec<ScriptObjectData>,
    pub(crate) objects_free: Vec<ScriptObject>,
    
    pub(crate) string_intern: HashMap<String, ScriptString>,
    pub(crate) string_intern_free: Vec<String>,
    pub(crate) strings: Vec<ScriptStringData>,
    pub(crate) strings_free: Vec<ScriptString>,
    
    pub(crate) arrays: Vec<ScriptArrayData>,
    pub(crate) arrays_free: Vec<ScriptArray>,
    
    pub(crate) type_check: Vec<ScriptTypeCheck>,
    pub(crate) type_index: HashMap<ScriptTypeId, ScriptTypeIndex>,
    
}

impl ScriptHeap{
    
    pub fn empty()->Self{
        let mut v = Self{
            roots: vec![],
            modules: ScriptObject::ZERO,
            objects: vec![Default::default()],
            arrays: vec![Default::default()],
            ..Default::default()
        };
        // object zero
        v.objects[0].tag.set_alloced();
        v.objects[0].tag.set_static();
        v.objects[0].tag.freeze();
        v.arrays[0].tag.set_alloced();
        v.arrays[0].tag.freeze();
        
        v.modules = v.new_with_proto(id!(mod).into()); 
        v.roots.push(v.modules);
        
        v
    }
    
    
    // New objects
    
    
    
    pub fn new_object(&mut self)->ScriptObject{
        if let Some(obj) = self.objects_free.pop(){
            let object = &mut self.objects[obj.index as usize];
            object.tag.set_alloced();
            object.proto = id!(object).into();
            obj
        }
        else{
            let index = self.objects.len();
            let mut object = ScriptObjectData::default();
            object.tag.set_alloced();
            object.proto = id!(object).into();
            self.objects.push(object);
            ScriptObject{index: index as _}
        }
    }
    
    
    pub fn new_with_proto_checked(&mut self, proto:ScriptValue, trap:&ScriptTrap)->ScriptObject{
        if let Some(ptr) = proto.as_object(){
            let object = &mut self.objects[ptr.index as usize];
            if object.tag.is_notproto(){
                trap.err_not_proto();
                return ScriptObject::ZERO;
            }
        }
        self.new_with_proto(proto)
    }
    
    pub fn new_with_proto(&mut self, proto:ScriptValue)->ScriptObject{
        let (proto_fwd, proto_index) = if let Some(ptr) = proto.as_object(){
            let object = &mut self.objects[ptr.index as usize];
            object.tag.set_reffed();
            (object.tag.proto_fwd(), ptr.index)
        }
        else{
            let ptr = self.new_object();
            self.objects[ptr.index as usize].proto = proto;
            return ptr
        };
                        
        if let Some(obj) = self.objects_free.pop(){
            let (object, proto_object) = if obj.index > proto_index{
                let (o1, o2) = self.objects.split_at_mut(obj.index as usize);
                (&mut o2[0], &mut o1[proto_index as usize])                    
            }else{
                let (o1, o2) = self.objects.split_at_mut(proto_index as usize);
                (&mut o1[obj.index as usize], &mut o2[0])                    
            };
            object.tag.set_proto_fwd(proto_fwd);
            object.proto = proto;
            // only copy vec if we are 'auto' otherwise we proto inherit normally
            if proto_object.tag.get_storage_type().is_auto(){
                object.vec.extend_from_slice(&proto_object.vec);
            }
            obj
        }
        else{
            let index = self.objects.len();
            let mut object = ScriptObjectData::with_proto(proto);
            object.tag.set_proto_fwd(proto_fwd);
            let proto_object = &self.objects[proto_index as usize];
            if proto_object.tag.get_storage_type().is_auto(){
                object.vec.extend_from_slice(&proto_object.vec);
            }
            self.objects.push(object);
            ScriptObject{index: index as _}
        }
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
    
    pub fn new_if_reffed(&mut self, ptr:ScriptObject)->ScriptObject{
        let obj = &self.objects[ptr.index as usize];
        if obj.tag.is_reffed(){
            let proto = obj.proto;
            return self.new_with_proto(proto);
        }
        return ptr;
    }
    
    pub fn new_module(&mut self, id:LiveId)->ScriptObject{
        let md = self.new_with_proto(id.into());
        self.set_value_def(self.modules, id.into(), md.into());
        md
    }
    
    
    // Strings
    
    
    pub fn new_string_from_str(&mut self,value:&str)->ScriptValue{
        self.new_string_with(|_,out|{
            out.push_str(value);
        })
    }
            
    pub fn new_string_with<F:FnOnce(&mut Self, &mut String)>(&mut self,cb:F)->ScriptValue{
        let mut out = if let Some(s) = self.string_intern_free.pop(){s} else {String::new()};
        
        cb(self, &mut out);
        
        if let Some(v) = ScriptValue::from_inline_string(&out){
            out.clear();
            self.string_intern_free.push(out);
            return v
        }
        // check intern table
        if let Some(index) = self.string_intern.get(&out){
            out.clear();
            self.string_intern_free.push(out);
            return (*index).into();
        }
        // fetch a free string
        if let Some(str) = self.strings_free.pop(){
            let string = &mut self.strings[str.index as usize];
            string.string.push_str(&out);
            string.tag.set_alloced();
            self.string_intern.insert(out, str);
            str
        }
        else{
            let index = self.strings.len();
            let mut string = ScriptStringData::default();
            string.tag.set_alloced();
            string.string.push_str(&out);
            self.strings.push(string);
            let ret = ScriptString{index: index as _};
            self.string_intern.insert(out, ret);
            ret
        }.into()
    }
    
    pub fn as_string_mut_self<R,F:FnOnce(&mut Self, &str)->R>(&mut self, value:ScriptValue, cb:F)->Option<R>{
        if let Some(s) = value.as_string(){
            let mut str = String::new();
            std::mem::swap(&mut self.strings[s.index as usize].string, &mut str);
            let r = cb(self, &str);
            std::mem::swap(&mut self.strings[s.index as usize].string, &mut str);
            return Some(r)
        }
        if let Some(r) = value.as_inline_string(|s|{
            cb(self, s)
        }){
            return Some(r)
        }
        None
    }
        
    pub fn check_intern_string(&self,value:&str)->ScriptValue{
        if let Some(v) = ScriptValue::from_inline_string(&value){
            v
        }
        else if let Some(idx) = self.string_intern.get(value){
            (*idx).into()
        }
        else{
            NIL
        }
    }
    
    pub fn string(&self, ptr: ScriptString)->&str{
        &self.strings[ptr.index as usize].string
    }
        
    pub fn string_to_bytes_array(&mut self, v:ScriptValue)->ScriptArray{
        let arr = self.new_array();
        if v.as_inline_string(|str|{
            let array = &mut self.arrays[arr.index as usize];
            if let ScriptArrayStorage::U8(v) = &mut array.storage{v.clear();v.extend(str.as_bytes())}
            else{array.storage = ScriptArrayStorage::U8(str.as_bytes().into());}
        }).is_some(){}
        else if let Some(str) = v.as_string(){
            let array = &mut self.arrays[arr.index as usize];
            let str = &self.strings[str.index as usize].string;
            if let ScriptArrayStorage::U8(v) = &mut array.storage{v.clear();v.extend(str.as_bytes())}
            else{array.storage = ScriptArrayStorage::U8(str.as_bytes().into());}
        }
        return arr
    }
        
    pub fn string_to_chars_array(&mut self, v:ScriptValue)->ScriptArray{
        let arr = self.new_array();
        if v.as_inline_string(|str|{
            let array = &mut self.arrays[arr.index as usize];
            if let ScriptArrayStorage::U32(v) = &mut array.storage{v.clear();for c in str.chars(){v.push(c as u32)}}
            else{array.storage = ScriptArrayStorage::U32(str.chars().map(|c| c as u32).collect());}
        }).is_some(){}
        else if let Some(str) = v.as_string(){
            let array = &mut self.arrays[arr.index as usize];
            let str = &self.strings[str.index as usize].string;
            if let ScriptArrayStorage::U32(v) = &mut array.storage{v.clear();for c in str.chars(){v.push(c as u32)}}
            else{array.storage = ScriptArrayStorage::U32(str.chars().map(|c| c as u32).collect());}
        }
        return arr
    }
        
    pub fn cast_to_string(&self, v:ScriptValue, out:&mut String){
                
        if v.as_inline_string(|s|{write!(out, "{s}")}).is_some(){
        }
        else if let Some(v) = v.as_string(){
            let str = self.string(v);
            out.push_str(str);
        }
        else if let Some(v) = v.as_f64(){
            write!(out, "{v}").ok();
        }
        else if let Some(v) = v.as_bool(){
            write!(out, "{v}").ok();
        }
        else if let Some(v) = v.as_id(){
            write!(out, "{v}").ok();
        }
        else if let Some(_v) = v.as_object(){
            write!(out, "[ScriptObject]").ok();
        }
        else if let Some(v) = v.as_color(){
            write!(out, "#{:08x}", v).ok();
        }
        else if v.is_nil(){
        }
        else if v.is_opcode(){
            write!(out, "[Opcode]").ok();
        }
        else if v.is_err(){
            write!(out, "[Error:{}]", v).ok();
        }
        else{
            write!(out, "[Unknown]").ok();
        }
    }
    
    
    // Arrays
    
    
    pub fn freeze_array(&mut self, array:ScriptArray){
        self.arrays[array.index as usize].tag.freeze()
    }
    
    pub fn new_array(&mut self)->ScriptArray{
        if let Some(arr) = self.arrays_free.pop(){
            let array = &mut self.arrays[arr.index as usize];
            array.tag.set_alloced();
            arr
        }
        else{
            let index = self.arrays.len();
            let mut array = ScriptArrayData::default();
            array.tag.set_alloced();
            self.arrays.push(array);
            ScriptArray{index: index as _}
        }
    }
    
    pub fn array_len(&self, array:ScriptArray)->usize{
        self.arrays[array.index as usize].storage.len()
    }
    
    pub fn array_push(&mut self, array:ScriptArray, value:ScriptValue, trap:&ScriptTrap){
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            trap.err_frozen();
            return 
        }
        array.tag.set_dirty();
        array.storage.push(value);
    }
    
    pub fn array_push_unchecked(&mut self, array:ScriptArray, value:ScriptValue){
        let array = &mut self.arrays[array.index as usize];
        array.tag.set_dirty();
        array.storage.push(value);
    }
    
    pub fn array_ref(&self, array:ScriptArray)->&ScriptArrayStorage{
        let array = &self.arrays[array.index as usize];
        &array.storage
    }
    
    pub fn array_mut(&mut self, array:ScriptArray,trap:&ScriptTrap)->Option<&mut ScriptArrayStorage>{
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            trap.err_frozen();
            return None
        }
        array.tag.set_dirty();
        Some(&mut array.storage)
    }
        
        
    pub fn array_remove(&mut self, array:ScriptArray, index: usize,trap:&ScriptTrap)->ScriptValue{
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            return trap.err_frozen();
        }
        array.tag.set_dirty();
        if index >= array.storage.len(){
            return trap.err_array_bound()
        }
        array.storage.remove(index)
    }
    
    pub fn array_push_vec(&mut self, array:ScriptArray, object:ScriptObject, trap:&ScriptTrap){
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            trap.err_frozen();
            return
        }
        array.tag.set_dirty();
        array.storage.push_vec(&self.objects[object.index as usize].vec);
    }
    
    pub fn array_pop(&mut self, array:ScriptArray, trap:&ScriptTrap)->ScriptValue{
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            return trap.err_frozen()
        }
        if let Some(value) = array.storage.pop(){
            array.tag.set_dirty();
            value
        }
        else{
            trap.err_array_bound()
        }
    }
    
    pub fn array_index(&self, array:ScriptArray, index:usize, trap:&ScriptTrap)->ScriptValue{
        if let Some(value) = self.arrays[array.index as usize].storage.index(index){
            return value
        }
        else{
            trap.err_array_bound()
        }
    }
    
    pub fn set_array_index(&mut self, array:ScriptArray, index:usize, value:ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            return trap.err_frozen();
        }
        array.tag.set_dirty();
        array.storage.set_index(index, value);
        NIL
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
                
    //pub fn object(&self, ptr:ScriptObject)->&ScriptObject{
    //    &self.objects[ptr.index as usize]
    //}
    
        
    pub fn cast_to_f64(&self, v:ScriptValue, ip:ScriptIp)->f64{
        if let Some(v) = v.as_f64(){
            v
        }
        else if let Some(v) = v.as_string(){
            let str = self.string(v);
            if let Ok(v) = str.parse::<f64>(){
                return v
            }
            else{
                return 0.0
            }
        }
        else if let Some(v) = v.as_bool(){
            return if v{1.0}else{0.0}
        }
        else if let Some(v) = v.as_color(){
            return v as f64
        }
        else if v.is_nil(){
            0.0
        }
        else {
            ScriptValue::from_f64_traced_nan(f64::NAN, ip).as_f64().unwrap()
        }
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
    /*
    pub fn swap_string(&mut self, ptr: ScriptString, swap:&mut String){
        std::mem::swap(swap, &mut self.strings[ptr.index as usize].string);
    }
    
    pub fn mut_string(&mut self, ptr: ScriptString)->&mut String{
        &mut self.strings[ptr.index as usize].string
    }*/
    
    
    
    // Setting object flags
    
    
    
    
    pub fn set_object_deep(&mut self, ptr:ScriptObject){
         self.objects[ptr.index as usize].tag.set_deep()
    }
    
    pub fn set_object_storage_type(&mut self, ptr:ScriptObject, ty: ScriptObjectStorageType){
        self.objects[ptr.index as usize].set_storage_type(ty)
    }
    
    pub fn set_first_applied_and_clean(&mut self, ptr:ScriptObject){
        self.objects[ptr.index as usize].tag.set_first_applied_and_clean()
    }
            
    pub fn clear_object_deep(&mut self, ptr:ScriptObject){
        self.objects[ptr.index as usize].tag.clear_deep()
    }
    
    pub fn freeze(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.freeze()
    }
    
    pub fn freeze_module(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.freeze_module()
    }
            
    pub fn freeze_component(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.freeze_component()
    }
            
    pub fn freeze_api(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.freeze_api()
    }
    
    pub fn freeze_with_type(&mut self, obj: ScriptObject, ty:ScriptTypeIndex){
        let object = &mut  self.objects[obj.index as usize];
        object.tag.set_tracked();
        object.tag.set_type_index(ty);
        object.tag.freeze_component();
    }
    // Writing object values 
        
        
    
    pub(crate) fn force_value_in_map(&mut self, ptr:ScriptObject, key: ScriptValue, this:ScriptValue){
        let object = &mut self.objects[ptr.index as usize];
        object.map_insert(key, this);
    }            
        
    fn set_value_index(&mut self, ptr: ScriptObject, index:ScriptValue, value: ScriptValue, trap:&ScriptTrap)->ScriptValue{
        // alright so. now what.
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){ // has rw flags
            return trap.err_vec_frozen()
        }
        
        let index = index.as_index();
        if index >= object.vec.len(){
            object.vec.resize(index + 1, ScriptVecValue::default());
        }
        object.vec[index].value = value;
        return NIL
    }
            
    fn set_value_prefixed(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return trap.err_vec_frozen()
        }
        for kv in object.vec.iter_mut().rev(){
            if kv.key == key{
                kv.value = value;
                return NIL
            }
        }
        // just append it
        object.vec.push(ScriptVecValue{key, value});
        NIL
    }
        
    fn set_value_deep(&mut self, ptr:ScriptObject, key: ScriptValue, value: ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let mut ptr = ptr;
        loop{
            let object = &mut self.objects[ptr.index as usize];
            if object.tag.is_frozen(){
                return trap.err_frozen()
            }
            for kv in object.vec.iter_mut().rev(){
                if kv.key == key{
                    kv.value = value;
                    return NIL
                }
            }
            if object.map_set_if_exist(key, value){
                return NIL
            }
            if let Some(next_ptr) = object.proto.as_object(){
                ptr = next_ptr
            }
            else{
                break;
            } 
        }
        // alright nothing found
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.get_storage_type().is_vec2(){
            object.vec.push(ScriptVecValue{key, value});
        }
        else{
            object.map_insert(key, value);
        }
        NIL
    }
    
    fn validate_type(&self, lhs:ScriptValue, rhs:ScriptValue)->bool{
        lhs.value_type().to_redux() == rhs.value_type().to_redux()
    }
    
    fn set_value_shallow_checked(&mut self, ptr:ScriptObject, key:ScriptValue, key_id:LiveId, value:ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let object = &self.objects[ptr.index as usize];
        if object.tag.is_frozen(){
            return trap.err_frozen()
        }
        if let Some(ty) = object.tag.as_type_index(){
            
            let check = &self.type_check[ty.0 as usize];
            if let Some(ty_id) = check.props.props.get(&key_id){
                if let Some(ty_index) = self.type_index.get(ty_id){
                    let check_prop = &self.type_check[ty_index.0 as usize];
                    if let Some(object) = &check_prop.object{
                        if !(*object.check)(self, value){
                            return trap.err_invalid_prop_type()
                        }
                    }
                }
                else{
                    println!("Trying to check a type that hasnt been registered yet for {} {}", key, value);
                    return trap.err_type_not_registered()
                }
            }
            else{
                return trap.err_invalid_prop_name()
            }
            let object = &mut self.objects[ptr.index as usize];
            object.map_insert(key, value);
            return NIL    
        }
        // check against prototype or type
        if object.tag.is_validated(){
            let mut ptr = ptr;
            loop{
                let object = &self.objects[ptr.index as usize];
                if object.tag.get_storage_type().is_vec2(){
                    for kv in object.vec.iter().rev(){
                        if kv.key == key{
                            if !self.validate_type(kv.value, value){
                                return trap.err_invalid_prop_type()
                            }
                            return self.set_value_shallow(ptr, key, value, trap);
                        }
                    }
                }
                if let Some(set_value) = object.map_get(&key){
                    if !self.validate_type(set_value, value){
                        return trap.err_invalid_prop_type()
                    }
                    return self.set_value_shallow(ptr, key, value, trap);
                }
                if let Some(next_ptr) = object.proto.as_object(){
                    ptr = next_ptr
                }
                else{ // not found
                    return trap.err_invalid_prop_name()
                } 
            }
        }
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_map_add(){
            if object.tag.get_storage_type().is_vec2(){
                for kv in object.vec.iter_mut().rev(){
                    if kv.key == key{
                        return trap.err_key_already_exists()
                    }
                }
                object.vec.push(ScriptVecValue{key, value});
                return NIL
            }
            if let Some(_) = object.map_get(&key){
                return trap.err_key_already_exists()
            }
            else{
                object.map_insert(key, value);
                return NIL    
            }
        }
        trap.err_unexpected()
    }
    
    fn set_value_shallow(&mut self, ptr:ScriptObject, key:ScriptValue, value:ScriptValue, _trap:&ScriptTrap)->ScriptValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.get_storage_type().is_vec2(){
            for kv in object.vec.iter_mut().rev(){
                if kv.key == key{
                    kv.value = value;
                    return NIL;
                }
            }
            object.vec.push(ScriptVecValue{key, value});
            return NIL
        }
        object.map_insert(key, value);
        NIL
    }
            
    
    pub fn set_value_def(&mut self, ptr:ScriptObject, key:ScriptValue, value:ScriptValue){
        self.set_value(ptr, key, value, &mut ScriptTrap::default());
    }
    
    pub fn set_value(&mut self, ptr:ScriptObject, key:ScriptValue, value:ScriptValue, trap:&ScriptTrap)->ScriptValue{
        if let Some(obj) = value.as_object(){
            self.set_reffed(obj);
        }
        if let Some(key_id) = key.as_id(){
            if key_id.is_prefixed(){
                return self.set_value_prefixed(ptr, key, value, trap)
            }
            let object = &self.objects[ptr.index as usize];
            if !object.tag.is_deep(){
                if object.tag.needs_checking(){
                    return self.set_value_shallow_checked(ptr, key, key_id, value, trap)
                }
                return self.set_value_shallow(ptr, key, value, trap);
            }
            else{
                return self.set_value_deep(ptr, key, value, trap)
            }
        }
        if key.is_index(){ // use vector
            return self.set_value_index(ptr, key, value, trap);
        }
        if key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            let object = &mut self.objects[ptr.index as usize];
            if !object.tag.is_deep(){
                if object.tag.needs_checking(){
                    return trap.err_invalid_key_type()
                }
                return self.set_value_shallow(ptr, key, value, trap);
            }
            else{
                return self.set_value_deep(ptr, key, value, trap)
            }
        }
        trap.err_invalid_key_type()
    }
    
    
    // scope specific value get/set
    
    
    pub fn set_scope_value(&mut self, ptr:ScriptObject, key:LiveId, value:ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let mut ptr = ptr;
        loop{
            let object = &mut self.objects[ptr.index as usize];
            if let Some(set) = object.map.get_mut(&key.into()){
                set.value = value;
                return NIL
            }
            if let Some(next_ptr) = object.proto.as_object(){
                ptr = next_ptr
            }
            else{
                break;
            } 
        }
        // alright nothing found
        trap.err_not_found()
    }
    
    pub fn scope_value(&self, ptr:ScriptObject, key: LiveId, trap:&ScriptTrap)->ScriptValue{
        let mut ptr = ptr;
        let key = key.into();
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(set) = object.map.get(&key){
                return set.value
            }
            if object.tag.get_storage_type().is_vec2(){
                for kv in object.vec.iter().rev(){
                    if kv.key == key{
                        return kv.value;
                    }
                }
            }
            if let Some(next_ptr) = object.proto.as_object(){
                ptr = next_ptr
            }
            else{
                break;
            } 
        }
        // alright nothing found
        trap.err_not_found()
    }
    
    pub fn def_scope_value(&mut self, ptr:ScriptObject, key:LiveId, value:ScriptValue)->Option<ScriptObject>{
        // if we already have this value we have to shadow the scope
        let object = &mut self.objects[ptr.index as usize];
        if let Some(_) = object.map.get(&key.into()){
            let new_scope = self.new_with_proto(ptr.into());
            let object = &mut self.objects[new_scope.index as usize];
            object.map.insert(key.into(), ScriptMapValue{
                tag: Default::default(),
                value
            });
            return Some(new_scope)
        }
        else{
            object.map.insert(key.into(), ScriptMapValue{
                tag: Default::default(),
                value
            });
            return None
        }
    }
        
    
    
    // Reading object values
    
    
    
    fn value_index(&self, ptr: ScriptObject, index: ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let object = &self.objects[ptr.index as usize];
        // most used path
        let index = index.as_index();
        if let Some(kv) = object.vec.get(index){
            return kv.value
        }
        trap.err_not_found()
    }
    
    fn value_prefixed(&self, ptr: ScriptObject, key: ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let object = &self.objects[ptr.index as usize];
        for kv in object.vec.iter().rev(){
            if kv.key == key{
                return kv.value;
            }
        }
        trap.err_not_found()
    }
    
    fn value_deep_map(&self, obj_ptr:ScriptObject, key: ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let mut ptr = obj_ptr;
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(value) = object.map_get(&key){
                return value
            }
            if let Some(next_ptr) = object.proto.as_object(){
                ptr = next_ptr
            }
            else{
                break;
            }
        }
        trap.err_not_found()
    }
    
    fn value_deep(&self, obj_ptr:ScriptObject, key: ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let mut ptr = obj_ptr;
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(value) = object.map_get(&key){
                return value
            }
            for kv in object.vec.iter().rev(){
                if kv.key.is_string_like() {
                   if let Some(id) = key.as_id(){
                       if id.as_string(|ks|{
                           if let Some(ks) = ks{
                               self.check_intern_string(ks)
                           }
                           else{
                               NIL
                           }
                       }) == kv.key{
                           return kv.value
                       }
                   } 
                }
                if kv.key == key{
                    return kv.value;
                }
            }
            if let Some(next_ptr) = object.proto.as_object(){
                ptr = next_ptr
            }
            else{
                break;
            }
        }
        trap.err_not_found()
    }

    pub fn object_method(&self, ptr:ScriptObject, key:ScriptValue, trap:&ScriptTrap)->ScriptValue{
        return self.value_deep_map(ptr, key, trap)
    }
    
    pub fn value_path(&self, ptr:ScriptObject, keys:&[LiveId], trap:&ScriptTrap)->ScriptValue{
        let mut value:ScriptValue = ptr.into();
        for key in keys{
            if let Some(obj) = value.as_object(){
                value = self.value(obj, key.into(), trap);
            }
            else{
                return trap.err_not_found();
            }
        }
        value
    }
    
    pub fn value(&self, ptr:ScriptObject, key:ScriptValue, trap:&ScriptTrap)->ScriptValue{
        if key.is_unprefixed_id(){
            return self.value_deep(ptr, key, trap)
        }
        if key.is_index(){
            return self.value_index(ptr, key, trap)
        }
        if key.is_prefixed_id(){
            return self.value_prefixed(ptr, key, trap)
        }
        if key.is_inline_string() || key.is_string() || key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            return self.value_deep(ptr, key, trap)
        }
        // TODO implement string lookup
        trap.err_not_found()
    }
    
    #[inline]
    pub fn value_apply_if_dirty(&mut self, obj:ScriptValue, key:ScriptValue)->Option<ScriptValue>{
        if let Some(ptr) = obj.as_object(){
            // only do top level if dirty
            let object = &mut self.objects[ptr.index as usize];
            if let Some(value) = object.map_get_if_dirty(&key){
                return Some(value)
            }
            // if we havent been applied before apply prototype chain too
            if !object.tag.is_first_applied(){
                let mut ptr = if let Some(next_ptr) = object.proto.as_object(){
                    next_ptr
                }
                else{
                    return None
                };
                loop{
                    let object = &self.objects[ptr.index as usize];
                    // skip the last prototype, since its already default valued on the Rust object
                    if !object.proto.is_object(){
                        return None
                    }
                    if let Some(value) = object.map_get(&key){
                        return Some(value)
                    }
                    if let Some(next_ptr) = object.proto.as_object(){
                        ptr = next_ptr
                    }
                    else{
                        return None
                    }
                }
            }
        }
        None    
    }
        
    
    // Vec Reading
    
    
    
    pub fn vec_key_value(&self, ptr:ScriptObject, index:usize, trap:&ScriptTrap)->ScriptVecValue{
        let object = &self.objects[ptr.index as usize];
        
        if let Some(value) = object.vec.get(index){
            return *value
        }
        ScriptVecValue{key:NIL, value:trap.err_vec_bound()}
    }
        
    pub fn vec_value(&self, ptr:ScriptObject, index:usize, trap:&ScriptTrap)->ScriptValue{
        let object = &self.objects[ptr.index as usize];
        if let Some(kv) = object.vec.get(index){
            return kv.value
        }
        trap.err_vec_bound()
    }
    
    pub fn vec_value_if_exist(&self, ptr:ScriptObject, index:usize)->Option<ScriptValue>{
        let object = &self.objects[ptr.index as usize];
        if let Some(kv) = object.vec.get(index){
            Some(kv.value)
        }
        else{
            None
        }
    }
        
    pub fn vec_len(&self, ptr:ScriptObject)->usize{
        let object = &self.objects[ptr.index as usize];
        object.vec.len()
    }
    
    pub fn vec_ref(&self, ptr:ScriptObject)->&[ScriptVecValue]{
        let object = &self.objects[ptr.index as usize];
        &object.vec
    }
    
    // Vec Writing
    
    
        
    pub fn vec_insert_value_at(&mut self, _ptr:ScriptObject, _key:ScriptValue, _value:ScriptValue, _before:bool, _ip:&ScriptTrap)->ScriptValue{
        NIL
    }
        
    pub fn vec_insert_value_begin(&mut self, _ptr:ScriptObject, _key:ScriptValue, _value:ScriptValue, _ip:&ScriptTrap)->ScriptValue{
        NIL
    }
        
    pub fn vec_push_vec(&mut self, target:ScriptObject, source:ScriptObject, trap:&ScriptTrap)->ScriptValue{
        if target == source{
            return trap.err_invalid_args()
        }
        let (target, source) = if target.index > source.index{
            let (o1, o2) = self.objects.split_at_mut(target.index as _);
            (&mut o2[0], &mut o1[source.index as usize])                    
        }else{
            let (o1, o2) = self.objects.split_at_mut(source.index as _);
            (&mut o1[target.index as usize], &mut o2[0])                    
        };
        if target.tag.is_vec_frozen(){
            return trap.err_vec_frozen()
        }
        target.push_vec_from_other(source);
        NIL
    }
        
    pub fn vec_push_vec_of_vec(&mut self, target:ScriptObject, source:ScriptObject, map:bool, trap:&ScriptTrap)->ScriptValue{
        let len = self.objects[source.index as usize].vec.len();
        for i in 0..len{
            if let Some(source) = self.objects[source.index as usize].vec[i].value.as_object(){
                if target == source{
                    return trap.err_invalid_args()
                }
                let (target, source) = if target.index > source.index{
                    let (o1, o2) = self.objects.split_at_mut(target.index as _);
                    (&mut o2[0], &mut o1[source.index as usize])
                }else{
                    let (o1, o2) = self.objects.split_at_mut(source.index as _);
                    (&mut o1[target.index as usize], &mut o2[0])
                };
                if target.tag.is_vec_frozen(){
                    return trap.err_vec_frozen()
                }
                target.push_vec_from_other(source);
                if map{
                    target.merge_map_from_other(source);
                }
            }
        }
        NIL
    }
        
    pub fn vec_push(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return trap.err_vec_frozen()
        }
        object.vec.push(ScriptVecValue{key,value});
        if let Some(obj) = value.as_object(){
            let object = &mut self.objects[obj.index as usize];
            object.tag.set_reffed();
        }
        NIL
    }
    
    pub fn vec_push_unchecked(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue){
        let object = &mut self.objects[ptr.index as usize];
        object.vec.push(ScriptVecValue{key,value});
        if let Some(obj) = value.as_object(){
            let object = &mut self.objects[obj.index as usize];
            object.tag.set_reffed();
        }
    }
            
    pub fn vec_remove(&mut self, ptr:ScriptObject, index:usize, trap:&ScriptTrap)->ScriptVecValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return ScriptVecValue{key:NIL, value:trap.err_vec_frozen()}
        }
        if index >= object.vec.len(){
            return ScriptVecValue{key:NIL, value:trap.err_vec_bound()}
        }
        object.vec.remove(index)
    }
        
    pub fn vec_pop(&mut self, ptr:ScriptObject, trap:&ScriptTrap)->ScriptVecValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return ScriptVecValue{key:NIL, value:trap.err_vec_frozen()}
        }
        object.vec.pop().unwrap_or_else(||  ScriptVecValue{key:NIL, value:trap.err_vec_bound()})
    }
    
    
    
        
    // Functions
        
        
        
    pub fn set_fn(&mut self, ptr: ScriptObject, fnptr: ScriptFnPtr){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_fn(fnptr);
    }
            
    pub fn as_fn(&self, ptr: ScriptObject,)->Option<ScriptFnPtr>{
        let object = &self.objects[ptr.index as usize];
        object.tag.as_fn()
    }
            
    pub fn is_fn(&self, ptr: ScriptObject,)->bool{
        let object = &self.objects[ptr.index as usize];
        object.tag.is_fn()
    }
            
    pub fn set_reffed(&mut self, ptr: ScriptObject,){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_reffed();
    }
            
    pub fn parent_as_fn(&self, ptr: ScriptObject,)->Option<ScriptFnPtr>{
        let object = &self.objects[ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            let fn_object = &self.objects[ptr.index as usize];
            fn_object.tag.as_fn()
        }
        else{
            None
        }
    }   
        
    pub fn unnamed_fn_arg(&mut self, top_ptr:ScriptObject, value:ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let object = &self.objects[top_ptr.index as usize];
        
        // which arg number?
        let index = object.map_len();
        
        if let Some(ptr) = object.proto.as_object(){
            let proto_object = &self.objects[ptr.index as usize];
            if let Some(kv) = proto_object.vec.get(index){
                let key = kv.key;
                if let Some(def) = object.vec.get(index){
                    if !def.value.is_nil() && def.value.value_type().to_redux() != value.value_type().to_redux(){
                        return trap.err_invalid_arg_type()
                    }
                }
                self.objects[top_ptr.index as usize].map_insert(key, value);
                if let Some(obj) = value.as_object(){
                    let object = &mut self.objects[obj.index as usize];
                    object.tag.set_reffed();
                }
                return NIL
            }
        }
        // only allow if we are varargs
        self.objects[top_ptr.index as usize].vec.push(ScriptVecValue{key:NIL, value});
        return NIL
    }
    
    pub fn named_fn_arg(&mut self, top_ptr:ScriptObject, key:ScriptValue, value:ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let object = &self.objects[top_ptr.index as usize];
            
        if let Some(ptr) = object.proto.as_object(){
            let object = &self.objects[ptr.index as usize];
            for kv in object.vec.iter(){
                if kv.key == key{
                    if !kv.value.is_nil() && kv.value.value_type().to_redux() != value.value_type().to_redux(){
                        return trap.err_invalid_arg_type()
                    }
                    self.objects[top_ptr.index as usize].map_insert(key, value);
                    return NIL    
                }
            }
            return trap.err_invalid_arg_name() 
        }
        trap.err_unexpected()
    }
    
    pub fn push_all_fn_args(&mut self, top_ptr:ScriptObject, args:&[ScriptValue], trap:&ScriptTrap)->ScriptValue{
        let object = &self.objects[top_ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            for (index, value) in args.iter().enumerate(){
                let object = &self.objects[ptr.index as usize];
                if let Some(v1) = object.vec.get(index){
                    let key = v1.key;
                    // typecheck against default arg
                    if let Some(def) = object.vec.get(index){
                        if !def.value.is_nil() && def.value.value_type().to_redux() != value.value_type().to_redux(){
                            return trap.err_invalid_arg_type()
                        }
                    }
                    self.objects[top_ptr.index as usize].map_insert(key, *value);
                    if let Some(obj) = value.as_object(){
                        let object = &mut self.objects[obj.index as usize];
                        object.tag.set_reffed();
                    }
                }
                else{
                    self.objects[top_ptr.index as usize].vec.push(ScriptVecValue{key:NIL, value:*value});
                }
            }
            return NIL
        }
        trap.err_unexpected()
    }
    
    
    
    // Debug and utility
    
    
    
        
    pub fn deep_eq(&self, a:ScriptValue, b:ScriptValue)->bool{
        if a == b{
            return true
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
                            }
                            else{
                                return Some(false)
                            }
                            None
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
                if self.arrays[arr1.index as usize].storage == self.arrays[arr2.index as usize].storage{
                    return true
                }
            }
            return false
        }
        false
    }
        
    pub fn print(&self, value:ScriptValue){
        if let Some(obj) = value.as_object(){
            let object = &self.objects[obj.index as usize];
            if object.tag.is_script_fn(){
                print!("Fn");
            }
            else if object.tag.is_native_fn(){
                print!("Native");
            }
            let mut ptr = obj;
            // scan up the chain to set the proto value
            print!("{{");
            let mut first = true;
            loop{
                let object = &self.objects[ptr.index as usize];
                object.map_iter(|key,value|{
                    if !first{print!(", ")}
                    if key != NIL{
                        print!("{}:", key)
                    }
                    self.print(value);
                    first = false;
                });
                for kv in object.vec.iter(){
                    if !first{print!(", ")}
                    if kv.key != NIL{
                        print!("{}:", kv.key)
                    }
                    self.print(kv.value);
                    first = false;
                }
                if let Some(next_ptr) = object.proto.as_object(){
                    if !first{print!(",")}
                    print!("^");
                    ptr = next_ptr
                }
                else{
                    print!("/{}", object.proto);
                    break;
                }
            }
            print!("}}");
        }
        else if let Some(arr) = value.as_array(){
            let array = &self.arrays[arr.index as usize];
            let len = array.storage.len();
            print!("[");
            for i in 0..len{
                if i!=0{print!(", ")}
                self.print(array.storage.index(i).unwrap());
            }
            print!("]");
        }
        else if let Some(s) = value.as_string(){
            let s = &self.strings[s.index as usize].string;
            print!("\"");
            print!("{}", s);
            print!("\"");
        }
        else if value.as_inline_string(|s|{
            print!("\"");
            print!("{}", s);
            print!("\"");
        }).is_some(){}
        else {
            print!("{}", value)
        }
    }
    
    pub fn write_json(&mut self, value:ScriptValue)->ScriptValue{
        self.new_string_with(|heap, s|{
            heap.write_json_inner(value, s);
        })
    }
    
    pub fn write_json_inner(&self, value:ScriptValue, out:&mut String){
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
                    self.write_json_inner(key, out);
                    out.push(':');
                    self.write_json_inner(value, out);
                    first = false;
                });
                for kv in object.vec.iter(){
                    if !first{out.push(',')}
                    first = false;
                    self.write_json_inner(kv.key, out);
                    out.push(':');
                    self.write_json_inner(kv.value, out);
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
                    self.write_json_inner(value, out);
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
            // alright. this is json eh. so.
        }
        else if let Some(s) = value.as_string(){
            let s = &self.strings[s.index as usize].string;
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
        else if let Some(v) = value.as_f64(){
            write!(out, "{}", v).ok();
        }
        else {
            out.push_str("null");
        }
    }
        
    // memory  usage
    pub fn objects_len(&self)->usize{
        self.objects.len()
    }
}
