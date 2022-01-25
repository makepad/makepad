use crate::cx::*;
use crate::draw_2d::draw_quad::DrawQuad;

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawImage {
    #[default_shader(self::shader)]
    #[custom_new()]
    pub texture: Texture2D,
    pub base: DrawQuad,
    pub pt1: Vec2,
    pub pt2: Vec2,
    pub alpha: f32
}

impl DrawImage {
    
    pub fn new(cx: &mut Cx, shader:Shader)->Self{
        Self{
            pt1: vec2(0.,0.),
            pt2: vec2(1.,1.),
            alpha: 1.0,
            ..Self::custom_new(cx, shader)
        }
    }
    
    pub fn style(cx: &mut Cx) {
        Self::register_draw_input(cx);
        live_body!(cx, {
            self::shader: Shader {
                use crate::drawquad::shader::*;

                draw_input: self::DrawImage;

                varying tc: vec2;
                varying v_pixel: vec2;
                //let dpi_dilate: float<Uniform>;
                
                fn vertex() -> vec4 {
                    // return vec4(geom.x-0.5, geom.y, 0., 1.);
                    let shift: vec2 = -draw_scroll.xy;
                    let clipped: vec2 = clamp(
                        geom * rect_size + rect_pos + shift,
                        draw_clip.xy,
                        draw_clip.zw
                    );
                    let pos = (clipped - shift - rect_pos) / rect_size;
                    tc = mix(pt1, pt2, pos);
                    v_pixel = clipped;
                    // only pass the clipped position forward
                    return camera_projection * vec4(clipped.x, clipped.y, draw_depth, 1.);
                }
                
                fn pixel() -> vec4 {
                    return vec4(sample2d(texture, tc.xy).rgb * alpha, alpha);
                }
            }
        });
    }
}

