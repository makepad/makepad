use crate::makepad_id::id::*;
use crate::value::*;
use crate::makepad_id_derive::*;
use std::fmt::Write;
use crate::object::*;
use crate::string::*;
use crate::thread::*;
use crate::traits::*;
use std::collections::HashMap;

pub struct ScriptHeap{
    pub modules: Object,
    pub(crate) mark_vec: Vec<usize>,
    pub(crate) objects: Vec<ObjectData>,
    pub(crate) roots: Vec<usize>,
    pub(crate) objects_free: Vec<usize>,
    pub(crate) strings: Vec<HeapStringData>,
    pub(crate) strings_free: Vec<usize>,
    pub(crate) type_check: Vec<ScriptTypeCheck>,
    pub(crate) type_index: HashMap<ScriptTypeId, ScriptTypeIndex>,
}

impl ScriptHeap{
    
    pub fn empty()->Self{
        let mut v = Self{
            modules: Object{index:0},
            mark_vec: Default::default(),
            objects: vec![Default::default()],
            roots: vec![0],
            objects_free: Default::default(),
            // index 0 is always an empty string
            strings: vec![Default::default()],
            strings_free: Default::default(),
            type_check: Default::default(),
            type_index: Default::default()
        };
        v.objects[0].tag.set_alloced();
        v.objects[0].tag.set_static();
        v.objects[0].tag.freeze();
        v.modules = v.new_with_proto(id!(mod).into()); 
        v
    }
    
    
    // New objects
    
    
    
    pub fn new(&mut self)->Object{
        if let Some(index) = self.objects_free.pop(){
            let object = &mut self.objects[index];
            object.tag.set_alloced();
            object.proto = id!(object).into();
            Object{index: index as _}
        }
        else{
            let index = self.objects.len();
            let mut object = ObjectData::default();
            object.tag.set_alloced();
            object.proto = id!(object).into();
            self.objects.push(object);
            Object{index: index as _}
        }
    }
    
    
    pub fn new_with_proto_check(&mut self, proto:Value, trap:&ScriptTrap)->Object{
        if let Some(ptr) = proto.as_object(){
            let object = &mut self.objects[ptr.index as usize];
            if object.tag.is_notproto(){
                trap.err_not_proto();
                return Object::ZERO;
            }
        }
        self.new_with_proto(proto)
    }
    
