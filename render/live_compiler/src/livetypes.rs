use crate::colors::Color;
use makepad_microserde::*;
use makepad_detok_derive::*;
use crate::math::*;
use std::f64::consts::PI;
use crate::detok::*;
use crate::token::Token;
use crate::ident::{Ident};
use crate::error::LiveError;

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Shader {
    pub shader_id: usize,
    pub location_hash: u64
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Geometry {
    pub geometry_id: usize,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Texture {
    pub texture_id: usize,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Font {
    pub font_id: usize,
}

#[derive(Clone, PartialEq)]
pub enum TextureFormat {
    Default,
    ImageBGRA,
    Depth32Stencil8,
    RenderBGRA,
    RenderBGRAf16,
    RenderBGRAf32,
    //    ImageBGRAf32,
    //    ImageRf32,
    //    ImageRGf32,
    //    MappedBGRA,
    //    MappedBGRAf32,
    //    MappedRf32,
    //    MappedRGf32,
}

#[derive(Clone, PartialEq)]
pub struct TextureDesc {
    pub format: TextureFormat,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub multisample: Option<usize>
}

impl Default for TextureDesc {
    fn default() -> Self {
        TextureDesc {
            format: TextureFormat::Default,
            width: None,
            height: None,
            multisample: None
        }
    }
}


#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct LiveId(pub u64);


#[derive(Clone, Debug, Copy, DeTok)]
pub struct TextStyle {
    pub font: Font,
    pub font_size: f32,
    pub brightness: f32,
    pub curve: f32,
    pub line_spacing: f32,
    pub top_drop: f32,
    pub height_factor: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font: Font {font_id: 0},
            font_size: 8.0,
            brightness: 1.0,
            curve: 0.6,
            line_spacing: 1.4,
            top_drop: 1.1,
            height_factor: 1.3,
        }
    }
}

impl DeTokSplat for TextStyle {
    fn de_tok_splat(p: &mut dyn DeTokParser) -> Result<Self,
    LiveError> {
        let ident_path = p.parse_ident_path() ?;
        let live_id = p.ident_path_to_live_id(&ident_path);
        if let Some(text_style) = p.get_live_styles().base.text_styles.get(&live_id) {
            return Ok(*text_style);
        }
        else{
            return Err(p.error(format!("Textstyle {} not found in splat", ident_path)));
        }
    }
    
}

#[derive(Copy, Clone, Debug, DeTok)]
pub enum LineWrap {
    None,
    NewLine,
    MaxSize(f32)
}
impl Default for LineWrap {
    fn default() -> Self {
        LineWrap::None
    }
}

#[derive(Copy, Clone, Default, Debug, DeTokSplat, DeTok)]
pub struct Layout {
    pub padding: Padding,
    pub align: Align,
    pub direction: Direction,
    pub line_wrap: LineWrap,
    pub new_line_padding: f32,
    pub abs_origin: Option<Vec2>,
    pub abs_size: Option<Vec2>,
    pub walk: Walk,
}

#[derive(Copy, Clone, Default, Debug, DeTokSplat, DeTok)]
pub struct Walk {
    pub margin: Margin,
    pub width: Width,
    pub height: Height,
}

impl Walk {
    pub fn wh(w: Width, h: Height) -> Self {
        Self {
            width: w,
            height: h,
            margin: Margin::zero(),
        }
    }
}

impl Layout {
    pub fn abs_origin_zero() -> Self {
        Layout {
            abs_origin: Some(Vec2::default()),
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Default, Debug, DeTokSplat, DeTok)]
pub struct Align {
    pub fx: f32,
    pub fy: f32
}

impl Align {
    pub fn left_top() -> Align {Align {fx: 0., fy: 0.}}
    pub fn center_top() -> Align {Align {fx: 0.5, fy: 0.0}}
    pub fn right_top() -> Align {Align {fx: 1.0, fy: 0.0}}
    pub fn left_center() -> Align {Align {fx: 0.0, fy: 0.5}}
    pub fn center() -> Align {Align {fx: 0.5, fy: 0.5}}
    pub fn right_center() -> Align {Align {fx: 1.0, fy: 0.5}}
    pub fn left_bottom() -> Align {Align {fx: 0., fy: 1.0}}
    pub fn center_bottom() -> Align {Align {fx: 0.5, fy: 1.0}}
    pub fn right_bottom() -> Align {Align {fx: 1.0, fy: 1.0}}
}

#[derive(Clone, Copy, Default, Debug, DeTokSplat, DeTok)]
pub struct Margin {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32
}

impl Margin {
    pub fn zero() -> Margin {
        Margin {l: 0.0, t: 0.0, r: 0.0, b: 0.0}
    }
    
    pub fn all(v: f32) -> Margin {
        Margin {l: v, t: v, r: v, b: v}
    }
    
    pub fn left(v: f32) -> Margin {
        Margin {l: v, t: 0.0, r: 0.0, b: 0.0}
    }
    
    pub fn top(v: f32) -> Margin {
        Margin {l: 0.0, t: v, r: 0.0, b: 0.0}
    }
    
    pub fn right(v: f32) -> Margin {
        Margin {l: 0.0, t: 0.0, r: v, b: 0.0}
    }
    
    pub fn bottom(v: f32) -> Margin {
        Margin {l: 0.0, t: 0.0, r: 0.0, b: v}
    }
    
}

#[derive(Clone, Copy, Default, Debug, DeTokSplat, DeTok)]
pub struct Padding {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32
}

impl Padding {
    pub fn zero() -> Padding {
        Padding {l: 0.0, t: 0.0, r: 0.0, b: 0.0}
    }
    pub fn all(v: f32) -> Padding {
        Padding {l: v, t: v, r: v, b: v}
    }
}


#[derive(Copy, Clone, Debug, DeTok)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down
}

impl Default for Direction {
    fn default() -> Self {
        Direction::Right
    }
}

#[derive(Copy, Clone, SerRon, DeRon, DeTok)]
pub enum Axis {
    Horizontal,
    Vertical
}

impl Default for Axis {
    fn default() -> Self {
        Axis::Horizontal
    }
}


#[derive(Copy, Clone, Debug, DeTok)]
pub enum Width {
    Fill,
    Fix(f32),
    Compute,
    ComputeFill,
    FillPad(f32),
    FillScale(f32),
    FillScalePad(f32, f32),
    Scale(f32),
    ScalePad(f32, f32),
}

#[derive(Copy, Clone, Debug, DeTok)]
pub enum Height {
    Fill,
    Fix(f32),
    Compute,
    ComputeFill,
    FillPad(f32),
    FillScale(f32),
    FillScalePad(f32, f32),
    Scale(f32),
    ScalePad(f32, f32),
}

impl Default for Width {
    fn default() -> Self {
        Width::Fill
    }
}


impl Default for Height {
    fn default() -> Self {
        Height::Fill
    }
}


impl Width {
    pub fn fixed(&self) -> f32 {
        match self {
            Width::Fix(v) => *v,
            _ => 0.
        }
    }
    
}

impl Height {
    pub fn fixed(&self) -> f32 {
        match self {
            Height::Fix(v) => *v,
            _ => 0.
        }
    }
}


#[derive(Clone, Copy, Default, Debug, PartialEq, SerRon, DeRon, DeTokSplat, DeTok)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32
}

