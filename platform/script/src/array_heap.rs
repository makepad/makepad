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
            // Reused array slots may come from typed buffers (U8/U16/U32/F32).
            // New arrays must start as generic ScriptValue storage.
            if !matches!(array.storage, ScriptArrayStorage::ScriptValue(_)) {
                array.storage = ScriptArrayStorage::ScriptValue(Default::default());
            } else {
                array.storage.clear();
            }
            array.tag.set_alloced();
            arr
        } else {
            if !self.charge_allocation(
                std::mem::size_of::<ScriptArrayData>(),
                "creating an array",
            ) {
                return self.allocation_poison_array;
            }
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
        if self.is_allocation_poison_array(array) || self.arrays[array].tag.is_immutable() {
            script_err_immutable!(trap, "array is immutable");
            return;
        }
        let bytes = self.arrays[array].storage.allocation_element_bytes();
        if !self.charge_allocation(bytes, "pushing an array element") {
            return;
        }
        self.escape_value(value);
        let array = &mut self.arrays[array];
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
        if self.is_allocation_poison_array(array) || self.arrays[array].tag.is_immutable() {
            script_err_immutable!(trap, "array is immutable");
            return;
        }
        let vec_len = self.objects[object].vec.len();
        let bytes = vec_len
            .checked_mul(self.arrays[array].storage.allocation_element_bytes())
            .unwrap_or(usize::MAX);
        if !self.charge_allocation(bytes, "extending an array") {
            return;
        }
        // escape barrier: the object's vec values become reachable from the array
        for i in 0..vec_len {
            let v = self.objects[object].vec[i].value;
            self.escape_value(v);
        }
        let array = &mut self.arrays[array];
        array.tag.set_dirty();
        let object = &self.objects[object];
        for kv in &object.vec {
            array.storage.push(kv.value);
        }
    }

    /// Merges all elements from source array into target array.
    /// Used by the splat operator (..) to spread one array into another.
    pub fn merge_array(&mut self, target: ScriptArray, source: ScriptArray, trap: ScriptTrap) {
        if self.is_allocation_poison_array(target) || self.arrays[target].tag.is_immutable() {
            script_err_immutable!(trap, "array is immutable");
            return;
        }
        let source_len = self.arrays[source].storage.len();
        let element_bytes = self.arrays[target].storage.allocation_element_bytes();
        // One temporary ScriptValue vector plus the target's new elements.
        let bytes = source_len
            .checked_mul(std::mem::size_of::<ScriptValue>().saturating_add(element_bytes))
            .unwrap_or(usize::MAX);
        if !self.charge_allocation(bytes, "merging arrays") {
            return;
        }
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

        // escape barrier: source values become reachable from target
        for v in &values {
            self.escape_value(*v);
        }
        let target_arr = &mut self.arrays[target];
        target_arr.tag.set_dirty();
        for v in values {
            target_arr.storage.push(v);
        }
    }

    pub fn array_push_unchecked(&mut self, array: ScriptArray, value: ScriptValue) {
        if self.is_allocation_poison_array(array) || self.allocation_exceeded() {
            return;
        }
        let bytes = self.arrays[array].storage.allocation_element_bytes();
        if !self.charge_allocation(bytes, "pushing an array element") {
            return;
        }
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
        if self.is_allocation_poison_array(ptr) {
            return ptr;
        }
        if !self.charge_allocation(data.capacity(), "creating a byte array") {
            return ptr;
        }
        let array = &mut self.arrays[ptr];
        array.tag.set_dirty();
        array.storage = ScriptArrayStorage::U8(data);
        ptr
    }

    pub fn new_array_from_slice_u8(&mut self, data: &[u8]) -> ScriptArray {
        let ptr = self.new_array();
        if self.is_allocation_poison_array(ptr) {
            return ptr;
        }
        if !self.charge_allocation(data.len(), "creating a byte array") {
            return ptr;
        }
        let array = &mut self.arrays[ptr];
        array.tag.set_dirty();
        array.storage = ScriptArrayStorage::U8(data.to_vec());
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
        if self.is_allocation_poison_array(array) || self.arrays[array].tag.is_immutable() {
            return script_err_immutable!(trap, "array is immutable");
        }
        let len = self.arrays[array].storage.len();
        if index >= len {
            let additional = index
                .checked_add(1)
                .and_then(|target| target.checked_sub(len))
                .and_then(|elements| {
                    elements.checked_mul(self.arrays[array].storage.allocation_element_bytes())
                })
                .unwrap_or(usize::MAX);
            if !self.charge_allocation(additional, "growing a sparse array index") {
                return NIL;
            }
        }
        self.escape_value(value);
        let array = &mut self.arrays[array];
        array.tag.set_dirty();
        array.storage.set_index(index, value);
        NIL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::ScriptArrayStorage;

    #[test]
    fn reused_array_slot_resets_to_script_value_storage() {
        let mut heap = ScriptHeap::default();

        let typed_array = heap.new_array_from_vec_u8(vec![1, 2, 3, 4]);
        let index = typed_array.index() as usize;

        // Simulate GC sweep/free for this slot.
        heap.arrays[typed_array].clear();
        heap.arrays.free_slot(typed_array.index());
        let new_gen = heap.arrays.generation(index);
        heap.arrays_free
            .push(ScriptArray::new(typed_array.index(), new_gen));

        let reused = heap.new_array();
        assert_eq!(reused.index(), typed_array.index());
        assert!(matches!(
            heap.arrays[reused].storage,
            ScriptArrayStorage::ScriptValue(_)
        ));
        assert_eq!(heap.array_len(reused), 0);
    }
}
