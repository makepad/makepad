pub use makepad_id;
pub use makepad_id::makepad_id_derive;
pub mod tokenizer; 
pub mod object;
pub mod colorhex;
pub mod parser;
pub mod heap;
pub mod string;
pub mod methods;
pub mod modules;
#[macro_use]
pub mod native;
pub mod vm;
pub mod thread;
pub mod opcode;
pub mod value;
pub mod opcodes;
pub mod gc;
pub mod value_map;
pub use makepad_id_derive::*;
pub use makepad_id::id::*;
pub use value::*;
pub use vm::ScriptVm;
pub use makepad_script_derive::*;
pub use vm::Script;
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
    
    // Our unit tests :)
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
        let t = 0 t = 2 t += 1 assert(t==3)
        let x = {f:2} x.f+=2 assert(x.f == 4)
        let x = [1,2] x[1]+=2 assert(x == [1 4])
        let c = 0 for x in 4{ if c == 3 break; c += 1} assert(c==3)
        let c = 0 for x in 5{ if c == 4{break;}c += 1} assert(c==4);
        let c = 0 for x in 7{ if x == 3 ||  x == 5 continue;c += 1} assert(c==5);
        let c = 0 loop{ c+=1; if c>5 break} assert(c==6)
        let c = 0 while c < 9 c+=1 assert(c==9);
        let c = 0 while c < 3{c+=1}assert(c==3);
        // access rights
        
    };
    
    let _code = script!{
        scope.import(mod.std)
        let iv = [1 2 3 4] let ov = []
        for v in iv ov.push(v) assert(iv == ov)
        ;
    };
    
    let _code = script!{
        scope.import(mod.std)
        let a = [1,2,3];
        a.retain(|v| v!=2);
        ~a;
        a.retain(|v|{~v;v>=3}) assert(a==[3 4]);
    };
    
    let _code = script!{
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(38);
    };
    
    let dt = std::time::Instant::now();
    
    vm.eval(code, &mut 0);
    println!("Duration {}", dt.elapsed().as_secs_f64())
    
}