impl Rect {
    
    pub fn contains(&self, x: f32, y: f32) -> bool {
        return x >= self.x && x <= self.x + self.w &&
        y >= self.y && y <= self.y + self.h;
    }
    pub fn intersects(&self, r: Rect) -> bool {
        !(
            r.x > self.x + self.w ||
            r.x + r.w < self.x ||
            r.y > self.y + self.h ||
            r.y + r.h < self.y
        )
    }
    
    pub fn contains_with_margin(&self, x: f32, y: f32, margin: &Option<Margin>) -> bool {
        if let Some(margin) = margin {
            return x >= self.x - margin.l && x <= self.x + self.w + margin.r &&
            y >= self.y - margin.t && y <= self.y + self.h + margin.b;
        }
        else {
            return self.contains(x, y);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Anim {
    pub play: Play,
    pub tracks: Vec<Track>
}

#[derive(Clone, DeTok, Debug)]
pub enum Ease {
    Lin,
    InQuad,
    OutQuad,
    InOutQuad,
    InCubic,
    OutCubic,
    InOutCubic,
    InQuart,
    OutQuart,
    InOutQuart,
    InQuint,
    OutQuint,
    InOutQuint,
    InSine,
    OutSine,
    InOutSine,
    InExp,
    OutExp,
    InOutExp,
    InCirc,
    OutCirc,
    InOutCirc,
    InElastic,
    OutElastic,
    InOutElastic,
    InBack,
    OutBack,
    InOutBack,
    InBounce,
    OutBounce,
    InOutBounce,
    Pow {begin: f64, end: f64},
    Bezier {cp0: f64, cp1: f64, cp2: f64, cp3: f64}
    /*
    Bounce{dampen:f64},
    Elastic{duration:f64, frequency:f64, decay:f64, ease:f64}, 
    */
}


impl Ease {
    pub fn map(&self, t: f64) -> f64 {
        match self {
            Ease::Lin => {
                return t.max(0.0).min(1.0);
            },
            Ease::Pow {begin, end} => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }
                let a = -1. / (begin * begin).max(1.0);
                let b = 1. + 1. / (end * end).max(1.0);
                let t2 = (((a - 1.) * -b) / (a * (1. - b))).powf(t);
                return (-a * b + b * a * t2) / (a * t2 - b);
            },
            
            Ease::InQuad => {
                return t * t;
            },
            Ease::OutQuad => {
                return t * (2.0 - t);
            },
            Ease::InOutQuad => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t;
                }
                else {
                    let t = t - 1.;
                    return -0.5 * (t * (t - 2.) - 1.);
                }
            },
            Ease::InCubic => {
                return t * t * t;
            },
            Ease::OutCubic => {
                let t2 = t - 1.0;
                return t2 * t2 * t2 + 1.0;
            },
            Ease::InOutCubic => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return 1. / 2. * (t * t * t + 2.);
                }
            },
            Ease::InQuart => {
                return t * t * t * t
            },
            Ease::OutQuart => {
                let t = t - 1.;
                return -(t * t * t * t - 1.);
            },
            Ease::InOutQuart => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return -0.5 * (t * t * t * t - 2.);
                }
            },
            Ease::InQuint => {
                return t * t * t * t * t;
            },
            Ease::OutQuint => {
                let t = t - 1.;
                return t * t * t * t * t + 1.;
            },
            Ease::InOutQuint => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return 0.5 * (t * t * t * t * t + 2.);
                }
            },
            Ease::InSine => {
                return -(t * PI * 0.5).cos() + 1.;
            },
            Ease::OutSine => {
                return (t * PI * 0.5).sin();
            },
            Ease::InOutSine => {
                return -0.5 * ((t * PI).cos() - 1.);
            },
            Ease::InExp => {
                if t < 0.001 {
                    return 0.;
                }
                else {
                    return 2.0f64.powf(10. * (t - 1.));
                }
            },
            Ease::OutExp => {
                if t > 0.999 {
                    return 1.;
                }
                else {
                    return -(2.0f64.powf(-10. * t)) + 1.;
                }
            },
            Ease::InOutExp => {
                if t<0.001 {
                    return 0.;
                }
                if t>0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * 2.0f64.powf(10. * (t - 1.));
                }
                else {
                    let t = t - 1.;
                    return 0.5 * (-(2.0f64.powf(-10. * t)) + 2.);
                }
            },
            Ease::InCirc => {
                return -((1. - t * t).sqrt() - 1.);
            },
            Ease::OutCirc => {
                let t = t - 1.;
                return (1. - t * t).sqrt();
            },
            Ease::InOutCirc => {
                let t = t * 2.;
                if t < 1. {
                    return -0.5 * ((1. - t * t).sqrt() - 1.);
                }
                else {
                    let t = t - 2.;
                    return 0.5 * ((1. - t * t).sqrt() + 1.);
                }
            },
            Ease::InElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t - 1.0;
                return -(2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
            },
            Ease::OutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                return 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
            },
            Ease::InOutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    let t = t - 1.0;
                    return -0.5 * (2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
                }
                else {
                    let t = t - 1.0;
                    return 0.5 * 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
                }
            },
            Ease::InBack => {
                let s = 1.70158;
                return t * t * ((s + 1.) * t - s);
            },
            Ease::OutBack => {
                let s = 1.70158;
                let t = t - 1.;
                return t * t * ((s + 1.) * t + s) + 1.;
            },
            Ease::InOutBack => {
                let s = 1.70158;
                let t = t * 2.0;
                if t < 1. {
                    let s = s * 1.525;
                    return 0.5 * (t * t * ((s + 1.) * t - s));
                }
                else {
                    let t = t - 2.;
                    return 0.5 * (t * t * ((s + 1.) * t + s) + 2.);
                }
            },
            Ease::InBounce => {
                return 1.0 - Ease::OutBounce.map(1.0 - t);
            },
            Ease::OutBounce => {
                if t < (1. / 2.75) {
                    return 7.5625 * t * t;
                }
                if t < (2. / 2.75) {
                    let t = t - (1.5 / 2.75);
                    return 7.5625 * t * t + 0.75;
                }
                if t < (2.5 / 2.75) {
                    let t = t - (2.25 / 2.75);
                    return 7.5625 * t * t + 0.9375;
                }
                let t = t - (2.625 / 2.75);
                return 7.5625 * t * t + 0.984375;
            },
            Ease::InOutBounce => {
                if t <0.5 {
                    return Ease::InBounce.map(t * 2.) * 0.5;
                }
                else {
                    return Ease::OutBounce.map(t * 2. - 1.) * 0.5 + 0.5;
                }
            },
            /* forgot the parameters to these functions
            Ease::Bounce{dampen}=>{
                if time < 0.{
                    return 0.;
                }
                if time > 1. {
                    return 1.;
                }

                let it = time * (1. / (1. - dampen)) + 0.5;
                let inlog = (dampen - 1.) * it + 1.0;
                if inlog <= 0. {
                    return 1.
                }
                let k = (inlog.ln() / dampen.ln()).floor();
                let d = dampen.powf(k);
                return 1. - (d * (it - (d - 1.) / (dampen - 1.)) - (it - (d - 1.) / (dampen - 1.)).powf(2.)) * 4.
            },
            Ease::Elastic{duration, frequency, decay, ease}=>{
                if time < 0.{
                    return 0.;
                }
                if time > 1. {
                    return 1.;
                }
                let mut easein = *ease;
                let mut easeout = 1.;
                if *ease < 0. {
                    easeout = -ease;
                    easein = 1.;
                }
                
                if time < *duration{
                    return Ease::Pow{begin:easein, end:easeout}.map(time / duration)
                }
                else {
                    // we have to snap the frequency so we end at 0
                    let w = ((0.5 + (1. - duration) * frequency * 2.).floor() / ((1. - duration) * 2.)) * std::f64::consts::PI * 2.;
                    let velo = (Ease::Pow{begin:easein, end:easeout}.map(1.001) - Ease::Pow{begin:easein, end:easeout}.map(1.) ) / (0.001 * duration);
                    return 1. + velo * ((((time - duration) * w).sin() / ((time - duration) * decay).exp()) / w)
                }
            },*/
            
            Ease::Bezier {cp0, cp1, cp2, cp3} => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }
                
                if (cp0 - cp1).abs() < 0.001 && (cp2 - cp3).abs() < 0.001 {
                    return t;
                }
                
                let epsilon = 1.0 / 200.0 * t;
                let cx = 3.0 * cp0;
                let bx = 3.0 * (cp2 - cp0) - cx;
                let ax = 1.0 - cx - bx;
                let cy = 3.0 * cp1;
                let by = 3.0 * (cp3 - cp1) - cy;
                let ay = 1.0 - cy - by;
                let mut u = t;
                
                for _i in 0..6 {
                    let x = ((ax * u + bx) * u + cx) * u - t;
                    if x.abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }
                    let d = (3.0 * ax * u + 2.0 * bx) * u + cx;
                    if d.abs() < 1e-6 {
                        break;
                    }
                    u = u - x / d;
                };
                
                if t > 1. {
                    return (ay + by) + cy;
                }
                if t < 0. {
                    return 0.0;
                }
                
                let mut w = 0.0;
                let mut v = 1.0;
                u = t;
                for _i in 0..8 {
                    let x = ((ax * u + bx) * u + cx) * u;
                    if (x - t).abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }
                    
                    if t > x {
                        w = u;
                    }
                    else {
                        v = u;
                    }
                    u = (v - w) * 0.5 + w;
                }
                
                return ((ay * u + by) * u + cy) * u;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Track {
    Float {
        live_id: LiveId,
        ease: Ease,
        cut_init: Option<f32>,
        keys: Vec<(f64, f32)>
    },
    Vec2 {
        live_id: LiveId,
        ease: Ease,
        cut_init: Option<Vec2>,
        keys: Vec<(f64, Vec2)>
    },
    Vec3 {
        live_id: LiveId,
        ease: Ease,
        cut_init: Option<Vec3>,
        keys: Vec<(f64, Vec3)>
    },
    Vec4 {
        live_id: LiveId,
        ease: Ease,
        cut_init: Option<Vec4>,
        keys: Vec<(f64, Vec4)>
    },
    Color {
        live_id: LiveId,
        ease: Ease,
        cut_init: Option<Color>,
        keys: Vec<(f64, Color)>
    },
}

