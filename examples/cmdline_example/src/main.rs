use std::collections::{HashMap};
use makepad_tinyserde::*;

#[derive(SerRon, DeRon, SerJson, DeJson, SerBin, DeBin, PartialEq, Debug, Clone)]
enum TestEnum{ 
    X{x:u32, y:Option<u32>},
    Y(u32, Option<TestNew>),
    Z
}

#[derive(SerRon, DeRon, SerJson, DeJson, SerBin, DeBin, PartialEq, Debug, Clone)]
struct TestNew(u32);

#[derive(SerRon, DeRon, SerJson, DeJson, SerBin, DeBin, PartialEq, Debug, Clone)]
struct TestStruct{
    t: [u32;4],
    s: Vec<TestStruct>,
    w: TestEnum,
    h: TestEnum,
    v: TestEnum
}

fn main() {
    let mut x = TestStruct {
        t:[1,2,3,4],
        s:vec![],
        w:TestEnum::Y(1, Some(TestNew(10))),
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

    //println!("{}", output); 
    
    //let y:TestStruct = DeJson::deserialize_json(&output).expect("can't parse");
    //println!("{:?}", y);
    // ok . lets serialise Test to a binary
    /*
    let x = TestStruct {
        t:[1,2,3,4],
        v:TestEnum::X{x:10,y:10},
        w:TestEnum::Y
    };
    let output = x.serialize_ron();
    println!("{}", output);
    let y: TestStruct = DeRon::deserialize_ron(&output).expect("can't parse");
    
    println!("{:?}", y);*/
}
/*

#[derive(SerRon, DeRon, PartialEq, Debug)]
struct TestTuple(u32, u32);

#[derive(SerRon, DeRon, PartialEq, Debug)]
enum TestEnum {
    A(TestTuple),
    B,
    C
}
#[derive(SerRon, DeRon, PartialEq, Debug)]
struct TestStruct {
    o: [Option<u8>;3],
    m: HashMap<u8,u8>,
    t: (u8,u8),
    v: Option<u8>,
    x: Option<u8>,
    z: bool,
    s: String,
    en: TestEnum,
    y: f32
}

fn main() {
    // ok . lets serialise Test to a binary
    
    let x = TestStruct {
        o: [None,Some(3),None],
        t: (10,30),
        m:{let mut m = HashMap::new();m.insert(3,4);m.insert(4,6);m},
        z: false,
        s: "hello".to_string(),
        y: 0.5,
        en:TestEnum::A(TestTuple(3,2)),
        v: None,
        x: Some(20)
    };
    let output = x.serialize_ron();
    println!("{}", output);
    let y: TestStruct = DeRon::deserialize_ron(&output).expect("can't parse");
    
    println!("{:?}", y);
}*/
/*

#[derive(SerBin, DeBin, PartialEq, Debug)]
struct TestStruct {
    t: Vec<u8>,
    //y: [u8;3],
    v: String,
    x: f64,
}

#[derive(SerBin, DeBin, PartialEq, Debug)]
struct TestTuple(u32, u32);

#[derive(SerBin, DeBin, PartialEq, Debug)]
enum TestEnum {
    A(TestTuple),
    B,
    C
}

fn main(){ 
    // ok . lets serialise Test to a binary
    
    let mut s = Vec::new();
    
    let x = TestStruct{x:10.0, v:"hello".to_string(), t:vec![1,2,3,4]};//, y:[1,2,3]};
    x.ser_bin(&mut s);
    
    let y: TestStruct = DeBin::de_bin(&mut 0, &s).expect("Could not parse");

    println!("{:?}", y);
    
    let mut s = Vec::new();
    let x = TestEnum::A(TestTuple(3,4));
    x.ser_bin(&mut s);
    let y: TestEnum = DeBin::de_bin(&mut 0, &s).expect("Could not parse");

    println!("{:?}", y);
    
    // lets deserialize it
    
}
*/