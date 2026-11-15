//~ E0277
//~ help: Mine: StaticAllocator
// Box::into_pin needs an allocator that keeps its blocks until they are freed (StaticAllocator,
// rustc 1.100.0): Global and System are, an allocator of the program's own is not.
use std::alloc::{AllocError, Allocator, Global, Layout};
use std::ptr::NonNull;

struct Mine;

unsafe impl Allocator for Mine {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Global.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { Global.deallocate(ptr, layout) }
    }
}

fn main() {
    let p = Box::into_pin(Box::new_in(1u8, Mine));
    println!("{}", *p);
}
