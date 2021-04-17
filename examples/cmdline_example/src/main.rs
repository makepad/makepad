//#![allow(unused_variables)]
//#![allow(unused_imports)]
use makepad_live_parser::*;
use makepad_live_parser::id::*;
use makepad_live_parser::liveregistry::LiveRegistry;

fn main() {
    // all in one test
    let file_1 = r#"
        SA: Struct {p1: 5.0}
        EA: Enum {}
        CA: Component {
            pa: 1.0
            pb: true
            pc: 2
            pd: #00f
            pe: id1,
            fn f1(a1) {let x = 1}
            r1: [1, 2, 3]
            o1: {x: 1, 1.0: 2}
            C2: Component {a1: r1 {}, b1: SA::p1 {}, c1: Component {x1: 6, x4: SA {}}}
        }
    "#;
    
    let file_2 = r#"
        use crate::file1::SA;
        use crate::file1::CA::C2;
        CB: C2 {b1: 6.0; c1.x1: 7, c1.x2: "hi", c1.x3: [1, 2, 3], c1.x4.p1: {3.0: h1}}
        CD: SA::B {prop: 1}; // error
        CC: CB {c1.x4.p1: "ho"}
    "#;
    
    let file_3 = r#"
        use crate::file1::SA;
        use crate::file2::CC;
        CE: CC {t: SA {}}
        CF: ERR{}
    "#;
    
    // okaaay now we can actually start processing this thing.
    let mut lr = LiveRegistry::default();
    match lr.parse_live_file("file3.live", id_check!(main), id_check!(file3), file_3.to_string()) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
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
    
    // now lets compare output and errors
    
    let mut out = String::new();
    out.push('\n');
    for file in lr.live_files {
        out.push_str(&format!("{}", file.document));
        out.push('\n');
    }
    let sources = [("file3", file_3), ("file1", file_1), ("file2", file_2)];
    for msg in errors {
        out.push_str(&format!("{}\n", msg.to_live_file_error(
            sources[msg.span.live_file_id().0 as usize].0,
            sources[msg.span.live_file_id().0 as usize].1
        )));
        //println!("Expand error {}", msg.to_live_file_error("", &source));
    }
    
    let compare = r#"
use SA::crate::file1
use CC::crate::file2
CE:CC {
    t:SA {}
}
CF:ERR {}
SA:Struct {p1:5.0}
EA:Enum {}
CA:Component {
    pa:1.0
    pb:true
    pc:2
    pd:65535
    pe:id1
    fn f1( a1 ) { let x = 1 } 
    r1:[1, 2, 3]
    o1:{x:1, 1.0:2}
    C2:Component {
        a1:r1 {}
        b1:SA::p1 {}
        c1:Component {
            x1:6
            x4:SA {}
        }
    }
}
use SA::crate::file1
use CA::C2::crate::file1
CB:C2 {
    b1:6.0
    c1.x1:7
    c1.x2:"hi"
    c1.x3:[1, 2, 3]
    c1.x4.p1:{3.0:h1}
}
CD:SA::B {prop:1}
CC:CB {
    c1.x4.p1:"ho"
}
file2: 5 12 - Cannot find class SA.B
file2: 5 12 - Cannot override items in non-class: SA::B
file3: 5 12 - Cannot find item on scope: ERR
"#;
    //println!("{}", out);
    if out != compare
    {
        println!("TEST FAIL");
    };
    
}



