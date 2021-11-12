use makepad_live_compiler::*;
//use std::any::Any;
use std::collections::HashMap;
/*

#[derive(Clone, Debug)]
pub struct LiveNode2 { // 3x u64
    pub token_id: TokenId,
    pub id: Id,
    pub value: LiveValue2,
}

#[derive(Clone, Debug)]
pub enum LiveValue2 {
    Str(&'static str),
    String(String),
    StringRef {
        string_start: usize,
        string_count: usize
    },
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    Vec2(Vec2),
    Vec3(Vec3),
    LiveType(LiveType),
    // ok so since these things are 
    EnumBare {base: Id, variant: Id},
    // stack items
    Array,
    EnumTuple {base: Id, variant: Id},
    EnumNamed {base: Id, variant: Id},
    ClassBare, // subnodes including this one
    ClassNamed {class: Id}, // subnodes including this one
    Close,
    // the shader code types
    Fn {
        token_start: usize,
        token_count: usize,
        scope_start: usize, 
        scope_count: u32
    },
    Const {
        token_start: usize,
        token_count: usize,
        scope_start: usize,
        scope_count: u32
    },
    VarDef { //instance/uniform def
        token_start: usize,
        token_count: usize,
        scope_start: usize,
        scope_count: u32
    },
    Use{
        crate_id:Id,
        module_id:Id,
    }
}
*/

fn main() {
    let source = r#" 
        Test:Component{
            x:MyEnum::Fn(1.0)
        }
    "#;
    
    let mut lr = LiveRegistry::default();
    
    match lr.parse_live_file(&format!("test.live"), ModulePath::from_str("test.live").unwrap(), source.to_string(), vec![], 0) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }

    //println!("{:?}", lr.live_files[0].document);

    
    let mut errors = Vec::new();
    lr.expand_all_documents(&mut errors);

    
    if errors.len() != 0 {
        for msg in errors {
            println!("{}\n", msg.to_live_file_error("", source, 0));
        }
        assert_eq!(true, false);
    }
    println!("{:?}",lr.expanded[0]);
    
}


fn _compare_no_ws(a_in: &str, b_in: &str) -> Option<String> {
    let mut b_str = b_in.to_string();
    b_str.retain( | c | !c.is_whitespace());
    let mut a_str = a_in.to_string();
    a_str.retain( | c | !c.is_whitespace());
    
    let b = b_str.as_bytes();
    let a = a_str.as_bytes();
    
    let mut start = 0;
    let mut changed = false;
    let len = b.len().min(a.len());
    for i in 0..len {
        if a[i] != b[i] {
            changed = true;
            break
        }
        start = i;
    }
    // now go from the back to i
    let mut end = 0;
    for i in 2..len {
        end = i - 2;
        if a[a.len() - i] != b[b.len() - i] {
            changed = true;
            break
        }
    }
    // okaay so we have to show the changed thing
    if changed {
        let range_a = if start < (a.len() - end - 1) {std::str::from_utf8(&a[start..(a.len() - end - 1)]).unwrap()} else {""};
        let range_b = if start < (b.len() - end - 1) {std::str::from_utf8(&b[start..(b.len() - end - 1)]).unwrap()} else {""};
        Some(format!(
            "########## NEW ########## {} to {}\n{}\n########## OLD ########## {} to {}\n{}\n########## END ##########\n\n########## NEW ALL ##########\n{}\n########## OLD ALL ##########\n{}",
            start,
            (a.len() - end - 1),
            range_a,
            start,
            (b.len() - end - 1),
            range_b,
            a_in,
            b_in,
        ))
    }
    else {
        None
    }
}