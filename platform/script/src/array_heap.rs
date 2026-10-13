use makepad_relax_api::relax_api;
use crate::heap::*;
use crate::trap::*;
use crate::value::*;
use crate::*;

/// arr_view's kind: the storage of the array.
pub const ARR_KIND_VALUES: u32 = 0;
pub const ARR_KIND_F32: u32 = 1;
pub const ARR_KIND_U32: u32 = 2;
pub const ARR_KIND_U16: u32 = 3;
pub const ARR_KIND_U8: u32 = 4;

/// arr_new's kind.
/// An empty array of values.
pub const ARR_NEW_EMPTY: u32 = 0;
/// A u8 array copying `len` bytes at `ptr`.
pub const ARR_NEW_U8_COPY: u32 = 1;
/// A u8 array taking over the global-allocator allocation at `ptr` (`len` bytes of `cap`).
pub const ARR_NEW_U8_OWNED: u32 = 2;

/// arr_edit's op.
pub const ARR_EDIT_PUSH: u32 = 0;
pub const ARR_EDIT_PUSH_UNCHECKED: u32 = 1;
pub const ARR_EDIT_SET_INDEX: u32 = 2;
pub const ARR_EDIT_FREEZE: u32 = 3;

/// A view of an array (arr_view): its tag and its elements in place. A value array is two runs
/// (the storage is a ring buffer: `ptr`/`len` then `ptr2`/`len2`, of `ScriptValue`); a typed array
/// is the one run (`ptr`/`len`, of f32, u32, u16 or u8 by `kind`). Borrowed from the heap: valid
/// until the next change to the array.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ArrView {
    pub tag: ScriptArrayTag,
    pub kind: u32,
    pub ptr: *const u8,
    pub len: usize,
    pub ptr2: *const u8,
    pub len2: usize,
}

impl ArrView {
    /// The elements, whatever the storage, as the script values they read as.
    #[inline]
    pub fn len_all(&self) -> usize {
        match self.kind {
            ARR_KIND_VALUES => self.len + self.len2,
            _ => self.len,
        }
    }

    /// The element at `index` read as a script value; None past the end.
    #[inline]
    pub fn index(&self, index: usize) -> Option<ScriptValue> {
        // SAFETY: the view's pointers and lengths describe the live runs of the storage it was taken from
        unsafe {
            match self.kind {
                ARR_KIND_VALUES => {
                    if index < self.len {
                        Some(*(self.ptr as *const ScriptValue).add(index))
                    } else if index - self.len < self.len2 {
                        Some(*(self.ptr2 as *const ScriptValue).add(index - self.len))
                    } else {
                        None
                    }
                }
                ARR_KIND_F32 => (index < self.len).then(|| (*(self.ptr as *const f32).add(index)).into()),
                ARR_KIND_U32 => (index < self.len).then(|| (*(self.ptr as *const u32).add(index)).into()),
                ARR_KIND_U16 => (index < self.len).then(|| (*(self.ptr as *const u16).add(index)).into()),
                _ => (index < self.len).then(|| (*self.ptr.add(index)).into()),
            }
        }
    }
}

#[relax_api]
impl ScriptHeap {
    // Arrays

    /// The array as a view into the heap.
    #[inline]
    #[relax_api]
    pub fn arr_view(&self, array: ScriptArray) -> ArrView {
        let data = &self.arrays[array];
        let tag = data.tag;
        match &data.storage {
            ScriptArrayStorage::ScriptValue(v) => {
                let (a, b) = v.as_slices();
                ArrView { tag, kind: ARR_KIND_VALUES, ptr: a.as_ptr() as *const u8, len: a.len(), ptr2: b.as_ptr() as *const u8, len2: b.len() }
            }
            ScriptArrayStorage::F32(v) => ArrView { tag, kind: ARR_KIND_F32, ptr: v.as_ptr() as *const u8, len: v.len(), ptr2: std::ptr::null(), len2: 0 },
            ScriptArrayStorage::U32(v) => ArrView { tag, kind: ARR_KIND_U32, ptr: v.as_ptr() as *const u8, len: v.len(), ptr2: std::ptr::null(), len2: 0 },
            ScriptArrayStorage::U16(v) => ArrView { tag, kind: ARR_KIND_U16, ptr: v.as_ptr() as *const u8, len: v.len(), ptr2: std::ptr::null(), len2: 0 },
            ScriptArrayStorage::U8(v) => ArrView { tag, kind: ARR_KIND_U8, ptr: v.as_ptr(), len: v.len(), ptr2: std::ptr::null(), len2: 0 },
        }
    }

