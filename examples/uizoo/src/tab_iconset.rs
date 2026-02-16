use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoIconSet = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# IconSet\n\nIconSet displays font-based icons."}
        }
        demos +: {
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
}
