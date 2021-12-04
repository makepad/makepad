use makepad_live_compiler::*;

fn main() {
    let source1 = r#" 
        MyThing:{x:1.0}
        
        Test:Component{
            v:MyThing{y:2.0}
        }

        Test2:Test{
            v:MyThing{y:3.0, x:4.0}
        }

        /*Test2:Test{
            x:{x:2}
            t:My::Prop{x:1.0}
            z:1.0
        }*/
    "#;
    let source2 = r#" 
        use test::source1::Test;
        Test3:Test{};
    "#;
    
    let mut lr = LiveRegistry::default();
    
    match lr.parse_live_file(&format!("test1.live"), LiveModuleId::from_str("test::source1").unwrap(), source1.to_string(), vec![], 0) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    match lr.parse_live_file(&format!("test2.live"), LiveModuleId::from_str("test::source2").unwrap(), source2.to_string(), vec![], 0) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    
    let mut errors = Vec::new();
    lr.expand_all_documents(&mut errors);
    
    if errors.len() != 0 {
        for msg in errors {
            println!("{}\n", msg.to_live_file_error("", source1, 0));
        }
        //assert_eq!(true, false);
    }
    println!("{}",lr.expanded[0].nodes.to_string(0,100));
    
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