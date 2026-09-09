//! The icon set stories: a row of font icons, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.IconSetOverview = StoryPage{
        flow: Right
        spacing: 30.
        IconSet{text: "\u{f015}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f2bd}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f03e}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f15b}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f030}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f133}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f0c2}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f0d1}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f164}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f118}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f025}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f0f3}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f007}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f075}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f0e0}" draw_text +: {color: #0ff}}
        IconSet{text: "\u{f1b9}" draw_text +: {color: #0ff}}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/iconset/overview",
    category: "Media",
    component: "IconSet",
    also: &[],
    name: "Overview",
    dsl: "IconSetOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# IconSet\n\nIconSet displays font-based icons.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];
