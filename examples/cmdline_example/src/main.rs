#![allow(unused_variables)]
#![allow(unused_imports)]

use std::path::Path;
use std::fs::File;
use std::io::prelude::*;
use makepad_live_parser::span::LiveFileId;
use makepad_live_parser::lex::lex;
use makepad_live_parser::token::TokenWithSpan;
use makepad_live_parser::liveparser::LiveParser;

use makepad_live_parser::livenode::*;

fn main() {
    // rust crate directory
    // lets concatenate paths
    let crate_path = Path::new(env!("CARGO_MANIFEST_DIR"));
    let file_path = crate_path.join("live").join("test.live");
    let display = file_path.display();
    
    let mut file = match File::open(&file_path) {
        Err(why) => panic!("couldn't open {}: {}", display, why),
        Ok(file) => file,
    };

    // Read the file contents into a string, returns `io::Result<usize>`
    let mut source = String::new();
    match file.read_to_string(&mut source) {
        Err(why) => panic!("couldn't read {}: {}", display, why),
        _=>()
    };

    let lex_result = match lex(source.chars(), LiveFileId(0)){
        Err(msg)=>panic!("Lex error {}", msg),
        Ok(lex_result)=>lex_result
    };
    
    let mut parser = LiveParser::new(&lex_result);
     
    // lets go parse this thing.
    println!("Hello {}",std::mem::size_of::<LiveNode>());
    
    // OK GREAT! We have tokens. Now
    // lets parse this DOM!
    let ld = match parser.parse_live_document(){
        Err(msg)=>panic!("Parse error {}", msg.to_live_file_error("", &source)),
        Ok(ld)=>ld
    }; 
    
    println!("{}", ld);
    
}



