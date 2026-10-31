// A trait object of auto traits only (`Box<dyn Send>`, `dyn Send + Sync`): no principal
// trait, its vtable is drop / size / align (rustc). Lines "| " are compared with rustc.
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        println!("| drop {}", self.0);
    }
}

fn retire<T: Send + 'static>(value: T) {
    type Retired = Box<dyn Send>;
    static DROPPER: Mutex<Option<Sender<Retired>>> = Mutex::new(None);
    let mut d = DROPPER.lock().unwrap();
    let v: Retired = Box::new(value);
    if d.is_none() {
        let (tx, rx) = channel::<Retired>();
        std::thread::spawn(move || {
            for x in rx {
                drop(x);
                println!("| retired");
            }
        });
        *d = Some(tx);
    }
    d.as_ref().unwrap().send(v).ok();
}

#[test]
fn d01_auto_only_objects() {
    let b: Box<dyn Send + Sync> = Box::new(D(1));
    drop(b);
    let v: Vec<Box<dyn Send>> = vec![Box::new(D(2)), Box::new(String::from("s")), Box::new(D(3))];
    drop(v);
    println!("| {} {}", std::mem::size_of::<Box<dyn Send>>(), std::mem::size_of_val(&*Box::new(D(9)) as &dyn Sync));
    retire(D(4));
    retire(vec![D(5), D(6)]);
    std::thread::sleep(std::time::Duration::from_millis(200));
}
