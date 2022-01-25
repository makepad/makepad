use {
    crate::makepad_math::*,
};

#[derive(Clone, Debug, Default,  PartialEq)]
pub struct XRButton {
    pub value: f32,
    pub pressed: bool
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct XRInput {
    pub active: bool,
    pub grip: Transform,
    pub ray: Transform,
    pub num_buttons: usize,
    pub buttons: [XRButton; 8],
    pub num_axes: usize,
    pub axes: [f32; 8],
}

#[derive(Clone, Debug, PartialEq)]
pub struct XRUpdateEvent {
    // alright what data are we stuffing in
    pub time: f64,
    pub head_transform: Transform,
    pub left_input: XRInput,
    pub last_left_input: XRInput,
    pub right_input: XRInput,
    pub last_right_input: XRInput,
    pub other_inputs: Vec<XRInput>
}
