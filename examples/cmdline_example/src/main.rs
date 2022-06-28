#![allow(unused)]
use makepad_procmac_example::*;

#[derive(DeriveExample)]
struct Pt{
    x:f32,
    y:f32
}

fn main() {
    let _x = Pt{x:1.0,y:2.0};
    
    let r = function_example!(3, 6);
    
    println!("Returned value {}", r);
}