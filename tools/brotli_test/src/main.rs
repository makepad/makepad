use std::io::Write;

fn main(){ 
    let data = std::fs::read("target/wasm32-unknown-unknown/release/makepad_wasm.wasm").expect("can't read");
    let mut result = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut result, 4096 /* buffer size */, 11, 22);
        writer.write_all(&data).expect("Can't write data");
    }
    println!("Size {} -> {}", data.len(), result.len())
}