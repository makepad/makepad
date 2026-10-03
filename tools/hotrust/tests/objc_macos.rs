// macOS only: the Objective-C runtime and system frameworks through the C ABI, the way
// platform/src/os/apple uses them. A class registered at runtime with `extern "C"` method
// IMPs taking and returning structs (HFA rect, 48-byte transform through x8, > 8 arguments
// with stack spill), called through objc_msgSend declared per signature (#[link_name]);
// real Foundation/CoreGraphics code returning structs and narrow integers; a GCD worker
// thread calling back into HotRust. Frameworks are found through #[link] (the loader
// dlopens them). Output matches rustc.

#[link(name = "Foundation", kind = "framework")]
extern "C" {}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    origin: Point,
    size: Point,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Range {
    location: u64,
    length: u64,
}

type Id = *mut u8;
type Sel = *mut u8;

extern "C" {
    fn objc_getClass(name: *const u8) -> Id;
    fn sel_registerName(name: *const u8) -> Sel;
    fn objc_allocateClassPair(sup: Id, name: *const u8, extra: usize) -> Id;
    fn objc_registerClassPair(c: Id);
    #[link_name = "class_addMethod"]
    fn add_rect_method(c: Id, sel: Sel, imp: extern "C" fn(Id, Sel, Rect, f64) -> Rect, types: *const u8) -> bool;
    #[link_name = "class_addMethod"]
    fn add_affine_method(c: Id, sel: Sel, imp: extern "C" fn(Id, Sel, Affine, f64) -> Affine, types: *const u8) -> bool;
    #[link_name = "class_addMethod"]
    fn add_many_method(c: Id, sel: Sel, imp: extern "C" fn(Id, Sel, i64, f64, i32, f64, u8, f64, i64, f64, i16, f64, i64, f64, u32, f64, Range) -> f64, types: *const u8) -> bool;
    #[link_name = "objc_msgSend"]
    fn send_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn send_rect_d(obj: Id, sel: Sel, r: Rect, d: f64) -> Rect;
    #[link_name = "objc_msgSend"]
    fn send_affine(obj: Id, sel: Sel, t: Affine, k: f64) -> Affine;
    #[link_name = "objc_msgSend"]
    fn send_many(obj: Id, sel: Sel, a: i64, b: f64, c: i32, d: f64, e: u8, f: f64, g: i64, h: f64, i: i16, j: f64, k: i64, l: f64, m: u32, n: f64, o: Range) -> f64;
    #[link_name = "objc_msgSend"]
    fn send_value_with_rect(cls: Id, sel: Sel, r: Rect) -> Id;
    #[link_name = "objc_msgSend"]
    fn send_rect(obj: Id, sel: Sel) -> Rect;
    #[link_name = "objc_msgSend"]
    fn send_str(cls: Id, sel: Sel, s: *const u8) -> Id;
    #[link_name = "objc_msgSend"]
    fn send_u64(obj: Id, sel: Sel) -> u64;
    #[link_name = "objc_msgSend"]
    fn send_char_at(obj: Id, sel: Sel, i: u64) -> u16;
    #[link_name = "objc_msgSend"]
    fn send_substr(obj: Id, sel: Sel, r: Range) -> Id;
    fn CGRectInset(r: Rect, dx: f64, dy: f64) -> Rect;
    fn CGRectIntersection(a: Rect, b: Rect) -> Rect;
    fn CGRectContainsPoint(r: Rect, p: Point) -> bool;
    fn CGAffineTransformMakeRotation(angle: f64) -> Affine;
    fn CGAffineTransformConcat(a: Affine, b: Affine) -> Affine;
    fn dispatch_get_global_queue(identifier: i64, flags: u64) -> Id;
    fn dispatch_async_f(queue: Id, ctx: *mut u8, work: extern "C" fn(*mut u8));
    fn dispatch_semaphore_create(value: i64) -> Id;
    fn dispatch_semaphore_signal(s: Id) -> i64;
    fn dispatch_semaphore_wait(s: Id, timeout: u64) -> i64;
    fn pthread_self() -> usize;
}

extern "C" fn inset_rect(_this: Id, _sel: Sel, r: Rect, d: f64) -> Rect {
    Rect { origin: Point { x: r.origin.x + d, y: r.origin.y + d }, size: Point { x: r.size.x - 2.0 * d, y: r.size.y - 2.0 * d } }
}

extern "C" fn scale_affine(_this: Id, _sel: Sel, t: Affine, k: f64) -> Affine {
    Affine { a: t.a * k, b: t.b * k, c: t.c * k, d: t.d * k, tx: t.tx + k, ty: t.ty - k }
}

