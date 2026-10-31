//! The colour story: the picker panel, the same panel under a swatch, the
//! controls it is made of, the strips, and the text field that repairs
//! itself.
use crate::makepad_widgets::color::{
    format_color_hex, ColorAlphaWidgetRefExt, ColorAreaWidgetRefExt, ColorFieldWidgetRefExt,
    ColorPickerWidgetRefExt, ColorSwatchWidgetRefExt, ColorWheelWidgetRefExt,
    PaletteStripWidgetRefExt,
};
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Readout = Label{
        width: Fit
        height: Fit
        draw_text +: {color: theme.color_on_surface_variant}
    }

    let Stack = View{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_1
    }

    mod.stories.ColorOverview = StoryPage{
        StoryNote{text: "One colour, and the several ways a person reaches for it. The ring picks a hue, the square picks how strong and how bright it is, the strip picks how much of it there is, and the six rows of numbers say all of that again for anyone who arrived with a value already in mind. Every one of them is a widget on its own; the picker is what you get when they are put in a panel together."}
        StoryHeading{text: "One picker, under the controls"}
        StoryNote{text: "One picker under the controls. Turn the alpha strip off for a control that chooses a colour and not a transparency; turn the recent strip off where there is no session worth remembering."}
        subject := ColorPicker{color: #x3B82F6FF}

        StoryHeading{text: "The panel"}
        StoryNote{text: "Wheel, square, alpha strip, three rows of numbers and the last few colours committed anywhere in the session. It reports twice: while the colour is moving under your hand, and once when the gesture ends. A host that wrote to a document on every report would write hundreds of times across one drag."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            spacing: theme.space_3
            inline := ColorPicker{color: #x3B82F6FF}
            Stack{
                following := Readout{text: "nothing moved yet"}
                recorded := Label{text: "nothing recorded yet"}
            }
        }

        StoryHeading{text: "The rows of numbers are tracks"}
        StoryNote{text: "Each row is a filled bar, and a filled bar is a promise: press it a third of the way along and the channel goes to a third of its range, right there, with the drag keeping it under the pointer until you let go. The two ends of the fill are the two ends of the range, so nothing and everything are both one press away. Try it on the R row of the panel above and watch the wheel, the square, the hex line and the H S V rows follow on the same frame — the readout beside it ticks over while you drag and records once when you stop."}
        StoryNote{text: "That leaves no room on the bar for a press that means something else, so typing is reached in time rather than in space: press a row twice in quick succession and it puts back the number it had before the pair and opens the editor. Return does the same once a press has left the keyboard on a row, the arrow keys step it, and the wheel steps it wherever the pointer is. The hex line below the rows is the one that is still a plain text field, because a colour written down is typed, not aimed at."}

        StoryHeading{text: "The same panel, under a swatch"}
        StoryNote{text: "In a property row there is no room for a panel, so the swatch IS the control and the panel hangs off it. It opens under the swatch and flips above when the bottom would run off the window, a press outside commits, and Escape puts back the colour that was there when it opened."}
        StoryRow{
            spacing: theme.space_2
            Label{text: "Tint"}
            button := ColorPickerButton{color: #xE8A317FF}
            button_read := Readout{text: "closed"}
        }

        StoryHeading{text: "The pieces on their own"}
        StoryNote{text: "The ring is an annulus with an empty hole; the picker puts the square in that hole rather than the ring nesting it, so either can be used without the other. A hue slider that is only a ring, a shade square under a fixed brand hue, an opacity strip beside a layer name — none of those need the rest of the panel."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            spacing: theme.space_3
            ring := ColorWheel{width: 150 height: 150 color: #x2FB344FF}
            Stack{
                spacing: theme.space_2
                square := ColorArea{width: 150 height: 110 color: #x2FB344FF}
                strip := ColorAlpha{width: 150 color: #x2FB344CC}
            }
            Stack{
                part_hue := Readout{text: "hue: waiting"}
                part_sv := Readout{text: "shade: waiting"}
                part_alpha := Readout{text: "alpha: waiting"}
            }
        }

        StoryHeading{text: "A colour as a block"}
        StoryNote{text: "The checker under a swatch is not decoration. Without it a colour at half alpha and the same colour at full alpha are the same picture, and showing alpha is the whole reason the block is not a plain rectangle. The third block below is the second one at a third of its opacity."}
        StoryRow{
            spacing: theme.space_2
            sw_a := ColorSwatch{color: #xE6A294FF}
            sw_b := ColorSwatch{color: #x3B82F6FF}
            sw_c := ColorSwatch{color: #x3B82F655}
            sw_d := ColorSwatch{color: #x2FB344FF selected: true}
            pressed := Readout{text: "no block pressed yet"}
        }

        StoryHeading{text: "A strip of them"}
        StoryNote{text: "The strip wraps to whatever width it is given and hit-tests by arithmetic rather than by a widget per cell, so a hundred colours cost one draw call. It binds to nothing: it reports which cell was chosen and the colour in it, and what that cell MEANS — a theme token, a layer's fill, a tag — is known only to whoever filled the strip."}
        palette := PaletteStrip{
            cell_size: 20.
            colors: "#E6A294 #D3C5A6 #9CB1DE #98E1B1 #8EBAEB #E8A317 #D83B3B #2FB344 #6E372B #5F533A #324367 #2F6A43 #274A72 #8A8A8A #FFFFFF #000000"
        }
        palette_read := Readout{text: "no cell chosen yet"}

        StoryHeading{text: "Written down"}
        StoryNote{text: "The field takes either spelling whichever one it shows: a short hex, a long one, one with alpha on the end, or the numeric form. Only the third was given with_alpha, so only it writes the alpha down and takes one that is typed."}
        StoryRow{
            flow: Down
            spacing: theme.space_1
            hex_field := ColorField{width: 260. color: #x3B82F6FF}
            rgb_field := ColorField{width: 260. color: #x2FB344FF notation: ColorNotation.Rgb}
            alpha_field := ColorField{width: 260. color: #xD83B3B99 with_alpha: true}
        }
        field_read := Readout{text: "no colour typed yet"}

        StoryHeading{text: "It repairs itself"}
        StoryNote{text: "Type a half-finished hex, or a word, into the field below and then click anywhere else. Losing the keyboard is treated exactly like pressing Enter: the field is read, and then rewritten from the colour that is actually held. A field that silently kept unparseable text would be a field that lies about what the document contains."}
        StoryRow{
            spacing: theme.space_2
            repair := ColorField{width: 280. color: #x8EBAEBFF}
            repair_read := Readout{text: "still #8ebaeb"}
        }

        StoryHeading{text: "It follows the theme"}
        StoryNote{text: "Every border, every panel face and both squares of every checker come from the theme, so the whole family follows a light page, a dark one and the skeleton grade. The only colours held back are the two the pucks are drawn in: a puck sits on the colour being chosen rather than on a surface, so it has to stay legible over red, over white and over black, and a dark outline with a light ring inside is the shape that manages that."}
    }

}

/// A colour as the page says it: the hex, and the alpha when there is any to
/// speak of.
fn say(c: Vec4f) -> String {
    format_color_hex([c.x, c.y, c.z, c.w], c.w < 0.999)
}

fn color_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let inline = root.color_picker(cx, ids!(inline));
    if let Some(c) = inline.changed(actions) {
        root.label(cx, ids!(following))
            .set_text(cx, &format!("following {}", say(c)));
    }
    // Two reports, not one. Changed is for following a drag; Ended is the one
    // a host writes down.
    if let Some(c) = inline.ended(actions) {
        root.label(cx, ids!(recorded))
            .set_text(cx, &format!("recorded {}", say(c)));
    }

    let button = root.color_picker(cx, ids!(button));
    if let Some(c) = button.ended(actions) {
        root.label(cx, ids!(button_read))
            .set_text(cx, &format!("committed {}", say(c)));
    }

    if let Some(c) = root.color_wheel(cx, ids!(ring)).changed(actions) {
        root.label(cx, ids!(part_hue))
            .set_text(cx, &format!("hue: {}", say(c)));
    }
    if let Some(c) = root.color_area(cx, ids!(square)).changed(actions) {
        root.label(cx, ids!(part_sv))
            .set_text(cx, &format!("shade: {}", say(c)));
    }
    if let Some(c) = root.color_alpha(cx, ids!(strip)).changed(actions) {
        root.label(cx, ids!(part_alpha))
            .set_text(cx, &format!("alpha: {:.2}", c.w));
    }

    for id in [ids!(sw_a), ids!(sw_b), ids!(sw_c), ids!(sw_d)] {
        if let Some(c) = root.color_swatch(cx, id).pressed(actions) {
            root.label(cx, ids!(pressed))
                .set_text(cx, &format!("pressed {}", say(c)));
        }
    }

    if let Some((index, c)) = root.palette_strip(cx, ids!(palette)).picked(actions) {
        root.label(cx, ids!(palette_read))
            .set_text(cx, &format!("cell {index} holds {}", say(c)));
    }

    for (name, id) in [
        ("hex", ids!(hex_field)),
        ("numeric", ids!(rgb_field)),
        ("with alpha", ids!(alpha_field)),
    ] {
        if let Some(c) = root.color_field(cx, id).ended(actions) {
            root.label(cx, ids!(field_read))
                .set_text(cx, &format!("the {name} field now holds {}", say(c)));
        }
    }
}

fn color_field_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(c) = root.color_field(cx, ids!(repair)).ended(actions) {
        root.label(cx, ids!(repair_read))
            .set_text(cx, &format!("now {}", say(c)));
    }
}

/// The page's one handler: the picker and its pieces, then the repair field.
fn color_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    color_actions(cx, root, actions);
    color_field_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[
    Story {
        key: "inputs/color-picker/overview",
        category: "Inputs",
        component: "ColorPicker",
        also: &[
            "ColorAlpha",
            "ColorArea",
            "ColorField",
            "ColorNotation",
            "ColorPickerButton",
            "ColorSwatch",
            "ColorSwatchWide",
            "ColorWheel",
            "PaletteStrip",
        ],
        name: "Overview",
        dsl: "ColorOverview",
        added: "2026-09-10",
        tags: &["new"],
        doc: "# ColorPicker\n\nOne colour, and the several ways a person reaches for it: a hue ring, a saturation/value square, an alpha strip, six rows of numbers, and the last few colours committed in this session.\n\n`ColorPicker` stands inline on a page. `ColorPickerButton` is the same widget with `popover: true`: the swatch is the whole control at rest and the panel opens over the page under it, flipping above when the bottom would run off the window. A press outside commits; Escape puts back the colour that was there when it opened.\n\nEvery piece is also a widget on its own — `ColorWheel` (the ring), `ColorArea` (the square), `ColorAlpha` (the strip), `ColorSwatch` (a colour as a block), `PaletteStrip` (a wrapped grid of cells) and `ColorField` (a colour written down). The picker places the square inside the ring's hole rather than the ring nesting it, so neither depends on the other.\n\n**A number row is a track.** Each of the six channel rows is a filled bar that behaves the way it reads: a press lands that channel at the point it fell on, the drag keeps it under the pointer, and the release ends the gesture. The ends of the fill are the ends of the range, which leaves the bar no room for a press that means anything else, so typing is reached in time instead: a second press on the heels of the first puts back the number the pair found and opens the editor, and Return does the same from the keyboard. The arrows step a focused row and the wheel steps the one under the pointer. The hex line is the one row that is still a text field — a colour written down is typed, not aimed at.\n\n**Hue is the state, not red-green-blue.** Each control holds hue, saturation, value and alpha, and derives the RGBA from that. Round-tripping through RGBA loses the hue of a grey and the hue of black, and a picker whose ring swings back to red the moment the value reaches zero is the classic way this control goes wrong.\n\nEverything reports twice: `Changed` while the colour moves under the hand, and `Ended` once when the gesture finishes. Follow the first, record the second.\n\n**What it is not.** There is no eyedropper — nothing here can read a screen pixel, and an OS capture path is a host's business. There is no colour space beyond sRGB and HSV. And a palette strip binds to nothing: it reports the cell that was picked and the colour in it, and what that cell means is known only to whoever filled the strip.\n\n## ColorField\n\nA colour written down, next to the colour itself. The control for a place where the colour usually arrives from somewhere else — a style sheet, a screenshot, a message — and a wheel would be four gestures where a paste is one.\n\n`notation` decides how the field WRITES a colour: `ColorNotation.Hex` gives `#ff8000`, `ColorNotation.Rgb` gives `rgb(255, 128, 0)`. It always READS both, plus the short `#f80` form and the eight-digit form with alpha, because a person pasting from elsewhere has whatever they had.\n\n`with_alpha` is off by default. Most fields choose a colour and not a transparency, and eight hex digits where six were expected is a good way to paste something wrong; a field without it keeps the alpha it already had.\n\n**It repairs itself.** Losing the keyboard is treated exactly like pressing Enter: the text is read, and then rewritten from the colour that is actually held. A field that silently kept unparseable text would be lying about what the document contains.",
        subject: "inline",
        feature: None,
        controls: &[
            Control { label: "Colour", target: "subject", kind: ControlKind::Color { prop: "color", default: 0x3B82F6FF } },
            Control { label: "Alpha strip", target: "subject", kind: ControlKind::Bool { prop: "with_alpha", default: true } },
            Control { label: "Recent colours", target: "subject", kind: ControlKind::Bool { prop: "with_recent", default: true } },
        ],
        on_actions: Some(color_page_actions),
    },
];