impl Track {
    
    pub fn compute_track_float(time: f64, track: &Vec<(f64, f32)>, cut_init: &mut Option<f32>, init: f32, ease: &Ease) -> f32 {
        if track.is_empty() {return init}
        fn lerp(a: f32, b: f32, f: f32) -> f32 {
            return a * (1.0 - f) + b * f;
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    pub fn compute_track_vec2(time: f64, track: &Vec<(f64, Vec2)>, cut_init: &mut Option<Vec2>, init: Vec2, ease: &Ease) -> Vec2 {
        if track.is_empty() {return init}
        fn lerp(a: Vec2, b: Vec2, f: f32) -> Vec2 {
            let nf = 1.0 - f;
            return Vec2 {x: a.x * nf + b.x * f, y: a.y * nf + b.y * f}
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    pub fn compute_track_vec3(time: f64, track: &Vec<(f64, Vec3)>, cut_init: &mut Option<Vec3>, init: Vec3, ease: &Ease) -> Vec3 {
        if track.is_empty() {return init}
        fn lerp(a: Vec3, b: Vec3, f: f32) -> Vec3 {
            let nf = 1.0 - f;
            return Vec3 {x: a.x * nf + b.x * f, y: a.y * nf + b.y * f, z: a.z * nf + b.z * f}
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    pub fn compute_track_vec4(time: f64, track: &Vec<(f64, Vec4)>, cut_init: &mut Option<Vec4>, init: Vec4, ease: &Ease) -> Vec4 {
        if track.is_empty() {return init}
        fn lerp(a: Vec4, b: Vec4, f: f32) -> Vec4 {
            let nf = 1.0 - f;
            return Vec4 {x: a.x * nf + b.x * f, y: a.y * nf + b.y * f, z: a.z * nf + b.z * f, w: a.w * nf + b.w * f}
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    pub fn compute_track_color(time: f64, track: &Vec<(f64, Color)>, cut_init: &mut Option<Color>, init: Color, ease: &Ease) -> Color {
        if track.is_empty() {return init}
        fn lerp(a: Color, b: Color, f: f32) -> Color {
            let nf = 1.0 - f;
            return Color {r: a.r * nf + b.r * f, g: a.g * nf + b.g * f, b: a.b * nf + b.b * f, a: a.a * nf + b.a * f}
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    pub fn live_id(&self) -> LiveId {
        match self {
            Track::Float{live_id,..} => {
                *live_id
            },
            Track::Vec2{live_id,..} => {
                *live_id
            }
            Track::Vec3{live_id,..} => {
                *live_id
            }
            Track::Vec4{live_id,..} => {
                *live_id
            }
            Track::Color{live_id,..} => {
                *live_id
            }
        }
    }
    
    pub fn reset_cut_init(&mut self) {
        match self {
            Track::Color{cut_init,..} => {
                *cut_init = None;
            },
            Track::Vec4{cut_init,..} => {
                *cut_init = None;
            },
            Track::Vec3{cut_init,..} => {
                *cut_init = None;
            },
            Track::Vec2{cut_init,..} => {
                *cut_init = None;
            },
            Track::Float{cut_init,..} => {
                *cut_init = None;
            }
        }
    }
    
    pub fn ease(&self) -> &Ease {
        match self {
            Track::Float{ease, ..} => {
                ease
            },
            Track::Vec2{ease, ..} => {
                ease
            }
            Track::Vec3{ease, ..} => {
                ease
            }
            Track::Vec4{ease, ..} => {
                ease
            }
            Track::Color{ease, ..} => {
                ease
            }
        }
    }
    
    
    pub fn set_ease(&mut self, new_ease: Ease) {
        match self {
            Track::Float{ease, ..} => {
                *ease = new_ease
            },
            Track::Vec2{ease, ..} => {
                *ease = new_ease
            },
            Track::Vec3{ease, ..} => {
                *ease = new_ease
            },
            Track::Vec4{ease, ..} => {
                *ease = new_ease
            },
            Track::Color{ease, ..} => {
                *ease = new_ease
            },
        }
    }
    
    pub fn set_live_id(&mut self, new_live_id: LiveId) {
        match self {
            Track::Float{live_id, ..} => {
                *live_id = new_live_id
            },
            Track::Vec2{live_id, ..} => {
                *live_id = new_live_id
            },
            Track::Vec3{live_id, ..} => {
                *live_id = new_live_id
            },
            Track::Vec4{live_id, ..} => {
                *live_id = new_live_id
            },
            Track::Color{live_id, ..} => {
                *live_id = new_live_id
            },
        }
    }
}

impl Anim {
    pub fn empty() -> Anim {
        Anim {
            play: Play::Cut {duration: 0.},
            tracks: vec![]
        }
    }
}

#[derive(Clone, DeTok, Debug)]
pub enum Play {
    Chain {duration: f64},
    Cut {duration: f64},
    Single {duration: f64, cut: bool, term: bool, end: f64},
    Loop {duration: f64, cut: bool, term: bool, repeats: f64, end: f64},
    Reverse {duration: f64, cut: bool, term: bool, repeats: f64, end: f64},
    Bounce {duration: f64, cut: bool, term: bool, repeats: f64, end: f64},
    Forever {duration: f64, cut: bool, term: bool},
    LoopForever {duration: f64, cut: bool, term: bool, end: f64},
    ReverseForever {duration: f64, cut: bool, term: bool, end: f64},
    BounceForever {duration: f64, cut: bool, term: bool, end: f64},
}

impl Play {
    pub fn duration(&self) -> f64 {
        match self {
            Play::Chain {duration, ..} => *duration,
            Play::Cut {duration, ..} => *duration,
            Play::Single {duration, ..} => *duration,
            Play::Loop {duration, ..} => *duration,
            Play::Reverse {duration, ..} => *duration,
            Play::Bounce {duration, ..} => *duration,
            Play::BounceForever {duration, ..} => *duration,
            Play::Forever {duration, ..} => *duration,
            Play::LoopForever {duration, ..} => *duration,
            Play::ReverseForever {duration, ..} => *duration,
        }
    }
    pub fn total_time(&self) -> f64 {
        match self {
            Play::Chain {duration, ..} => *duration,
            Play::Cut {duration, ..} => *duration,
            Play::Single {end, duration, ..} => end * duration,
            Play::Loop {end, duration, repeats, ..} => end * duration * repeats,
            Play::Reverse {end, duration, repeats, ..} => end * duration * repeats,
            Play::Bounce {end, duration, repeats, ..} => end * duration * repeats,
            Play::BounceForever {..} => std::f64::INFINITY,
            Play::Forever {..} => std::f64::INFINITY,
            Play::LoopForever {..} => std::f64::INFINITY,
            Play::ReverseForever {..} => std::f64::INFINITY,
        }
    }
    
    pub fn cut(&self) -> bool {
        match self {
            Play::Cut {..} => true,
            Play::Chain {..} => false,
            Play::Single {cut, ..} => *cut,
            Play::Loop {cut, ..} => *cut,
            Play::Reverse {cut, ..} => *cut,
            Play::Bounce {cut, ..} => *cut,
            Play::BounceForever {cut, ..} => *cut,
            Play::Forever {cut, ..} => *cut,
            Play::LoopForever {cut, ..} => *cut,
            Play::ReverseForever {cut, ..} => *cut,
        }
    }
    
    pub fn repeats(&self) -> f64 {
        match self {
            Play::Chain {..} => 1.0,
            Play::Cut {..} => 1.0,
            Play::Single {..} => 1.0,
            Play::Loop {repeats, ..} => *repeats,
            Play::Reverse {repeats, ..} => *repeats,
            Play::Bounce {repeats, ..} => *repeats,
            Play::BounceForever {..} => std::f64::INFINITY,
            Play::Forever {..} => std::f64::INFINITY,
            Play::LoopForever {..} => std::f64::INFINITY,
            Play::ReverseForever {..} => std::f64::INFINITY,
        }
    }
    
    pub fn term(&self) -> bool {
        match self {
            Play::Cut {..} => false,
            Play::Chain {..} => false,
            Play::Single {term, ..} => *term,
            Play::Loop {term, ..} => *term,
            Play::Reverse {term, ..} => *term,
            Play::Bounce {term, ..} => *term,
            Play::BounceForever {term, ..} => *term,
            Play::Forever {term, ..} => *term,
            Play::LoopForever {term, ..} => *term,
            Play::ReverseForever {term, ..} => *term,
        }
    }
    
    pub fn compute_time(&self, time: f64) -> f64 {
        match self {
            Play::Cut {duration, ..} => {
                time / duration
            },
            Play::Chain {duration, ..} => {
                time / duration
            },
            Play::Single {duration, ..} => {
                time / duration
            },
            Play::Loop {end, duration, ..} => {
                (time / duration) % end
            },
            Play::Reverse {end, duration, ..} => {
                end - (time / duration) % end
            },
            Play::Bounce {end, duration, ..} => {
                let mut local_time = (time / duration) % (end * 2.0);
                if local_time > *end {
                    local_time = 2.0 * end - local_time;
                };
                local_time
            },
            Play::BounceForever {end, duration, ..} => {
                let mut local_time = (time / duration) % (end * 2.0);
                if local_time > *end {
                    local_time = 2.0 * end - local_time;
                };
                local_time
            },
            Play::Forever {duration, ..} => {
                let local_time = time / duration;
                local_time
            },
            Play::LoopForever {end, duration, ..} => {
                let local_time = (time / duration) % end;
                local_time
            },
            Play::ReverseForever {end, duration, ..} => {
                let local_time = end - (time / duration) % end;
                local_time
            },
        }
    }
}


pub const fn live_location_hash(path: &str, line: u64, col: u64) -> u64 {
    // lets hash the path
    let path = path.as_bytes();
    let path_len = path.len();
    let mut i = 0;
    let mut o = 0;
    let mut value = 0u64;
    while i < path_len {
        value ^= (path[i] as u64) << ((o & 7) << 3);
        o += 1;
        i += 1;
    }
        value ^= line;
    value ^= col << 32;
    value
}

pub const fn live_str_to_id(modstr: &str, idstr: &str) -> LiveId {
    let modpath = modstr.as_bytes();
    let modpath_len = modpath.len();
    let id = idstr.as_bytes();
    let id_len = id.len();
    
    let mut value = 0u64;
    if id.len()>5
        && id[0] == 's' as u8
        && id[1] == 'e' as u8
        && id[2] == 'l' as u8
        && id[3] == 'f' as u8
        && id[4] == ':' as u8 {
        
        let mut o = 0;
        let mut i = 0;
        while i < modpath_len {
            value ^= (modpath[i] as u64) << ((o & 7) << 3);
            o += 1;
            i += 1;
        }
        let mut i = 4;
        while i < id_len {
            value ^= (id[i] as u64) << ((o & 7) << 3);
            o += 1;
            i += 1;
        }
        return LiveId(value)
    }
    if id.len()>6
        && id[0] == 'c' as u8
        && id[1] == 'r' as u8
        && id[2] == 'a' as u8
        && id[3] == 't' as u8
        && id[4] == 'e' as u8
        && id[5] == ':' as u8 {
        let mut o = 0;
        let mut i = 0;
        while i < modpath_len {
            if modpath[i] == ':' as u8 {
                break
            }
            value ^= (modpath[i] as u64) << ((o & 7) << 3);
            o += 1;
            i += 1;
        }
        let mut i = 5;
        while i < id_len {
            value ^= (id[i] as u64) << ((o & 7) << 3);
            o += 1;
            i += 1;
        }
        return LiveId(value)
    }
    let mut i = 0;
    let mut o = 0;
    while i < id_len {
        value ^= (id[i] as u64) << ((o & 7) << 3);
        o += 1;
        i += 1;
    }
        LiveId(value)
}