    /// Changes an array by `op` (ARR_EDIT_*): the result is the op's (an error value or NIL).
    #[inline]
    #[relax_api]
    pub fn arr_edit(&mut self, op: u32, array: ScriptArray, index: usize, value: ScriptValue, trap: ScriptTrap) -> ScriptValue {
        match op {
            ARR_EDIT_PUSH => {
                self.array_push_impl(array, value, trap);
                NIL
            }
            ARR_EDIT_PUSH_UNCHECKED => {
                self.array_push_unchecked_impl(array, value);
                NIL
            }
            ARR_EDIT_SET_INDEX => self.set_array_index_impl(array, index, value, trap),
            ARR_EDIT_FREEZE => {
                self.arrays[array].tag.freeze();
                NIL
            }
            _ => unreachable!("arr_edit op"),
        }
    }

    /// A new array by `kind` (ARR_NEW_*).
    ///
    /// # Safety
    /// For ARR_NEW_U8_COPY, `ptr` and `len` describe readable bytes; for ARR_NEW_U8_OWNED, `ptr`,
    /// `len` and `cap` satisfy Vec::from_raw_parts with the global allocator, and the caller
    /// relinquishes the allocation even when the VM rejects the charge.
    #[inline]
    #[relax_api]
    pub unsafe fn arr_new(&mut self, kind: u32, ptr: *mut u8, len: usize, cap: usize) -> ScriptArray {
        match kind {
            ARR_NEW_EMPTY => self.new_array_impl(),
            ARR_NEW_U8_COPY => self.new_array_from_slice_u8_impl(std::slice::from_raw_parts(ptr, len)),
            ARR_NEW_U8_OWNED => self.new_array_from_owned_u8_impl(ptr, len, cap),
            _ => unreachable!("arr_new kind"),
        }
    }

    #[inline]
    pub fn new_array(&mut self) -> ScriptArray {
        // SAFETY: ARR_NEW_EMPTY reads no pointer
        unsafe { self.arr_new(ARR_NEW_EMPTY, std::ptr::null_mut(), 0, 0) }
    }

    #[inline]
    pub fn freeze_array(&mut self, array: ScriptArray) {
        self.arr_edit(ARR_EDIT_FREEZE, array, 0, NIL, NoTrap);
    }

    /// Frozen (`arr.freeze()`): its contents can no longer change, so a
    /// host may keep what it derived from them.
    #[inline]
    pub fn array_is_frozen(&self, array: ScriptArray) -> bool {
        self.arr_view(array).tag.is_frozen()
    }

