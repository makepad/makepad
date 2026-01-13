
use crate::makepad_live_id::*;
use makepad_script_derive::*;
use crate::traits::*;
use crate::heap::*;

pub fn test(){
    let mut vmbase = ScriptVmBase::new();
    let vm = &mut vmbase.as_ref();
    
    #[derive(Script)]
    pub struct StructTest{
        #[live(1.0)] field:f64,
        #[live(EnumTest::Bare)] enm:EnumTest,
        #[live] opt: Option<f64>,
        #[live] vec: Vec<u8>
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
    
    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct ShaderTest{
        #[live] struct_field: f32
    }
        
    use crate::vm::*;
    use crate::value::*;
        
    impl ScriptHook for StructTest{
        fn on_proto_methods(vm:&mut ScriptVm, obj:ScriptObject){
            let ht = vm.new_handle_type(id!(myhandle));
                        
            vm.add_handle_method(ht, id_lut!(return_three), script_args_def!(o = 1.0), |_vm, _args|{
                return 3.into()
            });
                        
            vm.add_method(obj, id_lut!(return_two), script_args_def!(o = 1.0), |_vm, _args|{
                return 2.into()
            });
                        
            vm.add_method(obj, id_lut!(return_handle), script_args_def!(o = 1.0), move |_vm, _args|{
                return ScriptHandle{ty:ht,index:0}.into()
            });
        }
    }
    
    
    let code = script!{
        use mod.std.*
        use mod.shader
        use mod.pod.*
        use mod.math.*
        
        let sdf = struct{
            field: f32,
            p: vec4f,
            arr: array{f32 4},
            set_field: |v| self.field += v 
            new: || self(
                arr: array(1f,2f,3f,4f)
                p: vec4(0)
                field: 1.0
            )
        }
        
        let draw_uniforms = struct{
            field: f32
        }
        
        let vertices = struct{
            pos: vec4,
        }
        
        // alright. lets figure out the shader sself
        let test_shader = #(ShaderTest::script_shader(vm)){
            vtx: shader.vertex_buffer(vertices)
            unitest: shader.uniform(1.0)
            draw: shader.uniform_buffer(draw_uniforms)
            y: shader.instance(1.0)
            x: shader.instance(1.0)
            vy: shader.varying(1.0)
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            otherfn: |x| x + 1
            vertex: fn(){
                self.vy = 1.0
                self.vertex_pos = self.vtx.pos
            }
            fragment: fn(){
                let v = self.vy + self.unitest
                let t = self.draw.field + self.struct_field
                let x = sdf.new()
                x.set_field(1f)
                x.p.y = 1f
                x.arr[3] = 1f
                self.otherfn(1f)
                self.pixel = mix(#f00, #0f0, self.x + self.y)
            }
        }
        //~test_shader
        let x = sdf(0,vec4(0),array(1f,2f,3f,4f))
        ~shader.compile_draw(test_shader)
    };
    
    // lets define a handle type with some methods on it
    // Our unit tests :)
    let _code = script!{
        use mod.std.assert
        use mod.std.println
        use mod.pod
        
        // array operations
        let x = 1+2 assert(x == 3)
        let iv = [1 2 3 4] let ov = []
                
        for v in iv ov.push(v) assert(iv == ov)
        assert(ov.pop() == 4) assert(iv != ov)
        assert(ov[2] == 3);
                
        // functions
        let f = |x| x+1
        assert(f(1) == 2)
                
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
        let s = #(StructTest::script_api(vm));
        try{s{field:5}} assert(false) ok assert(true)
        try{s{field:true}} assert(true) ok assert(false)
        assert(s.return_two() == 2)
                
        // check handle features
        let h = s.return_handle();
        assert(h.return_three() == 3)
                
        // check enum
        let EnumTest = #(EnumTest::script_api(vm));
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
                
        // check the option
        try{s{opt:nil}} assert(false) ok assert(true)
        try{s{opt:1.0}} assert(false) ok assert(true)
        try{s{opt:false}} assert(true) ok assert(false)
                
        // check the vec
        let x = s{vec:[1 2 3 4]}
        assert(x.vec == [1 2 3 4])
        // check typechecking in a vec
        try{s{vec:[false]}} assert(true) ok assert(false)
        try{s{vec:[1,2]}} assert(false) ok assert(true)
                
        // string to array
        assert("hi".to_bytes().to_string() == "hi")
        let a = "12345".to_bytes();
        a.pop();
        assert(a.to_string() == "1234")
        assert("hi".to_chars().to_string() == "hi")
                
        // test json
        let x = {x:1 y:[1 2 3]};
        let y = x.to_json();
        let z = y.parse_json();
        
        // test string-like property acceseses 
        assert(z == x)
        assert(z["x"] == z.x)
        assert(x["y"] == [1 2 3])
        z.x = 2
        assert(z["x"] == 2)
        let x = {"key":3, x:2.0}
        assert(x.key == 3)
                
        // test callbacks and do chaining
        let f = |x, cb| cb(x)
        assert(2 == f(1) do |x| x+1)
                
        // using ok to ignore errors
        let x = {t:3}
        assert( ok{x.y.z} == nil)
        assert( ok{x.t} == 3)
                
        // string concats
        let x = {t:"a"}
        x.t  += "b" + "c" + 2
        assert(x.t == "abc2")
        let x = ["c"]
        x[0] += "b" + "a" + 3
        assert(x == ["cba3"])
        let x = "aaaaaaa"
        x = x + "b"
        assert(x == "aaaaaaab")
                
        let x = |a| a + 1
        assert(x(1) == 2)
        let x = fn{2}
        assert(x() == 2)
        fn x{3}
        assert(x() == 3)
        fn x(a = 2){a + 2}
        assert(x(3) == 5)
        assert(x() == 4)
        fn test(a,b){a+b}
        assert(test(2 3) == 5)
        println("Test done")
        
        // POD testing
        let struct_3 = pod.struct{ // extendable pods 
            a: pod.f32
            b: pod.f32
            c: pod.f32
            d: pod.array{pod.f32 2}
            method: || self.c
        }
        let x = struct_3(1,2,3,pod.array(4f 5f));
        assert(x.c == 3f);
        assert(x.d[1] == 5f)
        
        assert(x.method() == 3f)
        
        let x = pod.vec3f(1,2,3);
        assert(x.z == 3f);
        let x = pod.vec4f(pod.vec2f(1,2), pod.vec2f(3,4));
        assert(x.w == 4f);
        
        // swizzle
        let x = pod.vec3f(1,2,3);
        assert(x.zyzx.x == 3f)
        // nested construction and read access to substructures (with copy)
        let s1 = pod.struct{a:pod.f16, b:pod.f16}
        let s2 = pod.struct{x:pod.f16, y:s1}
        let v = s2(3,s1(1,2))
        assert(v.y.b == 2h)
        
        // test wildcard use
        let m = {a_wild:1, b_wild:2}
        use m.*
        assert(a_wild == 1)
        assert(b_wild == 2)
    };
        
    let _code = script!{
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(38);
    };
        
    let dt = std::time::Instant::now();
        
    vm.eval(code);
    println!("Duration {}", dt.elapsed().as_secs_f64())
        
}
