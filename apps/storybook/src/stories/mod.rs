//! One module per component. Each registers its templates under
//! `mod.stories` and exports a table of records; `tables()` lists them in
//! navigator order and `script_mod` registers them in the same order.
use crate::makepad_widgets::*;
use crate::registry::Story;
use std::collections::HashMap;
use std::sync::Mutex;

pub mod welcome;
pub mod slug;
pub mod foundations;
pub mod layout;
pub mod grid;
pub mod masonry;
pub mod splitter_more;
pub mod dock;
pub mod align_scroll;
pub mod scroll_more;
pub mod divider;
pub mod view;
pub mod corner_cap_view;
pub mod card;
pub mod accordion;
pub mod carousel;
pub mod pageflip;
pub mod moving_panels;
pub mod glasspanel;
pub mod glass_surfaces;
pub mod glass_controls;
pub mod splash;
pub mod label;
pub mod typography;
pub mod text_flow;
pub mod html;
pub mod markdown;
pub mod rich_text;
pub mod code_block;
pub mod marquee;
pub mod icon;
pub mod image;
pub mod media;
pub mod image_nine_slice;
pub mod svg;
pub mod vector;
pub mod playback_bar;
pub mod waveform;
pub mod button;
pub mod button_group;
pub mod floating_action;
pub mod textinput;
pub mod field_well;
pub mod number_field;
pub mod slider;
pub mod range_slider;
pub mod rotary;
pub mod rating;
pub mod tag_field;
pub mod date_picker;
pub mod calendar;
pub mod time_picker;
pub mod color;
pub mod dropzone;
pub mod form;
pub mod property_inspector;
pub mod checkbox;
pub mod radio_group;
pub mod select;
pub mod combobox;
pub mod chip;
pub mod wheel_picker;
pub mod column_picker;
pub mod svg_select;
pub mod toolbar;
pub mod window_chrome;
pub mod drop_controls;
pub mod tabs;
pub mod pill_nav;
pub mod nav_list;
pub mod hamburger_menu;
pub mod breadcrumb;
pub mod line_menu;
pub mod pagination;
pub mod stacknavigation;
pub mod tip;
pub mod popover;
pub mod menu;
pub mod pie_menu;
pub mod radial_menu;
pub mod command_palette;
pub mod dialog;
pub mod modal;
pub mod drawer;
pub mod floating_panel;
pub mod tour;
pub mod alert;
pub mod toast;
pub mod progress;
pub mod level_meter;
pub mod spinner;
pub mod placeholder;
pub mod empty_state;
pub mod lists;
pub mod list_item;
pub mod table;
pub mod tree;
pub mod filetree;
pub mod data_grid;
pub mod tile_list;
pub mod item_grid;
pub mod kanban;
pub mod log_list;
pub mod badge;
pub mod avatar;
pub mod kbd;
pub mod chart;
pub mod chart_more;
pub mod timeline;
pub mod chat;

/// Registered in the order the navigator reads, the same order as
/// [`tables`], though no story template leans on another file's.
pub fn script_mod(vm: &mut ScriptVm) {
    // 0 Overview
    welcome::script_mod(vm);
    crate::coverage::script_mod(vm);
    slug::script_mod(vm);
    // 1 Foundations
    foundations::script_mod(vm);
    // 2 Layout
    layout::script_mod(vm);
    grid::script_mod(vm);
    masonry::script_mod(vm);
    splitter_more::script_mod(vm);
    dock::script_mod(vm);
    align_scroll::script_mod(vm);
    scroll_more::script_mod(vm);
    divider::script_mod(vm);
    // 3 Containers
    view::script_mod(vm);
    corner_cap_view::script_mod(vm);
    card::script_mod(vm);
    accordion::script_mod(vm);
    carousel::script_mod(vm);
    pageflip::script_mod(vm);
    moving_panels::script_mod(vm);
    glasspanel::script_mod(vm);
    glass_surfaces::script_mod(vm);
    glass_controls::script_mod(vm);
    splash::script_mod(vm);
    // 4 Text
    label::script_mod(vm);
    typography::script_mod(vm);
    text_flow::script_mod(vm);
    html::script_mod(vm);
    markdown::script_mod(vm);
    rich_text::script_mod(vm);
    code_block::script_mod(vm);
    marquee::script_mod(vm);
    // 5 Media
    icon::script_mod(vm);
    image::script_mod(vm);
    media::script_mod(vm);
    image_nine_slice::script_mod(vm);
    svg::script_mod(vm);
    vector::script_mod(vm);
    playback_bar::script_mod(vm);
    waveform::script_mod(vm);
    // 6 Actions
    button::script_mod(vm);
    button_group::script_mod(vm);
    floating_action::script_mod(vm);
    // 7 Inputs
    textinput::script_mod(vm);
    field_well::script_mod(vm);
    number_field::script_mod(vm);
    slider::script_mod(vm);
    range_slider::script_mod(vm);
    rotary::script_mod(vm);
    rating::script_mod(vm);
    tag_field::script_mod(vm);
    date_picker::script_mod(vm);
    calendar::script_mod(vm);
    time_picker::script_mod(vm);
    color::script_mod(vm);
    dropzone::script_mod(vm);
    form::script_mod(vm);
    property_inspector::script_mod(vm);
    // 8 Selection
    checkbox::script_mod(vm);
    radio_group::script_mod(vm);
    select::script_mod(vm);
    combobox::script_mod(vm);
    chip::script_mod(vm);
    wheel_picker::script_mod(vm);
    column_picker::script_mod(vm);
    svg_select::script_mod(vm);
    // 9 Navigation
    toolbar::script_mod(vm);
    window_chrome::script_mod(vm);
    drop_controls::script_mod(vm);
    tabs::script_mod(vm);
    pill_nav::script_mod(vm);
    nav_list::script_mod(vm);
    hamburger_menu::script_mod(vm);
    breadcrumb::script_mod(vm);
    line_menu::script_mod(vm);
    pagination::script_mod(vm);
    stacknavigation::script_mod(vm);
    // 10 Overlay
    tip::script_mod(vm);
    popover::script_mod(vm);
    menu::script_mod(vm);
    pie_menu::script_mod(vm);
    radial_menu::script_mod(vm);
    command_palette::script_mod(vm);
    dialog::script_mod(vm);
    modal::script_mod(vm);
    drawer::script_mod(vm);
    floating_panel::script_mod(vm);
    tour::script_mod(vm);
    // 11 Feedback
    alert::script_mod(vm);
    toast::script_mod(vm);
    progress::script_mod(vm);
    level_meter::script_mod(vm);
    spinner::script_mod(vm);
    placeholder::script_mod(vm);
    empty_state::script_mod(vm);
    // 12 Collections
    lists::script_mod(vm);
    list_item::script_mod(vm);
    table::script_mod(vm);
    tree::script_mod(vm);
    filetree::script_mod(vm);
    data_grid::script_mod(vm);
    tile_list::script_mod(vm);
    item_grid::script_mod(vm);
    kanban::script_mod(vm);
    log_list::script_mod(vm);
    // 13 Data display
    badge::script_mod(vm);
    avatar::script_mod(vm);
    kbd::script_mod(vm);
    chart::script_mod(vm);
    chart_more::script_mod(vm);
    timeline::script_mod(vm);
    chat::script_mod(vm);
}

