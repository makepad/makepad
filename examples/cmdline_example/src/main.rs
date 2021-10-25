use makepad_live_parser::*;
//use std::any::Any;
//use std::collections::HashMap;

fn main() {
    // all in one test
    let source = r#"
        CA: Component {
            instance color:vec4 = #fff;
        }
        CB: CA{
            color: #f0f;
        }
        CC: CB{
            color: #00f;
        }
    "#;
    
    let mut lr = LiveRegistry::default();
    match lr.parse_live_file(&format!("{}.live", id!(main)), ModulePath::from_str("main").unwrap(), source.to_string(), vec![]) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    
    let mut errors = Vec::new();
    lr.expand_all_documents(&mut errors);
    
    for (_, file) in lr.expanded.iter().enumerate() {
        println!("{}", file)
    }
    
    for msg in errors {
        println!("{}\n", msg.to_live_file_error(
            "main",
            source
        ));
    }
    
}