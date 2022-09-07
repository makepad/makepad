use std::env;
use std::io::{ Write};
use std::fs;    
use deflate::deflate_bytes;

fn main() {
    let args:Vec<String> = env::args().collect();
    let file_path = &args[1];

    let data = fs::read(file_path).expect("Can't read file"); 
    // lets brotli it
    let compressed = deflate_bytes(&data);
    println!("Deflate compressed size {}kb", compressed.len());
    
    for i in 1..12{
        println!("Compressing {} level {} ...", file_path, i);
        let mut result = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut result, 4096 /* buffer size */, i, 22);
            writer.write_all(&data).expect("Can't write data");
        };
        println!("Brotli {} compressed size {}b", i, result.len());
    }
}

