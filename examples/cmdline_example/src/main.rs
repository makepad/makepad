use time::*;
use std::cell::RefCell;
use std::cell::RefMut;
use std::sync::RwLock;
use std::thread::ThreadId;

struct MyCx{
    id:ThreadId,
    x:i32
}


thread_local! {
    static FOO: RefCell<bool> = RefCell::new(false);
}


static mut GLOBAL_MY_CX: *mut RwLock<MyCx> = 0 as *mut _;

impl MyCx{
    
    fn get<'a>()->std::sync::RwLockWriteGuard<'a, MyCx>{
        /*FOO.with(|foo| {
            if *foo.borrow() == true{
                panic!("WRONG THREAD");
            }
        });*/
        let my_cx = unsafe{&mut *GLOBAL_MY_CX};
        let my_cx = my_cx.write().unwrap();
        if my_cx.id != std::thread::current().id(){
            panic!("exit")
        }
        my_cx
    }
    
    fn test(&mut self, t:i32){
        self.x |= t;
    }
}



fn main() {
    let start = precise_time_ns();
    
    let mut x = RwLock::new(MyCx{id:std::thread::current().id(), x:0});
    unsafe{GLOBAL_MY_CX = &mut x};
    
    let start = precise_time_ns();
    for i in 0..10000{
        let mut my_cx = MyCx::get();
        my_cx.test(i);
    } 
    let end = precise_time_ns();
    let my_cx = MyCx::get();
    println!("HELLO1 {} {}", (end-start) as f64/1_000_000.0, my_cx.x);
    
    let mut y = MyCx{id:std::thread::current().id(), x:0};
    let start = precise_time_ns();
    for i in 0..1000000{
        y.test(i );
    }
    let end = precise_time_ns();
    println!("HELLO2 {} {}", (end-start)/1_000_000, y.x);
}
