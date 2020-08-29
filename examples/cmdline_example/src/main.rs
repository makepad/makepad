
struct Cx{
}

impl Cx{
}

live!(cx, {"
    file::anim_default = Anim{
        mode: Cut,
        file::shader_bg::myinst: {
            ease: Linear,
            0.0: 0.0,
            1.0: 1.0
        }
    },
    file::mycolor = #ff0f,
    file::myslider = 1.0,
    file::shader_bg = Shader{
        instance myinst: vec2;
        use render::quad;
        let x = file::myslider;
        let y = file::mycolor;
    }
"})

fn main() {
    // lets run a const fn
    // 
}
