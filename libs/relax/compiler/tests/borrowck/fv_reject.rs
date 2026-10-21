// Field views (relaxed Rust): rejected -- the summaries overlap, or field views cannot see.
//@ relaxed
use std::cell::RefCell;
struct Cb { id: u32 }
struct S { callbacks: Vec<Cb>, counter: u32, cell: RefCell<u32>, ptr: *mut u32, inner: Box<Vec<u32>> }

trait Tr { fn touch(&mut self); }
impl Tr for S { fn touch(&mut self) { self.counter += 1; } }

impl S {
    fn clear_callbacks(&mut self) { self.callbacks.clear(); }
    fn via_callee(&mut self) { self.clear_callbacks(); }
    fn recurse(&mut self, n: u32) { if n > 0 { self.recurse(n - 1) } else { self.callbacks.clear() } }
    fn read_cell(&self) -> u32 { *self.cell.borrow() }
    fn raw(&mut self) { unsafe { *self.ptr += 1 } }
    fn items_mut(&mut self) -> &mut Vec<Cb> { &mut self.callbacks }
    fn whole(&mut self) -> &mut S { self }

    fn overlap(&mut self) -> u32 {
        let first = &self.callbacks[0];
        self.clear_callbacks(); //~ ERROR E0502
        first.id
    }
    fn overlap_transitive(&mut self) -> u32 {
        let first = &self.callbacks[0];
        self.via_callee(); //~ ERROR E0502
        first.id
    }
    fn overlap_recursive(&mut self) -> u32 {
        let first = &self.callbacks[0];
        self.recurse(3); //~ ERROR E0502
        first.id
    }
    fn interior_mutability(&mut self) -> u32 {
        let c = &mut self.cell;
        let v = self.read_cell(); //~ ERROR E0502
        *c.get_mut() + v
    }
    fn raw_pointer(&mut self) -> u32 {
        let first = &self.callbacks[0];
        self.raw(); //~ ERROR E0502
        first.id
    }
    fn dyn_call(&mut self, t: &mut dyn Tr) -> u32 {
        let first = &self.callbacks[0];
        let me: &mut dyn Tr = self; //~ ERROR E0502
        me.touch();
        let _ = t;
        first.id
    }
    fn returned_ref(&mut self) -> u32 {
        let items = self.items_mut();
        let first = &self.callbacks[0]; //~ ERROR E0502
        items.clear();
        first.id
    }
    fn escaping_self(&mut self) -> u32 {
        let first = &self.callbacks[0];
        let w = self.whole(); //~ ERROR E0502
        w.counter += 1;
        first.id
    }
    fn closure_overlap(&mut self) {
        let first = &self.callbacks[0];
        let mut f = || self.clear_callbacks(); //~ ERROR E0500
        f();
        let _ = first.id;
    }
}

// a receiver reached through fields of a `&mut` parameter
struct Vm { heap: Vec<u32>, log: Vec<String> }
impl Vm { fn note(&mut self, s: &str) { self.log.push(s.to_string()); } }
struct Cx { vm: Vm }
fn field_receiver_overlap(cx: &mut Cx) -> usize {
    let l = &cx.vm.log;
    cx.vm.note("x"); //~ ERROR E0502
    l.len()
}

// a reference into a container's buffer: the buffer is owned through a raw pointer, so any
// use of the container (here `pop`) overlaps it
fn into_buffer(v: &mut Vec<Vec<u32>>) {
    v[0].push(v.pop().unwrap().len() as u32); //~ ERROR E0499
}
// extended two-phase borrows: the meanwhile borrow is still live at the call
impl Vm { fn first(&mut self) -> &mut u32 { &mut self.heap[0] } }
fn reserved_then_live(vm: &mut Vm) {
    std::mem::swap(vm.first(), vm.first()); //~ ERROR E0499
}

fn main() {}
