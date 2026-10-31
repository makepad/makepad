// Field views (relaxed Rust): accepted -- each call only uses fields the live borrow does not.
//@ relaxed
struct Cb { id: u32 }
struct Engine { alive: Vec<u32> }
struct Host { engine: Engine }
struct Tween { callbacks: Vec<Cb>, host: Host, counter: u32, name: String }

impl Tween {
    fn reachable(&self, id: u32) -> bool {
        self.host.engine.alive.contains(&id)
    }
    fn bump(&mut self) {
        self.counter += 1;
    }
    fn bump_twice(&mut self) {
        self.bump();
        self.bump();
    }
    fn draw(&mut self, name: &str) {
        self.counter += name.len() as u32;
    }
    fn items_mut(&mut self) -> &mut Vec<Cb> {
        &mut self.callbacks
    }
    // widgets/core/src/tween_script.rs without the mem::take dance
    fn release(&mut self) {
        self.callbacks.retain(|c| self.reachable(c.id));
    }
    fn through_callee_of_callee(&mut self) -> u32 {
        let first = &self.callbacks[0];
        self.bump_twice();
        first.id
    }
    // tab.rs draw_walk without the mem::take of `name`
    fn draw_walk(&mut self) {
        self.draw(&self.name);
    }
    // a returned reference borrows only `callbacks`
    fn returned_ref(&mut self) {
        let items = self.items_mut();
        self.bump();
        items.clear();
    }
}

// any `&mut` parameter, not just self (`*vm`, `*cx`)
struct Vm { heap: Vec<u32>, stack: Vec<u32> }
fn push_stack(vm: &mut Vm, v: u32) {
    vm.stack.push(v);
}
fn param_views(vm: &mut Vm) -> u32 {
    let h = &vm.heap[0];
    push_stack(vm, 1);
    *h
}

// a receiver reached through fields of a `&mut` parameter (`cx.vm.note()` uses only `log`)
struct Vm2 { heap: Vec<u32>, log: Vec<String> }
impl Vm2 { fn note(&mut self, s: &str) { self.log.push(s.to_string()); } }
struct Cx2 { vm: Vm2 }
fn field_receiver(cx: &mut Cx2) {
    let h = &mut cx.vm.heap;
    cx.vm.note("x");
    h.push(1);
}

// extended two-phase borrows: `vm` reserved for the call, borrowed again meanwhile
impl Vm2 { fn push(&mut self, x: u32) { self.heap.push(x); } fn len(&mut self) -> u32 { self.heap.len() as u32 } }
fn add(vm: &mut Vm2, x: u32) { vm.heap.push(x); }
fn count(vm: &mut Vm2) -> u32 { vm.heap.push(0); vm.heap.len() as u32 }
fn reborrow_arg(vm: &mut Vm2) { add(vm, count(vm)); }
fn reborrow_receiver(vm: &mut Vm2) { vm.push(vm.len()); }

fn main() {}
