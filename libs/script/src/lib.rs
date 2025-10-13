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
        
        if let Some(object) = vm_call!(cx, self, on_draw(item)){
            object.get(id!(myfield));
            object.set(id!(my_field), 2.0.into());
        }
        
        if let Some(object) = cx.vm.call(self.into(), id!(on_draw), &[item.into()]).as_object(){
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
    enm1: EnumTest,
    enm2: EnumTest,
    enm3: EnumTest,
    _prop: f64    
}

impl RustTest{
    fn ty()->u32{1}
}

use crate::script::*;
use makepad_script_derive::*;

pub fn test(){
    let code = script!{
        //let EnumTest = #(EnumTest::def());
        scope.import(EnumTest);
        
        let Bare = @Bare // just a bare escaped id value
        let Tuple = || x// builtin fn that constructs something
        let Fields = {} // object with keys w defaults
        
        let MyView = #(RustTest::ty()){
            enm1: Bare,
            enm2: Tuple(2),
            enm3: Fields{field: 1.0}
        }
    };

    let code = script!{
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
    
    let _code = script!{
        if true let x = 5;
        ~x;
    };
    
    let _code = script!{
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(38);
    };
    
    let dt = std::time::Instant::now();
    let mut vm = ScriptVm::new();
    vm.eval(code);
    println!("Duration {}", dt.elapsed().as_secs_f64())
    
}