/// Every story table, in navigator order: the categories run in the order
/// they are listed here, and so do the components inside each and the pages
/// inside each component. A host page's file comes before the file of its
/// second page, so Overview heads its folder.
pub fn tables() -> &'static [&'static [Story]] {
    &[
        // 0 Overview
        welcome::STORIES,
        crate::coverage::STORIES,
        slug::STORIES,
        // 1 Foundations
        foundations::STORIES,
        // 2 Layout
        layout::STORIES,
        grid::STORIES,
        masonry::STORIES,
        splitter_more::STORIES,
        dock::STORIES,
        align_scroll::STORIES,
        scroll_more::STORIES,
        divider::STORIES,
        // 3 Containers
        view::STORIES,
        corner_cap_view::STORIES,
        card::STORIES,
        accordion::STORIES,
        carousel::STORIES,
        pageflip::STORIES,
        moving_panels::STORIES,
        glasspanel::STORIES,
        glass_surfaces::STORIES,
        glass_controls::STORIES,
        splash::STORIES,
        // 4 Text
        label::STORIES,
        typography::STORIES,
        text_flow::STORIES,
        html::STORIES,
        markdown::STORIES,
        rich_text::STORIES,
        code_block::STORIES,
        marquee::STORIES,
        // 5 Media
        icon::STORIES,
        image::STORIES,
        media::STORIES,
        image_nine_slice::STORIES,
        svg::STORIES,
        vector::STORIES,
        playback_bar::STORIES,
        waveform::STORIES,
        // 6 Actions
        button::STORIES,
        button_group::STORIES,
        floating_action::STORIES,
        // 7 Inputs
        textinput::STORIES,
        field_well::STORIES,
        number_field::STORIES,
        slider::STORIES,
        range_slider::STORIES,
        rotary::STORIES,
        rating::STORIES,
        tag_field::STORIES,
        date_picker::STORIES,
        calendar::STORIES,
        time_picker::STORIES,
        color::STORIES,
        dropzone::STORIES,
        form::STORIES,
        property_inspector::STORIES,
        // 8 Selection
        checkbox::STORIES,
        radio_group::STORIES,
        select::STORIES,
        combobox::STORIES,
        chip::STORIES,
        wheel_picker::STORIES,
        column_picker::STORIES,
        svg_select::STORIES,
        // 9 Navigation
        toolbar::STORIES,
        window_chrome::STORIES,
        drop_controls::STORIES,
        tabs::STORIES,
        pill_nav::STORIES,
        nav_list::STORIES,
        hamburger_menu::STORIES,
        breadcrumb::STORIES,
        line_menu::STORIES,
        pagination::STORIES,
        stacknavigation::STORIES,
        // 10 Overlay
        tip::STORIES,
        popover::STORIES,
        menu::STORIES,
        pie_menu::STORIES,
        radial_menu::STORIES,
        command_palette::STORIES,
        dialog::STORIES,
        modal::STORIES,
        drawer::STORIES,
        floating_panel::STORIES,
        tour::STORIES,
        // 11 Feedback
        alert::STORIES,
        toast::STORIES,
        progress::STORIES,
        level_meter::STORIES,
        spinner::STORIES,
        placeholder::STORIES,
        empty_state::STORIES,
        // 12 Collections
        lists::STORIES,
        list_item::STORIES,
        table::STORIES,
        tree::STORIES,
        filetree::STORIES,
        data_grid::STORIES,
        tile_list::STORIES,
        item_grid::STORIES,
        kanban::STORIES,
        log_list::STORIES,
        // 13 Data display
        badge::STORIES,
        avatar::STORIES,
        kbd::STORIES,
        chart::STORIES,
        chart_more::STORIES,
        timeline::STORIES,
        chat::STORIES,
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
