use std::io::{ Write};
use std::fs;    
    
fn main() {
   let data = fs::read("target/wasm32-unknown-unknown/release/makepad_studio.wasm").expect("Can't read file");
    // lets brotli it
    let mut result = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut result, 4096 /* buffer size */, 11, 22);
        writer.write_all(&data).expect("Can't write data");
    };
    println!("SIZE {}", result.len()/1024);
}