    fn new_array_impl(&mut self) -> ScriptArray {
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
            let mut array = ScriptArrayData::default();
            array.tag.set_alloced();
            let (index, generation) = self.arrays.push(array);
            ScriptArray::new(index, generation)
        }
    }

    #[inline]
    pub fn array_len(&self, array: ScriptArray) -> usize {
        self.arr_view(array).len_all()
    }

    #[inline]
    pub fn array_push(&mut self, array: ScriptArray, value: ScriptValue, trap: ScriptTrap) {
        self.arr_edit(ARR_EDIT_PUSH, array, 0, value, trap);
    }

    fn array_push_impl(&mut self, array: ScriptArray, value: ScriptValue, trap: ScriptTrap) {
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

    #[inline]
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

    #[inline]
    pub fn array_push_unchecked(&mut self, array: ScriptArray, value: ScriptValue) {
        self.arr_edit(ARR_EDIT_PUSH_UNCHECKED, array, 0, value, NoTrap);
    }

    fn array_push_unchecked_impl(&mut self, array: ScriptArray, value: ScriptValue) {
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

    /// A u8 array's bytes (a view into the heap); None for other storage.
    #[inline]
    pub fn array_u8(&self, array: ScriptArray) -> Option<&[u8]> {
        let view = self.arr_view(array);
        // SAFETY: the view borrows the heap's storage, which `&self` keeps unchanged
        (view.kind == ARR_KIND_U8).then(|| unsafe { std::slice::from_raw_parts(view.ptr, view.len) })
    }

    /// A u16 array's elements; None for other storage.
    #[inline]
    pub fn array_u16(&self, array: ScriptArray) -> Option<&[u16]> {
        let view = self.arr_view(array);
        // SAFETY: the view borrows the heap's storage, which `&self` keeps unchanged
        (view.kind == ARR_KIND_U16).then(|| unsafe { std::slice::from_raw_parts(view.ptr as *const u16, view.len) })
    }

    /// A u32 array's elements; None for other storage.
    #[inline]
    pub fn array_u32(&self, array: ScriptArray) -> Option<&[u32]> {
        let view = self.arr_view(array);
        // SAFETY: the view borrows the heap's storage, which `&self` keeps unchanged
        (view.kind == ARR_KIND_U32).then(|| unsafe { std::slice::from_raw_parts(view.ptr as *const u32, view.len) })
    }

    /// An f32 array's elements; None for other storage.
    #[inline]
    pub fn array_f32(&self, array: ScriptArray) -> Option<&[f32]> {
        let view = self.arr_view(array);
        // SAFETY: the view borrows the heap's storage, which `&self` keeps unchanged
        (view.kind == ARR_KIND_F32).then(|| unsafe { std::slice::from_raw_parts(view.ptr as *const f32, view.len) })
    }

    /// A value array's elements as its two contiguous runs (front, back); None for typed
    /// storage.
    #[inline]
    pub fn array_values(&self, array: ScriptArray) -> Option<(&[ScriptValue], &[ScriptValue])> {
        let view = self.arr_view(array);
        // SAFETY: the view borrows the heap's storage, which `&self` keeps unchanged
        (view.kind == ARR_KIND_VALUES).then(|| unsafe {
            (
                std::slice::from_raw_parts(view.ptr as *const ScriptValue, view.len),
                std::slice::from_raw_parts(view.ptr2 as *const ScriptValue, view.len2),
            )
        })
    }

    #[inline]
    pub fn array_storage(&self, array: ScriptArray) -> &ScriptArrayStorage {
        let array = &self.arrays[array];
        &array.storage
    }

    /// Copies at most `out.len()` converted bytes; returns the number written.
    pub fn copy_array_bytes(&self, array: ScriptArray, out: &mut [u8]) -> usize {
        let len = self.array_len(array).min(out.len());
        for (i, byte) in out[..len].iter_mut().enumerate() { *byte = self.array_byte_value(array, i); }
        len
    }

    /// The conversion to a byte used when applying a typed or value array to Vec<u8>.
    fn array_byte_value(&self, array: ScriptArray, index: usize) -> u8 {
        match &self.arrays[array].storage {
            ScriptArrayStorage::ScriptValue(v) => v[index].into(),
            ScriptArrayStorage::F32(v) => v[index] as u8,
            ScriptArrayStorage::U32(v) => v[index] as u8,
            ScriptArrayStorage::U16(v) => v[index] as u8,
            ScriptArrayStorage::U8(v) => v[index],
        }
    }

    pub fn new_array_from_vec_u8(&mut self, data: Vec<u8>) -> ScriptArray {
        let mut data = std::mem::ManuallyDrop::new(data);
        // SAFETY: ownership of this global-allocator Vec is transferred exactly once.
        unsafe { self.new_array_from_owned_u8(data.as_mut_ptr(), data.len(), data.capacity()) }
    }

    /// Takes ownership of a byte vector through its allocation, never its Rust layout.
    /// # Safety
    /// The arguments must satisfy Vec::from_raw_parts with the global allocator. The
    /// caller relinquishes ownership even when the VM rejects the allocation charge.
    #[inline]
    pub unsafe fn new_array_from_owned_u8(&mut self, data: *mut u8, len: usize, capacity: usize) -> ScriptArray {
        self.arr_new(ARR_NEW_U8_OWNED, data, len, capacity)
    }

    unsafe fn new_array_from_owned_u8_impl(&mut self, data: *mut u8, len: usize, capacity: usize) -> ScriptArray {
        let data = Vec::from_raw_parts(data, len, capacity);
        let ptr = self.new_array_impl();
        if self.is_allocation_poison_array(ptr) { return ptr; }
        if !self.charge_allocation(data.capacity(), "creating a byte array") { return ptr; }
        let array = &mut self.arrays[ptr];
        array.tag.set_dirty();
        array.storage = ScriptArrayStorage::U8(data);
        ptr
    }

    #[inline]
    pub fn new_array_from_slice_u8(&mut self, data: &[u8]) -> ScriptArray {
        // SAFETY: a slice's pointer and length are readable bytes
        unsafe { self.arr_new(ARR_NEW_U8_COPY, data.as_ptr() as *mut u8, data.len(), 0) }
    }

    fn new_array_from_slice_u8_impl(&mut self, data: &[u8]) -> ScriptArray {
        let ptr = self.new_array_impl();
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

    pub fn array_take_storage(&mut self, array: ScriptArray) -> ScriptArrayStorage {
        std::mem::replace(&mut self.arrays[array].storage, ScriptArrayStorage::ScriptValue(Default::default()))
    }

    pub fn array_swap_storage(&mut self, array: ScriptArray, storage: &mut ScriptArrayStorage) {
        std::mem::swap(&mut self.arrays[array].storage, storage);
    }

    pub fn array_mut_self_with<R, F: FnOnce(&mut Self, &ScriptArrayStorage) -> R>(
        &mut self,
        array: ScriptArray,
        cb: F,
    ) -> R {
        let mut storage = self.array_take_storage(array);
        let r = cb(self, &storage);
        self.array_swap_storage(array, &mut storage);
        r
    }

    pub fn array_mut_mut_self_with<R, F: FnOnce(&mut Self, &mut ScriptArrayStorage) -> R>(
        &mut self,
        array: ScriptArray,
        cb: F,
    ) -> R {
        let mut storage = self.array_take_storage(array);
        let r = cb(self, &mut storage);
        self.array_swap_storage(array, &mut storage);
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

    #[inline]
    pub fn array_index(&self, array: ScriptArray, index: usize, trap: ScriptTrap) -> ScriptValue {
        let view = self.arr_view(array);
        if let Some(value) = view.index(index) {
            return value;
        } else {
            script_err_out_of_bounds!(
                trap,
                "array index {} out of bounds (len={})",
                index,
                view.len_all()
            )
        }
    }

    #[inline]
    pub fn array_index_unchecked(&self, array: ScriptArray, index: usize) -> ScriptValue {
        if let Some(value) = self.arr_view(array).index(index) {
            return value;
        } else {
            NIL
        }
    }

    #[inline]
    pub fn set_array_index(
        &mut self,
        array: ScriptArray,
        index: usize,
        value: ScriptValue,
        trap: ScriptTrap,
    ) -> ScriptValue {
        self.arr_edit(ARR_EDIT_SET_INDEX, array, index, value, trap)
    }

    fn set_array_index_impl(
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

    #[test]
    fn capped_sparse_array_growth_is_refused_before_mutation() {
        let mut heap = ScriptHeap::empty();
        let array = heap.new_array();
        heap.set_max_heap_bytes(Some(usize::MAX));
        let baseline = heap.accounted_heap_bytes();
        heap.set_max_heap_bytes(Some(baseline + 1024));

        let trap = ScriptTrap::NoTrap;
        let before = heap.arrays[array].storage.retained_bytes();
        heap.set_array_index(array, 1 << 20, NIL, trap.pass());
        assert!(heap.take_heap_limit_exceeded());
        assert_eq!(heap.array_len(array), 0);
        assert_eq!(heap.arrays[array].storage.retained_bytes(), before);

        // A write that fits is accepted and charged.
        let accounted = heap.accounted_heap_bytes();
        heap.set_array_index(array, 3, NIL, trap.pass());
        assert!(!heap.take_heap_limit_exceeded());
        assert_eq!(heap.array_len(array), 4);
        assert!(heap.accounted_heap_bytes() > accounted);
    }

    #[test]
    fn numeric_storage_variants_report_retained_capacity() {
        let mut heap = ScriptHeap::empty();
        let bytes = heap.new_array_from_vec_u8(vec![1, 2, 3, 4]);
        assert!(heap.arrays[bytes].storage.retained_bytes() >= 4);
        let text = heap.new_string_from_str("héllo wörld");
        let chars = heap.string_to_chars_array(text);
        assert!(matches!(heap.arrays[chars].storage, ScriptArrayStorage::U32(_)));
        assert!(heap.arrays[chars].storage.retained_bytes() >= 11 * 4);
        let generic = heap.new_array();
        let trap = ScriptTrap::NoTrap;
        for index in 0..8 {
            heap.array_push(generic, ScriptValue::from_f64(index as f64), trap.pass());
        }
        assert!(
            heap.arrays[generic].storage.retained_bytes()
                >= 8 * std::mem::size_of::<ScriptValue>()
        );
    }
}
