pub use makepad_value;
pub use makepad_value::makepad_value_derive;
pub mod tokenizer; 
pub mod object;
pub mod colorhex;
pub mod parser;
pub mod heap;
pub mod string;
pub mod methods;
pub mod modules;
pub mod native;
pub mod script;
pub mod thread;
pub mod thread_opcode;

// can we refcount object roots on the heap?
// yea why not 
// we can make a super convenient ObjectRef type you can use to hold onto script objects
/*
pub trait ScriptLife{
    fn on_create(){
    }
    pub fn handle(&mut self, ){
        // now i wanna fire the onclick event somehow. how do we do that
        // we could simply have an rust_ref u64 -> object id map 
        cx.vm.call(self.get_refid(), id!(on_click))
        
        if let Some(object) = vm_call!(cx.vm, self, on_draw(item)){
            vm_get!(cx.vm, object, myfield)
        }
        
        if let Some(object) = cx.vm.call(self.rsid(), id!(on_draw), &[item.into()]).as_object(){
            // we have an ObjectPtr. can we read a value
            object.get(id!(myfield))
            object.set(id!(my_field), 2.0.into())
        }
    }
}*/


//#[derive(Scriptable)]
pub enum EnumTest{
    Bare,
    Tuple(u32),
    Fields{field:u32}
}

//#[derive(Scriptable)]
pub struct RustTest{
    _enm1: EnumTest,
    _enm2: EnumTest,
   _enm3: EnumTest,
    _prop: f64    
}

impl RustTest{
    fn ty()->u32{1}
}

use crate::script::*;
use makepad_script_derive::*;

pub fn test(){
    let mut vm = ScriptVm::new();
    
    //#[derive(Scriptable)]
    pub enum _EnumTest{
        Bare,
        Tuple(u32),
        Fields{field:u32}
    }
    
    let _code = script!{
        //let EnumTest = #(EnumTest::def(vm.ctx()));
        scope.import(EnumTest);
        
        let MyView = #(RustTest::ty()){
            enm1: Bare,
            enm2: Tuple(2),
            enm3: Fields{field: 1.0}
        }
    };

    let _code = script!{
        let x = Button{
            draw_bg:{
                pixel: ||{
                    let x = 1
                    return t(x)
                }
            }
        }
        
        let x = [@view,@bla]
        for sym in x t[sym]
        
        let View = {@view}
        let Window = {@window}
        let Button = {@button}
        let MyWindow = #(RustTest::ty()){
            size: 1.0
            $b1: Button{}
            $body: View{}
            $b2: Button{}
        }
        let x = MyWindow{
            $b1 : Checkbox{}
        }
        let x = if true 1 else 0
        let x = x{};
        for v in [1 2 3 4] ~v
        ~x;
    };
    // basic test script
    let code = script!{
        scope.import(mod.std)
        
        let x = 1+2 assert(x == 3)
        let iv = [1 2 3 4] let ov = []
        for v in iv ov.push(v) assert(iv == ov)
        ov.pop() assert(iv != ov)
        
        let oa = {y:1 z:2}
        let ob = {z:3 y:1}
        assert(oa != ob)
        ob.z = 2 assert(oa == ob)
        assert(oa !== ob)
        
        assert("123" == "123")
        assert("123" != "223")
        assert("123456" == "123456")
        assert("123456" != "123")
        
        let x = 1 x += 2 assert(x == 3)
        let t = 3 t ?= 2 assert(t == 3)
        let t t ?= 2 assert(t == 2)
        ;
    };
    
    let _code = script!{
    };
    
    let _code = script!{
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(38);
    };
    
    let dt = std::time::Instant::now();
    
    vm.eval(code);
    println!("Duration {}", dt.elapsed().as_secs_f64())
    
}
