use makepad_live_parser::*;
use std::any::Any;
use std::collections::HashMap;

#[derive(Default)]
pub struct LiveFactoriesTest {
    pub registry: LiveRegistry,
    pub factories: HashMap<(CrateModule, Id), Box<dyn LiveFactoryTest >>,
}

pub trait LiveFactoryTest {
    fn de_live_any(&self, lr: &LiveRegistry, file: usize, level: usize, start: usize) -> Result<Box<dyn Any>,
    DeLiveErr>;
}

impl LiveFactoriesTest {
    pub fn register_component(&mut self, crate_id: Id, module_id: Id, struct_id: Id, factory: Box<dyn LiveFactoryTest>) {
        self.factories.insert((CrateModule(crate_id, module_id), struct_id), factory);
    }
    pub fn create_component(&self, crate_id: Id, module_id: Id, ids: &[Id]) -> Option<Box<dyn Any >> {
        if let Some((crate_module, id, full_ptr)) = self.registry.find_component_origin(crate_id, module_id, ids) {
            if let Some(factory) = self.factories.get(&(crate_module, id)) {
                match factory.de_live_any(&self.registry, full_ptr.file_id.to_index(), full_ptr.local_ptr.level, full_ptr.local_ptr.index) {
                    Ok(result) => {
                        return Some(result)
                    }
                    Err(msg) => {
                        println!("Error {:?}", msg)
                    }
                }
            }
        }
        return None
    }
}

