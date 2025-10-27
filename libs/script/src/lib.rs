pub use makepad_live_id;
pub use makepad_live_id::makepad_live_id_macros;
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
pub mod traits;
pub mod prims;
pub mod array;
pub mod trap;

pub use makepad_live_id::*;
pub use value::*;
pub use vm::*;
pub use makepad_script_derive::*;
pub use traits::*;
pub use thread::*;
pub use heap::*;

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
            // we have an ScriptObject. can we read a value
            object.get(id!(myfield))
            object.set(id!(my_field), 2.0.into())
        }
    }
}*/


//#[derive(Script)]
pub enum EnumTest{
    Bare,
    Tuple(u32),
    Named{named:u32}
}


pub fn test(){
    let mut vmbase = ScriptVmBase::new();
    
    let mut vm = vmbase.as_ref();
    
    let net = vm.new_module(id!(test));
    vm.add_fn(net, id!(fetch), script_args!(url=NIL, options=NIL), |vm, args|{
        // how do we construct our options
        let _options = StructTest::script_from_value(vm, script_value!(vm, args.options));
        NIL
    });
    
    #[derive(Script)]
    pub struct StructTest{
       #[live(1.0)] field:f64,
       #[live(EnumTest::Bare)] enm:EnumTest,
       #[live] opt: Option<f64>,
       #[live] vec: Vec<f64>
    }
    
    #[derive(Script, ScriptHook)]
    pub enum EnumTest{
        #[pick]
        Bare,
        #[live(1.0)] 
        Tuple(f64),
        #[live{named_field:1.0}] 
        Named{named_field:f64}
    }
    
    use crate::vm::*;
    use crate::value::*;
    
    impl ScriptHook for StructTest{
        fn on_proto_methods(vm:&mut ScriptVm, obj:ScriptObject){
            vm.add_fn(obj, id_lut!(return_two), script_args_lut!(o = 1.0), |_vm, _args|{
                return 2.into()
            });
        }
    }    

    // Our unit tests :)
    let _code = script!{
        let t = mod.std;
        scope.import(mod.std)
        
        // array operations
        let x = 1+2 assert(x == 3)
        let iv = [1 2 3 4] let ov = []
        for v in iv ov.push(v) assert(iv == ov)
        ov.pop() assert(iv != ov)
        assert(ov[2] == 3);
        
        // functions
        let f = |x| x+1
        assert( f(1) == 2)
        
        // shallow and deep compare
        let oa = {y:1 z:2}
        let ob = {z:3 y:1}
        assert(oa != ob)
        ob.z = 2 assert(oa == ob)
        assert(oa !== ob)
        
        // string comparison
        assert("123" == "123")
        assert("123" != "223")
        assert("123456" == "123456")
        assert("123456" != "123")
        
        // test arrays        
        let x = 1 x += 2 assert(x == 3)
        let t = 3 t ?= 2 assert(t == 3)
        let t t ?= 2 assert(t == 2)
        let t = 0 t = 2 t += 1 assert(t==3)
        let x = {f:2} x.f+=2 assert(x.f == 4)
        let x = [1,2] x[1]+=2 assert(x == [1 4])
        
        // test loops
        let c = 0 for x in 4{ if c == 3 break; c += 1} assert(c==3)
        let c = 0 for x in 5{ if c == 4{break;}c += 1} assert(c==4);
        let c = 0 for x in 7{ if x == 3 ||  x == 5 continue;c += 1} assert(c==5);
        let c = 0 loop{ c+=1; if c>5 break} assert(c==6)
        let c = 0 while c < 9 c+=1 assert(c==9);
        let c = 0 while c < 3{c+=1}assert(c==3);
        
        // freezing
        let x = {x:1 y:2}.freeze_api();
        // property value unknown
        try {x{z:3}} assert(true) ok assert(false)
        // property value known
        let x2 = x{x:3} assert(x2.x == 3)
        let x2 = x{x:2}
        // property frozen
        try x.x = 2 assert(true) ok assert(false)
                
        // modules can be extended but not overwritten
        let x = {p:1}.freeze_module();
        try x.p = 2 assert(true) ok assert(false)
        try x.z = 2 assert(false) ok assert(true)
        // but we cant add items to its vec
        try {x{1}} assert(true) ok assert(false)
        
        let x = {p:1}.freeze_component();
        // cant write to it at all
        try x.x = 1 assert(true) ok assert(false)
        try x.p = 1 assert(true) ok assert(false)
        // can write with same type on derived        
        try {x{p:1}} assert(false) ok assert(true)
        // cant change value type   
        try {x{p:true}} assert(true) ok assert(false)
        // can append to vec  
        try {x{1}} assert(false) ok assert(true)
        
        // scope shadowing
        let x = 1
        let f = || x
        let x = 2
        let g =|| x
        assert(f() == 1)
        assert(g() == 2)
        
        // try undefined
        try{undef = 1} assert(true) ok assert(false)
        let t = 0 try{t = 1} assert(false) ok assert(true)
        
        // struct tests
        let s = #(StructTest::script_api(&mut vm));
        try{s{field:5}} assert(false) ok assert(true)
        try{s{field:true}} assert(true) ok assert(false)
        assert(s.return_two() == 2)
        
        // check enum
        let EnumTest = #(EnumTest::script_api(&mut vm));
        let x = EnumTest.Bare
        // test tuple typechecking
        try{EnumTest.Tuple(1.0)} assert(false) ok assert(true)
        try{EnumTest.Tuple(false)} assert(true) ok assert(false)
        try{EnumTest.Tuple()} assert(true) ok assert(false)
        try{EnumTest.Tuple(1,2)} assert(true) ok assert(false)
        try{EnumTest.Named{named_field:1.0}} assert(false) ok assert(true)
        try{EnumTest.Named{named_field:true}} assert(true) ok assert(false)
        
        assert(s.enm == EnumTest.Bare)
        try{s{enm: EnumTest.Bare}} assert(false) ok assert(true)
        try{s{enm: 1.0}} assert(true) ok assert(false)
        try{s{enm: EnumTest.Named{named_field:1.0}}} assert(false) ok assert(true)
        try{s{enm: EnumTest.Tuple(1.0)}} assert(false) ok assert(true)
        ~EnumTest
        
        // check the option
        try{s{opt:nil}} assert(false) ok assert(true)
        try{s{opt:1.0}} assert(false) ok assert(true)
        try{s{opt:false}} assert(true) ok assert(false)
        
        // check the vec
        let x = s{vec:[1,2,3,4]}
        assert(x.vec == [1,2,3,4])
        // check typechecking in a vec
        try{s{vec:[false]}} assert(true) ok assert(false)
        try{s{vec:[1,2]}} assert(false) ok assert(true)
    };
    
    let code = script!{
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(38);
    };
    
    let dt = std::time::Instant::now();
    
    vmbase.eval(code, &mut 0);
    println!("Duration {}", dt.elapsed().as_secs_f64())
    
}
