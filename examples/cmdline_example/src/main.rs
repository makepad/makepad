use makepad_tinyserde::*;
/*
#[derive(SerRon, DeRon, PartialEq, Debug)]
enum TestEnum{
    X{x:u32, y:Option<u32>},
    Y
}

#[derive(SerRon, DeRon,PartialEq, Debug)]
struct TestNew(u32);

#[derive(SerRon,  DeRon,PartialEq, Debug)]
struct TestStruct{
    t: [u32;4],
    v: TestNew,
    w: TestEnum
}

fn main() {
    let x = TestStruct {
        t:[1,2,3,4],
        v:TestNew(10),
        w:TestEnum::X{x:10,y:None}
    };
    let output = x.serialize_ron();
    println!("{}", output);
    
    let y:TestStruct = DeRon::deserialize_ron(&output).expect("can't parse");
    println!("{:?}", y);
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
}*/

/*
#[derive(SerRon, DeRon, PartialEq, Debug)]
struct TestStruct {
    o: [Option<u8>;3],
    m: HashMap<u8,u8>,
    t: (u8,u8),
    v: Option<u8>,
    x: Option<u8>,
    z: bool,
    s: String,
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
        v: None,
        x: Some(20)
    };
    let output = x.serialize_ron();
    println!("{}", output);
    let y: TestStruct = DeRon::deserialize_ron(&output).expect("can't parse");
    
    println!("{:?}", y);
}
*/

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
