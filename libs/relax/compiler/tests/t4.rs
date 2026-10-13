// more than 6 integer / 8 float arguments
fn many(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64, x: f64, y: f64, z: f64, w: f64, p: f64, q: f64, r: f64, s: f64, t: f64, u: f64) -> f64 {
    (a + 2 * b + 3 * c + 4 * d + 5 * e + 6 * f + 7 * g + 8 * h) as f64 + x + 2.0 * y + 3.0 * z + 4.0 * w + 5.0 * p + 6.0 * q + 7.0 * r + 8.0 * s + 9.0 * t + 10.0 * u
}

#[test]
fn stack_args() {
    let v = many(1, 2, 3, 4, 5, 6, 7, 8, 1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5, 9.5, 10.5);
    println!("{}", v);
    assert_eq!(v, 616.5);
}
