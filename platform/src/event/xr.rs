use {
    crate::makepad_math::*,
};

#[derive(Clone, Debug, Default)]
pub struct XRButton {
    pub value: f32,
    pub pressed: bool
}

#[derive(Clone, Debug, Default)]
pub struct XRInput {
    pub active: bool,
    pub hand: u32,
    pub grip: Transform,
    pub ray: Transform,
    pub buttons: Vec<XRButton>,
    pub axes: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct XRUpdateEvent {
    // alright what data are we stuffing in
    pub time: f64,
    pub head_transform: Transform,
    pub inputs: Vec<XRInput>,
    pub last_inputs: Option<Vec<XRInput>>
}
