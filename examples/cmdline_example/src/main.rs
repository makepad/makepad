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
    let file_1_check = r#"
        SA: Struct {p1: 5.0}
        EA: Enum {}
        CA: Component {
            pa: 1.0
            pb: true
            pc: 2
            pd: 65535
            pe: id1
            fn f1(a1) {let x = 1} "SA:[local], EA:[local], pa:[local], pb:[local], pc:[local], pd:[local], pe:[local]"
            r1: [1, 2, 3]
            o1: {x: 1, 1.0: 2}
            C2: Component {
                a1: [1, 2, 3]
                b1: 5.0
                c1: Component {
                    x1: 6
                    x4: Struct {p1: 5.0}
                }
            }
        }
        
    "#;
    
    let file_2 = r#"
        use crate::file1::SA;
        use crate::file1::CA::C2;
        use crate::file1::CA;
        CB: C2 {tst: CA::f1 {}, b1: 6.0; c1.x1: 7, c1.x2: "hi", c1.x3: [1, 2, 3], c1.x4.p1: {3.0: h1}}
        CD: SA::B {prop: 1}; // error
        CC: CB {c1.x4.p1: "ho"}
    "#;
    
    let file_2_check = r#"
        CB: Component {
            a1: [1, 2, 3]
            b1: 6.0
            c1: Component {
                x1: 7
                x4: Struct {
                    p1: {3.0: h1}
                }
                x2: "hi"
                x3: [1, 2, 3]
            }
            fn tst(a1) {let x = 1} "SA:main::file1, EA:main::file1, pa:main::file1, pb:main::file1, pc:main::file1, pd:main::file1, pe:main::file1"
        }
        CC: Component {
            a1: [1, 2, 3]
            b1: 6.0
            c1: Component {
                x1: 7
                x4: Struct {
                    p1: "ho"
                }
                x2: "hi"
                x3: [1, 2, 3]
            }
        }
        
    "#;
    
    let file_3 = r#"
        use crate::file1::SA;
        use crate::file2::CC;
        CE: CC {t: SA {}}
        CF: ERR {}
    "#;
    
    let file_3_check = r#"
        CE: Component {
            a1: [1, 2, 3]
            b1: 6.0
            c1: Component {
                x1: 7
                x4: Struct {
                    p1: "hi"
                }
                x2: "ho"
                x3: [1, 2, 3]
            }
            t: Struct {p1: 5.0}
        }
    "#;
    
    let error_check = r#"
        file2: 6 12 - Cannot find class SA.B
        file2: 6 12 - Cannot override items in non - class: SA::B
        file3: 5 12 - Cannot find item on scope: ERR
    "#;
    
    let sources = [(id_check!(file3), file_3, file_3_check), (id_check!(file1), file_1, file_1_check), (id_check!(file2), file_2, file_2_check)];
    
    let mut lr = LiveRegistry::default();
    
    for (name_id, source, _) in &sources {
        match lr.parse_live_file(&format!("{}.live", name_id), id_check!(main), *name_id, source.to_string()) {
            Err(why) => panic!("Couldnt parse file {}", why),
            _ => ()
        }
    }
    
    let mut errors = Vec::new();
    lr.expand_all_documents(&mut errors);
    
    fn compare_no_ws(a: &str, b: &str) -> bool {
        let mut b = b.to_string();
        let mut a = a.to_string();
        a.retain( | c | c != ' ' && c != '\n');
        b.retain( | c | c != ' ' && c != '\n');
        return a == b
    }
    
    for (crate_module, file) in lr.expanded {
        let out = format!("{}", file);
        for (name_id, _, check) in &sources {
            if crate_module.1 == *name_id {
                if !compare_no_ws(&out, check) {
                    println!("Unequal {}\n{}", crate_module, out)
                }
            }
        }
    }
    
    let mut err_cmp = String::new();
    for msg in errors {
        err_cmp.push_str(&format!("{}\n", msg.to_live_file_error(
            &format!("{}", sources[msg.span.live_file_id().0 as usize].0),
            sources[msg.span.live_file_id().0 as usize].1
        )));
    }
    
    if !compare_no_ws(&err_cmp, error_check) {
        println!("ERROR UNEQUAL\n{}", err_cmp)
    }
}



