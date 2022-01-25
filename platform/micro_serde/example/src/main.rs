// lets make tinyserde dep free too!

use makepad_micro_serde::*;

#[derive(SerBin, DeBin, SerJson, DeJson, SerRon, DeRon, PartialEq)]
struct MyStruct<T> where T: Clone {
    pub a: T,
    b: u32,
    c: Option<Vec<u32>>,
    d: Option<Vec<u32>>,
    e: MyEnum<T>,
    f: MyEnum<T>,
    g: MyEnum<T>,
    h: MyEnum<T>,
    i: MyEnum<T>,
    j: String,
    k: [u32;2]
} 

#[derive(SerBin, DeBin, SerJson, DeJson, SerRon, DeRon, PartialEq)]
enum MyEnum<T> where T: Clone {
    One,
    Two(T, u32),
    Three {x: u32, y: T},
    Four {z: Option<u32>, w: T},
}

fn main() {
    //let a = MyStruct{step1:1,step2:None};
    //let x = MyStruct2(1,2);
    let x = MyStruct {
        a: 1,
        b: 2,
        c: Some(vec![3]),
        d: None,
        e: MyEnum::One,
        f: MyEnum::Two(4, 5),
        g: MyEnum::Three {x: 6, y: 7},
        h: MyEnum::Four {z: None, w: 8},
        i: MyEnum::Four {z: Some(9), w: 8},
        j: "Hello".to_string(),
        k: [10,11]
    };
    let bin = x.serialize_bin();
    println!("Bin len: {}", bin.len());
    let y:MyStruct<usize> = DeBin::deserialize_bin(&bin).unwrap();
    println!("Bin roundtrip equality {}", x == y);
    
    let json = x.serialize_json();
    println!("JSON Output {}", json);
    let y:MyStruct<usize> = DeJson::deserialize_json(&json).unwrap();
    println!("JSON roundtrip equality {}", x == y);
    
    let ron = x.serialize_ron();
    println!("RON Output {}", ron);
    let y:MyStruct<usize> = DeRon::deserialize_ron(&ron).unwrap();
    println!("RON roundtrip equality {}", x == y);
}
