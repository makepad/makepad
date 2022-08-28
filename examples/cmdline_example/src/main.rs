pub use ::makepad_error_log::{self,*};

fn main() {
    makepad_error_log::set_panic_hook();
    
    let i = 1.0;
    for i in 0..32{
        let x = (i as f32).to_bits();
        log!("{}=>0x{:08x},", i, x);
        //panic!("HI");
    }
     
    eprintln!("HO!@");
    println!("HI!");
}