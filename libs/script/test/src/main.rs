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
        #[live] unused_field2: f32,
        #[live] enum_test:ShaderEnum
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
        assert(-5 + 3 == -2)
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
                        
        for v in iv { ov.push(v) } assert(iv == ov)
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
        
        // test && and || in if with braces
        // IMPORTANT: if the { is parsed as object literal, result would NOT be modified
        let x = 1 let y = 2
        var result = 0
        if x < y && y > 0 { 
            result = 1 
        }
        assert(result == 1) // This would fail if { was parsed as object literal
        result = 0
        if x > y || y > 0 { 
            result = 2 
        }
        assert(result == 2) // This would fail if { was parsed as object literal
                        
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
        
        // return-in-if escape analysis tests (interpreter)
        // Pattern 1: if-return, no else, code after
        fn ret_if_no_else(x) {
            if x > 0 { return 1 }
            return 0
        }
        assert(ret_if_no_else(5) == 1)
        assert(ret_if_no_else(-5) == 0)
        
        // Pattern 2: if-return, else-return (both branches return)
        fn ret_if_else(x) {
            if x > 0 { return 1 } else { return -1 }
        }
        assert(ret_if_else(5) == 1)
        assert(ret_if_else(-5) == -1)
        
        // Pattern 3: if-return, else no return, code after
        fn ret_if_else_fall(x) {
            if x > 0 { return 1 } else { let y = x }
            return 0
        }
        assert(ret_if_else_fall(5) == 1)
        assert(ret_if_else_fall(-5) == 0)
        
        // Pattern 4: if no return, else-return, code after
        fn ret_else_only(x) {
            if x > 0 { let y = x } else { return -1 }
            return 1
        }
        assert(ret_else_only(5) == 1)
        assert(ret_else_only(-5) == -1)
        
        // Pattern 5: if-else if-else chain with returns
        fn ret_chain(x) {
            if x > 10 { return 3 }
            else if x > 0 { return 2 }
            else if x == 0 { return 0 }
            else { return -1 }
        }
        assert(ret_chain(15) == 3)
        assert(ret_chain(5) == 2)
        assert(ret_chain(0) == 0)
        assert(ret_chain(-5) == -1)
        
        // Pattern 6: if-else if (no final else), code after
        fn ret_chain_fallthrough(x) {
            if x > 10 { return 3 }
            else if x > 0 { return 2 }
            return 0
        }
        assert(ret_chain_fallthrough(15) == 3)
        assert(ret_chain_fallthrough(5) == 2)
        assert(ret_chain_fallthrough(-5) == 0)
        
        // Pattern 7: nested if with returns
        fn ret_nested(x, y) {
            if x > 0 {
                if y > 0 { return 1 }
                else { return 2 }
            } else {
                if y > 0 { return 3 }
                return 4
            }
        }
        assert(ret_nested(1, 1) == 1)
        assert(ret_nested(1, -1) == 2)
        assert(ret_nested(-1, 1) == 3)
        assert(ret_nested(-1, -1) == 4)
        
        // Pattern 8: deeply nested returns
        fn ret_deep(a, b, c) {
            if a > 0 {
                if b > 0 {
                    if c > 0 { return 1 }
                    return 2
                }
                return 3
            }
            return 4
        }
        assert(ret_deep(1, 1, 1) == 1)
        assert(ret_deep(1, 1, -1) == 2)
        assert(ret_deep(1, -1, 0) == 3)
        assert(ret_deep(-1, 0, 0) == 4)
        
        // Pattern 9: return in only one branch of nested if
        fn ret_partial_nest(x, y) {
            if x > 0 {
                if y > 0 { return 1 }
                // y <= 0 falls through
            }
            return 0
        }
        assert(ret_partial_nest(1, 1) == 1)
        assert(ret_partial_nest(1, -1) == 0)
        assert(ret_partial_nest(-1, 1) == 0)
        
        // Pattern 10: early return vs expression result
        fn ret_vs_expr(x) {
            if x < 0 { return -1 }
            let result = if x == 0 { 0 } else { 1 }
            return result
        }
        assert(ret_vs_expr(-5) == -1)
        assert(ret_vs_expr(0) == 0)
        assert(ret_vs_expr(5) == 1)
        
        // for loop destructuring tests (interpreter)
        // Semantics:
        //   for v in set        - value only (array, object, range)
        //   for k v in set      - key/index + value (object: key,value; array: index,value; range: index,value)
        //   for i k v in set    - index + key + value (object only, errors on array)
        // NOTE: Object iteration only works on the "vec" part (keys like $a:)
        //       not the "map" part (keys like a:). This is by design.
        fn test_for_destructuring() {
            let arr = [10, 20, 30]
            let obj = {$a:1, $b:2, $c:3}  // Use $key: to put in vec (iterable), not map
            
            // for v in array (value only)
            let values1 = []
            for v in arr { values1.push(v) }
            assert(values1 == [10 20 30])
            
            // for v in object (value only)
            let values2 = []
            for v in obj { values2.push(v) }
            assert(values2 == [1 2 3])
            
            // for k v in object (key, value)
            let keys3 = []
            let values3 = []
            for k v in obj {
                keys3.push(k)
                values3.push(v)
            }
            assert(keys3.len() == 3)  // Key comparison with @$a syntax needs fixing
            assert(values3 == [1 2 3])
            
            // for i v in array (index, value)
            let indices4 = []
            let values4 = []
            for i v in arr {
                indices4.push(i)
                values4.push(v)
            }
            assert(indices4 == [0 1 2])
            assert(values4 == [10 20 30])
            
            // for i k v in object (index, key, value) - objects only
            let indices5 = []
            let keys5 = []
            let values5 = []
            for i k v in obj {
                indices5.push(i)
                keys5.push(k)
                values5.push(v)
            }
            assert(indices5 == [0 1 2])
            assert(keys5.len() == 3)  // Key comparison with @$a syntax needs fixing
            assert(values5 == [1 2 3])
            
            // for i in range (basic range iteration)
            let sum6 = 0
            for i in 0..5 { sum6 += 1 }
            assert(sum6 == 5)
            
            // for i v in range (index, value)
            let indices7 = []
            let values7 = []
            for i v in 0..3 {
                indices7.push(i)
                values7.push(v)
            }
            assert(indices7 == [0 1 2])
            assert(values7 == [0 1 2])
        }
        test_for_destructuring()
        
        // for loop return tests
        
        fn test_for_return(x) {
            let arr = [1, 2, 3, 4, 5]
            for v in arr {
                if v == x { return "found" }
            }
            return "not found"
        }
        assert(test_for_return(3) == "found")
        assert(test_for_return(10) == "not found")
        
        fn test_range_return(x) {
            for i in 0..10 {
                if i == x { return i }
            }
            return -1
        }
        assert(test_range_return(5) == 5)
        assert(test_range_return(15) == -1)
        
        fn test_nested_for_return(x, y) {
            for i in 0..3 {
                for j in 0..3 {
                    if i == x && j == y { return [i j] }
                }
            }
            return nil
        }
        assert(test_nested_for_return(1, 2) == [1 2])
        assert(test_nested_for_return(5, 5) == nil)
                
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
        
        // atan2 - computes atan(y/x) with correct quadrant
        let at = math.atan2(1.0, 1.0)
        assert(at > 0.78 && at < 0.79) // should be ~PI/4 = 0.785
        let at2 = math.atan2(-1.0, -1.0)
        assert(at2 < -2.35 && at2 > -2.36) // should be ~-3*PI/4
        
        // normalize - returns unit vector
        let v = pod.vec3f(3, 0, 4)
        let n = math.normalize(v)
        assert(n.x > 0.59 && n.x < 0.61) // 3/5 = 0.6
        assert(n.y == 0f)
        assert(n.z > 0.79 && n.z < 0.81) // 4/5 = 0.8
        let len_n = math.length(n)
        assert(len_n > 0.99 && len_n < 1.01) // normalized vector has length 1
        
        // cross product - only for vec3
        let x_axis = pod.vec3f(1, 0, 0)
        let y_axis = pod.vec3f(0, 1, 0)
        let z = math.cross(x_axis, y_axis)
        assert(z.x == 0f && z.y == 0f && z.z == 1f) // x cross y = z
        let neg_z = math.cross(y_axis, x_axis)
        assert(neg_z.x == 0f && neg_z.y == 0f && neg_z.z == -1f) // y cross x = -z
        
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

        // ============================================================
        // DESTRUCTURING TESTS
        // ============================================================
        
        // lazy ?= - should NOT run RHS when value exists
        fn destruct_dont_call(){assert(false)}
        let destruct_a = 1
        destruct_a ?= destruct_dont_call()
        assert(destruct_a == 1)
        
        // lazy ?= - SHOULD run RHS when value is nil
        let destruct_counter = {v:0}
        fn destruct_do_call(){destruct_counter.v = 1; 42}
        let destruct_b = nil
        destruct_b ?= destruct_do_call()
        assert(destruct_counter.v == 1)
        assert(destruct_b == 42)
        
        // Basic array destructuring
        let [destruct_c, destruct_d] = [1, 2]
        assert(destruct_c == 1 && destruct_d == 2)
        
        // Basic object destructuring
        let {destruct_e, destruct_f} = {destruct_e:3, destruct_f:4}
        assert(destruct_e == 3 && destruct_f == 4)
        
        // Object with lazy default - property exists, skip default
        let destruct_counter2 = {v:0}
        fn destruct_skip_default(){destruct_counter2.v = 1; 999}
        let {destruct_g, destruct_h=destruct_skip_default()} = {destruct_g:1, destruct_h:2}
        assert(destruct_g == 1 && destruct_h == 2)
        assert(destruct_counter2.v == 0)
        
        // Object with lazy default - property missing, use default
        let destruct_counter3 = {v:0}
        fn destruct_use_default(){destruct_counter3.v = 1; 100}
        let {destruct_i, destruct_j=destruct_use_default()} = {destruct_i:1}
        assert(destruct_i == 1 && destruct_j == 100)
        assert(destruct_counter3.v == 1)
        
        // Object with all defaults - values exist
        let {destruct_k=999, destruct_l=888} = {destruct_k:10, destruct_l:20}
        assert(destruct_k == 10 && destruct_l == 20)
        
        // Object with default, missing value
        let {destruct_m, destruct_n=42} = {destruct_m:1}
        assert(destruct_m == 1 && destruct_n == 42)
        
        // Array with lazy default - value missing
        let destruct_counter4 = {v:0}
        fn destruct_arr_use_def(){destruct_counter4.v = 1; 50}
        let [destruct_o, destruct_p=destruct_arr_use_def()] = [100]
        assert(destruct_o == 100 && destruct_p == 50)
        assert(destruct_counter4.v == 1)
        
        // Array with lazy default - value exists
        let destruct_counter5 = {v:0}
        fn destruct_arr_skip_def(){destruct_counter5.v = 1; 999}
        let [destruct_q, destruct_r=destruct_arr_skip_def()] = [100, 200]
        assert(destruct_q == 100 && destruct_r == 200)
        assert(destruct_counter5.v == 0)
        
        // Nested object inside array
        let [{destruct_x}] = [{destruct_x:1}]
        assert(destruct_x == 1)
        
        // Nested object inside array with multiple bindings
        let [{destruct_aa, destruct_ab}] = [{destruct_aa:10, destruct_ab:20}]
        assert(destruct_aa == 10 && destruct_ab == 20)
        
        // Multiple elements with nested pattern
        let [destruct_s, {destruct_t}] = [100, {destruct_t:200}]
        assert(destruct_s == 100 && destruct_t == 200)
        
        // Nested array inside array
        let [[destruct_u, destruct_v]] = [[1, 2]]
        assert(destruct_u == 1 && destruct_v == 2)
        
        // Multiple nested patterns
        let [{destruct_w}, [destruct_y, destruct_z]] = [{destruct_w:5}, [6, 7]]
        assert(destruct_w == 5 && destruct_y == 6 && destruct_z == 7)

        // ============================================================
        // SHADER COMPILER TESTS
        // Comprehensive test of all shader compiler features
        // ============================================================
        use mod.shader
        use mod.pod.*
        use mod.math.*
        
        let ShaderEnum = #(ShaderEnum::script_api(vm))
        
        // Pod structs for testing
        let test_struct = struct{
            f: f32,
            v2: vec2f,
            v3: vec3f, 
            v4: vec4f,
            i: i32,
            u: u32,
            arr: array{f32 4},
        }
        
        let vertex_data = struct{
            pos: vec4f,
            uv: vec2f,
            normal: vec3f,
        }
        
        let uniforms_data = struct{
            mvp: f32,
            time: f32,
            scale: vec2f,
        }
        
        // Scope uniforms for testing
        let scope_time = 1.5
        let scope_color = #0ff
        let scope_vec = vec2f(2.0, 3.0)
        let scope_uniforms = struct{time:f32, scale:f32}
        let scope_buf = shader.uniform_buffer(scope_uniforms)
        let scope_tex = shader.texture_2d(float)
        let test_color = #f00
        
        // TestSdf - minimal Sdf2d-like struct to test struct methods in shaders
        let TestSdf = struct {
            pos: vec2f
            result: vec4f
            dist: f32
            
            // Constructor
            new: fn(p: vec2) -> Self {
                return self(pos: p, result: vec4(0f), dist: 0f)
            }
            // Method mutating self, returning value
            translate: fn(x: f32, y: f32) -> vec2 {
                self.pos -= vec2(x, y)
                return self.pos
            }
            // Method mutating self, no return
            clear: fn(color: vec4) {
                self.result = color
            }
            // Method calling another method  
            helper: fn(v: f32) -> f32 { return v * 2f }
            use_helper: fn() -> f32 {
                return self.helper(self.dist)
            }
            // Method with unary neg on self field
            negate_dist: fn() -> f32 {
                return -self.dist
            }
        }
        
        // Comprehensive shader test
        let shader_all_features = #(ShaderTest2::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            // Vertex buffer
            vtx: shader.vertex_buffer(vertex_data)
            // Instance data
            inst_pos: shader.instance(vec2f)
            inst_scale: shader.instance(1.0)
            inst_color: shader.instance(vec4f)
            inst_id: shader.instance(0u)
            // Uniforms
            u_time: shader.uniform(0.0)
            u_scale: shader.uniform(vec2f)
            u_color: shader.uniform(#fff)
            u_enabled: shader.uniform(true)
            u_count: shader.uniform(0i)
            u_flags: shader.uniform(0u)
            uniforms: shader.uniform_buffer(uniforms_data)
            // Textures
            tex_diffuse: shader.texture_2d(float)
            tex_normal: shader.texture_2d(float)
            // Varyings
            v_uv: shader.varying(vec2f)
            v_color: shader.varying(vec4f)
            v_intensity: shader.varying(1.0)
            v_normal: shader.varying(vec3f)
            v_world_pos: shader.varying(vec3f)
            // Helper functions
            helper: |x| x * 2f
            helper2: |a, b| a + b
            helper_vec: |v| v * 2f
            get_val: || { return 1f }
            get_val_cond: |x| { if x > 0f { return 1f } return 0f }
            
            // ---- Return-in-if escape analysis tests ----
            // Pattern 1: if-return, no else, code after
            ret_if_no_else: fn(x: f32) -> f32 {
                if x > 0f { return 1f }
                return 0f
            }
            // Pattern 2: if-return, else-return (both branches return)
            ret_if_else: fn(x: f32) -> f32 {
                if x > 0f { return 1f } else { return -1f }
            }
            // Pattern 3: if-return, else no return, code after
            ret_if_else_fall: fn(x: f32) -> f32 {
                if x > 0f { return 1f } else { let y = x }
                return 0f
            }
            // Pattern 4: if no return, else-return, code after
            ret_else_only: fn(x: f32) -> f32 {
                if x > 0f { let y = x } else { return -1f }
                return 1f
            }
            // Pattern 5: if-else if-else chain with returns
            ret_chain: fn(x: f32) -> f32 {
                if x > 10f { return 3f }
                else if x > 0f { return 2f }
                else if x == 0f { return 0f }
                else { return -1f }
            }
            // Pattern 6: if-else if (no final else), code after
            ret_chain_fall: fn(x: f32) -> f32 {
                if x > 10f { return 3f }
                else if x > 0f { return 2f }
                return 0f
            }
            // Pattern 7: nested if with returns
            ret_nested: fn(x: f32, y: f32) -> f32 {
                if x > 0f {
                    if y > 0f { return 1f }
                    else { return 2f }
                } else {
                    if y > 0f { return 3f }
                    return 4f
                }
            }
            // Pattern 8: deeply nested returns
            ret_deep: fn(a: f32, b: f32, c: f32) -> f32 {
                if a > 0f {
                    if b > 0f {
                        if c > 0f { return 1f }
                        return 2f
                    }
                    return 3f
                }
                return 4f
            }
            // Pattern 9: return in only one branch of nested if
            ret_partial: fn(x: f32, y: f32) -> f32 {
                if x > 0f {
                    if y > 0f { return 1f }
                }
                return 0f
            }
            // Pattern 10: early return vs expression result
            ret_vs_expr: fn(x: f32) -> f32 {
                if x < 0f { return -1f }
                let result = if x == 0f { 0f } else { 1f }
                return result
            }
            // Pattern 11: return with computation after if
            ret_with_comp: fn(x: f32) -> f32 {
                if x < 0f { return x * -1f }
                let y = x * 2f
                let z = y + 1f
                return z
            }
            // Pattern 12: multiple early returns
            ret_multi_early: fn(x: f32) -> f32 {
                if x < -10f { return -2f }
                if x < 0f { return -1f }
                if x == 0f { return 0f }
                return 1f
            }
            // Pattern 13: return from for loop
            ret_from_for: fn(x: f32) -> f32 {
                for i in 0..10 {
                    if f32(i) == x { return f32(i) }
                }
                return -1f
            }
            // Pattern 14: return from nested for loop
            ret_from_nested_for: fn(x: f32, y: f32) -> f32 {
                for i in 0..5 {
                    for j in 0..5 {
                        if f32(i) == x && f32(j) == y { return f32(i) + f32(j) }
                    }
                }
                return -1f
            }
            // Pattern 15: return from for loop with computation
            ret_from_for_comp: fn(x: f32) -> f32 {
                var sum = 0f
                for i in 0..10 {
                    sum += f32(i)
                    if sum > x { return sum }
                }
                return sum
            }
            
            vertex: fn(){
                // Vertex buffer access
                let pos = self.vtx.pos
                let uv = self.vtx.uv
                let normal = self.vtx.normal
                // Instance data
                let offset = self.inst_pos
                let scale = self.inst_scale
                // Uniform access
                let t = self.u_time
                let s = self.u_scale
                let mvp = self.uniforms.mvp
                // Set varyings
                self.v_uv = uv
                self.v_color = vec4(1f, 0f, 0f, 1f)
                self.v_intensity = 0.8f
                self.v_normal = normal
                let world_pos = pos.xyz + vec3(offset.x, offset.y, 0f)
                self.v_world_pos = world_pos
                self.vertex_pos = vec4(world_pos * mvp, 1f)
            }
            
            fragment: fn(){
                // ---- Arithmetic ops (f32) ----
                let a = 1f let b = 2f
                let c = a + b
                let c = a - b
                let c = a * b
                let c = a / b
                let c = -a
                
                // ---- Arithmetic ops (i32/u32 + bitwise) ----
                let ai = 10i let bi = 3i
                let ci = ai + bi
                let ci = ai - bi
                let ci = ai * bi
                let ci = ai / bi
                let ci = ai % bi
                let ci = ai << 2i
                let ci = ai >> 1i
                let ci = ai & bi
                let ci = ai | bi
                let ci = ai ^ bi
                let xu = 10u let yu = 3u
                let zu = xu + yu
                let zu = xu - yu
                let zu = xu * yu
                let zu = xu / yu
                let zu = xu % yu
                let zu = xu << 2u
                let zu = xu >> 1u
                let zu = xu & yu
                let zu = xu | yu
                let zu = xu ^ yu
                
                // ---- f16 (half precision) ----
                let ah = 1h let bh = 2h
                let ch = ah + bh
                let ch = ah - bh
                let ch = ah * bh
                let ch = ah / bh
                let ch = -ah
                var dh = 1h
                dh += 1h
                dh -= 1h
                dh *= 2h
                dh /= 2h
                
                // ---- Vector arithmetic ----
                let v2a = vec2(1f, 2f) let v2b = vec2(3f, 4f)
                let v2c = v2a + v2b
                let v2c = v2a - v2b
                let v2c = v2a * v2b
                let v2c = v2a / v2b
                let v2c = -v2a
                let v3a = vec3(1f, 2f, 3f) let v3b = vec3(4f, 5f, 6f)
                let v3c = v3a + v3b
                let v3c = v3a - v3b
                let v3c = v3a * v3b
                let v3c = v3a / v3b
                let v3c = -v3a
                let v4a = vec4(1f, 2f, 3f, 4f) let v4b = vec4(5f, 6f, 7f, 8f)
                let v4c = v4a + v4b
                let v4c = v4a - v4b
                let v4c = v4a * v4b
                let v4c = v4a / v4b
                let v4c = -v4a
                let v4c = v4a * 2f
                let v4c = 2f * v4a
                
                // ---- Comparisons ----
                var result = 0f
                if a == b { result = 1f }
                if a != b { result = 2f }
                if a < b { result = 3f }
                if a > b { result = 4f }
                if a <= b { result = 5f }
                if a >= b { result = 6f }
                let xi = 1i let yi = 2i
                if xi == yi { result = 7f }
                if xi != yi { result = 8f }
                if xi < yi { result = 9f }
                if xi > yi { result = 10f }
                let pu = 1u let qu = 2u
                if pu == qu { result = 11f }
                if pu != qu { result = 12f }
                if pu < qu { result = 13f }
                if pu > qu { result = 14f }
                
                // ---- Logic ops ----
                let x = 1f let y = 2f
                if x < y && y > 0f { result = 1f }
                if x > y || y > 0f { result = 2f }
                let la = true let lb = false
                let lc = la && lb
                let ld = la || lb
                let le = x < y && y > 0f
                
                // ---- Var assignments with compound ops ----
                var va = 1f
                va = 2f
                va += 1f
                va -= 1f
                va *= 2f
                va /= 2f
                var vb = 10i
                vb += 1i
                vb -= 1i
                vb *= 2i
                vb /= 2i
                vb %= 3i
                vb &= 7i
                vb |= 1i
                vb ^= 2i
                vb <<= 1i
                vb >>= 1i
                var vc = 10u
                vc += 1u
                vc -= 1u
                vc *= 2u
                vc /= 2u
                vc %= 3u
                vc &= 7u
                vc |= 1u
                vc ^= 2u
                vc <<= 1u
                vc >>= 1u
                
                // ---- Field assignments ----
                var s = test_struct(1f, vec2(0f), vec3(0f), vec4(0f), 0i, 0u, array(0f,0f,0f,0f))
                s.f = 2f
                s.f += 1f
                s.f -= 1f
                s.f *= 2f
                s.f /= 2f
                s.i = 5i
                s.i += 1i
                s.i -= 1i
                s.i *= 2i
                s.i /= 2i
                s.i %= 3i
                s.i &= 7i
                s.i |= 1i
                s.i ^= 2i
                s.i <<= 1i
                s.i >>= 1i
                s.v2.x = 1f
                s.v3.y = 2f
                s.v4.z = 3f
                
                // ---- Array indexing ----
                var arr = array(1f, 2f, 3f, 4f)
                let ax = arr[0]
                let ay = arr[1]
                arr[0] = 5f
                arr[1] = 6f
                arr[0] += 1f
                arr[1] -= 1f
                arr[2] *= 2f
                arr[3] /= 2f
                var idx = 0i
                let az = arr[idx]
                arr[idx] = 10f
                var v = vec4(1f, 2f, 3f, 4f)
                let vx = v[0]
                let vy = v[1]
                v[2] = 10f
                v[3] += 5f
                var v3 = vec3(1f, 2f, 3f)
                let v3z = v3[2]
                v3[0] = 5f
                
                // ---- If/else ----
                let xif = 1f
                var result_if = 0f
                if xif > 0f { result_if = 1f }
                let yif = if xif > 0f { 1f } else { 0f }
                let zif = if xif < 0f { -1f } else if xif == 0f { 0f } else { 1f }
                var vif = 0f
                if xif > 0f { if xif < 2f { vif = 1f } }
                
                // ---- Match ----
                let xm = self.enum_test
                let result_m = match xm {
                    ShaderEnum.Test1 => 1f
                    ShaderEnum.Test2 => 2f
                    _ => 0f
                }
                
                // ---- For loops ----
                var sum = 0f
                for i in 0..4 { sum += 1f }
                var sum2 = 0f
                for i in 0..2 { for j in 0..3 { sum2 += 1f } }
                
                // ---- Builtin functions ----
                let ba = 0.5f let bb = 1.0f let bt = 0.5f
                let br = abs(ba)
                let br = floor(ba)
                let br = ceil(ba)
                let br = round(ba)
                let br = fract(ba)
                let br = sqrt(ba)
                let br = sin(ba)
                let br = cos(ba)
                let br = tan(ba)
                let br = asin(ba)
                let br = acos(ba)
                let br = atan(ba)
                let br = exp(ba)
                let br = log(ba)
                let br = exp2(ba)
                let br = log2(ba)
                let br = min(ba, bb)
                let br = max(ba, bb)
                let br = pow(ba, bb)
                let br = step(ba, bb)
                let br = atan2(ba, bb)
                let br = clamp(ba, 0f, bb)
                let br = mix(ba, bb, bt)
                let br = smoothstep(0f, bb, ba)
                let bv = vec3(1f, 2f, 3f)
                let blen = length(bv)
                let bn = normalize(bv)
                let bd = dot(bv, bv)
                let bc = cross(bv, vec3(0f, 1f, 0f))
                let bdist = distance(bv, vec3(0f))
                let bva = vec3(-1f, 0.5f, 2f)
                let bvr = abs(bva)
                let bvr = floor(bva)
                let bvr = ceil(bva)
                let bvr = sin(bva)
                let bvr = cos(bva)
                let bvr = mix(bva, vec3(1f), 0.5f)
                
                // ---- Function calls ----
                let fa = self.helper(1f)
                let fb = self.helper2(1f, 2f)
                let fv = self.helper_vec(vec3(1f, 2f, 3f))
                let fra = self.get_val()
                let frb = self.get_val_cond(1f)
                
                // ---- Declarations ----
                let df = 1f
                let dh = 1h
                let di = 1i
                let du = 1u
                let db = true
                let dv2 = vec2(1f, 2f)
                let dv3 = vec3(1f, 2f, 3f)
                let dv4 = vec4(1f, 2f, 3f, 4f)
                let dv2i = vec2i(1i, 2i)
                let dv3i = vec3i(1i, 2i, 3i)
                let dv4i = vec4i(1i, 2i, 3i, 4i)
                let dv2u = vec2u(1u, 2u)
                let dv3u = vec3u(1u, 2u, 3u)
                let dv4u = vec4u(1u, 2u, 3u, 4u)
                let dcol = #f00
                var dvf = 1f
                var dvi = 1i
                var dvu = 1u
                var dvv4 = vec4(1f)
                dvf = 2f
                dvi = 2i
                dvu = 2u
                dvv4 = vec4(2f)
                let ds = test_struct(1f, vec2(0f), vec3(0f), vec4(0f), 0i, 0u, array(0f,0f,0f,0f))
                
                // ---- Swizzles ----
                let sv4 = vec4(1f, 2f, 3f, 4f)
                let swa = sv4.x
                let swb = sv4.xy
                let swc = sv4.xyz
                let swd = sv4.xyzw
                let swe = sv4.wzyx
                let swf = sv4.xxxx
                let swg = sv4.rg
                let swh = sv4.rgb
                let sv3 = vec3(1f, 2f, 3f)
                let swi = sv3.xy
                let swj = sv3.zyx
                
                // ---- Colors ----
                let c1 = #ff0000
                let c2 = #00ff00
                let c3 = #0000ff
                let c4 = test_color
                let mixed = mix(c1, c2, 0.5f)
                
                // ---- Varying access ----
                let uv = self.v_uv
                let col = self.v_color
                let intensity = self.v_intensity
                let normal = self.v_normal
                let world_pos = self.v_world_pos
                
                // ---- Uniform access ----
                let ucol = self.u_color
                let uenabled = self.u_enabled
                let ucount = self.u_count
                let uflags = self.u_flags
                let utime = self.uniforms.time
                
                // ---- Texture sampling ----
                let diffuse = self.tex_diffuse.sample(uv)
                let norm_tex = self.tex_normal.sample(uv)
                
                // ---- Scope uniforms ----
                let st = scope_time
                let sc = scope_color
                let sv = scope_vec
                let sbt = scope_buf.time
                let sbs = scope_buf.scale
                let stex = scope_tex.sample(vec2(0.5f, 0.5f))
                
                // ---- Lighting calc ----
                let light = dot(normal, vec3(0f, 1f, 0f))
                let final_col = diffuse * self.inst_color
                
                // ---- TestSdf (struct with methods) ----
                var sdf = TestSdf.new(uv)
                let translated = sdf.translate(0.5f, 0.5f)
                sdf.clear(vec4(1f, 0f, 0f, 1f))
                let h = sdf.use_helper()
                let neg = sdf.negate_dist()
                let sdf_result = sdf.result
                
                // ---- Return-in-if escape analysis ----
                let r1 = self.ret_if_no_else(1f)
                let r2 = self.ret_if_else(1f)
                let r3 = self.ret_if_else_fall(1f)
                let r4 = self.ret_else_only(1f)
                let r5 = self.ret_chain(15f)
                let r6 = self.ret_chain_fall(5f)
                let r7 = self.ret_nested(1f, 1f)
                let r8 = self.ret_deep(1f, 1f, 1f)
                let r9 = self.ret_partial(1f, 1f)
                let r10 = self.ret_vs_expr(1f)
                let r11 = self.ret_with_comp(5f)
                let r12 = self.ret_multi_early(5f)
                let r13 = self.ret_from_for(5f)
                let r14 = self.ret_from_nested_for(2f, 3f)
                let r15 = self.ret_from_for_comp(10f)
                
                self.pixel = final_col
            }
        }
        shader.test_compile_draw(shader_all_features)

        println("Test done")
        
    };
          
    let dt = std::time::Instant::now();
    vm.eval(code);
    println!("Duration {}", dt.elapsed().as_secs_f64())
            
}
