// rejects: E0308
#![allow(dead_code)]
struct Unused<const N: u8 = "bad">;
