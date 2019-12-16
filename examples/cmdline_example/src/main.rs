use makepad_tinyserde::*;
use std::collections::{HashMap};

#[derive(SerRon, DeRon, SerJson, DeJson, SerBin, DeBin, PartialEq, Debug, Clone)]
enum TestEnum{ 
    X{x:u32, y:Option<u32>},
    Y(u32, Option<TestNew>),
    Z
}

#[derive(SerRon, DeRon, SerJson, DeJson, SerBin, DeBin, PartialEq, Debug, Clone)]
struct TestNew(String);

#[derive(SerRon, DeRon, SerJson, DeJson, SerBin, DeBin, PartialEq, Debug, Clone)]
struct TestStruct{
    t: [u32;4],
    s: Vec<TestStruct>,
    m: HashMap<String, u32>,
    w: TestEnum,
    h: TestEnum,
    v: TestEnum
}

fn main() {
    let mut x = TestStruct {
        t:[1,2,3,4],
        s:vec![],
        m:{let mut m = HashMap::new(); m.insert("hi".to_string(),1); m.insert("ho".to_string(),2);m},
        w:TestEnum::Y(1, Some(TestNew("hello \"world".to_string()))),
        h:TestEnum::X{x:10,y:Some(10)},
        v:TestEnum::Z
    };
    for _ in 0..1{
        x.s.push(x.clone());
    }
    
    let ron = x.serialize_ron();
    let y:TestStruct = DeRon::deserialize_ron(&ron).expect("cant parse");
    
    println!("RON equal: {}", x == y);

    let json = x.serialize_json();
    let y:TestStruct = DeJson::deserialize_json(&json).expect("cant parse");

    println!("JSON equal: {}", x == y);

    let bin = x.serialize_bin();
    let y:TestStruct = DeBin::deserialize_bin(&bin).expect("cant parse");

    println!("BIN equal: {}", x == y);
}