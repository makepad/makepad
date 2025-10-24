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
pub mod script;
pub use makepad_id_derive::*;
pub use makepad_id::id::*;
pub use value::*;
pub use vm::*;
pub use makepad_script_derive::*;
pub use script::*;
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
            // we have an Object. can we read a value
            object.get(id!(myfield))
            object.set(id!(my_field), 2.0.into())
        }
    }
}*/


//#[derive(Script)]
pub enum EnumTest{
    Bare,
    Tuple(u32),
    Fields{field:u32}
}


pub fn test(){
    let mut vm = ScriptVm::new();
    
    let net = vm.new_module(id!(test));
    vm.add_fn(net, id!(fetch), args!(url=NIL, options=NIL), |vm, args|{
        // how do we construct our options
        let _options = StructTest::script_from_value(vm, value!(vm, args.options));
        NIL
    });
    
    //#[derive(Script)]
    #[allow(unused)]
    pub enum EnumTest{
      //  #[pick]
        Bare,
        Tuple(f64),
        Named{field:f64}
    }
    
    impl ScriptHook for EnumTest{
    }
    
    impl ScriptNew for EnumTest{
        fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
        fn script_new(_vm:&mut Vm)->Self{Self::Bare}
        fn script_default(vm:&mut Vm)->Value{
            let proto = Self::script_proto(vm);
            vm.heap.value(proto.into(), id!(Bare).into(), &vm.thread.trap)
        }
        
        // alright so. hows this work
        fn script_type_check(_heap:&ScriptHeap, value:Value)->bool{
            // if its an id its a bare one. so lets check
            if let Some(id) = value.as_id(){
                if id == id!(Bare){return true}
            }
            // alright its an object
            else if let Some(_o) = value.as_object(){
                // we now have to fetch the proto Id of the object
                // the arguments have been checked by the function or the type checker
            }
            false
        }
        
        fn script_proto_build(vm:&mut Vm, _props:&mut ScriptTypeProps)->Value{
            let obj = vm.heap.new();
            
            // how do we typecheck an enum type eh
            vm.heap.set_value(obj, id_lut!(Bare).into(), id!(Bare).into(), &vm.thread.trap);
            
            // alright next one the tuple
            vm.add_fn(obj, id!(Tuple), &[], |vm, args|{
                let tuple = vm.heap.new_with_proto(id!(Tuple).into());
                // lets typecheck the args here codegenerated
                vm.heap.vec_push_vec(tuple, args, &vm.thread.trap);
                tuple.into()
            });
            
            // we can make a type index prop check for this thing
            let named = vm.heap.new_with_proto(id!(Named).into());
            let mut props = ScriptTypeProps::default();
            let value = (1.0).script_to_value(vm);
            props.props.insert(id!(field), f64::script_type_id_static());
            vm.heap.set_value(named, id_lut!(field).into(), value, &vm.thread.trap);
            let ty_check = ScriptTypeCheck{
                props,
                object: None
            };
            let ty_index = vm.heap.register_type(None, ty_check);
            vm.heap.freeze_with_type(named, ty_index);
            
            vm.heap.set_value(obj, id_lut!(Named).into(), named.into(), &vm.thread.trap);
            obj.into()
        }
    }
    
    impl ScriptToValue for EnumTest{
        fn script_to_value(&self, _vm:&mut Vm)->Value{
            // alright script to al
            NIL
        }
    }
    
    impl ScriptApply for EnumTest{
        fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
        fn script_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, _value:Value){
            // alright lets apply 'value'
            // we now have to 'deserialise' value into the right enum type
            // the types should already be checked
            
        }
    }
    
    //impl ScriptHook for EnumTest{}
    
    #[derive(Script)]
    pub struct StructTest{
        #[live(1.0)] field:f64,
        //#[live(EnumTest::Bare)] enm:EnumTest,
        //#[live] this: Object,
        //#[live] onclick: Object,
    }
    
    
    let s = StructTest::script_new(vm_ref!(vm));
    let v = s.script_to_value(vm_ref!(vm)).into();
    vm.heap.print(v);
    
    //use crate::scriptable::*;
    use crate::vm::*;
    use crate::value::*;
    
    
    impl ScriptHook for StructTest{
        fn on_proto_methods(vm:&mut Vm, obj:Object){
            vm.add_fn(obj, id_lut!(return_two), args_lut!(o = 1.0), |_vm, _args|{
                return 2.into()
            });
        }
    }    
    
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
        let t = mod.std;
        scope.import(mod.std)
        
        // array operations
        let x = 1+2 assert(x == 3)
        let iv = [1 2 3 4] let ov = []
        for v in iv ov.push(v) assert(iv == ov)
        ov.pop() assert(iv != ov)
        
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
        let x = #(StructTest::script_api(vm_ref!(vm)));
        try{x{field:5}} assert(false) ok assert(true)
        try{x{field:true}} assert(true) ok assert(false)
        assert(x.return_two() == 2)
        
    };
    
    let _code = script!{
        scope.import(mod.std)
        mod.test.fetch();
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
