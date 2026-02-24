use crate::heap::*;
use crate::trap::*;
use crate::value::*;
use crate::*;

impl ScriptHeap {
    // Arrays

    pub fn freeze_array(&mut self, array: ScriptArray) {
        self.arrays[array].tag.freeze()
    }

    pub fn new_array(&mut self) -> ScriptArray {
        if let Some(arr) = self.arrays_free.pop() {
            // arr already has the correct generation from gc.rs sweep
            let array = &mut self.arrays[arr];
            array.tag.set_alloced();
            arr
        } else {
            let index = self.arrays.len();
            let mut array = ScriptArrayData::default();
            array.tag.set_alloced();
            self.arrays.push(array);
            // New slot starts at generation 0
            ScriptArray::new(index as _, crate::value::GENERATION_ZERO)
        }
    }

    pub fn array_len(&self, array: ScriptArray) -> usize {
        self.arrays[array].storage.len()
    }

    pub fn array_push(&mut self, array: ScriptArray, value: ScriptValue, trap: ScriptTrap) {
        let array = &mut self.arrays[array];
        if array.tag.is_immutable() {
            script_err_immutable!(trap, "array is immutable");
            return;
        }
        array.tag.set_dirty();
        array.storage.push(value);
    }

    pub fn array_pop_front_option(&mut self, array: ScriptArray) -> Option<ScriptValue> {
        let array = &mut self.arrays[array];
        if array.tag.is_immutable() {
            return None;
        }
        array.tag.set_dirty();
        array.storage.pop_front()
    }

    pub fn array_push_vec(&mut self, array: ScriptArray, object: ScriptObject, trap: ScriptTrap) {
        let array = &mut self.arrays[array];
        if array.tag.is_immutable() {
            script_err_immutable!(trap, "array is immutable");
            return;
        }
        array.tag.set_dirty();
        let object = &self.objects[object];
        for kv in &object.vec {
            array.storage.push(kv.value);
        }
    }

    /// Merges all elements from source array into target array.
    /// Used by the splat operator (..) to spread one array into another.
    pub fn merge_array(&mut self, target: ScriptArray, source: ScriptArray, trap: ScriptTrap) {
        // Get the storage from source first
        let source_storage = &self.arrays[source].storage;
        let values: Vec<ScriptValue> = match source_storage {
            ScriptArrayStorage::ScriptValue(v) => v.iter().copied().collect(),
            ScriptArrayStorage::U8(v) => {
                v.iter().map(|x| ScriptValue::from_f64(*x as f64)).collect()
            }
            ScriptArrayStorage::U16(v) => {
                v.iter().map(|x| ScriptValue::from_f64(*x as f64)).collect()
            }
            ScriptArrayStorage::U32(v) => {
                v.iter().map(|x| ScriptValue::from_f64(*x as f64)).collect()
            }
            ScriptArrayStorage::F32(v) => {
                v.iter().map(|x| ScriptValue::from_f64(*x as f64)).collect()
            }
        };

        let target_arr = &mut self.arrays[target];
        if target_arr.tag.is_immutable() {
            script_err_immutable!(trap, "array is immutable");
            return;
        }
        target_arr.tag.set_dirty();
        for v in values {
            target_arr.storage.push(v);
        }
    }

    pub fn array_push_unchecked(&mut self, array: ScriptArray, value: ScriptValue) {
        let array = &mut self.arrays[array];
        array.tag.set_dirty();
        array.storage.push(value);
    }

    pub fn array_storage(&self, array: ScriptArray) -> &ScriptArrayStorage {
        let array = &self.arrays[array];
        &array.storage
    }

    pub fn new_array_from_vec_u8(&mut self, data: Vec<u8>) -> ScriptArray {
        let ptr = self.new_array();
        let array = &mut self.arrays[ptr];
        array.tag.set_dirty();
        array.storage = ScriptArrayStorage::U8(data);
        ptr
    }

    pub fn array_mut(
        &mut self,
        array: ScriptArray,
        trap: ScriptTrap,
    ) -> Option<&mut ScriptArrayStorage> {
        let array = &mut self.arrays[array];
        if array.tag.is_immutable() {
            script_err_immutable!(trap, "array is immutable");
            return None;
        }
        array.tag.set_dirty();
        Some(&mut array.storage)
    }

    pub fn array_mut_self_with<R, F: FnOnce(&mut Self, &ScriptArrayStorage) -> R>(
        &mut self,
        array: ScriptArray,
        cb: F,
    ) -> R {
        let mut storage = ScriptArrayStorage::ScriptValue(Default::default());
        std::mem::swap(&mut self.arrays[array].storage, &mut storage);
        let r = cb(self, &storage);
        std::mem::swap(&mut self.arrays[array].storage, &mut storage);
        r
    }

    pub fn array_mut_mut_self_with<R, F: FnOnce(&mut Self, &mut ScriptArrayStorage) -> R>(
        &mut self,
        array: ScriptArray,
        cb: F,
    ) -> R {
        let mut storage = ScriptArrayStorage::ScriptValue(Default::default());
        std::mem::swap(&mut self.arrays[array].storage, &mut storage);
        let r = cb(self, &mut storage);
        std::mem::swap(&mut self.arrays[array].storage, &mut storage);
        r
    }

    pub fn array_remove(
        &mut self,
        array: ScriptArray,
        index: usize,
        trap: ScriptTrap,
    ) -> ScriptValue {
        let array = &mut self.arrays[array];
        if array.tag.is_immutable() {
            return script_err_immutable!(trap, "array is immutable");
        }
        array.tag.set_dirty();
        if index >= array.storage.len() {
            return script_err_out_of_bounds!(
                trap,
                "array remove index {} out of bounds (len={})",
                index,
                array.storage.len()
            );
        }
        array.storage.remove(index)
    }

    pub fn array_pop(&mut self, array: ScriptArray, trap: ScriptTrap) -> ScriptValue {
        let array = &mut self.arrays[array];
        if array.tag.is_immutable() {
            return script_err_immutable!(trap, "array is immutable");
        }
        if let Some(value) = array.storage.pop() {
            array.tag.set_dirty();
            value
        } else {
            script_err_out_of_bounds!(trap, "array pop on empty array")
        }
    }

    pub fn array_clear(&mut self, array: ScriptArray, trap: ScriptTrap) {
        let array = &mut self.arrays[array];
        if array.tag.is_immutable() {
            script_err_immutable!(trap, "array is immutable");
            return;
        }
        if array.storage.len() != 0 {
            array.storage.clear();
            array.tag.set_dirty();
        }
    }

    pub fn array_index(&self, array: ScriptArray, index: usize, trap: ScriptTrap) -> ScriptValue {
        let storage = &self.arrays[array].storage;
        if let Some(value) = storage.index(index) {
            return value;
        } else {
            script_err_out_of_bounds!(
                trap,
                "array index {} out of bounds (len={})",
                index,
                storage.len()
            )
        }
    }

    pub fn array_index_unchecked(&self, array: ScriptArray, index: usize) -> ScriptValue {
        if let Some(value) = self.arrays[array].storage.index(index) {
            return value;
        } else {
            NIL
        }
    }

    pub fn set_array_index(
        &mut self,
        array: ScriptArray,
        index: usize,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        let array = &mut self.arrays[array];
        if array.tag.is_immutable() {
            return script_err_immutable!(trap, "array is immutable");
        }
        array.tag.set_dirty();
        array.storage.set_index(index, value);
        NIL
    }
}
