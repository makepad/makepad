// Port of box3d/src/arena_allocator.h/.c + block_allocator.h/.c
//
// DEVIATION FROM C: the C allocators hand out raw pointers into bump/block
// memory. Safe Rust cannot do that, and the ported engine call sites use plain
// Vec<T> scratch allocations instead. What remains here is:
//
// - Stack / Arena: bookkeeping ports that produce Vec<T> allocations while
//   tracking the same statistics the C versions expose (capacity, allocation,
//   max allocation, peak demand) so Counters and grow-on-sync behavior match.
// - BlockAllocator: a handle-based port. b3AllocateElement returns a stable
//   element index instead of a stable pointer; the C free list threaded
//   through element memory becomes a Vec<i32> of free indices. The engine's
//   only use (per-contact manifold arrays in physics_world) is replaced by
//   Vec<Manifold> in the port, so this exists mainly for API/test parity.

use crate::b3_assert;
use crate::core::NULL_INDEX;

pub const MAX_STACK_ENTRIES: usize = 32;

// 16-byte alignment for SSE2 + typical struct alignment.
pub const ARENA_ALIGNMENT: i32 = 16;

#[derive(Clone, Copy, Debug, Default)]
struct StackEntry {
    size: i32,
}

/// This is a stack-like arena allocator used for fast per step allocations.
/// You must nest allocate/free pairs.
#[derive(Clone, Debug, Default)]
pub struct Stack {
    capacity: i32,
    index: i32,

    allocation: i32,
    max_allocation: i32,

    entries: Vec<StackEntry>,
}

pub fn create_stack(capacity: i32) -> Stack {
    b3_assert!(capacity >= 0);
    Stack {
        capacity,
        index: 0,
        allocation: 0,
        max_allocation: 0,
        entries: Vec::with_capacity(MAX_STACK_ENTRIES),
    }
}

pub fn destroy_stack(stack: &mut Stack) {
    *stack = Stack::default();
}

/// b3StackAlloc: returns a zero-initialized scratch buffer of `count` items.
/// The buffer is a plain Vec; pass it back to stack_free in LIFO order.
pub fn stack_alloc<T: Clone + Default>(stack: &mut Stack, count: i32, _name: &str) -> Vec<T> {
    if stack.entries.len() == MAX_STACK_ENTRIES {
        b3_assert!(false);
        return Vec::new();
    }

    let size = count.max(0) * std::mem::size_of::<T>() as i32;

    // ensure allocation is 32 byte aligned to support 256-bit SIMD
    let size32 = if size > 0 { ((size - 1) | 0x1F) + 1 } else { 0 };

    stack.index += size32;
    stack.allocation += size32;
    if stack.allocation > stack.max_allocation {
        stack.max_allocation = stack.allocation;
    }

    stack.entries.push(StackEntry { size: size32 });

    vec![T::default(); count.max(0) as usize]
}

/// b3StackFree: frees the most recent allocation (LIFO).
pub fn stack_free<T>(stack: &mut Stack, mem: Vec<T>) {
    b3_assert!(!stack.entries.is_empty());
    let entry = stack.entries.pop().unwrap();
    stack.index -= entry.size;
    stack.allocation -= entry.size;
    drop(mem);
}

/// Grow the stack based on usage.
pub fn grow_stack(stack: &mut Stack) {
    // Stack must not be in use
    b3_assert!(stack.allocation == 0);

    if stack.max_allocation > stack.capacity {
        stack.capacity = stack.max_allocation + stack.max_allocation / 2;
    }
}

pub fn get_stack_capacity(stack: &Stack) -> i32 {
    stack.capacity
}

pub fn get_stack_allocation(stack: &Stack) -> i32 {
    stack.allocation
}

pub fn get_max_stack_allocation(stack: &Stack) -> i32 {
    stack.max_allocation
}

/// Bump arena (bookkeeping port). In C, b3Arena is passed by value so the bump
/// index auto-restores on function return; in Rust save `arena.index` before a
/// scope and restore it after (see arena_mark / arena_restore).
#[derive(Clone, Debug, Default)]
pub struct Arena {
    pub capacity: i32,
    pub index: i32,

    // C keeps these in a shared heap block so copies observe them; the Rust
    // port is not copied, so they live inline.
    max_index: i32,
    overflow_bytes: i32,
    peak_demand: i32,
}

