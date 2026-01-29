use crate::value::*;
use crate::heap::*;
use crate::trap::*;
use crate::*;

impl ScriptHeap{
        
        
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
        
    pub fn array_push(&mut self, array:ScriptArray, value:ScriptValue, trap:ScriptTrap){
        if let Some(obj) = value.as_object(){
            self.set_reffed(obj);
        }
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            trap.err_frozen();
            return 
        }
        array.tag.set_dirty();
        array.storage.push(value);
    }
        
    pub fn array_pop_front_option(&mut self, array:ScriptArray)->Option<ScriptValue>{
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            return None
        }
        array.tag.set_dirty();
        array.storage.pop_front()
    }
        
    pub fn array_push_vec(&mut self, array:ScriptArray, object:ScriptObject, trap:ScriptTrap){
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            trap.err_frozen();
            return 
        }
        array.tag.set_dirty();
        let object = &self.objects[object.index as usize];
        for kv in &object.vec{
            array.storage.push(kv.value);
        }
    }
    
    /// Merges all elements from source array into target array.
    /// Used by the splat operator (..) to spread one array into another.
    pub fn merge_array(&mut self, target:ScriptArray, source:ScriptArray, trap:ScriptTrap){
        // Get the storage from source first
        let source_storage = &self.arrays[source.index as usize].storage;
        let values: Vec<ScriptValue> = match source_storage {
            ScriptArrayStorage::ScriptValue(v) => v.iter().copied().collect(),
            ScriptArrayStorage::U8(v) => v.iter().map(|x| ScriptValue::from_f64(*x as f64)).collect(),
            ScriptArrayStorage::U16(v) => v.iter().map(|x| ScriptValue::from_f64(*x as f64)).collect(),
            ScriptArrayStorage::U32(v) => v.iter().map(|x| ScriptValue::from_f64(*x as f64)).collect(),
            ScriptArrayStorage::F32(v) => v.iter().map(|x| ScriptValue::from_f64(*x as f64)).collect(),
        };
        
        let target_arr = &mut self.arrays[target.index as usize];
        if target_arr.tag.is_frozen(){
            trap.err_frozen();
            return 
        }
        target_arr.tag.set_dirty();
        for v in values {
            target_arr.storage.push(v);
        }
    }
        
    pub fn array_push_unchecked(&mut self, array:ScriptArray, value:ScriptValue){
        if let Some(obj) = value.as_object(){
            self.set_reffed(obj);
        }
        let array = &mut self.arrays[array.index as usize];
        array.tag.set_dirty();
        array.storage.push(value);
    }
        
    pub fn array_storage(&self, array:ScriptArray)->&ScriptArrayStorage{
        let array = &self.arrays[array.index as usize];
        &array.storage
    }
        
    pub fn new_array_from_vec_u8(&mut self, data:Vec<u8>)->ScriptArray{
        let ptr = self.new_array();
        let array = &mut self.arrays[ptr.index as usize];
        array.tag.set_dirty();
        array.storage = ScriptArrayStorage::U8(data);
        ptr
    }
        
    pub fn array_mut(&mut self, array:ScriptArray,trap:ScriptTrap)->Option<&mut ScriptArrayStorage>{
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            trap.err_frozen();
            return None
        }
        array.tag.set_dirty();
        Some(&mut array.storage)
    }
        
    pub fn array_mut_self_with<R,F:FnOnce(&mut Self, &ScriptArrayStorage)->R>(&mut self, array:ScriptArray, cb:F)->R{
        let mut storage = ScriptArrayStorage::ScriptValue(Default::default());
        std::mem::swap(&mut self.arrays[array.index as usize].storage, &mut storage);
        let r = cb(self, &storage);
        std::mem::swap(&mut self.arrays[array.index as usize].storage, &mut storage);
        r
    }
        
    pub fn array_mut_mut_self_with<R,F:FnOnce(&mut Self, &mut ScriptArrayStorage)->R>(&mut self, array:ScriptArray, cb:F)->R{
        let mut storage = ScriptArrayStorage::ScriptValue(Default::default());
        std::mem::swap(&mut self.arrays[array.index as usize].storage, &mut storage);
        let r = cb(self, &mut storage);
        std::mem::swap(&mut self.arrays[array.index as usize].storage, &mut storage);
        r
    }
            
    pub fn array_remove(&mut self, array:ScriptArray, index: usize,trap:ScriptTrap)->ScriptValue{
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
        
    pub fn array_pop(&mut self, array:ScriptArray, trap:ScriptTrap)->ScriptValue{
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
        
    pub fn array_clear(&mut self, array:ScriptArray, trap:ScriptTrap){
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            trap.err_frozen();
            return
        }
        if array.storage.len() != 0{
            array.storage.clear();
            array.tag.set_dirty();
        }
    }
        
    pub fn array_index(&self, array:ScriptArray, index:usize, trap:ScriptTrap)->ScriptValue{
        if let Some(value) = self.arrays[array.index as usize].storage.index(index){
            return value
        }
        else{
            trap.err_array_bound()
        }
    }
        
    pub fn array_index_unchecked(&self, array:ScriptArray, index:usize)->ScriptValue{
        if let Some(value) = self.arrays[array.index as usize].storage.index(index){
            return value
        }
        else{
            NIL
        }
    }
        
    pub fn set_array_index(&mut self, array:ScriptArray, index:usize, value:ScriptValue, trap:ScriptTrap)->ScriptValue{
        let array = &mut self.arrays[array.index as usize];
        if array.tag.is_frozen(){
            return trap.err_frozen();
        }
        array.tag.set_dirty();
        array.storage.set_index(index, value);
        NIL
    }
}
