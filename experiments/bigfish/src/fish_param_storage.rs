use crate::makepad_micro_serde::*;

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishParamStorage {
    pub id: i32,
    pub name: String,
    pub int_value: i32,
    pub float_value: f32,
    pub text_value: String,
}

#[derive(Clone, Copy, Debug, SerRon, DeRon, Default)]
pub enum FishParameterType {
    Float,
    Int,
    Bool,
    String,
    VolumeDB,
    RatioDB,
    Frequency,
    Ratio,
    CircularDegrees,
    ColourHue,
    ColourSaturation,
    ColourValue,
    ColourR,
    ColourG,
    ColourB,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishParamDescriptor {
    pub id: i32,
    pub name: String,
    pub parameter_type: FishParameterType,
    pub default_value: FishParamStorage,
    pub min_value: FishParamStorage,
    pub max_value: FishParamStorage,
}