#[test]
fn expand() {
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
            pf: EA,
            fn f1(a1) {let x = 1}
            r1: [1, 2, 3]
            o1: {x: 1, 1.0: 2}
            C2: Component {
                vdef1 bla::bla::bla
                vdef2 pa: float;
                pa: 1.0,
                a1: r1 {},
                b1: SA::p1 {},
                c1: Component {x1: 6, x4: SA {}}
            }
        }
    "#;
    let file_1_check = r#"
        SA: Struct {p1: 5.0}
        EA: Enum {}
        CA: Component {
            pa: 1.0
            pb: true
            pc: 2
            pd: #0000ffff
            pe: id1
            pf: NodePtr {file: 1, level: 0, index: 1}
            fn f1(a1) {let x = 1} "SA:[local], EA:[local], pa:[local], pb:[local], pc:[local], pd:[local], pe:[local], pf:[local]"
            r1: [1, 2, 3]
            o1: {x: 1, 1.0: 2}
            C2: Component {
                vdef1:bla ::bla ::bla
                vdef2 pa: float "SA:[local], EA:[local], pa:[local], pb:[local], pc:[local], pd:[local], pe:[local], pf:[local], f1:[local], r1:[local], o1:[local], vdef1:[local]"
                pa: 1.0
                a1: [1, 2, 3]
                b1: 5.0
                c1: Component {
                    x1: 6
                    x4: NodePtr {file: 1, level: 0, index: 0} {p1: 5.0}
                }
            }
        }
        
        
    "#;
    
    let file_2 = r#"
        use crate::file1::SA;
        use crate::file1::CA::C2;
        use crate::file1::CA;
        CB: C2 {
            pa: 2.0,
            tst: CA::f1 {},
            b1: 6.0;
            c1.x1: 7,
            c1.x2: "hi",
            c1.x3: [1, 2, 3],
            c1.x4.p1: {3.0: h1}
        }
        CD: SA::B {prop: 1}; // error
        CC: CB {c1.x4.p1: "ho"}
    "#;
    
    let file_2_check = r#"
        CB: NodePtr {file: 1, level: 1, index: 10} {
            vdef1:bla ::bla ::bla
            vdef2 pa: float "SA:[F:1 L:0 I:0], EA:[F:1 L:0 I:1], pa:[F:1 L:1 I:1], pb:[F:1 L:1 I:2], pc:[F:1 L:1 I:3], pd:[F:1 L:1 I:4], pe:[F:1 L:1 I:5], pf:[F:1 L:1 I:6], f1:[F:1 L:1 I:7], r1:[F:1 L:1 I:8], o1:[F:1 L:1 I:9], vdef1:[F:1 L:2 I:7]"
            pa: 2.0
            a1: [1, 2, 3]
            b1: 6.0
            c1: Component {
                x1: 7
                x4: NodePtr {file: 1, level: 0, index: 0} {p1: 5.0}
                x2: "hi"
                x3: [1, 2, 3]
            }
            fn tst(a1) {let x = 1} "SA:[F:1 L:0 I:0], EA:[F:1 L:0 I:1], pa:[F:1 L:1 I:1], pb:[F:1 L:1 I:2], pc:[F:1 L:1 I:3], pd:[F:1 L:1 I:4], pe:[F:1 L:1 I:5], pf:[F:1 L:1 I:6]"
        }
        CC: NodePtr {file: 2, level: 0, index: 0} {
            vdef1:bla ::bla ::bla 
            vdef2 pa: float "SA:[F:1 L:0 I:0], EA:[F:1 L:0 I:1], pa:[F:1 L:1 I:1], pb:[F:1 L:1 I:2], pc:[F:1 L:1 I:3], pd:[F:1 L:1 I:4], pe:[F:1 L:1 I:5], pf:[F:1 L:1 I:6], f1:[F:1 L:1 I:7], r1:[F:1 L:1 I:8], o1:[F:1 L:1 I:9], vdef1:[F:1 L:2 I:7]"
            pa: 2.0
            a1: [1, 2, 3]
            b1: 6.0
            c1: Component {
                x1: 7
                x4: NodePtr {file: 1, level: 0, index: 0} {p1: 5.0}
                x2: "hi"
                x3: [1, 2, 3]
            }
            fn tst(a1) {let x = 1} "SA:[F:1 L:0 I:0], EA:[F:1 L:0 I:1], pa:[F:1 L:1 I:1], pb:[F:1 L:1 I:2], pc:[F:1 L:1 I:3], pd:[F:1 L:1 I:4], pe:[F:1 L:1 I:5], pf:[F:1 L:1 I:6]"
        }
        
    "#;
    
    let file_3 = r#"
        use crate::file1::SA;
        use crate::file2::CC;
        CE: CC {t: SA {}}
        CF: ERR {}
    "#;
    
    let file_3_check = r#"
        CE: NodePtr {file: 2, level: 0, index: 1} {
            vdef1:bla ::bla ::bla 
            vdef2 pa: float "SA:[F:1 L:0 I:0], EA:[F:1 L:0 I:1], pa:[F:1 L:1 I:1], pb:[F:1 L:1 I:2], pc:[F:1 L:1 I:3], pd:[F:1 L:1 I:4], pe:[F:1 L:1 I:5], pf:[F:1 L:1 I:6], f1:[F:1 L:1 I:7], r1:[F:1 L:1 I:8], o1:[F:1 L:1 I:9], vdef1:[F:1 L:2 I:7]"
            pa: 2.0
            a1: [1, 2, 3]
            b1: 6.0
            c1: Component {
                x1: 7
                x4: NodePtr {file: 1, level: 0, index: 0} {p1: 5.0}
                x2: "hi"
                x3: [1, 2, 3]
            }
            fn tst(a1) {let x = 1} "SA:[F:1 L:0 I:0], EA:[F:1 L:0 I:1], pa:[F:1 L:1 I:1], pb:[F:1 L:1 I:2], pc:[F:1 L:1 I:3], pd:[F:1 L:1 I:4], pe:[F:1 L:1 I:5], pf:[F:1 L:1 I:6]"
            t: NodePtr {file: 1, level: 0, index: 0} {p1: 5.0}
        }
    "#;
    
    let error_check = r#"
        file1: 9 16 - Cannot find item on scope: id1 - origin: render/live_parser/src/liveregistry.rs:652 
        file1: 13 17 - Cannot find item on scope: x - origin: render/live_parser/src/liveregistry.rs:652 
        file2: 12 28 - Cannot find item on scope: h1 - origin: render/live_parser/src/liveregistry.rs:652 
        file2: 12 22 - Cannot inherit with different node type c1.x4.p1 - origin: render/live_parser/src/livedocument.rs:232 
        file2: 14 12 - Cannot find class SA.B - origin: render/live_parser/src/liveregistry.rs:612 
        file2: 15 26 - Cannot inherit with different node type c1.x4.p1 - origin: render/live_parser/src/livedocument.rs:232 
        file3: 5 12 - Cannot find item on scope: ERR - origin: render/live_parser/src/liveregistry.rs:652 
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
        println!("{}", b);
        a.retain( | c | c != ' ' && c != '\n');
        b.retain( | c | c != ' ' && c != '\n');
        
        return a == b
    }
    
    for (index, file) in lr.expanded.iter().enumerate() {
        let crate_module = lr.find_crate_module_by_file_id(FileId::index(index)).unwrap();
        let out = format!("{}", file);
        for (name_id, _, check) in &sources {
            if crate_module.1 == *name_id {
                if !compare_no_ws(&out, check) {
                    println!("Output Unequal {}\n{}", crate_module, out);
                    assert_eq!(true, false);
                }
            }
        }
    }
    
    let mut err_cmp = String::new();
    for msg in errors {
        err_cmp.push_str(&format!("{}\n", msg.to_live_file_error(
            &format!("{}", sources[msg.span.file_id().to_index()].0),
            sources[msg.span.file_id().to_index()].1
        )));
    }
    
    if !compare_no_ws(&err_cmp, error_check) {
        println!("Errors Unequal\n{}", err_cmp);
        assert_eq!(true, false);
    }
    //        assert_eq!(true, false);
    
    // deserializer test
    
    #[derive(Debug, PartialEq, Eq, DeLive)]
    struct MyComponent {
        x: u32,
        y: u32,
        z: u32,
        e1: MyEnum,
        e2: MyEnum,
        e3: MyEnum,
    }
    
    #[derive(Debug, DeLive)]
    struct MyVec4(f32);
    
    #[derive(Debug, PartialEq, Eq, DeLive)]
    enum MyEnum {
        Value1,
        Value2(u32),
        Value3 {value: u32}
    }
    
    struct MyComponentFactory {}
    impl LiveFactoryTest for MyComponentFactory {
        fn de_live_any(&self, lr: &LiveRegistry, f: usize, l: usize, s: usize) -> Result<Box<dyn Any>,
        DeLiveErr> {
            let mv = MyComponent::de_live(lr, f, l, s) ?;
            Ok(Box::new(mv))
        }
    }
    
    let mut lr = LiveFactoriesTest::default();
    let source = r#"
        MyEnum: Enum {
            Value1: Variant
            Value2: Variant()
            Value3: Variant {}
        }
        
        MyComponent: Component {
            e1: MyEnum::Value1
            e2: MyEnum::Value2(2)
            e3: MyEnum::Value3 {value: 1}
        }
        MyDerive2: MyComponent {x: 1, y: 2, z: 5}
    "#;
    match lr.registry.parse_live_file("test.live", id_check!(main), id_check!(test), source.to_string()) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    
    let mut errors = Vec::new();
    lr.registry.expand_all_documents(&mut errors);
    
    if errors.len() != 0 {
        for msg in errors {
            println!("{}\n", msg.to_live_file_error("", source));
        }
        assert_eq!(true, false);
    }
    
    lr.register_component(id!(main), id!(test), id!(MyComponent), Box::new(MyComponentFactory {}));
    let val = lr.create_component(id!(main), id!(test), &[id!(MyDerive2)]);
    
    match val.unwrap().downcast_ref::<MyComponent>() {
        Some(comp) => {
            let check = MyComponent {
                x: 1,
                y: 2,
                z: 5,
                e1: MyEnum::Value1,
                e2: MyEnum::Value2(2),
                e3: MyEnum::Value3 {value: 1}
            };
            
            assert_eq!(*comp, check);
            println!("{:?}", comp);
        }
        None => {
            assert_eq!(true, false);
            println!("No Value");
        }
    }
}