    pub fn new_with_proto(&mut self, proto:Value)->Object{
        let (proto_fwd, proto_index) = if let Some(ptr) = proto.as_object(){
            let object = &mut self.objects[ptr.index as usize];
            object.tag.set_reffed();
            (object.tag.proto_fwd(), ptr.index as usize)
        }
        else{
            let ptr = self.new();
            self.objects[ptr.index as usize].proto = proto;
            return ptr
        };
                        
        if let Some(index) = self.objects_free.pop(){
            let (object, proto_object) = if index > proto_index{
                let (o1, o2) = self.objects.split_at_mut(index);
                (&mut o2[0], &mut o1[proto_index])                    
            }else{
                let (o1, o2) = self.objects.split_at_mut(proto_index);
                (&mut o1[index], &mut o2[0])                    
            };
            object.tag.set_proto_fwd(proto_fwd);
            object.proto = proto;
            // only copy vec if we are 'auto' otherwise we proto inherit normally
            if proto_object.tag.get_storage_type().is_auto(){
                object.vec.extend_from_slice(&proto_object.vec);
            }
            Object{index: index as _}
        }
        else{
            let index = self.objects.len();
            let mut object = ObjectData::with_proto(proto);
            object.tag.set_proto_fwd(proto_fwd);
            let proto_object = &self.objects[proto_index];
            if proto_object.tag.get_storage_type().is_auto(){
                object.vec.extend_from_slice(&proto_object.vec);
            }
            self.objects.push(object);
            Object{index: index as _}
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
    
    pub fn type_matches_id(&self, ptr:Object, type_id:ScriptTypeId)->bool{
        let obj = &self.objects[ptr.index as usize];
        if let Some(ti) = obj.tag.as_type_index(){
            if let Some(object) = &self.type_check[ti.0 as usize].object{
                return object.type_id == type_id
            }
        }
        false
    }
    
    pub fn new_if_reffed(&mut self, ptr:Object)->Object{
        let obj = &self.objects[ptr.index as usize];
        if obj.tag.is_reffed(){
            let proto = obj.proto;
            return self.new_with_proto(proto);
        }
        return ptr;
    }
    
    pub fn new_module(&mut self, id:Id)->Object{
        let md = self.new_with_proto(id.into());
        self.set_value_def(self.modules, id.into(), md.into());
        md
    }
    
    pub fn new_empty_string(&mut self)->HeapString{
        if let Some(index) = self.strings_free.pop(){
            self.strings[index].tag.set_alloced();
            HeapString{index: index as _}
        }
        else{
            let index = self.strings.len();
            let mut string = HeapStringData::default();
            string.tag.set_alloced();
            self.strings.push(string);
            HeapString{index: index as _}
        }
    }
        
    pub fn new_string_from_str(&mut self,value:&str)->HeapString{
        self.new_string_with(|_,out|{
            out.push_str(value);
        })
    }
            
    pub fn new_string_with<F:FnOnce(&mut Self, &mut String)>(&mut self,cb:F)->HeapString{
        let mut out = String::new();
        let ptr = self.new_empty_string();
        std::mem::swap(&mut out, &mut self.strings[ptr.index as usize].string);
        cb(self, &mut out);
        std::mem::swap(&mut out, &mut self.strings[ptr.index as usize].string);
        ptr
    }
            
    pub fn new_string_from_string(&mut self, string:String)->HeapString{
        let index = self.strings.len();
        self.strings.push(HeapStringData{
            tag: Default::default(),
            string
        });
        HeapString{index: index as u32}
    }
    
    pub fn null_string(&self)->HeapString{
        HeapString{index: 0}
    }
        
    
    
    // Accessors
    
            
    pub fn has_proto(&mut self, ptr:Object, rhs:Value)->bool{
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
     
    pub fn proto(&self, ptr:Object)->Value{
        self.objects[ptr.index as usize].proto
    }
    
    pub fn root_proto(&self, ptr:Object)->Value{
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
                
    //pub fn object(&self, ptr:Object)->&Object{
    //    &self.objects[ptr.index as usize]
    //}
        
            
    pub fn value_string(&self, value: Value)->&str{
        if let Some(ptr) = value.as_string(){
            return self.string(ptr)
        }
        else{
            ""
        }
    }
    
    pub fn string(&self, ptr: HeapString)->&str{
        &self.strings[ptr.index as usize].string
    }
        
    pub fn cast_to_string(&self, v:Value, out:&mut String){
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
            write!(out, "[Object]").ok();
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
        
    pub fn cast_to_f64(&self, v:Value, ip:ScriptIp)->f64{
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
            Value::from_f64_traced_nan(f64::NAN, ip).as_f64().unwrap()
        }
    }
    
    pub fn cast_to_bool(&self, v:Value)->bool{
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
    
    pub fn swap_string(&mut self, ptr: HeapString, swap:&mut String){
        std::mem::swap(swap, &mut self.strings[ptr.index as usize].string);
    }
    
    pub fn mut_string(&mut self, ptr: HeapString)->&mut String{
        &mut self.strings[ptr.index as usize].string
    }
    
    
    
    // Setting object flags
    
    
    
    
    pub fn set_object_deep(&mut self, ptr:Object){
         self.objects[ptr.index as usize].tag.set_deep()
    }
    
    pub fn set_object_storage_type(&mut self, ptr:Object, ty: ObjectStorageType){
        self.objects[ptr.index as usize].set_storage_type(ty)
    }
    
    pub fn set_first_applied_and_clean(&mut self, ptr:Object){
        self.objects[ptr.index as usize].tag.set_first_applied_and_clean()
    }
            
    pub fn clear_object_deep(&mut self, ptr:Object){
        self.objects[ptr.index as usize].tag.clear_deep()
    }
    
    pub fn freeze(&mut self, ptr: Object){
        self.objects[ptr.index as usize].tag.freeze()
    }
    
    pub fn freeze_module(&mut self, ptr: Object){
        self.objects[ptr.index as usize].tag.freeze_module()
    }
            
    pub fn freeze_component(&mut self, ptr: Object){
        self.objects[ptr.index as usize].tag.freeze_component()
    }
            
    pub fn freeze_api(&mut self, ptr: Object){
        self.objects[ptr.index as usize].tag.freeze_api()
    }
    
    pub fn freeze_with_type(&mut self, obj: Object, ty:ScriptTypeIndex){
        let object = &mut  self.objects[obj.index as usize];
        object.tag.set_tracked();
        object.tag.set_type_index(ty);
        object.tag.freeze_component();
    }
    // Writing object values 
        
        
    
    pub(crate) fn force_value_in_map(&mut self, ptr:Object, key: Value, this:Value){
        let object = &mut self.objects[ptr.index as usize];
        object.map_insert(key, this);
    }            
        
    fn set_value_index(&mut self, ptr: Object, index:Value, value: Value, trap:&ScriptTrap)->Value{
        // alright so. now what.
        let object = &mut self.objects[ptr.index as usize];
        let ty = object.tag.get_storage_type();
        
        if object.tag.is_vec_frozen(){ // has rw flags
            return trap.err_vec_frozen()
        }
        if ty.uses_vec2(){
            let index = index.as_index() * 2;
            if index + 1 >= object.vec.len(){
                object.vec.resize(index + 2, NIL);
            }
            object.vec[index] = NIL;
            object.vec[index+1] = value; 
            return NIL
        }
        if ty.is_vec1(){
            let index = index.as_index();
            if index>= object.vec.len(){
                object.vec.resize(index, NIL);
            }
            object.vec[index] = value;
            return NIL 
        }
        if ty.is_map(){
            object.map_insert(index, value);
            return NIL
        }
        if ty.is_typed(){ // typed array
            println!("Implement typed array set value");
            //todo IMPLEMENT IT
            return NIL
        }
        trap.err_unexpected()
    }
            
    fn set_value_prefixed(&mut self, ptr: Object, key: Value, value: Value, trap:&ScriptTrap)->Value{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return trap.err_vec_frozen()
        }
        for chunk in object.vec.rchunks_mut(2){
            if chunk[0] == key{
                chunk[1] = value;
                return NIL
            }
        }
        // just append it
        object.vec.extend_from_slice(&[key, value]);
        NIL
    }
        
    fn set_value_deep(&mut self, ptr:Object, key: Value, value: Value, trap:&ScriptTrap)->Value{
        let mut ptr = ptr;
        loop{
            let object = &mut self.objects[ptr.index as usize];
            if object.tag.is_frozen(){
                return trap.err_frozen()
            }
            if object.tag.get_storage_type().is_vec2(){
                for chunk in object.vec.rchunks_mut(2){
                    if chunk[0] == key{
                        chunk[1] = value;
                        return NIL
                    }
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
            object.vec.extend_from_slice(&[key, value]);
        }
        else{
            object.map_insert(key, value);
        }
        NIL
    }
    
    fn validate_type(&self, lhs:Value, rhs:Value)->bool{
        lhs.value_type().to_redux() == rhs.value_type().to_redux()
    }
    
    fn set_value_shallow_checked(&mut self, ptr:Object, key:Value, key_id:Id, value:Value, trap:&ScriptTrap)->Value{
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
                    for chunk in object.vec.rchunks(2){
                        if chunk[0] == key{
                            if !self.validate_type(chunk[1], value){
                                return trap.err_invalid_prop_type()
                            }
                            return self.set_value_shallow(ptr, key, value, trap);
                        }
                    }
                }
                if let Some(set_value) = object.map_get(&key){
                    if !self.validate_type(*set_value, value){
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
                for chunk in object.vec.rchunks_mut(2){
                    if chunk[0] == key{
                        return trap.err_key_already_exists()
                    }
                }
                object.vec.extend_from_slice(&[key, value]);
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
    
    fn set_value_shallow(&mut self, ptr:Object, key:Value, value:Value, _trap:&ScriptTrap)->Value{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.get_storage_type().is_vec2(){
            for chunk in object.vec.rchunks_mut(2){
                if chunk[0] == key{
                    chunk[1] = value;
                    return NIL
                }
            }
            object.vec.extend_from_slice(&[key, value]);
            return NIL
        }
        object.map_insert(key, value);
        NIL
    }
            
    
    pub fn set_value_def(&mut self, ptr:Object, key:Value, value:Value)->Value{
        self.set_value(ptr, key, value, &mut ScriptTrap::default())
    }
    
    pub fn set_value(&mut self, ptr:Object, key:Value, value:Value, trap:&ScriptTrap)->Value{
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
    
    
    pub fn set_scope_value(&mut self, ptr:Object, key:Id, value:Value, trap:&ScriptTrap)->Value{
        let mut ptr = ptr;
        loop{
            let object = &mut self.objects[ptr.index as usize];
            if let Some(set_value) = object.map.get_mut(&key.into()){
                *set_value = value;
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
    
    pub fn scope_value(&self, ptr:Object, key: Id, trap:&ScriptTrap)->Value{
        let mut ptr = ptr;
        let key = key.into();
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(value) = object.map.get(&key){
                return *value
            }
            if object.tag.get_storage_type().is_vec2(){
                for chunk in object.vec.rchunks(2){
                    if chunk[0] == key{
                        return chunk[1]
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
    
    pub fn def_scope_value(&mut self, ptr:Object, key:Id, value:Value)->Option<Object>{
        // if we already have this value we have to shadow the scope
        let object = &mut self.objects[ptr.index as usize];
        if let Some(_) = object.map.get(&key.into()){
            let new_scope = self.new_with_proto(ptr.into());
            let object = &mut self.objects[new_scope.index as usize];
            object.map.insert(key.into(), value);
            return Some(new_scope)
        }
        else{
            object.map.insert(key.into(), value);
            return None
        }
    }
        
    
    
    // Reading object values
    
    
    
    fn value_index(&self, ptr: Object, index: Value, trap:&ScriptTrap)->Value{
        let object = &self.objects[ptr.index as usize];
        
        let ty = object.tag.get_storage_type();
        // most used path
        if ty.uses_vec2(){
            let index = index.as_index();
            if let Some(value) = object.vec.get(index * 2 + 1){
                return *value
            }
            else{
                return trap.err_not_found()
            }
        }
        if ty.is_vec1(){
            let index = index.as_index();
            if let Some(value) = object.vec.get(index){
                return *value
            }
            else{
                return trap.err_not_found()
            }
        }
        if ty.is_typed(){ // typed access to the vec
            //todo IMPLEMENT IT
        }
        if ty.is_map(){
            if let Some(value) = object.map_get(&index){
                return *value
            }
            else{
                return trap.err_not_found()
            }
        }
        trap.err_not_found()
    }
    
    fn value_prefixed(&self, ptr: Object, key: Value, trap:&ScriptTrap)->Value{
        let object = &self.objects[ptr.index as usize];
        if object.tag.get_storage_type().uses_vec2(){
            for chunk in object.vec.rchunks(2){
                if chunk[0] == key{
                    return chunk[1]
                }
            }
        }
        trap.err_not_found()
    }
    
    fn value_deep_map(&self, obj_ptr:Object, key: Value, trap:&ScriptTrap)->Value{
        let mut ptr = obj_ptr;
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(value) = object.map_get(&key){
                return *value
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
    
    fn value_deep(&self, obj_ptr:Object, key: Value, trap:&ScriptTrap)->Value{
        let mut ptr = obj_ptr;
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(value) = object.map_get(&key){
                return *value
            }
            if object.tag.get_storage_type().is_vec2(){
                for chunk in object.vec.rchunks(2){
                    if chunk[0] == key{
                        return chunk[1]
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
        trap.err_not_found()
    }

        
    pub fn object_method(&self, ptr:Object, key:Value, trap:&ScriptTrap)->Value{
        return self.value_deep_map(ptr, key, trap)
    }
    
    pub fn value_path(&self, ptr:Object, keys:&[Id], trap:&ScriptTrap)->Value{
        let mut value:Value = ptr.into();
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
    
    pub fn value(&self, ptr:Object, key:Value, trap:&ScriptTrap)->Value{
        if key.is_unprefixed_id(){
            return self.value_deep(ptr, key, trap)
        }
        if key.is_index(){
            return self.value_index(ptr, key, trap)
        }
        if key.is_prefixed_id(){
            return self.value_prefixed(ptr, key, trap)
        }
        if key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            return self.value_deep(ptr, key, trap)
        }
        // TODO implement string lookup
        trap.err_not_found()
    }
    
    #[inline]
    pub fn value_apply_if_dirty(&mut self, obj:Value, key:Value)->Option<Value>{
        if let Some(ptr) = obj.as_object(){
            // only do top level if dirty
            let object = &mut self.objects[ptr.index as usize];
            if let Some(value) = object.map_get_if_dirty(&key){
                return Some(*value)
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
                        return Some(*value)
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
    
    
    
    pub fn vec_key_value(&self, ptr:Object, index:usize, trap:&ScriptTrap)->(Value,Value){
        let object = &self.objects[ptr.index as usize];
        if object.tag.get_storage_type().has_paired_vec(){
            if let Some(value) = object.vec.get(index * 2 + 1){
                return (object.vec[index * 2], *value)
            }
        }
        else if object.tag.get_storage_type().is_vec1(){
            if let Some(value) = object.vec.get(index){
                return (NIL,*value)
            }
        }
        (NIL, trap.err_vec_bound())
    }
        
    pub fn vec_value(&self, ptr:Object, index:usize, trap:&ScriptTrap)->Value{
        let object = &self.objects[ptr.index as usize];
        if object.tag.get_storage_type().has_paired_vec(){
            if let Some(value) = object.vec.get(index * 2 + 1){
                return *value
            }
        }
        else if object.tag.get_storage_type().is_vec1(){
            if let Some(value) = object.vec.get(index){
                return *value
            }
        }
        return trap.err_vec_bound()
    }
    
    pub fn vec_value_if_exist(&self, ptr:Object, index:usize)->Option<Value>{
        let object = &self.objects[ptr.index as usize];
        if object.tag.get_storage_type().has_paired_vec(){
            if let Some(value) = object.vec.get(index * 2 + 1){
                return Some(*value)
            }
        }
        else if object.tag.get_storage_type().is_vec1(){
            if let Some(value) = object.vec.get(index){
                return Some(*value)
            }
        }
        return None
    }
        
    pub fn vec_len(&self, ptr:Object)->usize{
        let object = &self.objects[ptr.index as usize];
        if object.tag.get_storage_type().has_paired_vec(){
            object.vec.len() >> 1
        }
        else if object.tag.get_storage_type().is_vec1(){
            object.vec.len()
        }
        else{
            0
        }
    }
    
    
    
    // Vec Writing
    
    
        
    pub fn vec_insert_value_at(&mut self, _ptr:Object, _key:Value, _value:Value, _before:bool, _ip:&mut ScriptTrap)->Value{
        NIL
    }
        
    pub fn vec_insert_value_begin(&mut self, _ptr:Object, _key:Value, _value:Value, _ip:&mut ScriptTrap)->Value{
        NIL
    }
        
    pub fn vec_push_vec(&mut self, target:Object, source:Object, trap:&ScriptTrap)->Value{
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
        
    pub fn vec_push_vec_of_vec(&mut self, target:Object, source:Object, map:bool, trap:&ScriptTrap)->Value{
        let len = self.objects[source.index as usize].vec.len();
        for i in 0..len{
            if let Some(source) = self.objects[source.index as usize].vec[i].as_object(){
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
        
    pub fn vec_push(&mut self, ptr: Object, key: Value, value: Value, trap:&ScriptTrap)->Value{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return trap.err_vec_frozen()
        }
        let ty = object.tag.get_storage_type();
        if ty.has_paired_vec(){
            object.vec.extend_from_slice(&[key, value]);
            if let Some(obj) = value.as_object(){
                let object = &mut self.objects[obj.index as usize];
                object.tag.set_reffed();
            }
        }
        else if ty.is_vec1(){
            object.vec.push(value);
        }
        else if ty.is_typed(){
            return trap.err_not_impl()
        }
        else{
            return trap.err_not_impl()
        }
        NIL
    }
            
    pub fn vec_remove(&mut self, ptr:Object, index:usize, trap:&ScriptTrap)->Value{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return trap.err_vec_frozen()
        }
        if object.tag.get_storage_type().has_paired_vec(){
            if index >= object.vec.len() * 2{
                return trap.err_vec_bound()
            }
            object.vec.remove(index * 2);
            return object.vec.remove(index * 2);
        }
        else if object.tag.get_storage_type().is_vec1(){
            if index >= object.vec.len(){
                return trap.err_vec_bound()
            }
            return object.vec.remove(index);
        }
        else{
            NIL
        }
    }
        
    pub fn vec_pop(&mut self, ptr:Object, trap:&ScriptTrap)->Value{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return trap.err_vec_frozen()
        }
        if object.tag.get_storage_type().has_paired_vec(){
            object.vec.pop();
            
            object.vec.pop().unwrap_or_else(|| trap.err_vec_bound())
        }
        else if object.tag.get_storage_type().is_vec1(){
            object.vec.pop().unwrap_or_else(|| trap.err_vec_bound())
        }
        else{
            trap.err_vec_bound()
        }
    }
    
    
    
        
    // Functions
        
        
        
    pub fn set_fn(&mut self, ptr: Object, fnptr: ScriptFnPtr){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_fn(fnptr);
    }
            
    pub fn as_fn(&self, ptr: Object,)->Option<ScriptFnPtr>{
        let object = &self.objects[ptr.index as usize];
        object.tag.as_fn()
    }
            
    pub fn is_fn(&self, ptr: Object,)->bool{
        let object = &self.objects[ptr.index as usize];
        object.tag.is_fn()
    }
            
    pub fn set_reffed(&mut self, ptr: Object,){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_reffed();
    }
            
    pub fn parent_as_fn(&self, ptr: Object,)->Option<ScriptFnPtr>{
        let object = &self.objects[ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            let fn_object = &self.objects[ptr.index as usize];
            fn_object.tag.as_fn()
        }
        else{
            None
        }
    }   
        
    pub fn unnamed_fn_arg(&mut self, top_ptr:Object, value:Value, trap:&ScriptTrap)->Value{
        let object = &self.objects[top_ptr.index as usize];
        
        // which arg number?
        let index = object.map_len();
        
        if let Some(ptr) = object.proto.as_object(){
            let object = &self.objects[ptr.index as usize];
            if let Some(key) = object.vec.get(index*2){
                let key = *key;
                if let Some(defvalue) = object.vec.get(index*2 + 1){
                    if !defvalue.is_nil() && defvalue.value_type().to_redux() != value.value_type().to_redux(){
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
        self.objects[top_ptr.index as usize].vec.extend_from_slice(&[NIL, value]);
        return NIL
    }
    
        
    pub fn named_fn_arg(&mut self, top_ptr:Object, key:Value, value:Value, trap:&ScriptTrap)->Value{
        let object = &self.objects[top_ptr.index as usize];
            
        if let Some(ptr) = object.proto.as_object(){
            let object = &self.objects[ptr.index as usize];
            for chunk in object.vec.chunks(2){
                if chunk[0] == key{
                    if !chunk[1].is_nil() && chunk[1].value_type().to_redux() != value.value_type().to_redux(){
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
    
    pub fn push_all_fn_args(&mut self, top_ptr:Object, args:&[Value], trap:&ScriptTrap)->Value{
        let object = &self.objects[top_ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            for (index, value) in args.iter().enumerate(){
                let object = &self.objects[ptr.index as usize];
                if let Some(key) = object.vec.get(index*2){
                    let key = *key;
                    // typecheck against default arg
                    if let Some(defvalue) = object.vec.get(index*2 + 1){
                        if !defvalue.is_nil() && defvalue.value_type().to_redux() != value.value_type().to_redux(){
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
                    self.objects[top_ptr.index as usize].vec.extend_from_slice(&[NIL, *value]);
                }
            }
            return NIL
        }
        trap.err_unexpected()
    }
    
    
    
    // Debug and utility
    
    
    
        
    pub fn deep_eq(&self, a:Value, b:Value)->bool{
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
                            if !self.deep_eq(*a, *b){
                                return false
                            }
                        }
                        if oa.map_len() != ob.map_len(){
                            return false
                        }
                        if let Some(ret) = oa.map_iter_ret(|k,v1|{
                            if let Some(v2) = ob.map_get(&k){
                                if !self.deep_eq(v1, *v2){
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
        else {
            self.shallow_eq(a, b)
        }
    }
        
    pub fn shallow_eq(&self, a:Value, b:Value)->bool{
        if a == b{
            return true
        }
        if let Some(cmp) = a.as_inline_string(|a|{
            if let Some(cmp) = b.as_inline_string(|b|{
                a == b
            }){return cmp}
            else{
                if let Some(b)  = b.as_string(){
                    self.string(b) == a
                }
                else{
                    false
                }
            }
        }){return cmp}
        else if let Some(a) = a.as_string(){
            let a = self.string(a);
            if let Some(cmp) = b.as_inline_string(|b|{
                a == b
            }){return cmp}
            else{
                if let Some(b)  = b.as_string(){
                    return self.string(b) == a
                }
            }
        }
        false
    }
    
    pub fn print_key_value(&self, key:Value, value:Value,str:&mut String){
        if let Some(obj) = value.as_object(){
            if !key.is_nil(){
                str.clear();self.cast_to_string(key, str);
                print!("{}:", str);
            }
            let object = &self.objects[obj.index as usize];
            if object.tag.is_script_fn(){
                print!("Fn");
                self.print(obj);
            }
            else if object.tag.is_native_fn(){
                print!("Native");
                self.print(obj);
            }
            else{
                self.print(obj);
            }
        }
        else{
            if !key.is_nil(){
                str.clear();self.cast_to_string(key, str);
                print!("{}:",str);
            }
            str.clear();self.cast_to_string(value, str);
            print!("{}",str);
        }
    }
    
    pub fn print(&self, set_ptr:Object){
        let mut ptr = set_ptr;
        let mut str = String::new();
        // scan up the chain to set the proto value
        print!("{{");
        let mut first = true;
        loop{
            let object = &self.objects[ptr.index as usize];
            object.map_iter(|key,value|{
                if !first{print!(",")}
                self.print_key_value(key, value, &mut str);
                first = false;
            });
            if object.tag.get_storage_type().has_paired_vec(){
                for chunk in object.vec.chunks(2){
                    if !first{print!(",")}
                    self.print_key_value(chunk[0], chunk[1], &mut str);
                    first = false;
                }
            }
            else if !object.tag.get_storage_type().is_typed(){
            }else{
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
    
    // memory  usage
    pub fn objects_len(&self)->usize{
        self.objects.len()
    }
}