pub fn create_arena(capacity: i32) -> Arena {
    let c = if capacity > 8 { capacity } else { 8 };
    Arena { capacity: c, ..Default::default() }
}

pub fn destroy_arena(arena: &mut Arena) {
    *arena = Arena::default();
}

/// Save the bump position (C: copying b3Arena by value).
#[inline]
pub fn arena_mark(arena: &Arena) -> i32 {
    arena.index
}

/// Restore the bump position (C: function return discarding the copy).
#[inline]
pub fn arena_restore(arena: &mut Arena, mark: i32) {
    arena.index = mark;
}

/// b3Bump + b3ArenaOverflowAlloc: returns a zero-initialized buffer of `count`
/// items and advances the bookkeeping bump pointer.
pub fn arena_alloc<T: Clone + Default>(arena: &mut Arena, count: i32) -> Vec<T> {
    let size = count.max(0) * std::mem::size_of::<T>() as i32;
    if size == 0 {
        return Vec::new();
    }

    let aligned = (arena.index + (ARENA_ALIGNMENT - 1)) & !(ARENA_ALIGNMENT - 1);

    if aligned + size > arena.capacity {
        // overflow allocation
        arena.overflow_bytes += size;
    } else {
        arena.index = aligned + size;
        if arena.index > arena.max_index {
            arena.max_index = arena.index;
        }
    }

    vec![T::default(); count.max(0) as usize]
}

/// Call between simulation steps. Grows the backing capacity if last step's
/// demand (maxIndex + overflowBytes) exceeded it.
pub fn arena_sync(arena: &mut Arena) {
    let demand = arena.max_index + arena.overflow_bytes;
    if demand > arena.peak_demand {
        arena.peak_demand = demand;
    }
    if demand > arena.capacity {
        let new_capacity = demand + demand / 2;
        arena.capacity = new_capacity;
    }

    arena.index = 0;
    arena.max_index = 0;
    arena.overflow_bytes = 0;
}

pub fn get_arena_capacity(arena: &Arena) -> i32 {
    arena.capacity
}

pub fn get_arena_peak_demand(arena: &Arena) -> i32 {
    arena.peak_demand
}

// ---------------------------------------------------------------------------
// Block allocator (block_allocator.h/.c)
// ---------------------------------------------------------------------------

pub const BLOCK_EXPONENT: i32 = 8;
pub const BLOCK_SIZE: i32 = 1 << BLOCK_EXPONENT;

/// Fixed-size element pool. The C version returns stable pointers into
/// 256-element chunks with an intrusive free list; the port returns stable
/// element indices with an explicit free list.
#[derive(Clone, Debug, Default)]
pub struct BlockAllocator {
    pub element_size: i32,
    free_list: Vec<i32>,
    next_index: i32,
    allocation_count: i32,
    block_count: i32,
}

// Element must be large enough to hold a pointer (C constraint, kept for parity).
pub fn create_block_allocator(element_size: i32, initial_count: i32) -> BlockAllocator {
    b3_assert!(element_size >= std::mem::size_of::<*const u8>() as i32);

    let mut allocator = BlockAllocator {
        element_size,
        ..Default::default()
    };

    if initial_count > 0 {
        allocator.block_count = (initial_count + BLOCK_SIZE - 1) >> BLOCK_EXPONENT;
    }

    allocator
}

pub fn destroy_block_allocator(allocator: &mut BlockAllocator) {
    *allocator = BlockAllocator::default();
}

/// Returns a stable element index (C: a stable pointer). Never NULL_INDEX.
pub fn allocate_element(allocator: &mut BlockAllocator) -> i32 {
    allocator.allocation_count += 1;

    // Pop from free list first
    if let Some(element) = allocator.free_list.pop() {
        return element;
    }

    let index = allocator.next_index;
    allocator.next_index += 1;

    let required_block_count = (index >> BLOCK_EXPONENT) + 1;
    if required_block_count > allocator.block_count {
        allocator.block_count = required_block_count;
    }

    index
}

pub fn free_element(allocator: &mut BlockAllocator, element: i32) {
    b3_assert!(element != NULL_INDEX);
    b3_assert!(allocator.allocation_count > 0);

    allocator.allocation_count -= 1;

    // Push onto free list
    allocator.free_list.push(element);
}
