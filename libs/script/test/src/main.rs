use makepad_script::apply::*;
use makepad_script::makepad_live_id::*;
use makepad_script::makepad_math::*;
use makepad_script::traits::*;
use makepad_script::heap::*;
use makepad_script::*;

pub fn main(){
    let vm = &mut ScriptVm{host:&mut 0, bx: Box::new(ScriptVmBase::new())}; 
        
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
    
    const fn make_val(x: u32) -> u32 { x * 10 }
    
    #[derive(Script, ScriptHook)]
    #[repr(u32)]
    pub enum ShaderEnum{
        #[pick]
        Test1 = 1,
        Test2 = 2,
        Test3 = make_val(3)    
    }
        
    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct ShaderTest{
        #[live] parent_field: f32, 
        #[live] unused_field1: f32
    }
        
    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct ShaderTest2{
        #[deref] parent: ShaderTest,
        #[live] color: Vec4f,
        #[live] child_field: f32, 
        #[live] unused_field2: f32
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
                return ScriptHandle::ZERO.into()
            });
        }
    }
    
    let _code = script!{
        use mod.std.assert
        ~"ShaderEnum values OK"
    };
    
    let _code = script!{
        use mod.std.*
        use mod.shader
        use mod.pod.*
        use mod.math.*
        
        let ShaderEnum = #(ShaderEnum::script_api(vm))
                
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
            field: f32,
        }
                
        let vertices = struct{
            pos: vec4,
        }
        let test_p1 = 1.0
        let test_col = #0f0
        let theme = {TEST_COL: #0f0}
        let test_obj = {test_p1:2.0 objfn:fn(){self.sub_obj.test_p1} sub_obj:{test_p1:3.0}}
        let test_uni = struct{p3:3.0}
        let test_buf = shader.uniform_buffer(test_uni)
        let test_tex = shader.texture_2d(float)
        // alright. lets figure out the shader sself
        let test_shader = #(ShaderTest2::script_shader(vm)){
            vtx: shader.vertex_buffer(vertices)
            unitest: shader.uniform(1.0)
            unitest2: shader.uniform(1.0)
            unicolor: shader.uniform(test_col)
            color: 1.0
            draw: shader.uniform_buffer(draw_uniforms)
            y: shader.instance(1.0)
            x: shader.instance(1.0)
            z_unused: shader.instance(1.0)
            vy: shader.varying(1.0)
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            otherfn: |x| x + 1
            testfn: ||{
                let c = self.unicolor;
                let k = test_p1
                let m = test_obj.test_p1
                let n = test_obj.objfn()
                let n = test_buf.p3
                let p = test_col
                let q = theme.TEST_COL
                let o = test_tex.sample(vec2(2.0))
                let s = 1u
                let k = match s{
                    ShaderEnum.Test1 => 1u
                    ShaderEnum.Test2 => 2u
                }
                return s
            }
            vertex: fn(){
                self.vy = 1.0
                self.vertex_pos = self.vtx.pos
            }
            fragment: fn(){
                let t = mix(#f0f, self.color, 0.5)
                let q = self.testfn()
                let v = self.unitest2 + self.vy + self.unitest
                let t = self.draw.field + self.parent_field + self.child_field
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
    let code = script!{
        use mod.std.assert
        use mod.std.println
        use mod.pod
                
        // arithmetic operations
        let x = 1+2 assert(x == 3)
        assert(10 - 3 == 7)
        assert(10 / 2 == 5)
        assert(10 % 3 == 1)
        assert((!true) == false)
        assert((!false) == true)
        assert((3 << 2) == 12)
        assert((12 >> 2) == 3)
        assert((5 & 3) == 1)
        assert((5 | 3) == 7)
        assert((5 ^ 3) == 6)
        let x = -5 assert(x == 0-5)
        assert(-5 == 0-5) assert(-5 != 0) assert(-(-5) == 5)
        
        // operator precedence tests
        assert(-5 + 3 == -2) assert(!true == false) assert(!false == true)
        assert(2 + 3 * 4 == 14) assert((2 + 3) * 4 == 20)
        assert(10 - 2 - 3 == 5) assert(10 - (2 - 3) == 11)
        assert(8 / 2 / 2 == 2) assert(8 / (2 / 2) == 8)
        assert(1 + 2 < 4) assert(!(1 + 2 < 2))
        assert(1 < 2 && 3 < 4) assert(!(1 > 2 && 3 < 4))
        assert(1 > 2 || 3 < 4) assert(!(1 > 2 || 3 > 4))
        assert((1 & 3) == 1) assert((1 | 2) == 3) assert((3 ^ 1) == 2)
        assert(1 << 2 == 4) assert(8 >> 2 == 2)
        
        // is type checks
        assert(5 is number) assert(5.0 is number) assert(!(5 is string))
        assert("hi" is string) assert(!("hi" is number))
        assert(true is bool) assert(false is bool) assert(!(true is number))
        assert(nil is nil) assert(!(5 is nil))
        assert({x:1} is object) assert(#f00 is color) assert([1 2] is array)
        
        // comparison operations
        assert(3 < 5) assert(!(5 < 3))
        assert(5 > 3) assert(!(3 > 5))
        assert(3 <= 3) assert(3 <= 5)
        assert(5 >= 5) assert(5 >= 3)
        assert(true && true) assert(!(true && false))
        assert(true || false) assert(!(false || false))
        // Short-circuit evaluation tests for ||, &&, |?
        // Basic behavior tests
        assert(true || false)
        assert(!(false || false))
        assert(true && true)
        assert(!(true && false))
        let x = nil let y = x |? 5 assert(y == 5)
        let x = 3 let y = x |? 5 assert(y == 3)
        
        // Short-circuit tests using side effects
        // || should not evaluate second operand if first is truthy
        let counter = {v:0}
        let inc = || { counter.v += 1; false }
        let result = true || inc()
        assert(result == true)
        assert(counter.v == 0) // inc() should NOT have been called
        
        let result = false || inc()
        assert(result == false)
        assert(counter.v == 1) // inc() SHOULD have been called
        
        // && should not evaluate second operand if first is falsy
        counter.v = 0
        let result = false && inc()
        assert(result == false)
        assert(counter.v == 0) // inc() should NOT have been called
        
        let result = true && inc()
        assert(result == false)
        assert(counter.v == 1) // inc() SHOULD have been called
        
        // |? should not evaluate second operand if first is not nil
        counter.v = 0
        let inc_ret = || { counter.v += 1; 99 }
        let x = 5
        let result = x |? inc_ret()
        assert(result == 5)
        assert(counter.v == 0) // inc_ret() should NOT have been called
        
        let x = nil
        let result = x |? inc_ret()
        assert(result == 99)
        assert(counter.v == 1) // inc_ret() SHOULD have been called
        
        // array operations
        let iv = [1 2 3 4] let ov = []
                        
        for v in iv ov.push(v) assert(iv == ov)
        assert(ov.pop() == 4) assert(iv != ov)
        assert(ov[2] == 3);
                        
        // functions
        let f = |x| x+1
        assert(f(1) == 2)
                
        // operator precedence
        let x = 2*3 + 4*5
        assert(x == 26)
        let x = 2*(3+4)*5
        assert(x == 70)
        let t = {x:2, y:3, z:4, w:5}
        let x = t.x*t.y + t.z*t.w
        assert(x == 26)
        let x = t.x*(t.y+t.z)*t.w
        assert(x == 70)
                                                
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
                        
        // compound assignment ops        
        let x = 1 x += 2 assert(x == 3)
        let x = 5 x -= 2 assert(x == 3)
        let x = 3 x *= 4 assert(x == 12)
        let x = 12 x /= 3 assert(x == 4)
        let x = 10 x %= 3 assert(x == 1)
        let x = 7 x &= 3 assert(x == 3)
        let x = 5 x |= 2 assert(x == 7)
        let x = 7 x ^= 3 assert(x == 4)
        let x = 3 x <<= 2 assert(x == 12)
        let x = 12 x >>= 2 assert(x == 3)
        let t = 3 t ?= 2 assert(t == 3)
        let t t ?= 2 assert(t == 2)
        let t = 0 t = 2 t += 1 assert(t==3)
        // field compound assignments
        let x = {f:2} x.f+=2 assert(x.f == 4)
        let x = {f:5} x.f-=2 assert(x.f == 3)
        let x = {f:3} x.f*=4 assert(x.f == 12)
        let x = {f:12} x.f/=3 assert(x.f == 4)
        let x = {f:10} x.f%=3 assert(x.f == 1)
        let x = {f:7} x.f&=3 assert(x.f == 3)
        let x = {f:5} x.f|=2 assert(x.f == 7)
        let x = {f:7} x.f^=3 assert(x.f == 4)
        let x = {f:3} x.f<<=2 assert(x.f == 12)
        let x = {f:12} x.f>>=2 assert(x.f == 3)
        let x = {f:3} x.f?=5 assert(x.f == 3)
        let x = {f:nil} x.f?=5 assert(x.f == 5)
        // index compound assignments
        let x = [1,2] x[1]+=2 assert(x == [1 4])
        let x = [1,5] x[1]-=2 assert(x[1] == 3)
        let x = [1,3] x[1]*=4 assert(x[1] == 12)
        let x = [1,12] x[1]/=3 assert(x[1] == 4)
        let x = [1,10] x[1]%=3 assert(x[1] == 1)
        let x = [1,7] x[1]&=3 assert(x[1] == 3)
        let x = [1,5] x[1]|=2 assert(x[1] == 7)
        let x = [1,7] x[1]^=3 assert(x[1] == 4)
        let x = [1,3] x[1]<<=2 assert(x[1] == 12)
        let x = [1,12] x[1]>>=2 assert(x[1] == 3)
        let x = [1,3] x[1]?=5 assert(x[1] == 3)
        let x = [1,nil] x[1]?=5 assert(x[1] == 5)
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
        
        try{s{field:"HI"}} assert(true) ok assert(false)
        assert(s.return_two() == 2)
                        
        // check handle features
        let h = s.return_handle();
        assert(h.return_three() == 3)
                        
        // check enum
        let EnumTest = #(EnumTest::script_api(vm));
        let x = EnumTest.Bare
        // test tuple typechecking
        try{EnumTest.Tuple(1.0)} assert(false) ok assert(true)
        try{EnumTest.Tuple("false")} assert(true) ok assert(false)
        try{EnumTest.Tuple()} assert(true) ok assert(false)
        try{EnumTest.Tuple(1,2)} assert(true) ok assert(false)
        try{EnumTest.Named{named_field:1.0}} assert(false) ok assert(true)
        try{EnumTest.Named{named_field:"true"}} assert(true) ok assert(false)
                        
        //assert(s.enm == EnumTest.Bare)
        try{s{enm: EnumTest.Bare}} assert(false) ok assert(true)
        try{s{enm: 1.0}} assert(true) ok assert(false)
        try{s{enm: EnumTest.Named{named_field:1.0}}} assert(false) ok assert(true)
        try{s{enm: EnumTest.Tuple(1.0)}} assert(false) ok assert(true)
                        
        // check the option
        try{s{opt:nil}} assert(false) ok assert(true)
        try{s{opt:1.0}} assert(false) ok assert(true)
        try{s{opt:"false"}} assert(true) ok assert(false)
                        
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
        
        // nil-safe field access with .?
        let x = {a:{b:5}, c:nil}
        assert(x.a.?b == 5)
        assert(x.c.?d == nil)
                        
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
        
        // math module tests
        use mod.math
        // constants
        assert(math.PI > 3.14 && math.PI < 3.15)
        assert(math.E > 2.71 && math.E < 2.72)
        
        // 1-arg scalar functions
        assert(math.abs(-5.0) == 5.0) assert(math.abs(5.0) == 5.0)
        assert(math.floor(3.7) == 3.0) assert(math.ceil(3.2) == 4.0)
        assert(math.round(3.5) == 4.0) assert(math.round(3.4) == 3.0)
        assert(math.sign(-5.0) == -1.0) assert(math.sign(5.0) == 1.0)
        assert(math.sqrt(4.0) == 2.0) assert(math.sqrt(9.0) == 3.0)
        assert(math.fract(3.75) == 0.75)
        assert(math.trunc(3.9) == 3.0) assert(math.trunc(-3.9) == -3.0)
        
        // 2-arg scalar functions
        assert(math.min(3.0, 5.0) == 3.0) assert(math.max(3.0, 5.0) == 5.0)
        assert(math.pow(2.0, 3.0) == 8.0)
        assert(math.modf(10.0, 3.0) == 1.0)
        assert(math.step(0.5, 0.3) == 0.0) assert(math.step(0.5, 0.7) == 1.0)
        
        // 3-arg scalar functions  
        assert(math.clamp(5.0, 0.0, 3.0) == 3.0)
        assert(math.clamp(-1.0, 0.0, 3.0) == 0.0)
        assert(math.clamp(1.5, 0.0, 3.0) == 1.5)
        assert(math.mix(0.0, 10.0, 0.5) == 5.0)
        assert(math.smoothstep(0.0, 1.0, 0.5) == 0.5)
        
        // vector operations
        let v1 = pod.vec2f(3, 4)
        assert(math.length(v1) == 5.0) // 3-4-5 triangle
        
        let v2 = pod.vec2f(1, 0)
        let v3 = pod.vec2f(0, 1)
        assert(math.dot(v2, v3) == 0.0) // perpendicular
        assert(math.dot(v2, v2) == 1.0) // unit vec dot itself
        
        // vector math functions
        let v = pod.vec3f(-1, 2, -3)
        let va = math.abs(v)
        assert(va.x == 1f && va.y == 2f && va.z == 3f)
        
        // mix with vectors
        let a = pod.vec2f(0, 0)
        let b = pod.vec2f(10, 20)
        let m = math.mix(a, b, 0.5)
        assert(m.x == 5f && m.y == 10f)
        
        // trig functions (basic sanity checks)
        assert(math.sin(0.0) == 0.0)
        assert(math.cos(0.0) == 1.0)
        let sinpi2 = math.sin(math.PI / 2.0)
        assert(sinpi2 > 0.99 && sinpi2 < 1.01)
        
        // exp/log
        assert(math.exp(0.0) == 1.0)
        assert(math.log(1.0) == 0.0)
        assert(math.exp2(3.0) == 8.0)
        assert(math.log2(8.0) == 3.0)
        
        // distance
        let p1 = pod.vec2f(0, 0)
        let p2 = pod.vec2f(3, 4)
        assert(math.distance(p1, p2) == 5.0)
        
        // clamp/min/max with vectors
        let v = pod.vec3f(5, -2, 10)
        let vmin = math.min(v, pod.vec3f(3, 3, 3))
        assert(vmin.x == 3f && vmin.y == -2f && vmin.z == 3f)
        let vmax = math.max(v, pod.vec3f(0, 0, 0))
        assert(vmax.x == 5f && vmax.y == 0f && vmax.z == 10f)
        
                
        // test wildcard use
        let m = {a_wild:1, b_wild:2}
        use m.*
        assert(a_wild == 1)
        assert(b_wild == 2)
                
        // test protoinheriting operators
        let x = {obj:{prop:1}}
        let y = x{obj +: {prop:2}}
        assert(x.obj.prop == 1)
        assert(y.obj.prop == 2)
                
        let x = {prop:1, x:1}
        x += {prop:2}
        assert(x.prop == 2 && x.x == 1)
                
        let x = {sub:{prop:1, x:1}}
        x.sub += {prop:2}
        assert(x.sub.prop == 2 && x.sub.x == 1)
                
        let x = {sub:[{prop:1, x:1}]}
        x.sub[0] += {prop:2}
        assert(x.sub[0].prop == 2 && x.sub[0].x == 1)
                
        // prefix use of .. splat operator
        let x = {a:1 b:2}
        let y = {b:3, ..x}
        assert(y.a == 1 && y.b == 3)
        
        // test the NORMAL version
        let x = 2
        let result = if x == 1{5}
        else if x == 2{6}
        else{7}
        assert(result == 6)
        
        let x = 1
        // We need to parse this syntax:
        let result = match x{ 
             1 => 5
             2 => {6}
             _=> {7}
        }
        assert(result == 5)
        
        // Test match with second arm
        let y = 2
        let result2 = match y{
            1 => true
            2 => {false}
        }
        assert(result2 == false)
        
        // Test match with wildcard default case
        let z = 99
        let result3 = match z{
            1 => "one"
            2 => "two"
            _ => "other"
        }
        assert(result3 == "other")
        
        // repr(u32) test
        let p = #(ShaderEnum::script_api(vm))
        assert(p.Test1._repr_u32_enum_value == 1)
        assert(p.Test2._repr_u32_enum_value == 2)
        assert(p.Test3._repr_u32_enum_value == 30)  // make_val(3) = 3 * 10 = 30

        println("Test done")
        
        // Match desugars to: let temp = expr; if temp == pattern1 body1 else if temp == pattern2 body2...
    };
            
    let _code = script!{
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(38);
    };
        
    let _code = script!{
        let x = {obj:{prop:1.0}}
        let y = x{obj +: {prop:2.0}}
    };
            
    let dt = std::time::Instant::now();
            
    vm.eval(code);
    println!("Duration {}", dt.elapsed().as_secs_f64())
            
}
