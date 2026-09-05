//! One module per component. Each registers its templates under
//! `mod.stories` and exports a table of records; `tables()` lists them in
//! navigator order and `script_mod` registers them in the same order.
use crate::makepad_widgets::*;
use crate::registry::Story;

pub mod welcome;
pub mod button;
pub mod checkbox;
pub mod dropdown;
pub mod label;
pub mod slider;
pub mod textinput;
pub mod radiobutton;
pub mod view;
pub mod layout;
pub mod align_scroll;
pub mod icon;
pub mod iconset;
pub mod image;
pub mod imageblend;
pub mod rotatedimage;
pub mod glasspanel;
pub mod linklabel;

pub fn script_mod(vm: &mut ScriptVm) {
    welcome::script_mod(vm);
    button::script_mod(vm);
    checkbox::script_mod(vm);
    dropdown::script_mod(vm);
    label::script_mod(vm);
    slider::script_mod(vm);
    textinput::script_mod(vm);
    radiobutton::script_mod(vm);
    view::script_mod(vm);
    layout::script_mod(vm);
    align_scroll::script_mod(vm);
    icon::script_mod(vm);
    iconset::script_mod(vm);
    image::script_mod(vm);
    imageblend::script_mod(vm);
    rotatedimage::script_mod(vm);
    glasspanel::script_mod(vm);
    linklabel::script_mod(vm);
}

pub fn tables() -> &'static [&'static [Story]] {
    &[
        welcome::STORIES,
        button::STORIES,
        checkbox::STORIES,
        dropdown::STORIES,
        label::STORIES,
        slider::STORIES,
        textinput::STORIES,
        radiobutton::STORIES,
        view::STORIES,
        layout::STORIES,
        align_scroll::STORIES,
        icon::STORIES,
        iconset::STORIES,
        image::STORIES,
        imageblend::STORIES,
        rotatedimage::STORIES,
        glasspanel::STORIES,
        linklabel::STORIES,
    ]
}
