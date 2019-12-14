use makepad_tinyserde::*;

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
    
    let mut s = SerBinData{dat:Vec::new()};
    
    let x = TestStruct{x:10.0, v:"hello".to_string(), t:vec![1,2,3,4], y:[1,2,3]};
    x.ser_bin(&mut s);
    
    let mut d = DeBinData{dat:s.dat, off:0};
    let y: TestStruct = DeBin::de_bin(&mut d);

    println!("{:?}", y);
    
    let mut s = SerBinData{dat:Vec::new()};
    let x = TestEnum::A(TestTuple(3,4));
    x.ser_bin(&mut s);
    let mut d = DeBinData{dat:s.dat, off:0};
    let y: TestEnum = DeBin::de_bin(&mut d);

    println!("{:?}", y);
    
    // lets deserialize it
    
}
