pub fn remainder(x: f64, y: f64) -> f64 {
    let (result, _) = super::remquo(x, y);
    result
}
