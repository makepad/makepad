//! One module per component. Each registers its templates under
//! `mod.stories` and exports a table of records; `tables()` lists them in
//! navigator order and `script_mod` registers them in the same order.
use crate::makepad_widgets::*;
use crate::registry::Story;
use std::collections::HashMap;
use std::sync::Mutex;

pub mod badge;
pub mod basics;
pub mod foundations;
pub mod placeholder;
pub mod button_more;
pub mod checkbox_more;
pub mod alert;
pub mod divider;
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
pub mod markdown;
pub mod html;
pub mod slug;
pub mod pageflip;
pub mod stacknavigation;
pub mod adaptiveview;
pub mod slidesview;
pub mod scrollbar;
pub mod portallist;
pub mod filetree;
pub mod spinner;
pub mod rotary;
pub mod video;

pub fn script_mod(vm: &mut ScriptVm) {
    welcome::script_mod(vm);
    crate::coverage::script_mod(vm);
    foundations::script_mod(vm);
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
    markdown::script_mod(vm);
    html::script_mod(vm);
    slug::script_mod(vm);
    pageflip::script_mod(vm);
    stacknavigation::script_mod(vm);
    adaptiveview::script_mod(vm);
    slidesview::script_mod(vm);
    scrollbar::script_mod(vm);
    portallist::script_mod(vm);
    filetree::script_mod(vm);
    spinner::script_mod(vm);
    rotary::script_mod(vm);
    video::script_mod(vm);
    basics::script_mod(vm);
    badge::script_mod(vm);
    placeholder::script_mod(vm);
    checkbox_more::script_mod(vm);
    button_more::script_mod(vm);
    alert::script_mod(vm);
    divider::script_mod(vm);
}

pub fn tables() -> &'static [&'static [Story]] {
    &[
        welcome::STORIES,
        crate::coverage::STORIES,
        foundations::STORIES,
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
        markdown::STORIES,
        html::STORIES,
        slug::STORIES,
        pageflip::STORIES,
        stacknavigation::STORIES,
        adaptiveview::STORIES,
        slidesview::STORIES,
        scrollbar::STORIES,
        portallist::STORIES,
        filetree::STORIES,
        spinner::STORIES,
        rotary::STORIES,
        video::STORIES,
        basics::STORIES,
        badge::STORIES,
        placeholder::STORIES,
        checkbox_more::STORIES,
        button_more::STORIES,
        alert::STORIES,
        divider::STORIES,
    ]
}

static COUNTERS: Mutex<Option<HashMap<LiveId, usize>>> = Mutex::new(None);

/// A per-story counter for demos that count their own clicks; returns the
/// count after the bump.
pub fn bump(key: LiveId) -> usize {
    let mut guard = COUNTERS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let n = map.entry(key).or_insert(0);
    *n += 1;
    *n
}
