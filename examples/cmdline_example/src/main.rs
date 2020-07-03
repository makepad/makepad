/*use makepad_shader_macro::*; 
use makepad_shader_compiler::uid;
use makepad_shader_compiler::shader::*;

struct Bla{}
impl Bla{
    fn counter()->Vec4Id{uid!()}
}

fn main() {
    shader!{"
        instance counter: Bla::counter();
        fn pixel() -> vec4 {
            df_viewport(pos * vec2(w, h));
            df_circle(0.5 * w, 0.5 * h, 0.5 * w);
            return df_fill(vec3(1.0,0.,0.));
            //return df_fill(mix(color!(green), color!(blue), abs(sin(counter))));
        }
    "};
}
*/

// lets make tinyserde dep free too!

use makepad_microserde::*;
/*
#[derive(SerBin)] 
struct MyStruct<T> where T:Clone{
    step1: T,
    step2: u32
}

#[derive(SerBin)] 
struct MyStruct2<T>(T, u32) where T:Clone;
*/
#[derive(SerBin, DeBin)] 
enum MyEnum<T> where T:Clone{
    One,
    Two(T,u32),
    Three{a:u32, b:T}
}

fn main(){
    //let a = MyStruct{step1:1,step2:2};
    //let x = MyStruct2(1,2);
    let e = MyEnum::Two(1,2);
    //println!("{}",x.0);
}