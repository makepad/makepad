// ui_task.rs / drag_drop.rs / thread.rs: an inferred ADT input and output.
use std::sync::Mutex;
fn assign(m: &Mutex<Option<u32>>) {
    *m.lock().unwrap_or_else(|e| e.into_inner()) = Some(1);
}
fn flag(m: &Mutex<bool>) {
    if !*m.lock().unwrap_or_else(|e| e.into_inner()) {
        *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }
}
fn main() { assign(&Mutex::new(None)); flag(&Mutex::new(false)); }
