
fn main() {
    println!("{}",stringify!("
        let counter: Self::counter();
        fn pixel() -> vec4 {
            df_viewport(pos * vec2(w, h));
            df_circle(0.5 * w, 0.5 * h, 0.5 * w);
            //return df_fill(color!(green));
            return df_fill(mix(color!(green), color!(blue), abs(sin(counter))));
        }
    "));
}