extern "C" fn many_args(_this: Id, _sel: Sel, a: i64, b: f64, c: i32, d: f64, e: u8, f: f64, g: i64, h: f64, i: i16, j: f64, k: i64, l: f64, m: u32, n: f64, o: Range) -> f64 {
    (a + c as i64 + e as i64 + g + i as i64 + k + m as i64) as f64 + b + d + f + h + j + l + n + (o.location * 1000 + o.length) as f64
}

fn cstr(s: &str) -> *const u8 {
    s as *const str as *const u8
}

fn p(r: Rect) {
    println!("rect {} {} {} {}", r.origin.x, r.origin.y, r.size.x, r.size.y);
}

fn pa(t: Affine) {
    println!("affine {:.6} {:.6} {:.6} {:.6} {} {}", t.a, t.b, t.c, t.d, t.tx, t.ty);
}

#[test]
fn runtime_class_with_struct_methods() {
    unsafe {
        let ns_object = objc_getClass(cstr("NSObject\0"));
        let cls = objc_allocateClassPair(ns_object, cstr("HotRustTestObject\0"), 0);
        let s_inset = sel_registerName(cstr("insetRect:by:\0"));
        let s_scale = sel_registerName(cstr("scaleAffine:by:\0"));
        let s_many = sel_registerName(cstr("many:\0"));
        add_rect_method(cls, s_inset, inset_rect, cstr("v@:\0"));
        add_affine_method(cls, s_scale, scale_affine, cstr("v@:\0"));
        add_many_method(cls, s_many, many_args, cstr("v@:\0"));
        objc_registerClassPair(cls);
        let obj = send_id(send_id(cls, sel_registerName(cstr("alloc\0"))), sel_registerName(cstr("init\0")));
        p(send_rect_d(obj, s_inset, Rect { origin: Point { x: 1.0, y: 2.0 }, size: Point { x: 30.0, y: 40.0 } }, 2.5));
        pa(send_affine(obj, s_scale, Affine { a: 1.0, b: 2.0, c: 3.0, d: 4.0, tx: 5.0, ty: 6.0 }, 0.5));
        println!("many {}", send_many(obj, s_many, 1, 0.5, -2, 0.25, 200, 0.125, 3, 1.5, -4, 2.5, 5, 3.5, 4000000000, 4.5, Range { location: 7, length: 9 }));
    }
}

#[test]
fn foundation_and_coregraphics() {
    unsafe {
        let r = Rect { origin: Point { x: 10.0, y: 20.0 }, size: Point { x: 300.0, y: 200.0 } };
        let ns_value = objc_getClass(cstr("NSValue\0"));
        let v = send_value_with_rect(ns_value, sel_registerName(cstr("valueWithRect:\0")), r);
        p(send_rect(v, sel_registerName(cstr("rectValue\0"))));
        let s = send_str(objc_getClass(cstr("NSString\0")), sel_registerName(cstr("stringWithUTF8String:\0")), cstr("HotRust\u{e9}\0"));
        let len = send_u64(s, sel_registerName(cstr("length\0")));
        let c = send_char_at(s, sel_registerName(cstr("characterAtIndex:\0")), 7);
        let sub = send_substr(s, sel_registerName(cstr("substringWithRange:\0")), Range { location: 3, length: 4 });
        println!("nsstring len {} char7 {} sub len {}", len, c, send_u64(sub, sel_registerName(cstr("length\0"))));
        p(CGRectInset(r, 5.0, 7.5));
        p(CGRectIntersection(r, Rect { origin: Point { x: 100.0, y: 0.0 }, size: Point { x: 400.0, y: 50.0 } }));
        println!("contains {} {}", CGRectContainsPoint(r, Point { x: 11.0, y: 21.0 }), CGRectContainsPoint(r, Point { x: 9.0, y: 21.0 }));
        let t = CGAffineTransformConcat(CGAffineTransformMakeRotation(0.5), Affine { a: 2.0, b: 0.0, c: 0.0, d: 3.0, tx: 4.0, ty: 5.0 });
        pa(t);
    }
}

static mut WORK_THREAD: usize = 0;
static mut WORK_SUM: u64 = 0;

extern "C" fn gcd_work(ctx: *mut u8) {
    unsafe {
        WORK_THREAD = pthread_self();
        let mut s = 0u64;
        let mut i = 0u64;
        while i < 1000 {
            s += i * i;
            i += 1;
        }
        WORK_SUM = s;
        dispatch_semaphore_signal(ctx);
    }
}

#[test]
fn gcd_worker_calls_back() {
    unsafe {
        let sem = dispatch_semaphore_create(0);
        dispatch_async_f(dispatch_get_global_queue(0, 0), sem, gcd_work);
        dispatch_semaphore_wait(sem, u64::MAX);
        println!("gcd sum {} other thread {}", WORK_SUM, WORK_THREAD != pthread_self());
    }
}
