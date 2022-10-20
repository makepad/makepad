// shader math glue layer

pub trait ShaderMath{
    fn abs(self)->Self;
    
    fn sin(self)->Self;
    fn cos(self)->Self;
    fn tan(self)->Self;
    fn asin(self)->Self;
    fn acos(self)->Self;
    fn atan(self)->Self;
    
    fn sinh(self)->Self;
    fn cosh(self)->Self;
    fn tanh(self)->Self;
    fn asinh(self)->Self;
    fn acosh(self)->Self;
    fn atanh(self)->Self;

    fn fract(self)->Self;
    fn ceil(self)->Self;
    fn floor(self)->Self;
    fn min(self, v:Self)->Self;
    fn max(self, v:Self)->Self;
    fn clamp(self, low:Self, high:Self)->Self;
    fn exp(self)->Self;
    fn exp2(self)->Self;
    fn ln(self)->Self;
    fn log2(self)->Self;
    fn log10(self)->Self;
    fn powf(self, v:Self)->Self;
    fn powi(self, p:i32)->Self;
}

impl ShaderMath for f32{
    fn abs(self)->Self{self.abs()}
    fn sin(self)->Self{self.sin()}
    fn cos(self)->Self{self.cos()}
    fn tan(self)->Self{self.tan()}
    fn asin(self)->Self{self.asin()}
    fn acos(self)->Self{self.acos()}
    fn atan(self)->Self{self.atan()}

    fn sinh(self)->Self{self.sinh()}
    fn cosh(self)->Self{self.cosh()}
    fn tanh(self)->Self{self.tanh()}
    fn asinh(self)->Self{self.asinh()}
    fn acosh(self)->Self{self.acosh()}
    fn atanh(self)->Self{self.atanh()}

    fn fract(self)->Self{self.fract()}
    fn ceil(self)->Self{self.ceil()}
    fn floor(self)->Self{self.floor()}
    fn min(self, v:Self)->Self{self.min(v)}
    fn max(self, v:Self)->Self{self.max(v)}
    fn clamp(self, low:Self, high:Self)->Self{self.max(low).min(high)}
    fn exp(self)->Self{self.exp()}
    fn exp2(self)->Self{self.exp2()}
    fn ln(self)->Self{self.ln()}
    fn log2(self)->Self{self.log2()}
    fn log10(self)->Self{self.log10()}
    fn powf(self, v:Self)->Self{self.powf(v)}
    fn powi(self, v:i32)->Self{self.powi(v)}
}

impl ShaderMath for f64{
    fn abs(self)->Self{self.abs()}

    fn sin(self)->Self{self.sin()}
    fn cos(self)->Self{self.cos()}
    fn tan(self)->Self{self.tan()}
    fn asin(self)->Self{self.asin()}
    fn acos(self)->Self{self.acos()}
    fn atan(self)->Self{self.atan()}

    fn sinh(self)->Self{self.sinh()}
    fn cosh(self)->Self{self.cosh()}
    fn tanh(self)->Self{self.tanh()}
    fn asinh(self)->Self{self.asinh()}
    fn acosh(self)->Self{self.acosh()}
    fn atanh(self)->Self{self.atanh()}


    fn fract(self)->Self{self.fract()}
    fn ceil(self)->Self{self.ceil()}
    fn floor(self)->Self{self.floor()}
    fn min(self, v:Self)->Self{self.min(v)}
    fn max(self, v:Self)->Self{self.max(v)}
    fn clamp(self, low:Self, high:Self)->Self{self.max(low).min(high)}
    fn exp(self)->Self{self.exp()}
    fn exp2(self)->Self{self.exp2()}
    fn ln(self)->Self{self.ln()}
    fn log2(self)->Self{self.log2()}
    fn log10(self)->Self{self.log10()}
    fn powf(self, v:Self)->Self{self.powf(v)}
    fn powi(self, v:i32)->Self{self.powi(v)}
}

pub fn abs<T:ShaderMath>(v:T)->T{v.abs()}

pub fn sin<T:ShaderMath>(v:T)->T{v.sin()}
pub fn cos<T:ShaderMath>(v:T)->T{v.cos()}
pub fn tan<T:ShaderMath>(v:T)->T{v.tan()}
pub fn asin<T:ShaderMath>(v:T)->T{v.asin()}
pub fn acos<T:ShaderMath>(v:T)->T{v.acos()}
pub fn atan<T:ShaderMath>(v:T)->T{v.atan()}

pub fn sinh<T:ShaderMath>(v:T)->T{v.sinh()}
pub fn cosh<T:ShaderMath>(v:T)->T{v.cosh()}
pub fn tanh<T:ShaderMath>(v:T)->T{v.tanh()}
pub fn asinh<T:ShaderMath>(v:T)->T{v.asinh()}
pub fn acosh<T:ShaderMath>(v:T)->T{v.acosh()}
pub fn atanh<T:ShaderMath>(v:T)->T{v.atanh()}

pub fn fract<T:ShaderMath>(v:T)->T{v.fract()}
pub fn ceil<T:ShaderMath>(v:T)->T{v.ceil()}
pub fn floor<T:ShaderMath>(v:T)->T{v.floor()}
pub fn min<T:ShaderMath>(v:T,l:T)->T{v.min(l)}
pub fn max<T:ShaderMath>(v:T,l:T)->T{v.max(l)}
pub fn clamp<T:ShaderMath>(v:T,l:T,h:T)->T{v.clamp(l,h)}
pub fn exp<T:ShaderMath>(v:T)->T{v.exp()}
pub fn exp2<T:ShaderMath>(v:T)->T{v.exp2()}
pub fn ln<T:ShaderMath>(v:T)->T{v.ln()}
pub fn log2<T:ShaderMath>(v:T)->T{v.log2()}
pub fn log10<T:ShaderMath>(v:T)->T{v.log10()}
pub fn pow<T:ShaderMath>(v:T,l:T)->T{v.powf(l)}
pub fn powf<T:ShaderMath>(v:T,l:T)->T{v.powf(l)}
pub fn powi<T:ShaderMath>(v:T,l:i32)->T{v.powi(l)}

/*
abs
asin
acos
atan
sin
cos
tan
ceil
floor
min
max
clamp

exp
exp2
log
log2
ln
pow
tanh
fract

cross
distance
dot
inverse
length
normalize
*/