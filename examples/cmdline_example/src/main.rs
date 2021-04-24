#![allow(unused_variables)]
#![allow(dead_code)]
use makepad_live_parser::*;
use std::any::Any;

#[derive(Debug, DeLive)]
struct MyComponent {
    x: f32,
    y: f32,
    z: f32,
    e1: MyEnum,
    e2: MyEnum,
    e3: MyEnum,
}

#[derive(Debug, DeLive)]
struct MyVec4(f32);

#[derive(Debug, DeLive)]
enum MyEnum {
    Value1,
    Value2(f32),
    Value3 {value: f32}
}

struct MyComponentFactory {}
impl DeLiveFactory for MyComponentFactory {
    fn de_live_any(&self, lr: &LiveRegistry, f: usize, l: usize, s: usize) -> Result<Box<dyn Any>,
    DeLiveErr> {
        let mv = MyComponent::de_live(lr, f, l, s) ?;
        Ok(Box::new(mv))
    }
}

fn main() {
    // ok lets do a deserialize
    let mut lr = LiveRegistry::default();
    let source = r#"
        MyEnum: Enum {
            Value1: Enum
            Value2: Enum()
            Value3: Enum {}
        }
        
        MyComponent: Component {
            e1: MyEnum::Value1
            e2: MyEnum::Value2(2)
            e3: MyEnum::Value3{value: 1} 
        }
        MyDerive2: MyComponent {x: 1.0, y: 2.0, z: 5.0}
    "#;
    match lr.parse_live_file("test.live", id_check!(main), id_check!(test), source.to_string()) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    
    let mut errors = Vec::new();
    lr.expand_all_documents(&mut errors);
    
    println!("{}", lr.expanded[0]);
    
    for msg in errors {
        println!("{}\n", msg.to_live_file_error("", source));
    }
    
    lr.register_component(id!(main), id!(test), id!(MyComponent), Box::new(MyComponentFactory {}));
    let val = lr.create_component(id!(main), id!(test), &[id!(MyDerive2)]);
    
    match val.unwrap().downcast_ref::<MyComponent>() {
        Some(comp) => {
            println!("{:?}", comp);
        }
        None => {
            println!("No Value");
        }
    }
    
    // ok now we should deserialize MyObj
    // we might wanna plug the shader-compiler in some kind of deserializer as well
}
