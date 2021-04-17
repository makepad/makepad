//#![allow(unused_variables)]
//#![allow(unused_imports)]

use std::path::Path;
use std::fs::File;
use std::io::prelude::*;
use makepad_live_parser::*;
use makepad_live_parser::id::*;
use makepad_live_parser::liveregistry::LiveRegistry;

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
        _ => ()
    };
    
    let file_1 = r#"
        ClassA: Component {
            prop_a: 1
        }
    "#;
    
    let file_2 = r#"
        use crate::file1::*;
        ClassB: ClassA{
            prop_b: 2
        }
    "#;
    
    // okaaay now we can actually start processing this thing.
    let mut lr = LiveRegistry::default();
    match lr.parse_live_file("file1.live", id_check!(main), id_check!(file1), file_1.to_string()) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    match lr.parse_live_file("file2.live", id_check!(main), id_check!(file2), file_2.to_string()) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    
    let mut errors = Vec::new();
    lr.expand_all_documents(&mut errors);
    
    //let ld2 = lr.expand_document(&ld, &mut errors);
    for msg in errors{
        println!("Expand error {}", msg.to_live_file_error("", &source));
    }
    //println!("{}", ld);
    //println!("-----\n{}", ld2);
}



