// M10: a Copy struct takes its new layout only when no running value of it is left. One the
// running code put in a trait object (a static holds it as `dyn Debug`, which its type does not
// show) may be alive: v1, a field added, restarts naming the trait objects.
use std::sync::Mutex;

#[derive(Debug, Clone, Copy)]
struct P {
    a: u32,
    b: u64,
}

static SHOWN: Mutex<Option<Box<dyn std::fmt::Debug + Send>>> = Mutex::new(None);

#[test]
fn run() {
    let mut shown = SHOWN.lock().unwrap();
    if shown.is_none() {
        *shown = Some(Box::new(P { a: 1, b: 2 }));
    }
    println!("out {:?}", shown.as_ref().unwrap());
}
