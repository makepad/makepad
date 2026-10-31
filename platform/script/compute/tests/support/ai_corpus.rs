//! Kernels written the way a language model writes them (Splash document
//! habits: helper functions calling helpers, local functions and
//! closures, palette constants, constant objects and arrays of objects,
//! early returns, loops with breaks, `if` as a value, shadowing, params
//! named like library functions, mixed integer and float arithmetic,
//! swizzles). Each writes `o` (f32 or vec4); tests/ai_style.rs checks the
//! values, tests/wasm.rs the wasm backend on the same text.
#![allow(dead_code)]

pub const AI_CORPUS: &[(&str, &str)] = &[
    (
        "helpers_call_helpers",
        "let o = output(f32)
fn sq(x) { x * x }
fn len2(x, y) { sqrt(sq(x) + sq(y)) }
fn falloff(d, r) { clamp(1.0 - d / r, 0.0, 1.0) }
fn element(i) {
    let x = float(i)
    o[i] = falloff(len2(x, 4.0), 10.0)
}",
    ),
    (
        "nested_fn",
        "let o = output(f32)
fn element(i) {
    fn ease(t) { t * t * (3.0 - 2.0 * t) }
    let t = float(i) / 4.0
    o[i] = ease(t)
}",
    ),
    (
        "closure_captures",
        "let o = output(f32)
let gain = param(2.0)
fn element(i) {
    let base = float(i)
    let scale = fn(x) { x * gain + base }
    o[i] = scale(3.0)
}",
    ),
    (
        "closures_call_closures",
        "let o = output(f32)
fn element(i) {
    let k = float(i)
    let add = fn(a) { a + k }
    let twice = fn(a) { add(add(a)) }
    o[i] = twice(1.0)
}",
    ),
    (
        "closure_mutates_captured",
        "let o = output(f32)
fn element(i) {
    var total = 0.0
    let acc = fn(x) { total += x }
    for k in 0..4 { acc(float(k)) }
    acc(float(i))
    o[i] = total
}",
    ),
    (
        "top_level_fn_value",
        "let o = output(f32)
let ease = fn(t) { t * t }
let ease_twice = fn(t) { ease(ease(t)) }
fn element(i) { o[i] = ease_twice(float(i)) }",
    ),
    (
        "param_named_like_top_level",
        "let o = output(f32)
let speed = 3.0
fn travel(speed, t) { speed * t }
fn element(i) { o[i] = travel(2.0, float(i)) + speed }",
    ),
    (
        "params_named_like_prelude",
        "let o = output(f32)
let fade = param(0.5)
let hash = param(3.0)
fn element(i) { o[i] = fade * float(i) + hash + hash01(i, 0) * 0.0 }",
    ),
    (
        "local_named_like_builtin",
        "let o = output(f32)
fn shade(mix, length) { mix * length }
fn element(i) {
    let min = float(i)
    let max = 2.0
    o[i] = shade(min, max) + mix(0.0, 10.0, 0.5)
}",
    ),
    (
        "palette",
        "let o = output(vec4)
let BG = #1a1a2e
let WHITE = #fff
let RED = linear_rgb(#ff0000)
let HALF = #80808080
fn element(i) {
    let t = float(i) / 3.0
    let c = mix(BG, WHITE, t)
    o[i] = vec4(c.r, c.g + RED.r, RED.g + HALF.a, c.a)
}",
    ),
    (
        "const_vectors",
        "let o = output(vec4)
let UP = vec3(0, 1, 0)
let ORIGIN = vec3(1, 2, 3) * 2.0
let SIZE = vec2(4, 8)
let ASPECT = SIZE.y / SIZE.x
fn element(i) {
    let p = ORIGIN + UP * float(i)
    o[i] = vec4(p, ASPECT)
}",
    ),
    (
        "object_let",
        "let o = output(vec4)
let CAM = {fov: 40, pos: vec3(0, 1, 5), clip: {near: 0.1, far: 100}}
fn element(i) {
    let f = CAM.fov * float(i)
    o[i] = vec4(f, CAM.pos.y, CAM.clip.far - CAM.clip.near, CAM.pos.z)
}",
    ),
    (
        "object_count_field",
        "let o = output(f32)
let CFG = {steps: 4, step: 0.5, enabled: true}
fn element(i) {
    var acc = 0.0
    for k in 0..CFG.steps { acc += CFG.step }
    if !CFG.enabled { acc = -1.0 }
    o[i] = acc * float(i)
}",
    ),
    (
        "object_with_strings",
        "let o = output(f32)
let STYLE = {name: \"hero\", width: 2.5, label: \"A\"}
fn element(i) { o[i] = STYLE.width * float(i) }",
    ),
    (
        "objects_array_runtime_index",
        "let o = output(vec4)
let LIGHTS = [
    {pos: vec3(1, 2, 3), color: #ff0000, power: 2}
    {pos: vec3(-1, 0, 4), color: #00ff00, power: 0.5}
    {power: 1, pos: vec3(0, 5, 0), color: #0000ff}
]
fn element(i) {
    let l = LIGHTS[i % 3]
    o[i] = vec4(l.pos.x, l.color.g, l.power, LIGHTS[1].pos.z)
}",
    ),
    (
        "objects_array_loop",
        "let o = output(f32)
let PTS = [{x: 1, w: 0.5}, {x: 2, w: 0.25}, {x: 4, w: 0.25}]
fn element(i) {
    var s = 0.0
    for k in 0..3 { s += PTS[k].x * PTS[k].w }
    o[i] = s + float(i)
}",
    ),
    (
        "object_passed_to_helper",
        "let o = output(f32)
let SHAPE = {radius: 2.0, center: vec2(1, 1)}
fn sdf(p, s) { length(p - s.center) - s.radius }
fn element(i) { o[i] = sdf(vec2(float(i) + 1.0, 1.0), SHAPE) }",
    ),
    (
        "object_of_arrays",
        "let o = output(f32)
let RAMP = {stops: [0.0, 0.25, 1.0], values: [vec2(0, 1), vec2(5, 6), vec2(10, 11)]}
fn element(i) { o[i] = RAMP.stops[i % 3] + RAMP.values[i % 3].y + RAMP.values[2].x }",
    ),
    (
        "local_object",
        "let o = output(f32)
fn element(i) {
    let s = {a: float(i), b: 2.0}
    s.a += 1.0
    o[i] = s.a * s.b
}",
    ),
    (
        "struct_methods_style",
        "let o = output(f32)
struct Particle { pos: vec2(0, 0), vel: vec2(1, 0), life: 1.0 }
fn step(p, dt) { p.pos = p.pos + p.vel * dt; p.life -= dt }
fn element(i) {
    let p = Particle { pos: vec2(float(i), 0), vel: vec2(2, 1) }
    step(p, 0.5)
    o[i] = p.pos.x + p.pos.y + p.life
}",
    ),
    (
        "early_returns",
        "let o = output(f32)
fn classify(x) {
    if x < 1.0 { return 0.0 }
    if x < 3.0 { return 1.0 }
    2.0
}
fn element(i) { o[i] = classify(float(i)) }",
    ),
    (
        "loops_with_break",
        "let o = output(f32)
fn element(i) {
    var n = 0
    var x = float(i) + 1.0
    loop {
        if x > 20.0 { break }
        x = x * 2.0
        n += 1
    }
    var m = 0
    while true {
        m += 1
        if m >= i { break }
    }
    var first = -1
    for k in 0..10 {
        if k * k > i { first = k\n break }
    }
    o[i] = float(n) * 100.0 + float(m) * 10.0 + float(first)
}",
    ),
    (
        "if_as_value",
        "let o = output(f32)
fn element(i) {
    let x = float(i)
    let y = if x > 1.5 { x * 10.0 } else { -x }
    let z = if i % 2 == 0 { 1.0 } elif i == 1 { 2.0 } else { 3.0 }
    let w = if i > 0 and i < 3 { 100.0 } else { 0.0 }
    let m = match i % 3 { 0 => 1000.0, 1 => 2000.0, _ => 3000.0 }
    o[i] = y + z + w + m
}",
    ),
    (
        "shadowing",
        "let o = output(f32)
let x = 100.0
fn element(i) {
    let x = float(i)
    let y = { let x = x * 2.0\n x + 1.0 }
    let x = x + y
    o[i] = x
}",
    ),
    (
        "mixed_int_float",
        "let o = output(f32)
let N = 8
fn element(i) {
    let half = i / 2
    let f = float(i) * 0.5
    let g = float(i) * 2 + 1
    let h = float(i + 1) * 1.5
    let w = float(i % N) / float(N)
    let c = min(i, 2)
    o[i] = float(half) + f + g + h + w + float(c)
}",
    ),
    (
        "int_times_fraction",
        "let o = output(f32)
fn element(i) { o[i] = i * 0.5 + i / 4.0 }",
    ),
    (
        "swizzles",
        "let o = output(vec4)
fn element(i) {
    let v = vec4(1.0, 2.0, 3.0, float(i))
    let a = v.xy + v.zw
    let b = v.wzyx
    var c = v.xyz
    c.y = 9.0
    let d = vec4(v.xyz, 1.0)
    o[i] = vec4(a.x, b.x, c.y, v.rgb.b + d.w)
}",
    ),
    (
        "const_keyword_and_annotations",
        "let o = output(f32)
const SCALE: f32 = 2.0
fn scaled(x: f32) -> f32 { x * SCALE }
fn element(i) {
    const BIAS = 0.5
    let x: float = float(i)
    o[i] = scaled(x) + BIAS
}",
    ),
];
