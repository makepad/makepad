use std::{
    fs,
    env
};
mod wasm_strip;
use wasm_strip::*;
    
pub fn main(){
    let args:Vec<String> = env::args().collect();
    let file_path = &args[1];
    if let Ok(data) = fs::read(file_path) {
        if let Ok(strip) = wasm_strip_debug(&data) {
            
            let uncomp_len = strip.len();
            //let mut enc = snap::Encoder::new();
            /*
            let mut result = Vec::new();
            {
                let mut writer = brotli::CompressorWriter::new(&mut result, 4096 /* buffer size */, 11, 22);
                writer.write_all(&strip).expect("Can't write data");
            }*/
            
            //let comp_len = if let Ok(compressed) = enc.compress_vec(&strip) {compressed.len()}else {0};
            
            if let Err(_) = fs::write(&file_path, strip) {
                eprintln!("Cannot write stripped wasm {}", file_path);
            }
            else {
                println!("Wasm file stripped size: {} kb", uncomp_len >> 10);
            }
        }
        else {
            eprintln!("Cannot parse wasm {}", file_path);
        }
    }
    else{
        eprintln!("Cannot read wasm file {}", file_path);
    }
}

