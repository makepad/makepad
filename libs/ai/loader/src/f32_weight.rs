use crate::WeightOwner;
use std::alloc::{alloc_zeroed, dealloc, handle_alloc_error, Layout};
use std::ptr::NonNull;
use std::sync::Arc;

// The largest VM page size of our hosts (arm64 macOS), and a multiple of
// 4 KiB: storage aligned and sized to this is page-aligned everywhere.
const PAGE: usize = 16384;

/// Page-aligned immutable f32 storage. Freed when the last Arc goes
/// (a Metal no-copy buffer holds one).
pub struct PageF32 {
    ptr: NonNull<f32>,
    len: usize,
    bytes: usize,
}

// SAFETY: Storage is immutable once built; construction has exclusive access.
unsafe impl Send for PageF32 {}
// SAFETY: Shared access only exposes immutable storage.
unsafe impl Sync for PageF32 {}

impl PageF32 {
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr().cast()
    }

    pub fn page_bytes(&self) -> usize {
        self.bytes
    }

    pub fn as_slice(&self) -> &[f32] {
        // SAFETY: The allocation contains len initialized f32 values and
        // remains alive for the lifetime of this shared reference.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for PageF32 {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.bytes, PAGE).unwrap();
        // SAFETY: This is the allocation's original pointer and layout.
        unsafe { dealloc(self.ptr.as_ptr().cast(), layout) };
    }
}

/// An f32 weight matrix: its storage, owner and key name within the owner.
#[derive(Clone)]
pub struct F32Weight {
    storage: Arc<PageF32>,
    owner: Arc<WeightOwner>,
    name: String,
}

impl F32Weight {
    pub fn new<E>(
        owner: &Arc<WeightOwner>,
        name: &str,
        len: usize,
        fill: impl FnOnce(&mut [f32]) -> Result<(), E>,
    ) -> Result<Self, E> {
        let bytes = len
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(PAGE - 1))
            .expect("f32 weight allocation size overflow")
            / PAGE * PAGE;
        let bytes = bytes.max(PAGE);
        let layout = Layout::from_size_align(bytes, PAGE)
            .expect("f32 weight allocation size overflow");
        // SAFETY: The layout has nonzero size and valid alignment. Zeroed
        // bytes are valid f32 values, including the page padding.
        let ptr = NonNull::new(unsafe { alloc_zeroed(layout) }.cast::<f32>())
            .unwrap_or_else(|| handle_alloc_error(layout));
        let storage = PageF32 { ptr, len, bytes };
        // SAFETY: No references to the allocation have escaped construction.
        fill(unsafe { std::slice::from_raw_parts_mut(storage.ptr.as_ptr(), len) })?;
        Ok(Self {
            storage: Arc::new(storage),
            owner: owner.clone(),
            name: name.to_owned(),
        })
    }

    pub fn as_slice(&self) -> &[f32] { self.storage.as_slice() }
    pub fn len(&self) -> usize { self.storage.len }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn owner(&self) -> &WeightOwner { &self.owner }
    pub fn name(&self) -> &str { &self.name }
    pub fn storage(&self) -> &Arc<PageF32> { &self.storage }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_storage_is_initialized_shared_and_fallible() {
        let owner = Arc::new(WeightOwner::new("f32-storage"));
        for len in [0, 3, PAGE / 4 + 1] {
            let weight = F32Weight::new(&owner, "matrix", len, |out| {
                for (i, value) in out.iter_mut().enumerate() { *value = i as f32 + 0.5; }
                Ok::<_, ()>(())
            }).unwrap();
            let storage = weight.storage();
            assert_eq!(storage.as_ptr() as usize % PAGE, 0, "page-aligned pointer");
            assert_eq!(storage.page_bytes() % PAGE, 0, "whole pages");
            assert!(storage.page_bytes() >= (len * 4).max(PAGE), "allocation covers values and at least one page");
            assert_eq!(weight.as_slice(), (0..len).map(|i| i as f32 + 0.5).collect::<Vec<_>>(), "fill writes contents");
            // SAFETY: All bytes in the allocation are initialized, including slack.
            let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr(), storage.page_bytes()) };
            assert!(bytes[len * 4..].iter().all(|&b| b == 0), "slack stays zero");
            let clone = weight.clone();
            assert!(Arc::ptr_eq(storage, clone.storage()), "clone shares storage");
        }
        let failed = F32Weight::new(&owner, "failed", 3, |_| Err("fill failed"));
        assert!(matches!(failed, Err("fill failed")), "fill errors propagate");
    }
}
