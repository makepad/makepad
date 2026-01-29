use crate::value::*;
use crate::traits::*;
use crate::heap::*;
use crate::makepad_live_id::*;
use crate::trap::*;
use crate::mod_shader::ShaderIoType;
use crate::*;

impl ScriptHeap{
        
        
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
        
    pub fn new_with_proto_checked(&mut self, proto:ScriptValue, trap:ScriptTrap)->ScriptObject{
        if let Some(ptr) = proto.as_object(){
            let object = &mut self.objects[ptr.index as usize];
            if object.tag.is_notproto(){
                err_not_proto!(trap);
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
            if proto_object.tag.is_auto(){
                object.vec.extend_from_slice(&proto_object.vec);
            }
            obj
        }
        else{
            let index = self.objects.len();
            let mut object = ScriptObjectData::with_proto(proto);
            object.tag.set_proto_fwd(proto_fwd);
            let proto_object = &self.objects[proto_index as usize];
            if proto_object.tag.is_auto(){
                object.vec.extend_from_slice(&proto_object.vec);
            }
            self.objects.push(object);
            ScriptObject{index: index as _}
        }
    }
    
    pub fn new_if_reffed(&mut self, ptr:ScriptObject)->ScriptObject{
        let obj = &self.objects[ptr.index as usize];
        if obj.tag.is_reffed(){
            let proto = obj.proto;
            return self.new_with_proto(proto);
        }
        return ptr;
    }
    
    // Object flagv
    
        
        
    pub fn set_object_deep(&mut self, ptr:ScriptObject){
        self.objects[ptr.index as usize].tag.set_deep()
    }
        
    pub fn set_object_storage_vec2(&mut self, ptr:ScriptObject){
        self.objects[ptr.index as usize].tag.set_vec2()
    }
        
    pub fn set_object_storage_auto(&mut self, ptr:ScriptObject){
        self.objects[ptr.index as usize].tag.set_auto()
    }
            
    pub fn set_object_pod_type(&mut self, ptr:ScriptObject, pt:ScriptPodType){
        self.objects[ptr.index as usize].tag.set_pod_type(pt)
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
                
    pub fn set_notproto(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.set_notproto()
    }
        
    pub fn freeze_module(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.freeze_module()
    }
                
    pub fn freeze_component(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.freeze_component()
    }
    
    pub fn freeze_shader(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.freeze_shader()
    }
    
    pub fn freeze_ext(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.freeze_ext()
    }
                        
    pub fn freeze_api(&mut self, ptr: ScriptObject){
        self.objects[ptr.index as usize].tag.freeze_api()
    }
    
    pub fn set_object_apply_transform(&mut self, ptr: ScriptObject, ni: NativeId){
        self.objects[ptr.index as usize].tag.set_apply_transform(ni)
    }
        
    pub fn set_type(&mut self, obj: ScriptObject, ty:ScriptTypeIndex){
        self.objects[obj.index as usize].tag.set_type_index(ty);
    }
        
    pub fn set_string_keys(&mut self, obj: ScriptObject){
        let object = &mut  self.objects[obj.index as usize];
        object.tag.set_string_keys();
    }
    
    pub fn set_shader_io(&mut self, obj: ScriptObject, io:ShaderIoType){
        let object = &mut  self.objects[obj.index as usize];
        object.tag.set_shader_io(io);
    }
    
    pub fn as_shader_io(&self, obj: ScriptObject)->Option<ShaderIoType>{
        let object = &self.objects[obj.index as usize];
        object.tag.as_shader_io()
    }
        
    // Writing object values 
            
        
    pub(crate) fn force_value_in_map(&mut self, ptr:ScriptObject, key: ScriptValue, sself:ScriptValue){
        let object = &mut self.objects[ptr.index as usize];
        object.map_insert(key, sself);
    }            
            
    fn set_value_index(&mut self, ptr: ScriptObject, index:ScriptValue, value: ScriptValue, trap:ScriptTrap)->ScriptValue{
        // alright so. now what.
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){ // has rw flags
            return err_vec_frozen!(trap)
        }
                
        let index = index.as_index();
        if index >= object.vec.len(){
            object.vec.resize(index + 1, ScriptVecValue::default());
        }
        object.vec[index].value = value;
        return NIL
    }
                
    fn set_value_prefixed(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue, trap:ScriptTrap)->ScriptValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return err_vec_frozen!(trap)
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
            
    fn set_value_deep(&mut self, ptr:ScriptObject, key: ScriptValue, value: ScriptValue, trap:ScriptTrap)->ScriptValue{
        let mut ptr = ptr;
        loop{
            let object = &mut self.objects[ptr.index as usize];
            if object.tag.is_frozen(){
                return err_frozen!(trap)
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
        if object.tag.is_vec2(){
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
        
    fn set_value_shallow_checked(&mut self, top_ptr:ScriptObject, key:ScriptValue, key_id:LiveId, value:ScriptValue, trap:ScriptTrap)->ScriptValue{
        
        let object = &self.objects[top_ptr.index as usize];
        if object.tag.is_frozen(){
            return err_frozen!(trap)
        }

        if let Some(ty) = object.tag.as_type_index(){
            let check = &self.type_check[ty.0 as usize];
            if let Some(type_prop) = check.props.props.get(&key_id){
                if let Some(ty_index) = self.type_index.get(&type_prop.ty){
                    let check_prop = &self.type_check[ty_index.0 as usize];
                    if let Some(object) = &check_prop.object{
                        //println!("SET VALUE {:?} {:?}", key, value);
                        if !(*object.check)(self, value){
                            return err_invalid_prop_type!(trap)
                        }
                    }
                }
                else{
                    println!("Trying to check a type that hasnt been registered yet for {} {}", key, value);
                    return err_type_not_registered!(trap)
                }
            }
            else if !object.tag.is_map_add(){
                return err_invalid_prop_name!(trap)
            }
            let object = &mut self.objects[top_ptr.index as usize];
            object.map_insert(key, value);
            return NIL    
        }
        // check against prototype or type
        if object.tag.is_validated(){
            let mut ptr = top_ptr;
            loop{
                let object = &self.objects[ptr.index as usize];
                if object.tag.is_vec2(){
                    for kv in object.vec.iter().rev(){
                        if kv.key == key{
                            if !self.validate_type(kv.value, value){
                                return err_invalid_prop_type!(trap)
                            }
                            return self.set_value_shallow(top_ptr, key, value, trap);
                        }
                    }
                }
                if let Some(set_value) = object.map_get(&key){
                    if !self.validate_type(set_value, value){
                        return err_invalid_prop_type!(trap)
                    }
                    return self.set_value_shallow(top_ptr, key, value, trap);
                }
                if let Some(next_ptr) = object.proto.as_object(){
                    ptr = next_ptr
                }
                else if !object.tag.is_map_add(){ // not found
                    return err_invalid_prop_name!(trap)
                } 
            }
        }
        let object = &mut self.objects[top_ptr.index as usize];
        if object.tag.is_map_add(){
            if object.tag.is_vec2(){
                for kv in object.vec.iter_mut().rev(){
                    if kv.key == key{
                        return err_key_already_exists!(trap)
                    }
                }
                object.vec.push(ScriptVecValue{key, value});
                return NIL
            }
            if let Some(_) = object.map_get(&key){
                return err_key_already_exists!(trap)
            }
            else{
                object.map_insert(key, value);
                return NIL    
            }
        }
        err_unexpected!(trap)
    }
        
    fn set_value_shallow(&mut self, ptr:ScriptObject, key:ScriptValue, value:ScriptValue, _trap:ScriptTrap)->ScriptValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec2(){
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
        self.set_value(ptr, key, value, NoTrap);
    }
        
    pub fn set_value(&mut self, ptr:ScriptObject, key:ScriptValue, value:ScriptValue, trap:ScriptTrap)->ScriptValue{
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
                if object.tag.is_string_keys(){
                    if let Some(skey) = key_id.as_string(|s|{
                        if let Some(s) = s{
                            // Try to get existing interned string
                            if let Some(existing) = self.check_intern_string(s){
                                Some(existing)
                            } else {
                                // Not interned yet - intern it now to maintain consistency
                                Some(self.new_string_from_str(s))
                            }
                        }
                        else{
                            None
                        }
                    }){
                        return self.set_value_shallow(ptr, skey, value, trap);
                    }
                    // LiveId couldn't be converted to string - fall through to use LiveId
                    // This happens for hashed IDs that lost their string representation
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
        if key.is_string_like() || key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            let object = &mut self.objects[ptr.index as usize];
            if !object.tag.is_deep(){
                if object.tag.needs_checking(){
                    return err_invalid_key_type!(trap)
                }
                return self.set_value_shallow(ptr, key, value, trap);
            }
            else{
                return self.set_value_deep(ptr, key, value, trap)
            }
        }
        err_invalid_key_type!(trap)
    }
        
        
    // scope specific value get/set
        
        
    pub fn set_scope_value(&mut self, ptr:ScriptObject, key:LiveId, value:ScriptValue, trap:ScriptTrap)->ScriptValue{
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
        err_not_found!(trap)
    }
        
    pub fn scope_value(&self, ptr:ScriptObject, key: LiveId, trap:ScriptTrap)->ScriptValue{
        let mut ptr = ptr;
        let key = key.into();
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(set) = object.map.get(&key){
                return set.value
            }
            if object.tag.is_vec2(){
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
        err_not_found!(trap)
    }
        
    pub fn def_scope_value(&mut self, ptr:ScriptObject, key:LiveId, value:ScriptValue)->Option<ScriptObject>{
        // if we already have sself value we have to shadow the scope
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
        
        
        
    fn value_index(&self, ptr: ScriptObject, index: ScriptValue, trap:ScriptTrap)->ScriptValue{
        let object = &self.objects[ptr.index as usize];
        // most used path
        let index = index.as_index();
        if let Some(kv) = object.vec.get(index){
            return kv.value
        }
        err_not_found!(trap)
    }
        
    fn value_prefixed(&self, ptr: ScriptObject, key: ScriptValue, trap:ScriptTrap)->ScriptValue{
        let object = &self.objects[ptr.index as usize];
        for kv in object.vec.iter().rev(){
            if kv.key == key{
                return kv.value;
            }
        }
        err_not_found!(trap)
    }
        
    fn value_deep_map(&self, obj_ptr:ScriptObject, key: ScriptValue, trap:ScriptTrap)->ScriptValue{
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
        err_not_found!(trap)
    }
        
    fn value_deep(&self, obj_ptr:ScriptObject, key: ScriptValue, trap:ScriptTrap)->ScriptValue{
        let mut ptr = obj_ptr;
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(value) = object.map_get(&key){
                return value
            }
            // handle auto conversion from string to id and back for json interop
            if object.tag.is_string_keys(){
                if let Some(id) = key.as_id(){
                    if let Some(value) = id.as_string(|s|{
                        if let Some(s) = s{
                            if let Some(idx) = self.check_intern_string(s){
                                object.map_get(&idx)
                            }
                            else{
                                None
                            }
                        }
                        else{
                            None
                        }
                    }){
                        return value
                    }
                }
            }
            else if key.is_string_like(){
                let id = if let Some(s) = key.as_string(){
                    if let Some(s) = &self.strings[s.index as usize]{LiveId::from_str(&s.string.0)}else{LiveId(0)}
                }
                else {
                    key.as_inline_string(|s| LiveId::from_str(s)).unwrap()
                };
                if let Some(value) = object.map_get(&id.into()){
                    return value
                }
            }
            for kv in object.vec.iter().rev(){
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
        err_not_found!(trap)
    }
    
    pub fn object_method(&self, ptr:ScriptObject, key:ScriptValue, trap:ScriptTrap)->ScriptValue{
        return self.value_deep_map(ptr, key, trap)
    }
        
    pub fn value_path(&self, ptr:ScriptObject, keys:&[LiveId], trap:ScriptTrap)->ScriptValue{
        let mut value:ScriptValue = ptr.into();
        for key in keys{
            if let Some(obj) = value.as_object(){
                value = self.value(obj, key.into(), trap);
            }
            else{
                return err_not_found!(trap);
            }
        }
        value
    }
        
    pub fn value(&self, ptr:ScriptObject, key:ScriptValue, trap:ScriptTrap)->ScriptValue{
        if key.is_unprefixed_id(){
            return self.value_deep(ptr, key, trap)
        }
        if key.is_index(){
            return self.value_index(ptr, key, trap)
        }
        if key.is_prefixed_id(){
            return self.value_prefixed(ptr, key, trap)
        }
        if key.is_string_like() || key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            return self.value_deep(ptr, key, trap)
        }
        // TODO implement string lookup
        err_not_found!(trap)
    }
    
    /// Create a default instance for a type-checked field that doesn't exist on the prototype.
    /// This is used for deep prototypical inheritance - when accessing obj.field where field
    /// only exists in the type-check structure, we create a new instance and set it on obj.
    pub fn proto_field_from_type_check(&mut self, obj: ScriptObject, field_id: LiveId, trap: ScriptTrap) -> ScriptValue {
        // Get the field's type_id from the type-check structure
        if let Some(field_type_id) = self.field_type_from_type_check(obj, field_id) {
            // Look up the type_default for this type
            if let Some(default_obj) = self.type_default_for_id(field_type_id) {
                // Create a new object with the default as prototype
                let new_obj = self.new_with_proto(default_obj.into());
                // Set it on the parent object
                self.set_value(obj, field_id.into(), new_obj.into(), trap);
                return new_obj.into();
            }
        }
        err_not_found!(trap)
    }
    
    /// Handle proto_field access for a value that exists on the prototype chain.
    /// If the value is an object that comes from a prototype (not directly on obj),
    /// create a new object with it as prototype and set it on obj.
    pub fn proto_field_from_value(&mut self, obj: ScriptObject, field: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        // First check if the field exists directly on this object
        let obj_data = &self.objects[obj.index as usize];
        if let Some(value) = obj_data.map_get(&field) {
            // Field exists directly on object, return as-is
            return value;
        }
        // Handle is_string_keys: convert LiveId to string key
        if obj_data.tag.is_string_keys() {
            if let Some(id) = field.as_id() {
                if let Some(value) = id.as_string(|s| {
                    if let Some(s) = s {
                        if let Some(idx) = self.check_intern_string(s) {
                            self.objects[obj.index as usize].map_get(&idx)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }) {
                    return value;
                }
            }
        }
        
        // Field doesn't exist directly - get from prototype chain
        let value = self.value(obj, field, trap);
        
        // If it's an object from prototype, create a new instance
        if let Some(value_obj) = value.as_object() {
            // Create a new object with the prototype value as its proto
            let new_obj = self.new_with_proto(value_obj.into());
            // Set it on the current object
            self.set_value(obj, field, new_obj.into(), trap);
            return new_obj.into();
        }
        
        // Not an object (primitive or nil) - return as-is
        value
    }
        
    pub fn value_for_apply(&mut self, obj:ScriptValue, key:ScriptValue)->Option<ScriptValue>{
        
        if let Some(ptr) = obj.as_object(){
            // only do top level if dirty
            let object = &mut self.objects[ptr.index as usize];
            if let Some(value) = object.map_get(&key){
                return Some(value)
            }
            // if we havent been applied before apply prototype chain too
            let mut ptr = if let Some(next_ptr) = object.proto.as_object(){
                next_ptr
            }
            else{
                return None
            };
            loop{
                let object = &self.objects[ptr.index as usize];
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
        None    
    }
        
    pub fn map_ref(&self, object:ScriptObject)->&ScriptObjectMap{
        let object = &self.objects[object.index as usize];
        &object.map
    }
            
    pub fn map_mut_with<S,R,F:FnOnce(S, &mut ScriptObjectMap)->R>(&mut self, s:S, object:ScriptObject, f:F)->R{
        let mut map = ScriptObjectMap::default();
        std::mem::swap(&mut map, &mut self.objects[object.index as usize].map);
        let r = f(s, &mut map);
        std::mem::swap(&mut map, &mut self.objects[object.index as usize].map);
        r
    }
            
        
    // Vec Reading
        
        
        
    pub fn vec_key_value(&self, ptr:ScriptObject, index:usize, trap:ScriptTrap)->ScriptVecValue{
        let object = &self.objects[ptr.index as usize];
                
        if let Some(value) = object.vec.get(index){
            return *value
        }
        ScriptVecValue{key:NIL, value:err_vec_bound!(trap)}
    }
            
    pub fn vec_value(&self, ptr:ScriptObject, index:usize, trap:ScriptTrap)->ScriptValue{
        let object = &self.objects[ptr.index as usize];
        if let Some(kv) = object.vec.get(index){
            return kv.value
        }
        err_vec_bound!(trap)
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
        
        
            
    pub fn vec_insert_value_at(&mut self, _ptr:ScriptObject, _key:ScriptValue, _value:ScriptValue, _before:bool, _ip:ScriptTrap)->ScriptValue{
        NIL
    }
            
    pub fn vec_insert_value_begin(&mut self, _ptr:ScriptObject, _key:ScriptValue, _value:ScriptValue, _ip:ScriptTrap)->ScriptValue{
        NIL
    }
            
    pub fn vec_push_vec(&mut self, target:ScriptObject, source:ScriptObject, trap:ScriptTrap)->ScriptValue{
        if target == source{
            return err_invalid_args!(trap)
        }
        let (target, source) = if target.index > source.index{
            let (o1, o2) = self.objects.split_at_mut(target.index as _);
            (&mut o2[0], &mut o1[source.index as usize])                    
        }else{
            let (o1, o2) = self.objects.split_at_mut(source.index as _);
            (&mut o1[target.index as usize], &mut o2[0])                    
        };
        if target.tag.is_vec_frozen(){
            return err_vec_frozen!(trap)
        }
        target.push_vec_from_other(source);
        NIL
    }
            
    pub fn vec_push_vec_of_vec(&mut self, target:ScriptObject, source:ScriptObject, map:bool, trap:ScriptTrap)->ScriptValue{
        let len = self.objects[source.index as usize].vec.len();
        for i in 0..len{
            if let Some(source) = self.objects[source.index as usize].vec[i].value.as_object(){
                if target == source{
                    return err_invalid_args!(trap)
                }
                let (target, source) = if target.index > source.index{
                    let (o1, o2) = self.objects.split_at_mut(target.index as _);
                    (&mut o2[0], &mut o1[source.index as usize])
                }else{
                    let (o1, o2) = self.objects.split_at_mut(source.index as _);
                    (&mut o1[target.index as usize], &mut o2[0])
                };
                if target.tag.is_vec_frozen(){
                    return err_vec_frozen!(trap)
                }
                target.push_vec_from_other(source);
                if map{
                    target.merge_map_from_other(source);
                }
            }
        }
        NIL
    }
    
    /// Merges the vec and map parts of a source object into a target object.
    /// Used by the splat operator (..) to spread one object into another.
    /// Map entries from source are only added if they don't already exist in target.
    pub fn merge_object(&mut self, target:ScriptObject, source:ScriptObject, trap:ScriptTrap)->ScriptValue{
        if target == source{
            return err_invalid_args!(trap)
        }
        let (target, source) = if target.index > source.index{
            let (o1, o2) = self.objects.split_at_mut(target.index as _);
            (&mut o2[0], &mut o1[source.index as usize])
        }else{
            let (o1, o2) = self.objects.split_at_mut(source.index as _);
            (&mut o1[target.index as usize], &mut o2[0])
        };
        if !target.tag.is_vec_frozen(){
            target.push_vec_from_other(source);
        }
        // Only add map entries that don't already exist in target
        target.merge_map_from_other_no_overwrite(source);
        NIL
    }
            
    pub fn vec_push(&mut self, ptr: ScriptObject, key: ScriptValue, value: ScriptValue, trap:ScriptTrap)->ScriptValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return err_vec_frozen!(trap)
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
                
    pub fn vec_remove(&mut self, ptr:ScriptObject, index:usize, trap:ScriptTrap)->ScriptVecValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return ScriptVecValue{key:NIL, value:err_vec_frozen!(trap)}
        }
        if index >= object.vec.len(){
            return ScriptVecValue{key:NIL, value:err_vec_bound!(trap)}
        }
        object.vec.remove(index)
    }
            
    pub fn vec_pop(&mut self, ptr:ScriptObject, trap:ScriptTrap)->ScriptVecValue{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.is_vec_frozen(){
            return ScriptVecValue{key:NIL, value:err_vec_frozen!(trap)}
        }
        object.vec.pop().unwrap_or_else(||  ScriptVecValue{key:NIL, value:err_vec_bound!(trap)})
    }
}
