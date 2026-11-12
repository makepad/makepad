include!(concat!(env!("OUT_DIR"), "/gen.rs"));

extern "C" {
    fn cn_answer() -> i32;
}

pub fn answer() -> i32 {
    (unsafe { cn_answer() }) + 2
}

pub fn generated() -> Generated {
    Generated(7)
}
