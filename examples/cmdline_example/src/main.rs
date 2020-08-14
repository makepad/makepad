use std::io::{Read, Write};
use std::fs::File;
use image_formats::jpeg;
use image_formats::bmp;

fn load(name: &str) {
    println!("loading {}...",name);
    let mut infile = File::open(&name).unwrap();
    let mut buffer = Vec::new();
    infile.read_to_end(&mut buffer).unwrap();
    match jpeg::decode(&buffer) {
        Ok(image) => {
            let outname = (&name[0 .. name.len() - 4]).to_string() + ".bmp";
            match bmp::encode(&image) {
                Ok(value) => {
                    let mut outfile = File::create(&outname).unwrap();
                    outfile.write_all(&value).unwrap();
                },
                Err(msg) => {
                    println!("    Error: {}",msg);
                }
            };
        },
        Err(msg) => {
            println!("    Error: {}",msg);
        }
    }
}

fn main() {
   load("./examples/cmdline_example/test.jpg");
}
