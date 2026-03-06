use makepad_script::apply::*;
use makepad_script::heap::*;
use makepad_script::makepad_live_id::*;
use makepad_script::makepad_math::*;
use makepad_script::traits::*;
use makepad_script::*;
use std::collections::BTreeMap;

pub fn main() {
    let vm = &mut ScriptVm {
        host: &mut 0,
        bx: Box::new(ScriptVmBase::new()),
    };

    #[derive(Script)]
    pub struct StructTest {
        #[live(1.0)]
        field: f64,
        #[live(EnumTest::Bare)]
        enm: EnumTest,
        #[live]
        opt: Option<f64>,
        #[live]
        vec: Vec<u8>,
    }

    #[derive(Script, ScriptHook)]
    pub enum EnumTest {
        #[pick]
        Bare,
        #[live(1.0)]
        Tuple(f64),
        #[live{named_field:1.0}]
        Named { named_field: f64 },
    }

    const fn make_val(x: u32) -> u32 {
        x * 10
    }

    #[derive(Script, ScriptHook)]
    #[repr(u32)]
    pub enum ShaderEnum {
        #[pick]
        Test1 = 1,
        Test2 = 2,
        Test3 = make_val(3),
    }

    // Test enum with Vec<LiveId> field - reproducing the MenuItem issue
    #[derive(Clone, Debug, Script, ScriptHook)]
    pub enum MenuTest {
        #[live { items: Vec::new() }]
        Main { items: Vec<LiveId> },

        #[live { name: String::new(), items: Vec::new() }]
        Sub { name: String, items: Vec<LiveId> },

        #[pick]
        Line,
    }

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct ShaderTest {
        #[live]
        parent_field: f32,
        #[live]
        unused_field1: f32,
    }

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct ShaderTest2 {
        #[deref]
        parent: ShaderTest,
        #[live]
        color: Vec4f,
        #[live]
        child_field: f32,
        #[live]
        unused_field2: f32,
        #[live]
        enum_test: ShaderEnum,
    }

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct GpuShaderStageTest {
        #[live]
        dummy: f32,
    }

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct RustUniformBufferTest {
        #[live]
        pick: Vec2f,
        #[live]
        scale: Vec2f,
        #[live]
        gain: f32,
        #[live]
        pad: f32,
    }

    fn rust_uniform_buffer_test_pod(vm: &mut ScriptVm) -> ScriptValue {
        let pod = RustUniformBufferTest::script_pod(vm).expect("Cant make a pod type");
        vm.bx
            .heap
            .pod_type_name_set(pod, id_lut!(RustUniformBufferTest));
        pod.into()
    }

    // Test struct for script_apply_eval stress test
    // Mimics the draw_bg pattern used in widgets
    #[derive(Script, ScriptHook, Default)]
    pub struct DrawBgTest {
        #[source]
        source: ScriptObjectRef,
        #[live]
        is_even: f32,
        #[live]
        color: Vec4f,
    }

    use crate::value::*;
    use crate::vm::*;

    impl ScriptHook for StructTest {
        fn on_proto_methods(vm: &mut ScriptVm, obj: ScriptObject) {
            let ht = vm.new_handle_type(id!(myhandle));

            vm.add_handle_method(
                ht,
                id_lut!(return_three),
                script_args_def!(o = 1.0),
                |_vm, _args| return 3.into(),
            );

            vm.add_method(
                obj,
                id_lut!(return_two),
                script_args_def!(o = 1.0),
                |_vm, _args| return 2.into(),
            );

            vm.add_method(
                obj,
                id_lut!(return_handle),
                script_args_def!(o = 1.0),
                move |vm, _args| {
                    struct DummyHandle;
                    impl ScriptHandleGc for DummyHandle {
                        fn gc(&mut self) {}
                    }
                    vm.bx.heap.new_handle(ht, Box::new(DummyHandle)).into()
                },
            );

            // Returns a BTreeMap<String, Vec<String>> like HTTP headers
            vm.add_method(
                obj,
                id_lut!(return_headers),
                script_args_def!(),
                |vm, _args| {
                    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
                    headers.insert("Content-Type".to_string(), vec!["text/html".to_string()]);
                    headers.insert(
                        "Set-Cookie".to_string(),
                        vec!["session=abc123".to_string(), "lang=en".to_string()],
                    );
                    headers.insert("X-Custom".to_string(), vec!["hello".to_string()]);
                    headers.script_to_value(vm)
                },
            );
        }
    }

    // lets define a handle type with some methods on it
    // Our unit tests :)
    let code = script! {
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

        // BTreeMap<String, Vec<String>> test (like HTTP headers)
        let headers = s.return_headers()
        // Access by string key with bracket notation
        let ct = headers["Content-Type"]
        assert(ct[0] == "text/html")
        let sc = headers["Set-Cookie"]
        assert(sc[0] == "session=abc123")
        assert(sc[1] == "lang=en")
        let xc = headers["X-Custom"]
        assert(xc[0] == "hello")
        // Verify that headers work as a proper string-keyed map
        // This should NOT crash or error
        println(headers)

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

        // Test MenuTest enum with Vec<LiveId> field
        let MenuTest = #(MenuTest::script_api(vm));
        let m = MenuTest.Main{items: []}
        let m2 = MenuTest.Sub{name: "File", items: []}
        let m3 = MenuTest.Line
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

        // OpenAI stream chunk parse sanity
        let openai_chunk = "{\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"content\":\".\"}}],\"created\":1771967391,\"id\":\"chatcmpl-2P5VBQwKP9Ds5B70faNiVc2O9UMkxvyR\",\"model\":\"gpt-3.5-turbo\",\"system_fingerprint\":\"b6247-92f7f0a5\",\"object\":\"chat.completion.chunk\"}";
        let openai_parsed = openai_chunk.parse_json();
        assert(openai_parsed.choices[0].index == 0)
        assert(openai_parsed.choices[0].delta.content == ".")
        assert(openai_parsed.choices[0].finish_reason == nil)

        // Same payload with SSE "data: " prefix
        let openai_sse = "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"content\":\".\"}}],\"created\":1771967391,\"id\":\"chatcmpl-2P5VBQwKP9Ds5B70faNiVc2O9UMkxvyR\",\"model\":\"gpt-3.5-turbo\",\"system_fingerprint\":\"b6247-92f7f0a5\",\"object\":\"chat.completion.chunk\"}";
        let openai_sse_parsed = openai_sse.parse_json();
        assert(openai_sse_parsed.data.choices[0].index == 0)
        assert(openai_sse_parsed.data.choices[0].delta.content == ".")

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
        // Object iteration works on both "vec" (keys using :=) and "map" (keys using :) parts.
        fn test_for_destructuring() {
            let arr = [10, 20, 30]
            let obj = {a := 1, b := 2, c := 3}  // Use := to put in vec (iterable), not map

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
            assert(keys3.len() == 3)
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
            assert(keys5.len() == 3)
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

        // for (v, i) in — parenthesized style (identical to for v, i in)
        fn test_for_paren_style() {
            let arr = [10, 20, 30]
            let obj = {a := 1, b := 2, c := 3}

            // for (v) in array — single binding with parens
            let values1 = []
            for (v) in arr { values1.push(v) }
            assert(values1 == [10 20 30])

            // for (i, v) in array — two bindings with parens
            let indices2 = []
            let values2 = []
            for (i, v) in arr {
                indices2.push(i)
                values2.push(v)
            }
            assert(indices2 == [0 1 2])
            assert(values2 == [10 20 30])

            // for (k, v) in object — two bindings with parens
            let keys3 = []
            let values3 = []
            for (k, v) in obj {
                keys3.push(k)
                values3.push(v)
            }
            assert(keys3.len() == 3)
            assert(values3 == [1 2 3])

            // for (i, k, v) in object — three bindings with parens
            let indices4 = []
            let keys4 = []
            let values4 = []
            for (i, k, v) in obj {
                indices4.push(i)
                keys4.push(k)
                values4.push(v)
            }
            assert(indices4 == [0 1 2])
            assert(keys4.len() == 3)
            assert(values4 == [1 2 3])

            // for (i) in range — single binding with parens
            let sum5 = 0
            for (i) in 0..5 { sum5 += 1 }
            assert(sum5 == 5)

            // for (i, v) in range — two bindings with parens
            let indices6 = []
            let values6 = []
            for (i, v) in 0..3 {
                indices6.push(i)
                values6.push(v)
            }
            assert(indices6 == [0 1 2])
            assert(values6 == [0 1 2])
        }
        test_for_paren_style()

        // for k v in on map-based objects (inline {"key": val} syntax)
        fn test_for_map_objects() {
            let map_obj = {"alpha": 1, "beta": 2, "gamma": 3}
            let count = 0
            let vals = []
            for k v in map_obj {
                count += 1
                vals.push(v)
            }
            assert(count == 3)
            assert(vals.len() == 3)

            // for v in map object (value only)
            let vals2 = []
            for v in map_obj { vals2.push(v) }
            assert(vals2.len() == 3)

            // mixed vec + map object
            let mixed = {a := 10, "b": 20}
            let mixed_vals = []
            for k v in mixed {
                mixed_vals.push(v)
            }
            assert(mixed_vals.len() == 2)
        }
        test_for_map_objects()

        // obj[variable] = val — bracket assign with variable key must resolve the variable
        fn test_bracket_assign_variable_key() {
            // string variable as key
            let obj = {}
            let key = "hello"
            obj[key] = 42
            assert(obj["hello"] == 42)

            // variable key in a for loop over map entries
            let target = {}
            let src = {"a": 1, "b": 2, "c": 3}
            for k v in src {
                target[k] = v
            }
            assert(target["a"] == 1)
            assert(target["b"] == 2)
            assert(target["c"] == 3)

            // array index assign
            let arr = [0, 0, 0]
            arr[1] = 99
            assert(arr[1] == 99)

            // array index assign with variable
            let arr2 = [10, 20, 30]
            let idx = 2
            arr2[idx] = 777
            assert(arr2[2] == 777)

            // variable holding a LiveId key
            let obj2 = {x := 10, y := 20}
            let k2 = @x
            obj2[k2] = 99
            assert(obj2.x == 99)
        }
        test_bracket_assign_variable_key()

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

        // ============================================================
        // := (ASSIGN_ME_VEC) TESTS
        // := stores key-value pairs in the object's vec (ordered storage)
        // : stores in the map (hash storage)
        // ============================================================

        // Basic := stores in vec, not map
        let obj = {a := 1, b := 2}
        // Iteration works on vec items
        let vals = []
        for v in obj { vals.push(v) }
        assert(vals == [1 2])

        // := and : coexist — map props and vec props are separate
        let obj = {map_prop: 10, vec_prop := 20}
        assert(obj.map_prop == 10)
        // vec_prop is in the vec, accessible via deep lookup
        assert(obj.vec_prop == 20)

        // Modifying a := property via dot access
        let obj = {prop := {val: 1}}
        obj.prop.val = 2
        assert(obj.prop.val == 2)

        // +: merge on a := defined property
        let base = {child := {x: 1, y: 2}}
        let derived = base{child +: {x: 10}}
        assert(derived.child.x == 10)
        assert(derived.child.y == 2)
        // original unchanged
        assert(base.child.x == 1)

        // +: merge on nested := property
        let base = {inner := {sub: {a: 1, b: 2}}}
        let derived = base{inner +: {sub +: {a: 99}}}
        assert(derived.inner.sub.a == 99)
        assert(derived.inner.sub.b == 2)

        // := in a type-checked object (+: merge)
        // Simulates the Window/body pattern: type has body := View{},
        // then derived object does body +: {extra: stuff}
        let Widget = {flow: 1}.freeze_component()
        let Window = Widget{body := Widget{flow: 2}}
        let app = Window{body +: {flow: 3}}
        assert(app.body.flow == 3)

        // Multiple := props, then +: on one of them
        let Base = {
            a := {x: 1}
            b := {x: 2}
            c := {x: 3}
        }
        let derived = Base{b +: {x: 20}}
        assert(derived.a.x == 1)
        assert(derived.b.x == 20)
        assert(derived.c.x == 3)

        // body +: adding children inside the merged object
        let View = {flow: 0, width: 100}.freeze_component()
        let Window2 = View{body := View{flow: 1}}
        let app2 = Window2{body +: {
            flow: 2
            width: 200
        }}
        assert(app2.body.flow == 2)
        assert(app2.body.width == 200)

        // Verify body +: creates a new object (doesn't mutate prototype)
        let Base2 = {x: 1}.freeze_component()
        let Mid = Base2{child := Base2{x: 2}}
        let D1 = Mid{child +: {x: 10}}
        let D2 = Mid{child +: {x: 20}}
        assert(D1.child.x == 10)
        assert(D2.child.x == 20)
        assert(Mid.child.x == 2) // original untouched

        // := property with nested := children, then +: on outer
        let Inner = {val: 0}.freeze_component()
        let Outer = Inner{
            panel := Inner{val: 1}
            sidebar := Inner{val: 2}
        }
        let page = Outer{panel +: {val: 10}}
        assert(page.panel.val == 10)
        assert(page.sidebar.val == 2)

        // Reading a := property that only exists on prototype (not overridden)
        let Proto = {m: 1}.freeze_component()
        let Parent = Proto{child := Proto{m: 5}}
        let Child = Parent{}
        assert(Child.child.m == 5)

        // dot access write through to a := property
        let Base3 = {v: 0}.freeze_component()
        let Obj = Base3{sub := Base3{v: 1}}
        let inst = Obj{}
        inst.sub.v = 99
        assert(inst.sub.v == 99)

        // ============================================================
        // := vec/map storage introspection tests
        // These verify the key bits that widget on_after_apply relies on
        // ============================================================

        // := puts items in vec, : puts items in map
        let obj = {map_a: 1, vec_b := 2, vec_c := 3}
        assert(obj.vec_len() == 2)
        assert(obj.map_len() == 1)

        // vec_key returns an escaped id for the key at given vec index
        ~@vec_b
        ~@vec_c
        assert(@vec_b !== @vec_c)
        ~obj.vec_key(0)
        ~obj.vec_key(1)
        assert(obj.vec_key(0) !== obj.vec_key(1))
        assert(obj.vec_key(0) == @vec_b)
        assert(obj.vec_key(1) == @vec_c)

        // After proto-inherit (+:), vec keys should be preserved
        let Widget2 = {flow: 0}.freeze_component()
        let Win2 = Widget2{body := Widget2{flow: 1}}
        assert(Win2.vec_len() == 1)
        assert(Win2.vec_key(0) == @body)
        let app2 = Win2{body +: {flow: 2}}
        assert(app2.vec_len() == 1)
        assert(app2.vec_key(0) == @body)
        assert(app2.body.flow == 2)

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
        // FOR-LOOP CLOSURE CAPTURE TESTS
        // Each iteration should capture its own copy of the loop variable
        // ============================================================

        // Closure capturing loop index from for-in-range
        let fns = []
        for i in 0..3 {
            fns.push(|| i)
        }
        assert(fns[0]() == 0)
        assert(fns[1]() == 1)
        assert(fns[2]() == 2)

        // Closure capturing loop index from for-in-array
        let fns2 = []
        let arr = [10, 20, 30]
        for i, v in arr {
            fns2.push(|| [i, v])
        }
        assert(fns2[0]() == [0, 10])
        assert(fns2[1]() == [1, 20])
        assert(fns2[2]() == [2, 30])

        // Closure capturing loop variable with mutation after capture
        let fns3 = []
        for i in 0..3 {
            fns3.push(|x| i + x)
        }
        assert(fns3[0](100) == 100)
        assert(fns3[1](100) == 101)
        assert(fns3[2](100) == 102)

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
            // Pattern 16: return from loop
            ret_from_loop: fn(x: f32) -> f32 {
                var i = 0f
                loop {
                    i += 1f
                    if i == x { return i }
                    if i > 100f { break }
                }
                return -1f
            }
            // Pattern 17: return from while
            ret_from_while: fn(x: f32) -> f32 {
                var i = 0f
                while i < 100f {
                    i += 1f
                    if i == x { return i }
                }
                return -1f
            }
            // Pattern 18: loop with continue and accumulation
            loop_continue_sum: fn(n: f32) -> f32 {
                var sum = 0f
                var i = 0f
                loop {
                    i += 1f
                    if i > n { break }
                    if i == 3f { continue }
                    sum += i
                }
                return sum
            }

            // ---- Void return tests (fn() with return inside if) ----
            // Pattern 19: void fn with early return in if (no else)
            void_ret_if: fn(x: f32) {
                if x > 0f { return }
                self.v_intensity = x
            }
            // Pattern 20: void fn with return in if/else
            void_ret_if_else: fn(x: f32) {
                if x > 0f {
                    self.v_intensity = 1f
                    return
                } else {
                    self.v_intensity = 0f
                    return
                }
            }
            // Pattern 21: void fn with multiple early returns
            void_ret_multi: fn(x: f32) {
                if x < -10f { return }
                if x < 0f { return }
                self.v_intensity = x
            }
            // Pattern 22: void fn with || condition and return
            void_ret_or_cond: fn(x: f32, y: f32) {
                if x > 0f || y > 0f {
                    return
                }
                self.v_intensity = x + y
            }
            // Pattern 23: helper fn returning float, called from void context with ||
            check_outside: fn(center: vec2, radius: f32, clip: vec4) -> f32 {
                if radius < 0.5f { return 0f }
                if center.x + radius < clip.x { return 1f }
                if center.y + radius < clip.y { return 1f }
                if center.x - radius > clip.z { return 1f }
                if center.y - radius > clip.w { return 1f }
                return 0f
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

                // ---- Void return test: simplest case ----
                if scale > 0.5f {
                    self.vertex_pos = vec4(0f, 0f, 0f, 0f)
                    return
                }

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

                // ---- Loop / while / break / continue ----
                // Basic loop with break
                var loop_count = 0f
                loop {
                    loop_count += 1f
                    if loop_count >= 5f { break }
                }

                // While loop (uses LOOP + BREAKIFNOT)
                var while_count = 0f
                while while_count < 10f {
                    while_count += 1f
                }

                // Loop with continue
                var cont_sum = 0f
                var cont_i = 0f
                loop {
                    cont_i += 1f
                    if cont_i > 6f { break }
                    if cont_i == 3f { continue }
                    cont_sum += cont_i
                }

                // Nested loops with break
                var outer_count = 0f
                var inner_total = 0f
                loop {
                    outer_count += 1f
                    if outer_count > 3f { break }
                    var inner_count = 0f
                    loop {
                        inner_count += 1f
                        if inner_count > 2f { break }
                        inner_total += 1f
                    }
                }

                // While with early break
                var wb = 0f
                while wb < 100f {
                    wb += 1f
                    if wb == 5f { break }
                }

                // Loop inside for
                var lif = 0f
                for i in 0..3 {
                    var j = 0f
                    loop {
                        j += 1f
                        if j > 2f { break }
                        lif += 1f
                    }
                }

                // For inside loop
                var fil = 0f
                var fil_iter = 0f
                loop {
                    fil_iter += 1f
                    if fil_iter > 2f { break }
                    for i in 0..3 {
                        fil += 1f
                    }
                }

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
                let r16 = self.ret_from_loop(5f)
                let r17 = self.ret_from_while(5f)
                let r18 = self.loop_continue_sum(5f)

                self.pixel = final_col
            }
        }
        shader.test_compile_draw(shader_all_features)

    };

    vm.eval(code);

    let gpu_mb3d_shader_stages = script! {
        use mod.std.println
        use mod.pod.*
        use mod.math.*
        use mod.shader

        println("GPU stage 0: base shader")
        let gpu_stage_0 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)
            sky0: shader.uniform(#x535f73)
            sky1: shader.uniform(#xa8b4c4)

            sky_for_y: fn(y) {
                let t = clamp(pow(1.0 - y, 0.7), 0.0, 1.0)
                return mix(self.sky0.rgb, self.sky1.rgb, t)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = vec4(self.sky_for_y(self.v_uv.y), 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_0)

        println("GPU stage 1: double-single helpers")
        let gpu_stage_1 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            ds_make: fn(v) {
                return vec2(v, 0.0)
            }

            ds_norm: fn(v) {
                let s = v.x + v.y
                let e = v.y - (s - v.x)
                return vec2(s, e)
            }

            ds_add: fn(a, b) {
                let s = a.x + b.x
                let bb = s - a.x
                let e = (a.x - (s - bb)) + (b.x - bb) + a.y + b.y
                return self.ds_norm(vec2(s, e))
            }

            ds_sub: fn(a, b) {
                return self.ds_add(a, vec2(-b.x, -b.y))
            }

            ds_add_f: fn(a, b) {
                return self.ds_add(a, vec2(b, 0.0))
            }

            ds_mul_f: fn(a, b) {
                return self.ds_norm(vec2(a.x * b, a.y * b))
            }

            ds_mul: fn(a, b) {
                let p = a.x * b.x
                let e = a.x * b.y + a.y * b.x + a.y * b.y
                return self.ds_norm(vec2(p, e))
            }

            ds_div: fn(a, b) {
                let q1 = a.x / b.x
                let r = self.ds_sub(a, self.ds_mul_f(b, q1))
                let q2 = r.x / b.x
                return self.ds_norm(vec2(q1, q2))
            }

            ds_abs: fn(a) {
                if a.x < 0.0 || (a.x == 0.0 && a.y < 0.0) {
                    return vec2(-a.x, -a.y)
                }
                return a
            }

            ds_box_fold: fn(a, fold) {
                let plus_abs = self.ds_abs(self.ds_add_f(a, fold))
                let minus_abs = self.ds_abs(self.ds_add_f(a, -fold))
                return self.ds_sub(self.ds_sub(plus_abs, minus_abs), a)
            }

            ds_to_f: fn(a) {
                return a.x + a.y
            }

            ds_sqrt: fn(a) {
                let root = sqrt(max(self.ds_to_f(a), 0.0))
                return vec2(root, 0.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let a = self.ds_make(1.0)
                let b = self.ds_mul(self.ds_add_f(a, 2.0), vec2(3.0, 0.1))
                let c = self.ds_box_fold(b, 1.0)
                let d = self.ds_div(self.ds_sqrt(self.ds_mul(c, c)), vec2(2.0, 0.0))
                let v = clamp(self.ds_to_f(d) * 0.1, 0.0, 1.0)
                self.pixel = vec4(v, 1.0 - v, 0.3, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_1)

        println("GPU stage 1a: scalar helper inference through traced call")
        let gpu_stage_1a = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            ds_make: fn(v) { return vec2(v, 0.0) }
            ds_two_sum: fn(a, b) {
                let s = a + b
                let bb = s - a
                let e = (a - (s - bb)) + (b - bb)
                return vec2(s, e)
            }
            ds_add: fn(a, b) {
                let s = self.ds_two_sum(a.x, b.x)
                return vec2(s.x, s.y + a.y + b.y)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let v = self.ds_add(vec2(1.0, 0.1), vec2(2.0, 0.2))
                self.pixel = vec4(v.x * 0.1, v.y * 0.1 + 0.5, 0.3, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_1a)

        println("GPU stage 1b: nested scalar helper chain")
        let gpu_stage_1b = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            ds_quick_two_sum: fn(a, b) {
                let s = a + b
                let e = b - (s - a)
                return vec2(s, e)
            }
            ds_split: fn(a) {
                let c = 4097.0 * a
                let hi = c - (c - a)
                let lo = a - hi
                return vec2(hi, lo)
            }
            ds_two_prod: fn(a, b) {
                let p = a * b
                let a_split = self.ds_split(a)
                let b_split = self.ds_split(b)
                let e = ((a_split.x * b_split.x - p) + a_split.x * b_split.y + a_split.y * b_split.x) + a_split.y * b_split.y
                return vec2(p, e)
            }
            ds_norm: fn(v) {
                return self.ds_quick_two_sum(v.x, v.y)
            }
            ds_mul: fn(a, b) {
                let p = self.ds_two_prod(a.x, b.x)
                return self.ds_norm(self.ds_quick_two_sum(p.x, p.y + a.x * b.y + a.y * b.x + a.y * b.y))
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let v = self.ds_mul(vec2(1.0, 0.125), vec2(2.0, 0.25))
                self.pixel = vec4(v.x * 0.1, v.y * 0.1 + 0.5, 0.3, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_1b)

        println("GPU stage 2: hybrid_de skeleton")
        let gpu_stage_2 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            rot0: shader.uniform(vec3f)
            rot1: shader.uniform(vec3f)
            rot2: shader.uniform(vec3f)
            slot0_iters: shader.uniform(1.0)
            slot1_iters: shader.uniform(1.0)
            repeat_from_slot: shader.uniform(0.0)
            ab_scale: shader.uniform(-1.0)
            ab_scale_div_min_r2: shader.uniform(-1.0)
            ab_min_r2: shader.uniform(0.25)
            ab_fold: shader.uniform(1.0)
            menger_scale: shader.uniform(3.0)
            menger_cx: shader.uniform(1.0)
            menger_cy: shader.uniform(1.0)
            menger_cz: shader.uniform(0.5)
            rstop: shader.uniform(20.0)
            max_iters: shader.uniform(48.0)

            ds_make: fn(v) { return vec2(v, 0.0) }
            ds_norm: fn(v) {
                let s = v.x + v.y
                let e = v.y - (s - v.x)
                return vec2(s, e)
            }
            ds_add: fn(a, b) {
                let s = a.x + b.x
                let bb = s - a.x
                let e = (a.x - (s - bb)) + (b.x - bb) + a.y + b.y
                return self.ds_norm(vec2(s, e))
            }
            ds_sub: fn(a, b) { return self.ds_add(a, vec2(-b.x, -b.y)) }
            ds_add_f: fn(a, b) { return self.ds_add(a, vec2(b, 0.0)) }
            ds_mul_f: fn(a, b) { return self.ds_norm(vec2(a.x * b, a.y * b)) }
            ds_mul: fn(a, b) {
                let p = a.x * b.x
                let e = a.x * b.y + a.y * b.x + a.y * b.y
                return self.ds_norm(vec2(p, e))
            }
            ds_div: fn(a, b) {
                let q1 = a.x / b.x
                let r = self.ds_sub(a, self.ds_mul_f(b, q1))
                let q2 = r.x / b.x
                return self.ds_norm(vec2(q1, q2))
            }
            ds_abs: fn(a) {
                if a.x < 0.0 || (a.x == 0.0 && a.y < 0.0) {
                    return vec2(-a.x, -a.y)
                }
                return a
            }
            ds_box_fold: fn(a, fold) {
                let plus_abs = self.ds_abs(self.ds_add_f(a, fold))
                let minus_abs = self.ds_abs(self.ds_add_f(a, -fold))
                return self.ds_sub(self.ds_sub(plus_abs, minus_abs), a)
            }
            ds_to_f: fn(a) { return a.x + a.y }
            ds_sqrt: fn(a) {
                let root = sqrt(max(self.ds_to_f(a), 0.0))
                return vec2(root, 0.0)
            }

            hybrid_de: fn(px, py, pz) {
                let cx = px
                let cy = py
                let cz = pz
                var x = px
                var y = py
                var z = pz
                var w = vec2(1.0, 0.0)
                var r2 = vec2(0.0, 0.0)
                var iters = 0.0
                var slot = 0.0
                var remaining = self.slot0_iters

                for i in 0..16 {
                    if remaining <= 0.0 {
                        slot += 1.0
                        if slot >= 2.0 {
                            slot = self.repeat_from_slot
                        }
                        if slot < 0.5 {
                            remaining = self.slot0_iters
                        } else {
                            remaining = self.slot1_iters
                        }
                    }

                    if slot < 0.5 {
                        x = self.ds_box_fold(x, self.ab_fold)
                        y = self.ds_box_fold(y, self.ab_fold)
                        z = self.ds_box_fold(z, self.ab_fold)

                        let rr = self.ds_to_f(self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z)))
                        var m = self.ab_scale
                        if rr < self.ab_min_r2 {
                            m = self.ab_scale_div_min_r2
                        } else if rr < 1.0 {
                            m = self.ab_scale / max(rr, 0.0000001)
                        }
                        w = self.ds_mul_f(w, m)
                        x = self.ds_add(self.ds_mul_f(x, m), cx)
                        y = self.ds_add(self.ds_mul_f(y, m), cy)
                        z = self.ds_add(self.ds_mul_f(z, m), cz)
                    } else {
                        x = self.ds_abs(x)
                        y = self.ds_abs(y)
                        z = self.ds_abs(z)

                        if self.ds_to_f(x) < self.ds_to_f(y) {
                            let t = x
                            x = y
                            y = t
                        }
                        if self.ds_to_f(x) < self.ds_to_f(z) {
                            let t = x
                            x = z
                            z = t
                        }
                        if self.ds_to_f(y) < self.ds_to_f(z) {
                            let t = y
                            y = z
                            z = t
                        }

                        let nx = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot0.x), self.ds_mul_f(y, self.rot0.y)), self.ds_mul_f(z, self.rot0.z))
                        let ny = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot1.x), self.ds_mul_f(y, self.rot1.y)), self.ds_mul_f(z, self.rot1.z))
                        let nz = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot2.x), self.ds_mul_f(y, self.rot2.y)), self.ds_mul_f(z, self.rot2.z))

                        let sf = self.menger_scale - 1.0
                        x = self.ds_add_f(self.ds_mul_f(nx, self.menger_scale), -self.menger_cx * sf)
                        y = self.ds_add_f(self.ds_mul_f(ny, self.menger_scale), -self.menger_cy * sf)
                        let z_scaled = self.ds_mul_f(nz, self.menger_scale)
                        let c = self.menger_cz * sf
                        z = self.ds_add_f(self.ds_abs(self.ds_add_f(z_scaled, -c)), -c)
                        z = vec2(-z.x, -z.y)
                        w = self.ds_mul_f(w, self.menger_scale)
                    }

                    iters += 1.0
                    remaining -= 1.0
                    r2 = self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z))
                    if self.ds_to_f(r2) > self.rstop || iters >= self.max_iters {
                        break
                    }
                }

                let r = self.ds_sqrt(r2)
                let de = self.ds_div(r, self.ds_abs(w))
                return vec3(iters, de.x, de.y)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let h = self.hybrid_de(vec2(0.1, 0.0), vec2(0.2, 0.0), vec2(0.3, 0.0))
                self.pixel = vec4(h.x * 0.02, clamp(h.y, 0.0, 1.0), clamp(h.z + 0.5, 0.0, 1.0), 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_2)

        println("GPU stage 2a: loop no early return")
        let gpu_stage_2a = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            loop_probe: fn() {
                var t = 0.0
                for i in 0..16 {
                    t += 1.0
                }
                return vec2(t, 0.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let hit = self.loop_probe()
                self.pixel = vec4(hit.x * 0.05, 0.2, 0.3, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_2a)

        println("GPU stage 2b: loop with direct early return")
        let gpu_stage_2b = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            loop_probe: fn() {
                var t = 0.0
                for i in 0..16 {
                    if t > 3.0 {
                        return vec2(t, 1.0)
                    }
                    t += 1.0
                }
                return vec2(-1.0, 0.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let hit = self.loop_probe()
                self.pixel = vec4(hit.x * 0.05, hit.y * 0.5, 0.3, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_2b)

        println("GPU stage 2c: loop with if/else early return")
        let gpu_stage_2c = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            loop_probe: fn() {
                var t = 0.0
                for i in 0..16 {
                    if t < 3.0 {
                        t += 1.0
                    } else {
                        return vec2(t, 1.0)
                    }
                }
                return vec2(-1.0, 0.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let hit = self.loop_probe()
                self.pixel = vec4(hit.x * 0.05, hit.y * 0.5, 0.3, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_2c)

        println("GPU stage 2d: calc_de inside loop")
        let gpu_stage_2d = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            rot0: shader.uniform(vec3f)
            rot1: shader.uniform(vec3f)
            rot2: shader.uniform(vec3f)
            slot0_iters: shader.uniform(1.0)
            slot1_iters: shader.uniform(1.0)
            repeat_from_slot: shader.uniform(0.0)
            ab_scale: shader.uniform(-1.0)
            ab_scale_div_min_r2: shader.uniform(-1.0)
            ab_min_r2: shader.uniform(0.25)
            ab_fold: shader.uniform(1.0)
            menger_scale: shader.uniform(3.0)
            menger_cx: shader.uniform(1.0)
            menger_cy: shader.uniform(1.0)
            menger_cz: shader.uniform(0.5)
            rstop: shader.uniform(20.0)
            max_iters: shader.uniform(48.0)
            de_floor: shader.uniform(0.00025)
            de_stop: shader.uniform(0.001)

            ds_make: fn(v) { return vec2(v, 0.0) }
            ds_norm: fn(v) {
                let s = v.x + v.y
                let e = v.y - (s - v.x)
                return vec2(s, e)
            }
            ds_add: fn(a, b) {
                let s = a.x + b.x
                let bb = s - a.x
                let e = (a.x - (s - bb)) + (b.x - bb) + a.y + b.y
                return self.ds_norm(vec2(s, e))
            }
            ds_sub: fn(a, b) { return self.ds_add(a, vec2(-b.x, -b.y)) }
            ds_add_f: fn(a, b) { return self.ds_add(a, vec2(b, 0.0)) }
            ds_mul_f: fn(a, b) { return self.ds_norm(vec2(a.x * b, a.y * b)) }
            ds_mul: fn(a, b) {
                let p = a.x * b.x
                let e = a.x * b.y + a.y * b.x + a.y * b.y
                return self.ds_norm(vec2(p, e))
            }
            ds_div: fn(a, b) {
                let q1 = a.x / b.x
                let r = self.ds_sub(a, self.ds_mul_f(b, q1))
                let q2 = r.x / b.x
                return self.ds_norm(vec2(q1, q2))
            }
            ds_abs: fn(a) {
                if a.x < 0.0 || (a.x == 0.0 && a.y < 0.0) { return vec2(-a.x, -a.y) }
                return a
            }
            ds_box_fold: fn(a, fold) {
                let plus_abs = self.ds_abs(self.ds_add_f(a, fold))
                let minus_abs = self.ds_abs(self.ds_add_f(a, -fold))
                return self.ds_sub(self.ds_sub(plus_abs, minus_abs), a)
            }
            ds_to_f: fn(a) { return a.x + a.y }
            ds_sqrt: fn(a) {
                let root = sqrt(max(self.ds_to_f(a), 0.0))
                return vec2(root, 0.0)
            }

            hybrid_de: fn(px, py, pz) {
                let cx = px
                let cy = py
                let cz = pz
                var x = px
                var y = py
                var z = pz
                var w = vec2(1.0, 0.0)
                var r2 = vec2(0.0, 0.0)
                var iters = 0.0
                var slot = 0.0
                var remaining = self.slot0_iters
                for i in 0..16 {
                    if remaining <= 0.0 {
                        slot += 1.0
                        if slot >= 2.0 { slot = self.repeat_from_slot }
                        if slot < 0.5 { remaining = self.slot0_iters } else { remaining = self.slot1_iters }
                    }
                    if slot < 0.5 {
                        x = self.ds_box_fold(x, self.ab_fold)
                        y = self.ds_box_fold(y, self.ab_fold)
                        z = self.ds_box_fold(z, self.ab_fold)
                        let rr = self.ds_to_f(self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z)))
                        var m = self.ab_scale
                        if rr < self.ab_min_r2 { m = self.ab_scale_div_min_r2 } else if rr < 1.0 { m = self.ab_scale / max(rr, 0.0000001) }
                        w = self.ds_mul_f(w, m)
                        x = self.ds_add(self.ds_mul_f(x, m), cx)
                        y = self.ds_add(self.ds_mul_f(y, m), cy)
                        z = self.ds_add(self.ds_mul_f(z, m), cz)
                    } else {
                        x = self.ds_abs(x)
                        y = self.ds_abs(y)
                        z = self.ds_abs(z)
                        if self.ds_to_f(x) < self.ds_to_f(y) { let t = x x = y y = t }
                        if self.ds_to_f(x) < self.ds_to_f(z) { let t = x x = z z = t }
                        if self.ds_to_f(y) < self.ds_to_f(z) { let t = y y = z z = t }
                        let nx = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot0.x), self.ds_mul_f(y, self.rot0.y)), self.ds_mul_f(z, self.rot0.z))
                        let ny = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot1.x), self.ds_mul_f(y, self.rot1.y)), self.ds_mul_f(z, self.rot1.z))
                        let nz = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot2.x), self.ds_mul_f(y, self.rot2.y)), self.ds_mul_f(z, self.rot2.z))
                        let sf = self.menger_scale - 1.0
                        x = self.ds_add_f(self.ds_mul_f(nx, self.menger_scale), -self.menger_cx * sf)
                        y = self.ds_add_f(self.ds_mul_f(ny, self.menger_scale), -self.menger_cy * sf)
                        let z_scaled = self.ds_mul_f(nz, self.menger_scale)
                        let c = self.menger_cz * sf
                        z = self.ds_add_f(self.ds_abs(self.ds_add_f(z_scaled, -c)), -c)
                        z = vec2(-z.x, -z.y)
                        w = self.ds_mul_f(w, self.menger_scale)
                    }
                    iters += 1.0
                    remaining -= 1.0
                    r2 = self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z))
                    if self.ds_to_f(r2) > self.rstop || iters >= self.max_iters { break }
                }
                let r = self.ds_sqrt(r2)
                let de = self.ds_div(r, self.ds_abs(w))
                return vec3(iters, de.x, de.y)
            }

            calc_de: fn(px, py, pz) {
                let raw = self.hybrid_de(px, py, pz)
                let de_raw = max(raw.y + raw.z, self.de_floor)
                return vec2(raw.x, de_raw)
            }

            march_probe: fn(px, py, pz) {
                var t = 0.0
                let first_eval = self.calc_de(px, py, pz)
                if first_eval.x >= self.max_iters || first_eval.y < self.de_stop {
                    return vec2(0.0, first_eval.x)
                }
                for i in 0..16 {
                    let eval = self.calc_de(px, py, pz)
                    var de = eval.y
                    if de > 1.0 {
                        de = 1.0
                    }
                    if eval.x < self.max_iters && de >= self.de_stop {
                        t += 1.0
                    } else {
                        return vec2(t, eval.x)
                    }
                }
                return vec2(-1.0, 0.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let hit = self.march_probe(vec2(0.1, 0.0), vec2(0.2, 0.0), vec2(0.3, 0.0))
                self.pixel = vec4(hit.x * 0.05, hit.y * 0.05, 0.3, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_2d)

        println("GPU stage 2e: loop statement-only if with nested return")
        let gpu_stage_2e = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)
            limit: shader.uniform(8.0)

            march_probe: fn() {
                var t = 0.0
                for i in 0..16 {
                    if t < self.limit {
                        t += 1.0
                        if t > 32.0 {
                            return vec2(-1.0, 0.0)
                        }
                    } else {
                        return vec2(t, 1.0)
                    }
                }
                return vec2(-1.0, 0.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let hit = self.march_probe()
                self.pixel = vec4(clamp(hit.x * 0.1, 0.0, 1.0), clamp(hit.y * 0.1, 0.0, 1.0), 0.3, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_2e)

        println("GPU stage 3: ray march main loop")
        let gpu_stage_3 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            rot0: shader.uniform(vec3f)
            rot1: shader.uniform(vec3f)
            rot2: shader.uniform(vec3f)
            slot0_iters: shader.uniform(1.0)
            slot1_iters: shader.uniform(1.0)
            repeat_from_slot: shader.uniform(0.0)
            ab_scale: shader.uniform(-1.0)
            ab_scale_div_min_r2: shader.uniform(-1.0)
            ab_min_r2: shader.uniform(0.25)
            ab_fold: shader.uniform(1.0)
            menger_scale: shader.uniform(3.0)
            menger_cx: shader.uniform(1.0)
            menger_cy: shader.uniform(1.0)
            menger_cz: shader.uniform(0.5)
            rstop: shader.uniform(20.0)
            max_iters: shader.uniform(48.0)
            step_width: shader.uniform(0.001)
            de_stop: shader.uniform(0.001)
            de_stop_factor: shader.uniform(0.0)
            s_z_step_div: shader.uniform(1.0)
            ms_de_sub: shader.uniform(1.0)
            mct_mh04_zsd: shader.uniform(1.0)
            de_floor: shader.uniform(0.00025)
            max_ray_length: shader.uniform(128.0)

            ds_make: fn(v) { return vec2(v, 0.0) }
            ds_norm: fn(v) {
                let s = v.x + v.y
                let e = v.y - (s - v.x)
                return vec2(s, e)
            }
            ds_add: fn(a, b) {
                let s = a.x + b.x
                let bb = s - a.x
                let e = (a.x - (s - bb)) + (b.x - bb) + a.y + b.y
                return self.ds_norm(vec2(s, e))
            }
            ds_sub: fn(a, b) { return self.ds_add(a, vec2(-b.x, -b.y)) }
            ds_add_f: fn(a, b) { return self.ds_add(a, vec2(b, 0.0)) }
            ds_mul_f: fn(a, b) { return self.ds_norm(vec2(a.x * b, a.y * b)) }
            ds_mul: fn(a, b) {
                let p = a.x * b.x
                let e = a.x * b.y + a.y * b.x + a.y * b.y
                return self.ds_norm(vec2(p, e))
            }
            ds_div: fn(a, b) {
                let q1 = a.x / b.x
                let r = self.ds_sub(a, self.ds_mul_f(b, q1))
                let q2 = r.x / b.x
                return self.ds_norm(vec2(q1, q2))
            }
            ds_abs: fn(a) {
                if a.x < 0.0 || (a.x == 0.0 && a.y < 0.0) {
                    return vec2(-a.x, -a.y)
                }
                return a
            }
            ds_box_fold: fn(a, fold) {
                let plus_abs = self.ds_abs(self.ds_add_f(a, fold))
                let minus_abs = self.ds_abs(self.ds_add_f(a, -fold))
                return self.ds_sub(self.ds_sub(plus_abs, minus_abs), a)
            }
            ds_to_f: fn(a) { return a.x + a.y }
            ds_sqrt: fn(a) {
                let root = sqrt(max(self.ds_to_f(a), 0.0))
                return vec2(root, 0.0)
            }

            hybrid_de: fn(px, py, pz) {
                let cx = px
                let cy = py
                let cz = pz
                var x = px
                var y = py
                var z = pz
                var w = vec2(1.0, 0.0)
                var r2 = vec2(0.0, 0.0)
                var iters = 0.0
                var slot = 0.0
                var remaining = self.slot0_iters
                for i in 0..16 {
                    if remaining <= 0.0 {
                        slot += 1.0
                        if slot >= 2.0 { slot = self.repeat_from_slot }
                        if slot < 0.5 { remaining = self.slot0_iters } else { remaining = self.slot1_iters }
                    }
                    if slot < 0.5 {
                        x = self.ds_box_fold(x, self.ab_fold)
                        y = self.ds_box_fold(y, self.ab_fold)
                        z = self.ds_box_fold(z, self.ab_fold)
                        let rr = self.ds_to_f(self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z)))
                        var m = self.ab_scale
                        if rr < self.ab_min_r2 { m = self.ab_scale_div_min_r2 } else if rr < 1.0 { m = self.ab_scale / max(rr, 0.0000001) }
                        w = self.ds_mul_f(w, m)
                        x = self.ds_add(self.ds_mul_f(x, m), cx)
                        y = self.ds_add(self.ds_mul_f(y, m), cy)
                        z = self.ds_add(self.ds_mul_f(z, m), cz)
                    } else {
                        x = self.ds_abs(x)
                        y = self.ds_abs(y)
                        z = self.ds_abs(z)
                        if self.ds_to_f(x) < self.ds_to_f(y) { let t = x x = y y = t }
                        if self.ds_to_f(x) < self.ds_to_f(z) { let t = x x = z z = t }
                        if self.ds_to_f(y) < self.ds_to_f(z) { let t = y y = z z = t }
                        let nx = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot0.x), self.ds_mul_f(y, self.rot0.y)), self.ds_mul_f(z, self.rot0.z))
                        let ny = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot1.x), self.ds_mul_f(y, self.rot1.y)), self.ds_mul_f(z, self.rot1.z))
                        let nz = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot2.x), self.ds_mul_f(y, self.rot2.y)), self.ds_mul_f(z, self.rot2.z))
                        let sf = self.menger_scale - 1.0
                        x = self.ds_add_f(self.ds_mul_f(nx, self.menger_scale), -self.menger_cx * sf)
                        y = self.ds_add_f(self.ds_mul_f(ny, self.menger_scale), -self.menger_cy * sf)
                        let z_scaled = self.ds_mul_f(nz, self.menger_scale)
                        let c = self.menger_cz * sf
                        z = self.ds_add_f(self.ds_abs(self.ds_add_f(z_scaled, -c)), -c)
                        z = vec2(-z.x, -z.y)
                        w = self.ds_mul_f(w, self.menger_scale)
                    }
                    iters += 1.0
                    remaining -= 1.0
                    r2 = self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z))
                    if self.ds_to_f(r2) > self.rstop || iters >= self.max_iters { break }
                }
                let r = self.ds_sqrt(r2)
                let de = self.ds_div(r, self.ds_abs(w))
                return vec3(iters, de.x, de.y)
            }

            calc_de: fn(px, py, pz) {
                let raw = self.hybrid_de(px, py, pz)
                let de_raw = max(raw.y + raw.z, self.de_floor)
                return vec2(raw.x, de_raw)
            }

            pos_x: fn(ox, dir, t) { return self.ds_add(ox, self.ds_mul_f(self.ds_make(t), dir.x)) }
            pos_y: fn(oy, dir, t) { return self.ds_add(oy, self.ds_mul_f(self.ds_make(t), dir.y)) }
            pos_z: fn(oz, dir, t) { return self.ds_add(oz, self.ds_mul_f(self.ds_make(t), dir.z)) }

            ray_march: fn(ox, oy, oz, dir) {
                var t = 0.0
                var last_de = 0.0
                var last_step = 0.0
                var rsfmul = 1.0

                let first_eval = self.calc_de(ox, oy, oz)
                let first_destop = self.de_stop
                if first_eval.x >= self.max_iters || first_eval.y < first_destop {
                    return vec2(0.0, first_eval.x)
                }

                last_de = first_eval.y
                last_step = max(first_eval.y * self.s_z_step_div, 0.11 * self.step_width)

                for step_idx in 0..16 {
                    let depth_steps = abs(t) / max(self.step_width, 0.0000001)
                    let current_destop = self.de_stop * (1.0 + depth_steps * self.de_stop_factor)
                    let px = self.pos_x(ox, dir, t)
                    let py = self.pos_y(oy, dir, t)
                    let pz = self.pos_z(oz, dir, t)
                    let eval = self.calc_de(px, py, pz)
                    var de = eval.y
                    if de > last_de + last_step {
                        de = last_de + last_step
                    }
                    if eval.x < self.max_iters && de >= current_destop {
                        var step = max((de - self.ms_de_sub * current_destop) * self.s_z_step_div * rsfmul, 0.11 * self.step_width)
                        let max_step_here = max(current_destop, 0.4 * self.step_width) * self.mct_mh04_zsd
                        if max_step_here < step {
                            step = max_step_here
                        }
                        if last_de > de + 0.0000001 {
                            let ratio = last_step / max(last_de - de, 0.0000001)
                            if ratio < 1.0 { rsfmul = max(ratio, 0.5) } else { rsfmul = 1.0 }
                        } else {
                            rsfmul = 1.0
                        }
                        last_de = de
                        last_step = step
                        t += step
                        if t > self.max_ray_length {
                            return vec2(-1.0, 0.0)
                        }
                    } else {
                        return vec2(t, eval.x)
                    }
                }
                return vec2(-1.0, 0.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let hit = self.ray_march(vec2(0.1, 0.0), vec2(0.2, 0.0), vec2(0.3, 0.0), vec3(0.0, 0.0, 1.0))
                self.pixel = vec4(clamp(hit.x * 0.1, 0.0, 1.0), clamp(hit.y * 0.02, 0.0, 1.0), 0.4, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_3)

        println("GPU stage 4: ray march refinement loop")
        let gpu_stage_4 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            rot0: shader.uniform(vec3f)
            rot1: shader.uniform(vec3f)
            rot2: shader.uniform(vec3f)
            slot0_iters: shader.uniform(1.0)
            slot1_iters: shader.uniform(1.0)
            repeat_from_slot: shader.uniform(0.0)
            ab_scale: shader.uniform(-1.0)
            ab_scale_div_min_r2: shader.uniform(-1.0)
            ab_min_r2: shader.uniform(0.25)
            ab_fold: shader.uniform(1.0)
            menger_scale: shader.uniform(3.0)
            menger_cx: shader.uniform(1.0)
            menger_cy: shader.uniform(1.0)
            menger_cz: shader.uniform(0.5)
            rstop: shader.uniform(20.0)
            max_iters: shader.uniform(48.0)
            step_width: shader.uniform(0.001)
            de_stop: shader.uniform(0.001)
            de_stop_factor: shader.uniform(0.0)
            s_z_step_div: shader.uniform(1.0)
            ms_de_sub: shader.uniform(1.0)
            mct_mh04_zsd: shader.uniform(1.0)
            de_floor: shader.uniform(0.00025)
            max_ray_length: shader.uniform(128.0)

            ds_make: fn(v) { return vec2(v, 0.0) }
            ds_norm: fn(v) {
                let s = v.x + v.y
                let e = v.y - (s - v.x)
                return vec2(s, e)
            }
            ds_add: fn(a, b) {
                let s = a.x + b.x
                let bb = s - a.x
                let e = (a.x - (s - bb)) + (b.x - bb) + a.y + b.y
                return self.ds_norm(vec2(s, e))
            }
            ds_sub: fn(a, b) { return self.ds_add(a, vec2(-b.x, -b.y)) }
            ds_add_f: fn(a, b) { return self.ds_add(a, vec2(b, 0.0)) }
            ds_mul_f: fn(a, b) { return self.ds_norm(vec2(a.x * b, a.y * b)) }
            ds_mul: fn(a, b) {
                let p = a.x * b.x
                let e = a.x * b.y + a.y * b.x + a.y * b.y
                return self.ds_norm(vec2(p, e))
            }
            ds_div: fn(a, b) {
                let q1 = a.x / b.x
                let r = self.ds_sub(a, self.ds_mul_f(b, q1))
                let q2 = r.x / b.x
                return self.ds_norm(vec2(q1, q2))
            }
            ds_abs: fn(a) {
                if a.x < 0.0 || (a.x == 0.0 && a.y < 0.0) { return vec2(-a.x, -a.y) }
                return a
            }
            ds_box_fold: fn(a, fold) {
                let plus_abs = self.ds_abs(self.ds_add_f(a, fold))
                let minus_abs = self.ds_abs(self.ds_add_f(a, -fold))
                return self.ds_sub(self.ds_sub(plus_abs, minus_abs), a)
            }
            ds_to_f: fn(a) { return a.x + a.y }
            ds_sqrt: fn(a) {
                let root = sqrt(max(self.ds_to_f(a), 0.0))
                return vec2(root, 0.0)
            }
            hybrid_de: fn(px, py, pz) {
                let cx = px
                let cy = py
                let cz = pz
                var x = px
                var y = py
                var z = pz
                var w = vec2(1.0, 0.0)
                var r2 = vec2(0.0, 0.0)
                var iters = 0.0
                var slot = 0.0
                var remaining = self.slot0_iters
                for i in 0..16 {
                    if remaining <= 0.0 {
                        slot += 1.0
                        if slot >= 2.0 { slot = self.repeat_from_slot }
                        if slot < 0.5 { remaining = self.slot0_iters } else { remaining = self.slot1_iters }
                    }
                    if slot < 0.5 {
                        x = self.ds_box_fold(x, self.ab_fold)
                        y = self.ds_box_fold(y, self.ab_fold)
                        z = self.ds_box_fold(z, self.ab_fold)
                        let rr = self.ds_to_f(self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z)))
                        var m = self.ab_scale
                        if rr < self.ab_min_r2 { m = self.ab_scale_div_min_r2 } else if rr < 1.0 { m = self.ab_scale / max(rr, 0.0000001) }
                        w = self.ds_mul_f(w, m)
                        x = self.ds_add(self.ds_mul_f(x, m), cx)
                        y = self.ds_add(self.ds_mul_f(y, m), cy)
                        z = self.ds_add(self.ds_mul_f(z, m), cz)
                    } else {
                        x = self.ds_abs(x)
                        y = self.ds_abs(y)
                        z = self.ds_abs(z)
                        if self.ds_to_f(x) < self.ds_to_f(y) { let t = x x = y y = t }
                        if self.ds_to_f(x) < self.ds_to_f(z) { let t = x x = z z = t }
                        if self.ds_to_f(y) < self.ds_to_f(z) { let t = y y = z z = t }
                        let nx = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot0.x), self.ds_mul_f(y, self.rot0.y)), self.ds_mul_f(z, self.rot0.z))
                        let ny = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot1.x), self.ds_mul_f(y, self.rot1.y)), self.ds_mul_f(z, self.rot1.z))
                        let nz = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot2.x), self.ds_mul_f(y, self.rot2.y)), self.ds_mul_f(z, self.rot2.z))
                        let sf = self.menger_scale - 1.0
                        x = self.ds_add_f(self.ds_mul_f(nx, self.menger_scale), -self.menger_cx * sf)
                        y = self.ds_add_f(self.ds_mul_f(ny, self.menger_scale), -self.menger_cy * sf)
                        let z_scaled = self.ds_mul_f(nz, self.menger_scale)
                        let c = self.menger_cz * sf
                        z = self.ds_add_f(self.ds_abs(self.ds_add_f(z_scaled, -c)), -c)
                        z = vec2(-z.x, -z.y)
                        w = self.ds_mul_f(w, self.menger_scale)
                    }
                    iters += 1.0
                    remaining -= 1.0
                    r2 = self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z))
                    if self.ds_to_f(r2) > self.rstop || iters >= self.max_iters { break }
                }
                let r = self.ds_sqrt(r2)
                let de = self.ds_div(r, self.ds_abs(w))
                return vec3(iters, de.x, de.y)
            }
            calc_de: fn(px, py, pz) {
                let raw = self.hybrid_de(px, py, pz)
                let de_raw = max(raw.y + raw.z, self.de_floor)
                return vec2(raw.x, de_raw)
            }
            pos_x: fn(ox, dir, t) { return self.ds_add(ox, self.ds_mul_f(self.ds_make(t), dir.x)) }
            pos_y: fn(oy, dir, t) { return self.ds_add(oy, self.ds_mul_f(self.ds_make(t), dir.y)) }
            pos_z: fn(oz, dir, t) { return self.ds_add(oz, self.ds_mul_f(self.ds_make(t), dir.z)) }

            ray_march: fn(ox, oy, oz, dir) {
                var t = 0.0
                var last_de = 0.0
                var last_step = 0.0
                var rsfmul = 1.0
                let first_eval = self.calc_de(ox, oy, oz)
                let first_destop = self.de_stop
                if first_eval.x >= self.max_iters || first_eval.y < first_destop {
                    return vec2(0.0, first_eval.x)
                }
                last_de = first_eval.y
                last_step = max(first_eval.y * self.s_z_step_div, 0.11 * self.step_width)
                for step_idx in 0..16 {
                    let depth_steps = abs(t) / max(self.step_width, 0.0000001)
                    let current_destop = self.de_stop * (1.0 + depth_steps * self.de_stop_factor)
                    let px = self.pos_x(ox, dir, t)
                    let py = self.pos_y(oy, dir, t)
                    let pz = self.pos_z(oz, dir, t)
                    let eval = self.calc_de(px, py, pz)
                    var de = eval.y
                    if de > last_de + last_step { de = last_de + last_step }
                    if eval.x < self.max_iters && de >= current_destop {
                        var step = max((de - self.ms_de_sub * current_destop) * self.s_z_step_div * rsfmul, 0.11 * self.step_width)
                        let max_step_here = max(current_destop, 0.4 * self.step_width) * self.mct_mh04_zsd
                        if max_step_here < step { step = max_step_here }
                        if last_de > de + 0.0000001 {
                            let ratio = last_step / max(last_de - de, 0.0000001)
                            if ratio < 1.0 { rsfmul = max(ratio, 0.5) } else { rsfmul = 1.0 }
                        } else {
                            rsfmul = 1.0
                        }
                        last_de = de
                        last_step = step
                        t += step
                        if t > self.max_ray_length { return vec2(-1.0, 0.0) }
                    } else {
                        var refine_t = t
                        var refine_step = -0.5 * last_step
                        for i in 0..8 {
                            refine_t += refine_step
                            let rx = self.pos_x(ox, dir, refine_t)
                            let ry = self.pos_y(oy, dir, refine_t)
                            let rz = self.pos_z(oz, dir, refine_t)
                            let depth_steps = abs(refine_t) / max(self.step_width, 0.0000001)
                            let stop_here = self.de_stop * (1.0 + depth_steps * self.de_stop_factor)
                            let reval = self.calc_de(rx, ry, rz)
                            if reval.x >= self.max_iters || reval.y < stop_here {
                                refine_step = -abs(refine_step) * 0.55
                            } else {
                                refine_step = abs(refine_step) * 0.55
                            }
                        }
                        let fx = self.pos_x(ox, dir, refine_t)
                        let fy = self.pos_y(oy, dir, refine_t)
                        let fz = self.pos_z(oz, dir, refine_t)
                        let final_eval = self.calc_de(fx, fy, fz)
                        return vec2(refine_t, final_eval.x)
                    }
                }
                return vec2(-1.0, 0.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let hit = self.ray_march(vec2(0.1, 0.0), vec2(0.2, 0.0), vec2(0.3, 0.0), vec3(0.0, 0.0, 1.0))
                self.pixel = vec4(clamp(hit.x * 0.1, 0.0, 1.0), clamp(hit.y * 0.02, 0.0, 1.0), 0.4, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4)

        println("GPU stage 4a: vec2 if-expression")
        let gpu_stage_4a = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)
            sel: shader.uniform(0.0)
            uv0: shader.uniform(vec2f)

            choose: fn() {
                let cx = if self.sel > 0.5 { self.uv0 } else { vec2(0.25, 0.75) }
                return cx
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let c = self.choose()
                self.pixel = vec4(c.x, c.y, 0.0, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4a)

        println("GPU stage 4a2: vec2 if-expression from uniform buffer")
        let gpu_stage_4a2_uniforms = struct{
            pick: vec2f,
        }
        let gpu_stage_4a2 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)
            sel: shader.uniform(0.0)
            ubuf: shader.uniform_buffer(gpu_stage_4a2_uniforms)

            choose: fn(px) {
                let cx = if self.sel > 0.5 { self.ubuf.pick } else { px }
                return cx
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let c = self.choose(vec2(0.25, 0.75))
                self.pixel = vec4(c.x, c.y, 0.0, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4a2)

        println("GPU stage 4a3: vec2 if-expression from Rust POD uniform buffer")
        let gpu_stage_4a3_uniforms = #(rust_uniform_buffer_test_pod(vm))
        let gpu_stage_4a3 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)
            sel: shader.uniform(0.0)
            ubuf: shader.uniform_buffer(gpu_stage_4a3_uniforms)

            choose: fn(px) {
                let cx = if self.sel > 0.5 { self.ubuf.pick } else { px }
                return cx
            }

            ds_quick_two_sum: fn(a, b) {
                let s = a + b
                let e = b - (s - a)
                return vec2(s, e)
            }

            tweak: fn(v) {
                let s = self.ds_quick_two_sum(v.x, self.ubuf.scale.x)
                return vec2(s.x, s.y)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let c = self.choose(vec2(0.25, 0.75))
                let t = self.tweak(c)
                self.pixel = vec4(t.x, t.y, 0.0, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4a3)

        println("GPU stage 4b: vec2 var reassignment in nested loop")
        let gpu_stage_4b = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            loop_probe: fn() {
                var refine_t = vec2(0.0, 0.0)
                var refine_step = vec2(-0.5, 0.125)
                for i in 0..8 {
                    refine_t = refine_t + refine_step
                    if refine_t.x < 0.0 {
                        refine_step = vec2(-abs(refine_step.x), -abs(refine_step.y)) * 0.55
                    } else {
                        refine_step = vec2(abs(refine_step.x), abs(refine_step.y)) * 0.55
                    }
                }
                return refine_t
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let c = self.loop_probe()
                self.pixel = vec4(c.x, c.y, 0.0, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4b)

        println("GPU stage 4c: branch-local var in nested loop")
        let gpu_stage_4c = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            probe: fn(sel) {
                if sel > 0.5 {
                    return vec2(1.0, 0.0)
                } else {
                    var refine_t = vec2(0.0, 0.0)
                    var refine_step = vec2(-0.5, 0.125)
                    for i in 0..8 {
                        refine_t = refine_t + refine_step
                        if refine_t.x < 0.0 {
                            refine_step = vec2(-abs(refine_step.x), -abs(refine_step.y)) * 0.55
                        } else {
                            refine_step = vec2(abs(refine_step.x), abs(refine_step.y)) * 0.55
                        }
                    }
                    return refine_t
                }
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let c = self.probe(0.0)
                self.pixel = vec4(c.x, c.y, 0.0, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4c)

        println("GPU stage 4d: branch-local var with nested helper calls")
        let gpu_stage_4d = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            ds_new: fn(v) { return vec2(v, 0.0) }
            ds_add: fn(a, b) { return vec2(a.x + b.x, a.y + b.y) }
            ds_abs: fn(a) {
                if a.x < 0.0 || (a.x == 0.0 && a.y < 0.0) {
                    return vec2(-a.x, -a.y)
                }
                return a
            }
            ds_mul: fn(a, b) {
                return vec2(a.x * b.x, a.y * b.x + a.x * b.y)
            }

            probe: fn(sel) {
                if sel > 0.5 {
                    return vec2(1.0, 0.0)
                } else {
                    var last_step = vec2(1.0, 0.125)
                    var refine_t = vec2(0.0, 0.0)
                    var refine_step = self.ds_mul(last_step, self.ds_new(-0.5))
                    for i in 0..8 {
                        refine_t = self.ds_add(refine_t, refine_step)
                        if refine_t.x < 0.0 {
                            refine_step = self.ds_mul(self.ds_abs(refine_step), self.ds_new(-0.55))
                        } else {
                            refine_step = self.ds_mul(self.ds_abs(refine_step), self.ds_new(0.55))
                        }
                    }
                    return refine_t
                }
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let c = self.probe(0.0)
                self.pixel = vec4(c.x, c.y, 0.0, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4d)

        println("GPU stage 4e: outer var captured by else-local helper init")
        let gpu_stage_4e = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            ds_new: fn(v) { return vec2(v, 0.0) }
            ds_add: fn(a, b) { return vec2(a.x + b.x, a.y + b.y) }
            ds_abs: fn(a) {
                if a.x < 0.0 || (a.x == 0.0 && a.y < 0.0) {
                    return vec2(-a.x, -a.y)
                }
                return a
            }
            ds_mul: fn(a, b) {
                return vec2(a.x * b.x, a.y * b.x + a.x * b.y)
            }

            probe: fn(sel) {
                var last_step = vec2(1.0, 0.125)
                var t = vec2(0.0, 0.0)
                for i in 0..2 {
                    if sel > 0.5 {
                        last_step = vec2(0.25, 0.0625)
                    } else {
                        var refine_step = self.ds_mul(last_step, self.ds_new(-0.5))
                        for j in 0..4 {
                            t = self.ds_add(t, refine_step)
                            if t.x < 0.0 {
                                refine_step = self.ds_mul(self.ds_abs(refine_step), self.ds_new(-0.55))
                            } else {
                                refine_step = self.ds_mul(self.ds_abs(refine_step), self.ds_new(0.55))
                            }
                        }
                    }
                }
                return t
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                let c = self.probe(0.0)
                self.pixel = vec4(c.x, c.y, 0.0, 1.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4e)

        println("GPU stage 4f: branch return reads outer vec2 var fields")
        let gpu_stage_4f = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            ds_add: fn(a, b) { return vec2(a.x + b.x, a.y + b.y) }

            probe: fn(sel) {
                var t = vec2(0.0, 0.0)
                let step = vec2(1.0, 0.125)
                for i in 0..2 {
                    if sel > 0.5 {
                        t = self.ds_add(t, step)
                    } else {
                        return vec4(t.x, t.y, 0.0, 1.0)
                    }
                }
                return vec4(0.0, 0.0, 0.0, 1.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = self.probe(0.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4f)

        println("GPU stage 4f2: unary not on helper call inside if")
        let gpu_stage_4f2 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            is_small: fn(v) {
                return v < 0.5
            }

            probe: fn(sel) {
                if !self.is_small(sel) {
                    return vec4(1.0, 0.0, 0.0, 1.0)
                } else {
                    return vec4(0.0, 1.0, 0.0, 1.0)
                }
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = self.probe(0.25)
            }
        }
        shader.test_compile_draw(gpu_stage_4f2)

        println("GPU stage 4f3: loop bool-and before else vec4 return")
        let gpu_stage_4f3 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            probe: fn(sel) {
                var t = vec2(0.0, 0.0)
                for i in 0..2 {
                    if sel > 0.5 && sel < 1.0 {
                        t = vec2(1.0, 0.25)
                    } else {
                        return vec4(t.x, t.y, 0.0, 1.0)
                    }
                }
                return vec4(0.0, 0.0, 0.0, 1.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = self.probe(0.25)
            }
        }
        shader.test_compile_draw(gpu_stage_4f3)

        println("GPU stage 4f4: loop bool-and helper call before else vec4 return")
        let gpu_stage_4f4 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            is_small: fn(v) {
                return v < 1.0
            }

            probe: fn(sel) {
                var t = vec2(0.0, 0.0)
                for i in 0..2 {
                    if sel > 0.5 && self.is_small(sel) {
                        t = vec2(1.0, 0.25)
                    } else {
                        return vec4(t.x, t.y, 0.0, 1.0)
                    }
                }
                return vec4(0.0, 0.0, 0.0, 1.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = self.probe(0.25)
            }
        }
        shader.test_compile_draw(gpu_stage_4f4)

        println("GPU stage 4f5: prior if plus bool-and before else vec4 return")
        let gpu_stage_4f5 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            is_small: fn(v) {
                return v < 1.0
            }

            probe: fn(sel) {
                var t = vec2(0.0, 0.0)
                var de = 0.0
                for i in 0..2 {
                    if de > 1.0 {
                        de = 1.0
                    }
                    if sel > 0.5 && self.is_small(sel) {
                        t = vec2(1.0, 0.25)
                    } else {
                        return vec4(t.x, t.y, 0.0, 1.0)
                    }
                }
                return vec4(0.0, 0.0, 0.0, 1.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = self.probe(0.25)
            }
        }
        shader.test_compile_draw(gpu_stage_4f5)

        println("GPU stage 4f6: vec2 helper logic before else vec4 return")
        let gpu_stage_4f6 = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            ds_lt: fn(a, b) { return a.x < b.x || (a.x == b.x && a.y < b.y) }
            ds_gt: fn(a, b) { return a.x > b.x || (a.x == b.x && a.y > b.y) }

            probe: fn(sel) {
                var t = vec2(0.0, 0.0)
                var de = vec2(0.0, 0.0)
                let max_de = vec2(-1.0, 0.0)
                let current_stop = vec2(-2.0, 0.0)
                for i in 0..2 {
                    if self.ds_gt(de, max_de) {
                        de = max_de
                    }
                    if sel > 0.5 && !self.ds_lt(de, current_stop) {
                        t = vec2(1.0, 0.25)
                    } else {
                        return vec4(t.x, t.y, 0.0, 1.0)
                    }
                }
                return vec4(0.0, 0.0, 0.0, 1.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = self.probe(0.25)
            }
        }
        shader.test_compile_draw(gpu_stage_4f6)

        println("GPU stage 4g: reduced ray_march structure")
        let gpu_stage_4g = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            controls0: shader.uniform(vec4f)
            step_width: shader.uniform(vec2f)
            max_ray_length: shader.uniform(vec2f)
            s_z_step_div: shader.uniform(vec2f)

            ds_new: fn(v) { return vec2(v, 0.0) }
            ds_add: fn(a, b) { return vec2(a.x + b.x, a.y + b.y) }
            ds_sub: fn(a, b) { return vec2(a.x - b.x, a.y - b.y) }
            ds_mul: fn(a, b) { return vec2(a.x * b.x, a.y * b.x + a.x * b.y) }
            ds_div: fn(a, b) { return vec2(a.x / b.x, 0.0) }
            ds_lt: fn(a, b) { return a.x < b.x || (a.x == b.x && a.y < b.y) }
            ds_gt: fn(a, b) { return a.x > b.x || (a.x == b.x && a.y > b.y) }
            ds_max: fn(a, b) {
                if self.ds_lt(a, b) { return b }
                return a
            }

            scene_destop_at_steps: fn(depth_steps) {
                if depth_steps.x < 0.5 {
                    return vec2(0.001, 0.0)
                }
                return vec2(0.02, 0.0)
            }

            calc_de: fn(px, py, pz) {
                return vec4(1.0, 0.01, 0.0, 0.0)
            }

            ray_march: fn(ox, oy, oz, dx, dy, dz) {
                var t = vec2(0.0, 0.0)
                var last_de = vec2(0.0, 0.0)
                var last_step = vec2(0.0, 0.0)

                let first_eval = self.calc_de(ox, oy, oz)
                let first_de = vec2(first_eval.y, first_eval.z)
                let first_stop = self.scene_destop_at_steps(self.ds_div(t, self.step_width))
                if first_eval.x >= self.controls0.y || self.ds_lt(first_de, first_stop) {
                    return vec4(t.x, t.y, first_eval.x, 1.0)
                }

                last_step = self.ds_max(
                    self.ds_mul(first_de, self.s_z_step_div),
                    self.ds_mul(self.step_width, self.ds_new(0.75))
                )
                last_de = first_de

                for step_idx in 0..8 {
                    let depth_steps = self.ds_div(t, self.step_width)
                    let current_stop = self.scene_destop_at_steps(depth_steps)
                    let px = self.ds_add(ox, self.ds_mul(dx, t))
                    let py = self.ds_add(oy, self.ds_mul(dy, t))
                    let pz = self.ds_add(oz, self.ds_mul(dz, t))
                    let eval = self.calc_de(px, py, pz)
                    var de = vec2(eval.y, eval.z)

                    let max_de = self.ds_add(last_de, last_step)
                    if self.ds_gt(de, max_de) {
                        de = max_de
                    }

                    if eval.x < self.controls0.y && !self.ds_lt(de, current_stop) {
                        let step = self.ds_max(
                            self.ds_mul(de, self.s_z_step_div),
                            self.ds_mul(self.step_width, self.ds_new(0.75))
                        )
                        last_de = de
                        last_step = step
                        t = self.ds_add(t, step)

                        if self.ds_gt(t, self.max_ray_length) {
                            return vec4(0.0, 0.0, 0.0, 0.0)
                        }
                    } else {
                        return vec4(t.x, t.y, 0.0, 1.0)
                    }
                }

                return vec4(0.0, 0.0, 0.0, 0.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = self.ray_march(
                    vec2(0.0, 0.0),
                    vec2(0.0, 0.0),
                    vec2(-3.0, 0.0),
                    vec2(0.0, 0.0),
                    vec2(0.0, 0.0),
                    vec2(1.0, 0.0)
                )
            }
        }
        shader.test_compile_draw(gpu_stage_4g)

        println("GPU stage 4h: statement-only if before else-return")
        let gpu_stage_4h = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            ds_add: fn(a, b) { return vec2(a.x + b.x, a.y + b.y) }
            ds_gt: fn(a, b) { return a.x > b.x || (a.x == b.x && a.y > b.y) }

            probe: fn(sel) {
                var t = vec2(0.0, 0.0)
                var de = vec2(0.0, 0.0)
                let max_de = vec2(-1.0, 0.0)
                for i in 0..2 {
                    if self.ds_gt(de, max_de) {
                        de = max_de
                    }
                    if sel > 0.5 {
                        t = self.ds_add(t, vec2(1.0, 0.125))
                    } else {
                        return vec4(t.x, t.y, 0.0, 1.0)
                    }
                }
                return vec4(0.0, 0.0, 0.0, 1.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = self.probe(0.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4h)

        println("GPU stage 4i: vec2 var from call fields before else-return")
        let gpu_stage_4i = #(GpuShaderStageTest::script_shader(vm)){
            vertex_pos: shader.vertex_position(vec4f)
            pixel: shader.fragment_output(0, vec4f)
            v_uv: shader.varying(vec2f)

            calc_de: fn(px, py, pz) {
                return vec4(1.0, 0.01, 0.0, 0.0)
            }

            probe: fn(sel) {
                var t = vec2(0.0, 0.0)
                for i in 0..2 {
                    let eval = self.calc_de(vec2(0.0, 0.0), vec2(0.0, 0.0), vec2(0.0, 0.0))
                    var de = vec2(eval.y, eval.z)
                    if sel > 0.5 {
                        de = vec2(1.0, 0.0)
                    } else {
                        return vec4(t.x, t.y, 0.0, 1.0)
                    }
                }
                return vec4(0.0, 0.0, 0.0, 1.0)
            }

            vertex: fn() {
                self.v_uv = vec2(0.5, 0.5)
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
            }

            fragment: fn() {
                self.pixel = self.probe(0.0)
            }
        }
        shader.test_compile_draw(gpu_stage_4i)
    };
    vm.eval(gpu_mb3d_shader_stages);

    // ========================================
    // Regex tests
    // ========================================
    let regex_test = script! {
        use mod.std.assert
        use mod.std.regex

        // ============================================================
        // REGEX CONSTRUCTOR & TYPE CHECKS
        // ============================================================

        // Basic construction
        let re = regex("hello", "")
        assert(re.is_regex())
        assert(!re.is_string())
        assert(!re.is_number())

        // Construction with flags
        let re_g = regex("abc", "g")
        assert(re_g.is_regex())
        assert(re_g.global == true)
        let re_no_g = regex("abc", "")
        assert(re_no_g.global == false)

        // Source property returns the pattern
        let re = regex("foo.*bar", "gi")
        assert(re.source == "foo.*bar")

        // Interning: same pattern + flags returns same object
        let r1 = regex("test", "g")
        let r2 = regex("test", "g")
        assert(r1 === r2)

        // Different flags = different object
        let r3 = regex("test", "")
        assert(r1 !== r3)

        // Different pattern = different object
        let r4 = regex("other", "g")
        assert(r1 !== r4)

        // ============================================================
        // REGEX.TEST()
        // ============================================================

        let re = regex("hello", "")
        assert(re.test("hello world") == true)
        assert(re.test("goodbye world") == false)

        // Case insensitive flag
        let re_i = regex("hello", "i")
        assert(re_i.test("HELLO WORLD") == true)
        assert(re_i.test("Hello") == true)

        // Pattern with special regex chars
        let re_digits = regex("[0-9]+", "")
        assert(re_digits.test("abc123") == true)
        assert(re_digits.test("abcdef") == false)

        // Anchored patterns
        let re_start = regex("^hello", "")
        assert(re_start.test("hello world") == true)
        assert(re_start.test("say hello") == false)

        let re_end = regex("world$", "")
        assert(re_end.test("hello world") == true)
        assert(re_end.test("world hello") == false)

        // Dot matches any non-newline by default
        let re_dot = regex("a.b", "")
        assert(re_dot.test("axb") == true)
        assert(re_dot.test("a\nb") == false)

        // With 's' flag, dot matches newlines too
        let re_dot_s = regex("a.b", "s")
        assert(re_dot_s.test("a\nb") == true)

        // ============================================================
        // REGEX.EXEC()
        // ============================================================

        let re = regex("(\\w+)@(\\w+)", "")
        let result = re.exec("user@host")
        assert(result != nil)
        assert(result.value == "user@host")
        assert(result.index == 0)
        assert(result.captures[0] == "user@host")
        assert(result.captures[1] == "user")
        assert(result.captures[2] == "host")

        // No match returns nil
        let re2 = regex("xyz", "")
        assert(re2.exec("abc") == nil)

        // Exec with match not at start
        let re3 = regex("(\\d+)", "")
        let r3 = re3.exec("abc123def")
        assert(r3 != nil)
        assert(r3.value == "123")
        assert(r3.index == 3)

        // ============================================================
        // STRING.SEARCH()
        // ============================================================

        // Search with regex
        let re = regex("\\d+", "")
        assert("abc123def".search(re) == 3)
        assert("abcdef".search(re) == -1)

        // Search with string
        assert("hello world".search("world") == 6)
        assert("hello world".search("xyz") == -1)

        // Search finds first occurrence
        let re = regex("o", "")
        assert("fooboo".search(re) == 1)

        // ============================================================
        // STRING.MATCH_STR() - non-global regex
        // ============================================================

        // Non-global regex returns detail object
        let re = regex("(\\d{2})-(\\d{2})", "")
        let m = "date: 12-25 end".match_str(re)
        assert(m != nil)
        assert(m.value == "12-25")
        assert(m.index == 6)
        assert(m.captures[0] == "12-25")
        assert(m.captures[1] == "12")
        assert(m.captures[2] == "25")

        // No match returns nil
        assert("no numbers".match_str(re) == nil)

        // ============================================================
        // STRING.MATCH_STR() - global regex
        // ============================================================

        // Global regex returns array of matched strings
        let re_g = regex("\\d+", "g")
        let matches = "a1b22c333".match_str(re_g)
        assert(matches[0] == "1")
        assert(matches[1] == "22")
        assert(matches[2] == "333")

        // Global with no matches returns empty-ish (array with no elements)
        let re_g2 = regex("xyz", "g")
        let matches2 = "hello world".match_str(re_g2)

        // ============================================================
        // STRING.MATCH_STR() - string pattern
        // ============================================================

        let m = "hello world hello".match_str("world")
        assert(m != nil)
        assert(m.value == "world")
        assert(m.index == 6)

        assert("hello".match_str("xyz") == nil)

        // ============================================================
        // STRING.MATCH_ALL()
        // ============================================================

        let re = regex("(\\w+)=(\\w+)", "g")
        let results = "a=1 b=2 c=3".match_all(re)
        assert(results[0].value == "a=1")
        assert(results[0].index == 0)
        assert(results[0].captures[1] == "a")
        assert(results[0].captures[2] == "1")
        assert(results[1].value == "b=2")
        assert(results[1].index == 4)
        assert(results[2].value == "c=3")
        assert(results[2].index == 8)
        assert(results[2].captures[1] == "c")
        assert(results[2].captures[2] == "3")

        // match_all with no captures
        let re2 = regex("\\d+", "g")
        let r2 = "x10y20z30".match_all(re2)
        assert(r2[0].value == "10")
        assert(r2[1].value == "20")
        assert(r2[2].value == "30")

        // ============================================================
        // STRING.SPLIT() with regex
        // ============================================================

        // Simple split
        let re = regex("[,;]", "")
        let parts = "a,b;c,d".split(re)
        assert(parts[0] == "a")
        assert(parts[1] == "b")
        assert(parts[2] == "c")
        assert(parts[3] == "d")

        // Split by whitespace
        let re_ws = regex("\\s+", "")
        let words = "hello   world\tfoo".split(re_ws)
        assert(words[0] == "hello")
        assert(words[1] == "world")
        assert(words[2] == "foo")

        // Split with capture groups (captured text included in result)
        let re_cap = regex("([-])", "")
        let parts = "a-b-c".split(re_cap)
        assert(parts[0] == "a")
        assert(parts[1] == "-")
        assert(parts[2] == "b")
        assert(parts[3] == "-")
        assert(parts[4] == "c")

        // Split with string (existing behavior still works)
        let parts = "a,b,c".split(",")
        assert(parts[0] == "a")
        assert(parts[1] == "b")
        assert(parts[2] == "c")

        // ============================================================
        // STRING.REPLACE() with regex
        // ============================================================

        // Simple replacement (non-global = first only)
        let re = regex("\\d+", "")
        let result = "abc123def456".replace(re, "NUM")
        assert(result == "abcNUMdef456")

        // Global replacement
        let re_g = regex("\\d+", "g")
        let result = "abc123def456".replace(re_g, "NUM")
        assert(result == "abcNUMdefNUM")

        // $& in replacement = whole match
        let re = regex("\\w+", "g")
        let result = "hello world".replace(re, "[$&]")
        assert(result == "[hello] [world]")

        // $1, $2 capture group references
        let re = regex("(\\w+)@(\\w+)", "g")
        let result = "user@host admin@server".replace(re, "$1 at $2")
        assert(result == "user at host admin at server")

        // $$ = literal $
        let re = regex("price", "")
        let result = "the price is".replace(re, "$$5")
        assert(result == "the $5 is")

        // Replace with string (existing behavior)
        let result = "hello world".replace("world", "earth")
        assert(result == "hello earth")

        // String replace only replaces first occurrence
        let result = "aaa".replace("a", "b")
        assert(result == "baa")

        // ============================================================
        // EDGE CASES
        // ============================================================

        // Empty pattern regex
        let re_empty = regex("", "g")
        let result = "abc".split(re_empty)
        // Empty regex matches between every char

        // Regex with alternation
        let re_alt = regex("cat|dog", "g")
        assert(re_alt.test("I have a cat") == true)
        assert(re_alt.test("I have a dog") == true)
        assert(re_alt.test("I have a bird") == false)
        let result = "my cat and dog".replace(re_alt, "pet")
        assert(result == "my pet and pet")

        // Regex with quantifiers
        let re = regex("a{2,4}", "")
        assert(re.test("aa") == true)
        assert(re.test("aaaa") == true)
        assert(re.test("a") == false)

        // Regex with character classes
        let re = regex("[a-z]+", "")
        assert(re.test("hello") == true)
        assert(re.test("123") == false)

        // Nested groups
        let re = regex("((\\d+)-(\\d+))", "")
        let m = re.exec("test 42-99 end")
        assert(m != nil)
        assert(m.value == "42-99")
        assert(m.captures[1] == "42-99")
        assert(m.captures[2] == "42")
        assert(m.captures[3] == "99")

        // Replace with no matches = original string
        let re = regex("xyz", "g")
        let result = "hello world".replace(re, "!")
        assert(result == "hello world")

        // Search/match on empty string
        let re = regex(".*", "")
        assert("".search(re) == 0)

        // Multiple regex operations on same string
        let text = "The quick brown fox jumps over the lazy dog"
        let re_word = regex("\\w+", "g")
        let words = text.match_str(re_word)

        let re_o = regex("o", "g")
        let o_matches = text.match_all(re_o)
        assert(o_matches[0].value == "o")

        let idx = text.search(regex("fox", ""))
        assert(idx == 16)

        // ============================================================
        // GC INTERACTION
        // ============================================================
        // Create regex objects in a loop, ensure GC doesn't break things
        for i in 0..100 {
            let re = regex("test" + i, "")
            let result = re.test("test50")
        }

        // Interned regex survives GC
        let re_before = regex("survivor", "g")
        // Create garbage
        for i in 0..200 {
            let garbage = {x: i, y: [1 2 3]}
        }
        // Regex should still work
        assert(re_before.test("find the survivor here") == true)
        assert(re_before.source == "survivor")
    };

    vm.eval(regex_test);
    println!("Regex tests passed");

    // ========================================
    // HTML parse + query tests
    // ========================================
    let html_test = script! {
        use mod.std.assert
        let html = "<div class='container' id='main'><p>Hello</p><p class='bold'>World</p><span>!</span></div>"
        let doc = html.parse_html()
        assert(doc.length == 1)
        let ps = doc.query("p")
        assert(ps.length == 2)
        let first_p = doc.query("p[0]")
        assert(first_p.text == "Hello")
        let second_p = doc.query("p[1]")
        assert(second_p.text == "World")
        assert(doc.query("span").text == "!")
        assert(doc.query("div@class") == "container")
        assert(doc.query("div@id") == "main")
        assert(doc.query("p.bold").length == 1)
        assert(doc.query("p.bold").text == "World")
        let texts = doc.query("p.text")
        assert(texts.len() == 2)
        assert(texts[0] == "Hello")
        assert(texts[1] == "World")
        let attrs = doc.query("p@class")
        assert(attrs.len() == 2)
        let items = doc.query("p").array()
        assert(items.len() == 2)
        assert(items[0].text == "Hello")
        assert(items[1].text == "World")
        let chained = doc.query("div").query("p")
        assert(chained.length == 2)
        let child = doc.query("div > p")
        assert(child.length == 2)
        let nested = "<ul><li><a href='http://x'>Link1</a></li><li><a href='http://y'>Link2</a></li></ul>"
        let ndoc = nested.parse_html()
        assert(ndoc.query("a").length == 2)
        assert(ndoc.query("a@href").len() == 2)
        assert(ndoc.query("a@href")[0] == "http://x")
        assert(ndoc.query("a[0]").text == "Link1")
        assert(ndoc.query("li > a").length == 2)
        assert(ndoc.query("ul a").length == 2)
        let deep = "<div><div><p>Deep</p></div></div>"
        let ddoc = deep.parse_html()
        assert(ddoc.query("div p").length == 1)
        assert(ddoc.query("div p").text == "Deep")
        assert(ddoc.query("div > p").length == 1)
        assert(ddoc.query("div > div > p").length == 1)
        let empty = "<div></div>".parse_html()
        assert(empty.query("p").length == 0)
        assert(empty.query("p").text == "")
        let multi = "<p>A</p><p>B</p><p>C</p>".parse_html()
        assert(multi.length == 3)
        assert(multi.query("p").length == 3)
        assert(multi.query("p[2]").text == "C")
        let wild = "<div><p>X</p><span>Y</span></div>".parse_html()
        assert(wild.query("div > *").length == 2)
        let by_id = "<div><p id='target'>Found</p><p>Other</p></div>".parse_html()
        assert(by_id.query("#target").length == 1)
        assert(by_id.query("#target").text == "Found")
        assert(by_id.query("p#target").text == "Found")
        let with_attr = doc.attr("class")
        assert(with_attr == "container")
    };

    vm.eval(html_test);
    println!("HTML tests passed");

    // ========================================
    // GC stress test - separate code block
    // ========================================
    let gc_test = script! {
        use mod.std.assert
        use mod.gc

        // Test 1: Create garbage in a simple loop
        for i in 0..1000 {
            let obj = {x: i, y: i * 2, data: [1, 2, 3, 4, 5]}
        }
        gc.run()

        // Test 2: Nested object creation
        for i in 0..500 {
            let nested = {
                a: {b: {c: {d: i}}}
                arr: [[1, 2], [3, 4], [5, 6]]
            }
        }
        gc.run()

        // Test 3: Function that creates and returns garbage
        fn make_garbage(n) {
            let result = []
            for j in 0..n {
                result.push({id: j, name: "item"})
            }
            return result
        }

        for i in 0..100 {
            let garbage = make_garbage(50)
        }
        gc.run()

        // Test 4: Recursive function creating objects
        fn recursive_create(depth) {
            if depth <= 0 {
                return {leaf: true}
            }
            return {
                value: depth
                left: recursive_create(depth - 1)
                right: recursive_create(depth - 1)
            }
        }

        for i in 0..20 {
            let tree = recursive_create(6)
        }
        gc.run()

        // Test 5: String concatenation creating garbage
        for i in 0..500 {
            let s = "hello_world"
            let parts = s.to_bytes()
        }
        gc.run()

        // Test 6: Array operations creating garbage
        for i in 0..200 {
            let arr = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
            arr.push(i)
            arr.push(i)
            let popped = arr.pop()
        }
        gc.run()

        // Test 7: Mixed operations - objects, arrays, strings
        fn create_record(id) {
            return {
                id: id
                name: "record"
                tags: ["tag1", "tag2", "tag3"]
                metadata: {
                    created: id * 100
                    modified: id * 200
                }
            }
        }

        for i in 0..300 {
            let record = create_record(i)
            let json = record.to_json()
            let parsed = json.parse_json()
        }
        gc.run()

        // Test 8: Closures capturing values
        for i in 0..200 {
            let captured = {value: i}
            let closure = || captured.value * 2
            let result = closure()
        }
        gc.run()

        // Test 9: Object with prototype chain
        let base = {
            method: |s| s.x * 2
        }
        for i in 0..500 {
            let derived = base{x: i, y: i + 1}
        }
        gc.run()

        // Test 10: Verify live objects survive GC
        let live_obj = {important: 42, data: [1, 2, 3]}
        let live_arr = [10, 20, 30, 40, 50]

        // Create lots of garbage
        for i in 0..1000 {
            let a = i + 1
            let b = i + 2
            let garbage = {temp: i, arr: [i, a, b]}
        }

        gc.run()

        // Verify live objects are still valid
        assert(live_obj.important == 42)
        assert(live_obj.data == [1, 2, 3])
        assert(live_arr == [10, 20, 30, 40, 50])

        // Test 11: Static containers are fully immutable
        let static_state = {status: "boot", nested: {v: 1}}
        gc.set_static(static_state)
        try static_state.status = "state1" assert(true) ok assert(false)
        try static_state.extra = 10 assert(true) ok assert(false)
        try static_state.nested.v = 2 assert(true) ok assert(false)
        try static_state.delete("status") assert(true) ok assert(false)
        assert(static_state.status == "boot")
        assert(static_state.nested.v == 1)

        let static_arr = [1, 2, 3]
        gc.set_static(static_arr)
        try static_arr.push(4) assert(true) ok assert(false)
        try static_arr.pop() assert(true) ok assert(false)
        try static_arr.remove(0) assert(true) ok assert(false)
        try static_arr.clear() assert(true) ok assert(false)
        try static_arr[0] = 9 assert(true) ok assert(false)
        assert(static_arr == [1, 2, 3])

        gc.run()
        assert(static_state.status == "boot")
        assert(static_arr == [1, 2, 3])

        // Test 12: Objects with string keys must survive GC key marking
        let by_string_key = {"style_and_keywords": "ok", "visual_description": "scene"}
        gc.run()
        assert(by_string_key.style_and_keywords == "ok")
        assert(by_string_key.visual_description == "scene")

        let parsed = "{\"style_and_keywords\":\"ok2\",\"visual_description\":\"scene2\"}".parse_json()
        gc.run()
        assert(parsed.style_and_keywords == "ok2")
        assert(parsed.visual_description == "scene2")
    };

    vm.eval(gc_test);

    // ========================================
    // script_apply_eval stress test
    // This tests the pattern used in PortalList/FlatList
    // where script_apply_eval is called repeatedly in a loop
    // ========================================

    // Create items with source objects so script_apply_eval works
    let mut items: Vec<DrawBgTest> = Vec::new();
    for _ in 0..10 {
        let mut item = DrawBgTest::default();
        // Initialize the source object for the item
        let obj = vm.heap_mut().new_object();
        item.source = vm.heap_mut().new_object_ref(obj);
        items.push(item);
    }

    // Simulate the draw loop pattern from PortalList
    for iteration in 0..1000 {
        for (idx, item) in items.iter_mut().enumerate() {
            let is_even_f = if idx % 2 == 0 { 1.0f32 } else { 0.0f32 };

            // This is the pattern that causes garbage accumulation
            script_apply_eval!(vm, item, {
                is_even: #(is_even_f)
            });
        }

        // Run GC periodically to stress test
        if iteration % 100 == 0 {
            vm.gc();
        }
    }

    // Final GC to verify no issues
    vm.gc();
    let code = script! {
        let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
        ~fib(20);
    };
    let dt = std::time::Instant::now();

    vm.eval(code);
    println!("Duration {}", dt.elapsed().as_secs_f64());

    println!("Test done");

    // ========================================
    // Streaming (incremental) parser test
    // Mimics what Splash widget does: feeds a growing
    // string to tokenizer+parser incrementally, compares
    // opcodes against a full non-incremental parse of the
    // same final string at each step.
    // ========================================
    println!("Running streaming parser test...");
    {
        use makepad_script::parser::ScriptParser;
        use makepad_script::tokenizer::ScriptTokenizer;

        let prefix = "use mod.prelude.widgets.*View{height:Fit, ";
        // A splash-like body that gets streamed in
        let body = r#"flow: Down height: Fit spacing: 10 padding: 20
View{
    flow: Right height: Fit spacing: 10
    SolidView{width: 50 height: 50 draw_bg.color: #f00}
    SolidView{width: 50 height: 50 draw_bg.color: #0f0}
    SolidView{width: 50 height: 50 draw_bg.color: #00f}
}
View{
    flow: Right height: Fit spacing: 10
    Button{text: "Buttoqwkehrqlkwjerhqwjkerhqlkwjehrlqkjwehrqlkjwehrqklwjehrlqkwjehrqlkwjehrkqjlwehrlqkwjehrlkqwejhrlkqjwehrlkjwqehn 1"}
    Button{text: "Button 2"}
    Button{text: "Button 3"}
    Button{text: "Button 4"}
}"#;

        let full_code = format!("{}{}", prefix, body);

        // --- Part 1: Opcode comparison (manual tokenizer/parser) ---
        {
            let mut inc_tokenizer = ScriptTokenizer::default();
            let mut inc_parser = ScriptParser::default();
            let mut prev_len = 0usize;
            let mut checkpoint: Option<makepad_script::parser::ParserCheckpoint> = None;

            for end in 1..=full_code.len() {
                let code_so_far = &full_code[..end];

                if let Some(cp) = checkpoint.take() {
                    inc_parser.restore_checkpoint(cp);
                }

                let new_chars = &code_so_far[prev_len..];
                if !new_chars.is_empty() {
                    inc_tokenizer.tokenize(new_chars, &mut vm.heap_mut());
                }
                prev_len = end;

                let unfinished = inc_tokenizer.intern_unfinished_string(&mut vm.heap_mut());
                let cp = inc_parser.parse_streaming(&inc_tokenizer, "", (0, 0), &[], unfinished);

                let mut ref_tokenizer = ScriptTokenizer::default();
                let mut ref_parser = ScriptParser::default();
                ref_tokenizer.tokenize(code_so_far, &mut vm.heap_mut());
                let ref_unfinished = ref_tokenizer.intern_unfinished_string(&mut vm.heap_mut());
                ref_parser.parse_streaming(&ref_tokenizer, "", (0, 0), &[], ref_unfinished);

                let tok_match = ref_tokenizer.tokens.len() == inc_tokenizer.tokens.len();
                let op_match = ref_parser.opcodes == inc_parser.opcodes;
                if !tok_match || !op_match {
                    let mut msg = format!(
                        "STREAM MISMATCH at byte {}/{}\n  code so far: {:?}\n  tokens: ref={} inc={}\n  opcodes: ref={} inc={}",
                        end, full_code.len(), code_so_far,
                        ref_tokenizer.tokens.len(), inc_tokenizer.tokens.len(),
                        ref_parser.opcodes.len(), inc_parser.opcodes.len(),
                    );
                    for i in 0..ref_parser.opcodes.len().max(inc_parser.opcodes.len()) {
                        let r = ref_parser.opcodes.get(i);
                        let s = inc_parser.opcodes.get(i);
                        let marker = if r != s { " <<< DIFF" } else { "" };
                        msg.push_str(&format!(
                            "\n  opcode[{}]: ref={:?} inc={:?}{}",
                            i, r, s, marker
                        ));
                    }
                    panic!("{}", msg);
                }
                checkpoint = Some(cp);
            }
            println!("  Opcode comparison passed ({} steps)", full_code.len());
        }

        // --- Part 2: Actual execution via eval_with_append_source (byte at a time) ---
        {
            for end in 1..=full_code.len() {
                let code_so_far = &full_code[..end];
                let script_mod = ScriptMod {
                    cargo_manifest_path: String::new(),
                    module_path: String::new(),
                    file: "streaming_test".to_string(),
                    line: 0,
                    column: 0,
                    code: String::new(),
                    values: vec![],
                };
                // Execute incrementally — errors are expected for incomplete code,
                // but panics/crashes would indicate bad opcode generation.
                let _value = vm.eval_with_append_source(script_mod, code_so_far, NIL.into());
            }
            println!("  Execution passed ({} steps)", full_code.len());
        }

        // --- Part 3: 20-char chunks like aichat fake streaming ---
        {
            let mut pos = 0usize;
            let mut steps = 0;
            while pos < full_code.len() {
                let mut end = (pos + 20).min(full_code.len());
                // Align to char boundary
                while end < full_code.len() && !full_code.is_char_boundary(end) {
                    end += 1;
                }
                let code_so_far = &full_code[..end];
                let script_mod = ScriptMod {
                    cargo_manifest_path: String::new(),
                    module_path: String::new(),
                    file: "streaming_20char".to_string(),
                    line: 0,
                    column: 0,
                    code: String::new(),
                    values: vec![],
                };
                let _value = vm.eval_with_append_source(script_mod, code_so_far, NIL.into());
                pos = end;
                steps += 1;
            }
            println!("  20-char chunk execution passed ({} steps)", steps);
        }
    }
}
