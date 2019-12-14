use makepad_tinyserde::*;

#[derive(SerRon, DeRon, PartialEq, Debug)]
struct TestStruct {
    t: u8,
    v: Option<u8>,
    x: Option<u8>,
    z: bool,
    y: f32
}


fn main() {
    // ok . lets serialise Test to a binary
    
    let x = TestStruct {t: 10,z:false, y:0.5, v: Some(20), x: None};
    let output = x.serialize_ron();
    println!("{}", output);
    let y: TestStruct = DeRon::deserialize_ron(&output).expect("can't parse");
    
    println!("{:?}", y);
}

/*
#[derive(SerBin, DeBin, PartialEq, Debug)]
struct TestStruct {
    t: Vec<u8>,
    y: [u8;3],
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
    
    let x = TestStruct{x:10.0, v:"hello".to_string(), t:vec![1,2,3,4], y:[1,2,3]};
    x.ser_bin(&mut s);
    
    let y: TestStruct = DeBin::de_bin(&mut 0, &s);

    println!("{:?}", y);
    
    let mut s = Vec::new();
    let x = TestEnum::A(TestTuple(3,4));
    x.ser_bin(&mut s);
    let y: TestEnum = DeBin::de_bin(&mut 0, &s);

    println!("{:?}", y);
    
    // lets deserialize it
    
}
*/